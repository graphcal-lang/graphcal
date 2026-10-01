//! Check each semantic instance's own bodies and specialize the rest of its
//! checked trees from its template's, without re-inferring the template.

use std::collections::HashMap;

use crate::cancellation::CancellationToken;
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::graphcal_error::GraphcalError;
use crate::outcome::Outcome;
use crate::source_id::SourceId;
use crate::tir::texpr::{CheckedBodies, CheckedBody, NominalObservation, TBody};
use crate::tir::typed::program::{TirRead, UncheckedTir};
use crate::tir::typed::specialization::specialize_expression_type;

use super::body_specialization::DerivedTrees;
use super::{DimCheckContext, check_decl_expr_type, infer};

fn check_retained_reconciliations(
    dag: &crate::tir::typed::model::DagTIR,
    declaration: &crate::resolved_name::ResolvedDeclName,
    observations: &[NominalObservation],
) -> Result<(), GraphcalError> {
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
                            NominalObservation::Field { identity, field } => {
                                (identity, format!("field `{field}` of type `{overridden}`"))
                            }
                            NominalObservation::Constructor {
                                identity,
                                constructor,
                            } => (
                                identity,
                                format!(
                                    "constructor `{}` of type `{overridden}`",
                                    constructor.as_str()
                                ),
                            ),
                            NominalObservation::TypeArgument(identity) => {
                                (identity, format!("type `{overridden}`"))
                            }
                            NominalObservation::IndexLabel { .. }
                            | NominalObservation::IndexArgument(_) => continue,
                        };
                        (identity == source || identity == replacement)
                            .then(|| (overridden.to_string(), "type", detail))
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
                            NominalObservation::IndexLabel { identity, variant } => {
                                (identity, format!("index label `{overridden}#{variant}`"))
                            }
                            NominalObservation::IndexArgument(identity) => {
                                (identity, format!("index `{overridden}`"))
                            }
                            NominalObservation::Field { .. }
                            | NominalObservation::Constructor { .. }
                            | NominalObservation::TypeArgument(_) => continue,
                        };
                        (identity.declared_resolved() == Some(source)
                            || replacement.to_symbolic().matches_ref(identity))
                        .then(|| (overridden.to_string(), "index", detail))
                    }
                };
                if let Some((overridden, kind, detail)) = matched {
                    return Err(GraphcalError::IncludeMustReconcileOverride {
                        overridden,
                        overridden_kind: kind.to_string(),
                        orphan_decl: reconciliation.orphan_decl().to_string(),
                        detail,
                        src: reconciliation.src,
                        span: reconciliation.include_span.into(),
                    });
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
) -> Result<(), Outcome<GraphcalError>> {
    let template_defaults = inherited_defaults(template);
    for entry in ctx.env.dag.params() {
        ctx.checkpoint()?;
        let Some(default) = &entry.default else {
            continue;
        };
        let id = default.id();
        let inherited = template_defaults.contains(id);
        if !inherited {
            check_decl_expr_type(ctx, entry.name(), &entry.identity(), &entry.type_ann)?;
            continue;
        }
        let internal = |message: String| {
            GraphcalError::internal_error(
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
            return Err(GraphcalError::DimensionMismatchInAnnotation {
                declared: expected.format(&ctx.env.registry.dimensions),
                inferred: specialized.format(&ctx.env.registry.dimensions),
                src: ctx.env.src,
                span: default.span.into(),
            }
            .into());
        }
    }
    Ok(())
}

/// The checked trees of every semantic instance, and the template plot
/// channel shapes to specialize for each instance that rebinds a defaulted
/// dimension port (taken in the view where that port is rigid, like the
/// instance's trees).
pub(super) struct InstanceBodies {
    pub(super) bodies: Vec<(crate::dag_id::DagId, CheckedBodies)>,
    pub(super) port_generic_plot_channels:
        HashMap<crate::dag_id::DagId, super::plot::CheckedPlotChannelShapes>,
}

/// Check every semantic instance's own defaults and specialize the rest of its
/// checked trees from its template's `canonical` trees.
pub(super) fn instance_bodies(
    tir: &UncheckedTir,
    canonical: &HashMap<crate::dag_id::DagId, CheckedBodies>,
    src: SourceId,
    cancellation: &CancellationToken,
) -> Result<InstanceBodies, Outcome<GraphcalError>> {
    let checking = crate::tir::typed::CheckingTir {
        tir,
        bodies: canonical,
    };
    let internal =
        |message: String| GraphcalError::internal_error(message, src, DiagnosticAnchor::WholeFile);
    let mut port_generic_plot_channels = HashMap::new();
    let mut published = Vec::new();
    let instances = tir.local_dags().filter_map(|(owner, dag)| {
        dag.frame()
            .specialization()
            .map(|specialization| (owner, dag, specialization))
    });
    for (owner, dag, specialization) in instances {
        cancellation.checkpoint()?;
        let template = tir
            .dags
            .get(&specialization.template)
            .ok_or_else(|| internal("instance has no canonical template".to_owned()))?;
        let template_bodies = checking
            .checked_bodies(&specialization.template)
            .ok_or_else(|| {
                internal(format!(
                    "canonical template `{}` has no published typed bodies",
                    specialization.template
                ))
            })?;
        // A rebound defaulted dimension port: the template's trees saw its
        // default, so the instance's trees come from the view where it is rigid.
        let ports = tir
            .project_type_store()
            .bound_defaulted_dimension_ports(&specialization.substitution);
        let port_generic = if ports.is_empty() {
            DerivedTrees::default()
        } else {
            let generic = super::template_closure::port_generic_trees(
                tir,
                template,
                &ports,
                src,
                cancellation,
            )?;
            port_generic_plot_channels.insert(owner.clone(), generic.plot_channels);
            generic.trees
        };
        let observations = infer::hir::BodyObservations::default();
        let ctx = DimCheckContext {
            env: infer::hir::InferEnv {
                dag,
                tir: &checking,
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
        let claimed =
            crate::tir::texpr::claim_roots(&rebound_defaults(template, dag), finished.typed)
                .map_err(|error| internal(error.to_string()))?;
        let independent = DerivedTrees {
            bodies: claimed.roots.into_iter().collect(),
            nominal_uses: finished.nominal_uses,
            calls: claimed.calls,
        };
        let bodies = super::body_specialization::specialize_instance_bodies(
            dag,
            &checking,
            independent,
            template_bodies,
            &port_generic,
            &specialization.substitution,
            src,
        )?;
        published.push((owner.clone(), bodies));
    }
    Ok(InstanceBodies {
        bodies: published,
        port_generic_plot_channels,
    })
}
