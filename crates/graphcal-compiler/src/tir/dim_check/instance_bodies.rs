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
use crate::tir::texpr::{CheckedBodies, CheckedBody, NominalObservation, TBody};
use crate::tir::typed::checking_tir::PublishedBodies;
use crate::tir::typed::dag_slots::LocalDagFacts;
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
fn check_instance_defaults(
    ctx: &DimCheckContext<'_>,
    template: &crate::tir::typed::model::DagTIR,
    template_bodies: &CheckedBodies,
    substitution: &crate::ir::static_substitution::StaticSubstitution,
) -> Result<(), Outcome<SemanticError>> {
    let template_defaults = inherited_defaults(template);
    for entry in ctx.env.dag.params() {
        ctx.checkpoint()?;
        let Some(default) = &entry.default else {
            continue;
        };
        let id = default.id();
        let inherited = template_defaults.contains(id);
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
        let internal = |message: String| {
            SemanticError::internal_error(
                message,
                ctx.env.src,
                DiagnosticAnchor::Source(default.span),
            )
        };
        let body = template_bodies
            .get(id)
            .ok_or_else(|| internal(format!("missing checked expression: {id:?}")))?;
        let declaration = entry.identity();
        check_retained_reconciliations(
            ctx.env.dag,
            &declaration,
            template_bodies.nominal_uses(id),
        )?;
        let checked_type = match body {
            CheckedBody::Executable(TBody::Value(tree)) => tree.ty().to_symbolic(),
            CheckedBody::Deferred(TBody::Value(tree)) => tree.ty().clone(),
            CheckedBody::Executable(TBody::Contextual(_))
            | CheckedBody::Deferred(TBody::Contextual(_)) => {
                return Err(
                    internal("parameter default has no value checking result".to_owned()).into(),
                );
            }
        };
        let specialized =
            specialize_expression_type(&checked_type, substitution, ctx.env.tir, ctx.env.src)?;
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

/// One local body once the canonical trees are published: a canonical body
/// with its trees and checked plot shapes, or a semantic instance still to
/// specialize from its template.
pub(super) enum CanonicalStage<'t> {
    Canonical {
        bodies: CheckedBodies,
        plot_shapes: CheckedPlotChannelShapes,
    },
    Instance(InstanceOf<'t>),
}

/// A semantic instance body and the specialization it was materialized with.
#[derive(Clone, Copy)]
pub(super) struct InstanceOf<'t> {
    pub(super) position: crate::tir::typed::dag_position::DagPosition,
    pub(super) dag: &'t crate::tir::typed::model::DagTIR,
    pub(super) specialization: &'t crate::ir::static_substitution::StaticSpecializationId,
}

impl PublishedBodies for CanonicalStage<'_> {
    fn published(&self) -> Option<&CheckedBodies> {
        match self {
            Self::Canonical { bodies, .. } => Some(bodies),
            Self::Instance(_) => None,
        }
    }
}

/// Where the plot shapes of one local body come from: a canonical body's
/// own check, or an instance's template, specialized.
pub(super) enum PlotsStage<'t> {
    Canonical(CheckedPlotChannelShapes),
    Instance {
        instance: InstanceOf<'t>,
        /// When the instance rebinds a defaulted dimension port, the
        /// template's shapes in the view where that port is rigid, like the
        /// instance's trees.
        port_generic: Option<CheckedPlotChannelShapes>,
    },
}

/// The checked trees of every local body, and where its plot shapes come
/// from: a canonical body keeps its published trees; a semantic instance
/// checks its own defaults and specializes the rest of its trees from its
/// template's `canonical` trees.
pub(super) fn local_bodies<'t>(
    tir: &UncheckedTir,
    canonical: &LocalDagFacts<CanonicalStage<'t>>,
    src: SourceId,
    cancellation: &CancellationToken,
) -> Result<LocalDagFacts<(CheckedBodies, PlotsStage<'t>)>, Outcome<SemanticError>> {
    let checking = crate::tir::typed::checking_tir::CheckingTir {
        tir,
        bodies: canonical,
    };
    canonical.try_map_ref(|stage| match stage {
        CanonicalStage::Canonical {
            bodies,
            plot_shapes,
        } => Ok((bodies.clone(), PlotsStage::Canonical(plot_shapes.clone()))),
        CanonicalStage::Instance(instance) => {
            instance_bodies(&checking, *instance, src, cancellation).map(
                |(bodies, port_generic)| {
                    (
                        bodies,
                        PlotsStage::Instance {
                            instance: *instance,
                            port_generic,
                        },
                    )
                },
            )
        }
    })
}

/// Check one semantic instance's own defaults and specialize the rest of its
/// checked trees from its template's, with the template plot channel shapes
/// to specialize when it rebinds a defaulted dimension port.
fn instance_bodies(
    checking: &crate::tir::typed::checking_tir::CheckingTir<'_, CanonicalStage<'_>>,
    InstanceOf {
        position,
        dag,
        specialization,
    }: InstanceOf<'_>,
    src: SourceId,
    cancellation: &CancellationToken,
) -> Result<(CheckedBodies, Option<CheckedPlotChannelShapes>), Outcome<SemanticError>> {
    let tir = checking.tir;
    let internal =
        |message: String| SemanticError::internal_error(message, src, DiagnosticAnchor::WholeFile);
    cancellation.checkpoint()?;
    let (template_position, template, template_bodies) = tir
        .dags
        .position(&specialization.template)
        .and_then(|template| {
            checking
                .checked_at(template)
                .map(|(dag, bodies)| (template, dag, bodies))
        })
        .ok_or_else(|| {
            internal(format!(
                "instance has no checked canonical template `{}`",
                specialization.template
            ))
        })?;
    // A rebound defaulted dimension port: the template's trees saw its
    // default, so the instance's trees come from the view where it is rigid.
    let ports = tir
        .project_type_store()
        .bound_defaulted_dimension_ports(&specialization.substitution);
    let (port_generic, port_generic_plot_channels) = if ports.is_empty() {
        (DerivedTrees::default(), None)
    } else {
        let generic = super::template_closure::port_generic_trees(
            tir,
            template_position,
            template,
            &ports,
            src,
            cancellation,
        )?;
        (generic.trees, Some(generic.plot_channels))
    };
    let observations = infer::hir::BodyObservations::default();
    let ctx = DimCheckContext {
        position,
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
    check_instance_defaults(
        &ctx,
        template,
        template_bodies,
        &specialization.substitution,
    )?;
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
        &specialization.substitution,
        src,
    )?;
    Ok((bodies, port_generic_plot_channels))
}
