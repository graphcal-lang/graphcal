//! Check each semantic instance's own bodies and specialize the rest of its
//! checked trees from its template's, without re-inferring the template.

use crate::cancellation::CancellationToken;
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::outcome::Outcome;
use crate::semantic_error::SemanticError;
use crate::semantic_error::dimension::DimensionError;
use crate::semantic_error::visibility::OverriddenKind;
use crate::semantic_error::visibility::OverrideMention;
use crate::semantic_error::visibility::VisibilityError;
use crate::source_id::SourceId;
use crate::tir::texpr::{CheckedBodies, NominalObservation};
use crate::tir::typed::complete_substitution::CompleteSubstitution;
use crate::tir::typed::instance_graph::{
    CanonicalFacts, InstanceFacts, InstanceGraph, InstanceOrigin, TemplateFact,
};
use crate::tir::typed::program::UncheckedTir;
use crate::tir::typed::specialization::specialize_expression_type;

use super::body_specialization::DerivedTrees;
use super::plot::CheckedPlotChannelShapes;
use super::{DimCheckContext, check_decl_expr_type, infer};

fn check_retained_reconciliations(
    dag: &crate::tir::typed::model::DagTIR,
    declaration: &crate::resolved_name::ResolvedDeclName,
    observations: &[NominalObservation],
) -> Result<(), SemanticError> {
    use crate::tir::typed::model::OverrideTarget;
    for reconciliation in dag
        .semantic
        .override_reconciliations
        .get(declaration)
        .into_iter()
        .flatten()
    {
        for target in &reconciliation.targets {
            for observation in observations {
                let matched = match (target, observation) {
                    (
                        OverrideTarget::Type {
                            overridden,
                            source,
                            replacement,
                        },
                        observation,
                    ) => {
                        let (identity, detail) = match observation {
                            NominalObservation::Field { identity, field } => (
                                identity,
                                OverrideMention::Field {
                                    field: field.clone(),
                                    owner: overridden.clone(),
                                },
                            ),
                            NominalObservation::Constructor {
                                identity,
                                constructor,
                            } => (
                                identity,
                                OverrideMention::Constructor {
                                    constructor: constructor.to_unowned_def_name(),
                                    owner: overridden.clone(),
                                },
                            ),
                            NominalObservation::TypeArgument(identity) => {
                                (identity, OverrideMention::TypeArgument(overridden.clone()))
                            }
                            NominalObservation::IndexLabel { .. }
                            | NominalObservation::IndexArgument(_) => continue,
                        };
                        (identity == source || identity == replacement)
                            .then(|| (overridden.atom().clone(), OverriddenKind::Type, detail))
                    }
                    (
                        OverrideTarget::Index {
                            overridden,
                            source,
                            replacement,
                        },
                        observation,
                    ) => {
                        let (identity, detail) = match observation {
                            NominalObservation::IndexLabel { identity, variant } => (
                                identity,
                                OverrideMention::IndexLabel {
                                    index: overridden.clone(),
                                    variant: variant.clone(),
                                },
                            ),
                            NominalObservation::IndexArgument(identity) => {
                                (identity, OverrideMention::IndexArgument(overridden.clone()))
                            }
                            NominalObservation::Field { .. }
                            | NominalObservation::Constructor { .. }
                            | NominalObservation::TypeArgument(_) => continue,
                        };
                        (identity.declared_resolved() == Some(source)
                            || replacement.to_symbolic().matches_ref(identity))
                        .then(|| (overridden.atom().clone(), OverriddenKind::Index, detail))
                    }
                };
                if let Some((overridden, kind, detail)) = matched {
                    return Err(SemanticError::located(
                        reconciliation.src,
                        reconciliation.include_span,
                        VisibilityError::IncludeMustReconcileOverride {
                            overridden,
                            overridden_kind: kind,
                            orphan_decl: reconciliation.orphan_decl(),
                            detail,
                        },
                    ));
                }
            }
        }
    }
    Ok(())
}

/// The template's parameter defaults an instance inherits.
///
/// An instance keeps each template default it does not rebind, and expression
/// identities are unique across lowered bodies, so a default is inherited
/// exactly when it is one of the template's parameter defaults.
fn inherited_defaults(
    template: &crate::tir::typed::model::DagTIR,
) -> std::collections::HashSet<&crate::expression_id::ExprId> {
    template
        .params()
        .filter_map(|entry| entry.default.as_deref())
        .map(crate::hir::expr::Expr::id)
        .collect()
}

/// The parameter defaults an instance rebinds, checked independently of its
/// template.
fn rebound_defaults<'d>(
    template: &crate::tir::typed::model::DagTIR,
    instance: &'d crate::tir::typed::model::DagTIR,
) -> Vec<&'d crate::hir::expr::Expr> {
    let inherited = inherited_defaults(template);
    instance
        .params()
        .filter_map(|entry| entry.default.as_deref())
        .filter(|default| !inherited.contains(default.id()))
        .collect()
}

/// Check the instance's parameter defaults: a rebound default is inferred
/// independently; an inherited one keeps its template tree's type,
/// specialized, and must still match its annotation.
///
/// The template checked each of its defaults against its own annotation, so
/// an inherited default's tree has the template parameter's declared type.
/// An instance's parameters are its template's, in the same order:
/// specialization rebases the template's declaration table, rebinding only
/// defaults.
fn check_instance_defaults(
    ctx: &DimCheckContext<'_>,
    template: &crate::tir::typed::model::DagTIR,
    template_bodies: &CheckedBodies,
    substitution: &CompleteSubstitution<'_>,
) -> Result<(), Outcome<SemanticError>> {
    for (template_entry, entry) in template.params().zip(ctx.env.dag.params()) {
        debug_assert_eq!(template_entry.name(), entry.name());
        ctx.checkpoint()?;
        let Some(default) = &entry.default else {
            continue;
        };
        let id = default.id();
        let inherited = template_entry
            .default
            .as_deref()
            .is_some_and(|template_default| template_default.id() == id);
        if !inherited {
            check_decl_expr_type(
                ctx,
                entry.name(),
                &entry.identity(),
                &entry.type_ann,
                default,
            )?;
            continue;
        }
        let declaration = entry.identity();
        check_retained_reconciliations(
            ctx.env.dag,
            &declaration,
            template_bodies.nominal_uses(id),
        )?;
        let checked_type = template_entry.type_ann.checked().declared().to_symbolic();
        let specialized = specialize_expression_type(&checked_type, substitution, ctx.env.src)?;
        let expected = entry.type_ann.checked().declared();
        if specialized != expected.to_symbolic() {
            return Err(SemanticError::located(
                ctx.env.src,
                default.span,
                DimensionError::DimensionMismatchInAnnotation {
                    declared: expected.spelling(&ctx.env.registry.dimensions),
                    inferred: specialized.spelling(&ctx.env.registry.dimensions),
                },
            )
            .into());
        }
    }
    Ok(())
}

/// A canonical body's checked trees and plot shapes.
pub(super) struct CanonicalChecked {
    pub(super) bodies: CheckedBodies,
    pub(super) plot_shapes: CheckedPlotChannelShapes,
}

/// A semantic instance body and the specialization it was materialized with.
pub(super) struct InstanceOf<'t> {
    pub(super) position: crate::tir::typed::dag_position::DagPosition,
    pub(super) dag: &'t crate::tir::typed::model::DagTIR,
    pub(super) origin: &'t InstanceOrigin,
    /// The complete view of the origin's substitution.
    pub(super) substitution: CompleteSubstitution<'t>,
}

/// A semantic instance's checked trees and, when it rebinds a defaulted
/// dimension port, the template's plot shapes in the view where that port is
/// rigid, like its trees.
pub(super) struct InstanceChecked {
    pub(super) bodies: CheckedBodies,
    pub(super) port_generic: Option<CheckedPlotChannelShapes>,
}

/// The template trees and DAG of `origin`: a local template's published
/// trees, or an imported template's.
fn template_of<'a>(
    tir: &'a UncheckedTir,
    canonical: &'a CanonicalFacts<CanonicalChecked>,
    origin: &'a InstanceOrigin,
) -> (&'a crate::tir::typed::model::DagTIR, &'a CheckedBodies) {
    let dag = tir.dags.at(origin.template_position());
    match canonical.template(origin.template()) {
        TemplateFact::Local(checked) => (dag, &checked.bodies),
        TemplateFact::Shared(shared) => (dag, shared.bodies()),
    }
}

/// The checked trees of every semantic instance: each checks its own
/// defaults and specializes the rest of its trees from its template's
/// `canonical` trees.
pub(super) fn instance_bodies<'t>(
    tir: &'t UncheckedTir,
    graph: &InstanceGraph,
    canonical: &CanonicalFacts<CanonicalChecked>,
    instances: &InstanceFacts<InstanceOf<'t>>,
    src: SourceId,
    cancellation: &CancellationToken,
) -> Result<InstanceFacts<InstanceChecked>, Outcome<SemanticError>> {
    let published = graph.join(
        canonical.map_ref(|checked| Some(&checked.bodies)),
        instances.map_ref(|_| None),
    );
    let checking = crate::tir::typed::checking_tir::CheckingTir {
        tir,
        bodies: &published,
    };
    instances.try_map_ref(|instance| {
        let (template, template_bodies) = template_of(tir, canonical, instance.origin);
        instance_body(
            &checking,
            instance,
            template,
            template_bodies,
            src,
            cancellation,
        )
    })
}

/// Check one semantic instance's own defaults and specialize the rest of its
/// checked trees from its template's, with the template plot channel shapes
/// to specialize when it rebinds a defaulted dimension port.
fn instance_body(
    checking: &crate::tir::typed::checking_tir::CheckingTir<'_, Option<&CheckedBodies>>,
    InstanceOf {
        position,
        dag,
        origin,
        substitution,
    }: &InstanceOf<'_>,
    template: &crate::tir::typed::model::DagTIR,
    template_bodies: &CheckedBodies,
    src: SourceId,
    cancellation: &CancellationToken,
) -> Result<InstanceChecked, Outcome<SemanticError>> {
    let tir = checking.tir;
    let internal =
        |message: String| SemanticError::internal_error(message, src, DiagnosticAnchor::WholeFile);
    cancellation.checkpoint()?;
    // A rebound defaulted dimension port: the template's trees saw its
    // default, so the instance's trees come from the view where it is rigid.
    let ports = tir
        .project_type_store()
        .bound_defaulted_dimension_ports(&origin.specialization().substitution);
    let (port_generic, port_generic_plot_channels) = if ports.is_empty() {
        (DerivedTrees::default(), None)
    } else {
        let generic = super::template_closure::port_generic_trees(
            tir,
            origin.template_position(),
            template,
            &ports,
            src,
            cancellation,
        )?;
        (generic.trees, Some(generic.plot_channels))
    };
    let observations = infer::hir::BodyObservations::default();
    let ctx = DimCheckContext {
        position: *position,
        env: infer::hir::InferEnv {
            dag,
            tir: checking,
            registry: tir.registry(),
            src,
        },
        assembly: tir,
        cancellation,
        observations: &observations,
    };
    check_instance_defaults(&ctx, template, template_bodies, substitution)?;
    // Instance bodies are specialized from the template; only the trees
    // of independently checked defaults are the instance's own.
    let finished = observations.finish();
    let claimed = crate::tir::texpr::claim_roots(&rebound_defaults(template, dag), finished.typed)
        .map_err(|error| internal(error.to_string()))?;
    let independent = DerivedTrees {
        bodies: claimed.roots.into_iter().collect(),
        nominal_uses: finished.nominal_uses,
        calls: claimed.calls,
    };
    let bodies = super::body_specialization::specialize_instance_bodies(
        dag,
        checking,
        independent,
        template_bodies,
        &port_generic,
        substitution,
        src,
    )?;
    Ok(InstanceChecked {
        bodies,
        port_generic: port_generic_plot_channels,
    })
}
