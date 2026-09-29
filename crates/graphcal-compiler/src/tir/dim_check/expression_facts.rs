//! Specialize retained results without re-inferring reusable source expressions.

use miette::NamedSource;
use std::collections::HashMap;
use std::sync::Arc;

use crate::cancellation::CancellationToken;
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::hir::expr::visit_expr;
use crate::registry::checked_type::{CheckedType, Symbolic};
use crate::registry::error::GraphcalError;
use crate::tir::expression_facts::{
    CheckedExpressionFacts, CheckingEnvironment, ExpressionFact, ValueFact,
};
use crate::tir::typed::model::TIR;
use crate::tir::typed::specialization::{specialize_expression_type, specialize_index_ref};

use super::expression_axes::{check_materializable, checked_index_cardinality};
use super::{DimCheckContext, check_decl_expr_type, infer};

/// Discharge one bound's retained Nat/type/shape obligations in its canonical
/// environment. This does not infer the source body and grants no capabilities.
#[expect(
    clippy::implicit_hasher,
    reason = "canonical binding services retain this exact map type"
)]
pub fn specialize_bound_expression_facts(
    tir: &TIR,
    dag: &crate::tir::typed::model::DagTIR,
    root: &crate::hir::expr::Expr,
    bindings: &HashMap<crate::hir::types::GenericParamId, u64>,
    src: &NamedSource<Arc<String>>,
) -> Result<CheckedExpressionFacts, GraphcalError> {
    let diagnostic =
        |message| GraphcalError::internal_error(message, src, DiagnosticAnchor::Source(root.span));
    let facts = dag
        .expression_facts()
        .map_err(|error| diagnostic(error.to_string()))?;
    let mut ids = Vec::new();
    visit_expr(root, &mut |expr| ids.push(expr.id().clone()));
    let substitution = crate::tir::typed::Substitution::for_nats(bindings);
    let records = ids
        .into_iter()
        .map(|id| {
            let record = facts
                .get(&id)
                .map_err(|error| diagnostic(error.to_string()))?;
            let record = specialize_record(
                record,
                dag,
                tir,
                &FactSubstitution::Generic(&substitution),
                src,
                root.span,
                &record.environment,
            )?;
            Ok((id, record))
        })
        .collect::<Result<_, GraphcalError>>()?;
    CheckedExpressionFacts::publish(
        dag.dag_id().clone(),
        dag.body_revision().clone(),
        &[root],
        records,
        &|index| checked_index_cardinality(tir, index),
    )
    .and_then(|facts| {
        facts.executable_value(root.id())?;
        Ok(facts)
    })
    .map_err(|error| match error {
        crate::tir::expression_facts::ExpressionFactsError::StaticIndex(error) => {
            GraphcalError::EvalError {
                message: error.to_string(),
                src: src.clone(),
                span: root.span.into(),
            }
        }
        error => diagnostic(error.to_string()),
    })
}

fn check_retained_reconciliations(
    dag: &crate::tir::typed::model::DagTIR,
    declaration: &crate::resolved_name::ResolvedDeclName,
    observations: &[crate::tir::expression_facts::NominalObservation],
) -> Result<(), GraphcalError> {
    use crate::tir::expression_facts::NominalObservation;
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
                        src: reconciliation.src.clone(),
                        span: reconciliation.include_span.into(),
                    });
                }
            }
        }
    }
    Ok(())
}

fn check_instance_defaults(
    ctx: &DimCheckContext<'_>,
    template: &crate::tir::typed::model::DagTIR,
    facts: &CheckedExpressionFacts,
    substitution: &crate::ir::static_substitution::StaticSubstitution,
) -> Result<(), GraphcalError> {
    // An instance keeps each template default it does not rebind, and
    // expression identities are unique across lowered bodies, so a default is
    // inherited exactly when it is one of the template's parameter defaults.
    let template_defaults = template
        .params()
        .filter_map(|entry| entry.default.as_deref())
        .map(crate::hir::expr::Expr::id)
        .collect::<std::collections::HashSet<_>>();
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
        let record = facts.get(id).map_err(|error| {
            GraphcalError::internal_error(
                error.to_string(),
                ctx.env.src,
                DiagnosticAnchor::Source(default.span),
            )
        })?;
        let declaration = entry.identity();
        check_retained_reconciliations(ctx.env.dag, &declaration, record.nominal_observations())?;
        let Some(value) = record.fact.symbolic_value() else {
            return Err(GraphcalError::internal_error(
                "parameter default has no value checking result",
                ctx.env.src,
                DiagnosticAnchor::Source(default.span),
            ));
        };
        let specialized = specialize_expression_type(
            &value.checked_type,
            substitution,
            ctx.env.tir,
            ctx.env.src,
        )?;
        let expected = entry.type_ann.checked().declared();
        if specialized != expected.to_symbolic() {
            return Err(GraphcalError::DimensionMismatchInAnnotation {
                declared: expected.format(&ctx.env.registry.dimensions),
                inferred: specialized.format(&ctx.env.registry.dimensions),
                src: ctx.env.src.clone(),
                span: default.span.into(),
            });
        }
    }
    Ok(())
}

enum FactSubstitution<'a> {
    Static(&'a crate::ir::static_substitution::StaticSubstitution),
    Generic(&'a crate::tir::typed::Substitution),
}

impl FactSubstitution<'_> {
    fn value_type(
        &self,
        ty: &CheckedType<Symbolic>,
        tir: &TIR,
        src: &NamedSource<Arc<String>>,
        span: crate::syntax::span::Span,
    ) -> Result<CheckedType<Symbolic>, GraphcalError> {
        match self {
            Self::Static(substitution) => specialize_expression_type(ty, substitution, tir, src),
            Self::Generic(substitution) => substitution
                .instantiate(ty, span)
                .map(|ty| ty.to_symbolic())
                .map_err(|error| error.into_graphcal(src)),
        }
    }

    fn index(
        &self,
        index: &crate::registry::checked_type::IndexTypeRef<Symbolic>,
        src: &NamedSource<Arc<String>>,
        span: crate::syntax::span::Span,
    ) -> Result<crate::registry::checked_type::IndexTypeRef<Symbolic>, GraphcalError> {
        match self {
            Self::Static(substitution) => Ok(specialize_index_ref(index, substitution)),
            Self::Generic(substitution) => substitution
                .instantiate_index(index, span)
                .map(|index| index.to_symbolic())
                .map_err(|error| error.into_graphcal(src)),
        }
    }
}

/// Map retained meaning directly. Do not clone the old type/shape/constructor
/// arguments merely to discard them immediately during specialization.
fn specialize_record(
    record: &crate::tir::expression_facts::CheckedExpressionRecord,
    dag: &crate::tir::typed::model::DagTIR,
    tir: &TIR,
    substitution: &FactSubstitution<'_>,
    src: &NamedSource<Arc<String>>,
    span: crate::syntax::span::Span,
    environment: &Arc<CheckingEnvironment>,
) -> Result<Box<crate::tir::expression_facts::CheckedExpressionRecord>, GraphcalError> {
    use crate::tir::expression_facts::{
        CheckedExpressionRecord, ConstructorApplication, ConstructorMatch, StaticIndexRequirement,
    };
    let fact = match record.fact.symbolic_value() {
        None => record.fact.clone(),
        Some(value) => {
            let checked_type = substitution.value_type(&value.checked_type, tir, src, span)?;
            check_materializable(&checked_type, tir, src, span)?;
            let constructor = value
                .constructor
                .as_ref()
                .map(|application| {
                    let CheckedType::Struct(_, args) = &checked_type else {
                        return Err(GraphcalError::internal_error(
                            "constructor specialization has no nominal result",
                            src,
                            DiagnosticAnchor::Source(span),
                        ));
                    };
                    Ok(Box::new(ConstructorApplication {
                        runtime_type: dag.runtime_struct_type_identity(application.definition()),
                        constructor: application.constructor.clone(),
                        generic_args: args.clone(),
                    }))
                })
                .transpose()?;
            ExpressionFact::Symbolic(ValueFact {
                checked_type,
                constructor,
            })
        }
    };
    let static_indexes = record
        .static_indexes
        .iter()
        .map(|requirement| {
            Ok(StaticIndexRequirement {
                operand: requirement.operand.clone(),
                axis: substitution.index(&requirement.axis, src, span)?,
                position: requirement.position,
                usage: requirement.usage,
            })
        })
        .collect::<Result<_, GraphcalError>>()?;
    let constructor_matches = record
        .constructor_matches
        .iter()
        .map(|(id, target)| {
            (
                id.clone(),
                ConstructorMatch {
                    definition: target.definition.clone(),
                    runtime_type: dag.runtime_struct_type_identity(&target.definition),
                    constructor: target.constructor.clone(),
                },
            )
        })
        .collect();
    Ok(Box::new(CheckedExpressionRecord {
        environment: Arc::clone(environment),
        fact,
        static_indexes,
        constructor_matches,
        operation: record.operation.clone(),
        children: record.children.clone(),
        unit_dependencies: record.unit_dependencies.clone(),
        nominal_observations: record.nominal_observations.clone(),
    }))
}

/// Specialize and publish every semantic instance's expression facts.
///
/// Returns the template plot channel shapes to specialize for each instance
/// that rebinds a defaulted dimension port (taken in the view where that port
/// is rigid, like the instance's facts).
pub(super) fn install_instance_expression_facts(
    tir: &mut TIR,
    src: &NamedSource<Arc<String>>,
    cancellation: &CancellationToken,
) -> Result<HashMap<crate::dag_id::DagId, super::plot::CheckedPlotChannelShapes>, GraphcalError> {
    let mut port_generic_plot_channels = HashMap::new();
    let owners: Vec<_> = tir
        .local_dags()
        .filter(|(_, dag)| dag.is_semantic_instance())
        .map(|(owner, _)| owner.clone())
        .collect();
    for owner in owners {
        cancellation.checkpoint()?;
        let dag = tir.dags.get(&owner).ok_or_else(|| {
            GraphcalError::internal_error("instance disappeared", src, DiagnosticAnchor::WholeFile)
        })?;
        let specialization = dag.semantic_specialization.as_ref().ok_or_else(|| {
            GraphcalError::internal_error(
                "instance has no Static application",
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
        let template = tir.dags.get(&specialization.template).ok_or_else(|| {
            GraphcalError::internal_error(
                "instance has no canonical template",
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
        let facts = template.expression_facts().map_err(|error| {
            GraphcalError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
        })?;
        // A rebound defaulted dimension port: the template's facts saw its
        // default, so the bodies' facts come from the view where it is rigid.
        let ports = tir
            .project_type_store()
            .bound_defaulted_dimension_ports(&specialization.substitution);
        let port_generic_records = if ports.is_empty() {
            HashMap::new()
        } else {
            let generic = super::template_closure::port_generic_facts(
                tir,
                template,
                &ports,
                src,
                cancellation,
            )?;
            port_generic_plot_channels.insert(owner.clone(), generic.plot_channels);
            generic.records
        };
        let observations = infer::hir::BodyObservations::new(dag);
        let ctx = DimCheckContext {
            env: infer::hir::InferEnv {
                dag,
                tir,
                registry: &tir.registry,
                src,
            },
            cancellation,
            observations: &observations,
        };
        check_instance_defaults(&ctx, template, facts, &specialization.substitution)?;
        let mut records = observations.finish();
        let environment =
            CheckingEnvironment::new(dag.dag_id().clone(), dag.body_revision().clone());
        let mut inventory = Vec::new();
        dag.owned_expression_roots().for_each(|root| {
            visit_expr(root, &mut |expr| inventory.push(expr));
        });
        for expr in inventory {
            let span = expr.span;
            let id = expr.id();
            if records.contains_key(id) {
                continue;
            }
            let record = match port_generic_records.get(id) {
                Some(record) => record,
                None => facts.get(id).map_err(|error| {
                    GraphcalError::internal_error(
                        error.to_string(),
                        src,
                        DiagnosticAnchor::Source(span),
                    )
                })?,
            };
            let record = specialize_record(
                record,
                dag,
                tir,
                &FactSubstitution::Static(&specialization.substitution),
                src,
                span,
                &environment,
            )?;
            records.insert(id.clone(), record);
        }
        publish_instance_facts(tir, &owner, records, src)?;
    }
    Ok(port_generic_plot_channels)
}

fn publish_instance_facts(
    tir: &mut TIR,
    owner: &crate::dag_id::DagId,
    records: HashMap<
        crate::expression_id::ExprId,
        Box<crate::tir::expression_facts::CheckedExpressionRecord>,
    >,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let disappeared = || {
        GraphcalError::internal_error(
            "instance disappeared during publication",
            src,
            DiagnosticAnchor::WholeFile,
        )
    };
    let dag = tir.dags.get(owner).ok_or_else(disappeared)?;
    let published = CheckedExpressionFacts::publish(
        owner.clone(),
        dag.body_revision().clone(),
        &dag.owned_expression_roots().collect::<Vec<_>>(),
        records,
        &|index| checked_index_cardinality(tir, index),
    )
    .map_err(|error| {
        GraphcalError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
    })?;
    tir.dags
        .get_mut(owner)
        .ok_or_else(disappeared)?
        .semantic
        .expression_facts = Some(published);
    Ok(())
}
