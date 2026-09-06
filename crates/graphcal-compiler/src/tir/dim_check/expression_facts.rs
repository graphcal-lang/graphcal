//! Specialize retained results without re-inferring reusable source expressions.

use miette::NamedSource;
use std::collections::HashMap;
use std::sync::Arc;

use crate::cancellation::CancellationToken;
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::hir::expr::visit_expr;
use crate::registry::declared_type::DeclaredType;
use crate::registry::error::GraphcalError;
use crate::tir::expression_facts::{CheckedExpressionFacts, CheckingEnvironment, ExpressionFact};
use crate::tir::typed::model::TIR;
use crate::tir::typed::specialization::{specialize_expression_type, specialize_index_ref};

use super::expression_axes::{checked_expression_shape, checked_index_cardinality};
use super::{DimCheckContext, check_decl_expr_type, infer};

#[cfg(test)]
mod tests;

#[derive(Debug, thiserror::Error)]
enum BoundNatError {
    #[error(transparent)]
    Overflow(#[from] crate::nat::NatOverflowError),
    #[error("Nat variable `{0}` has no retained lexical owner")]
    MissingScope(crate::syntax::type_name::GenericParamName),
    #[error("required Nat binding is missing: {0:?}")]
    MissingBinding(crate::hir::types::GenericParamId),
}

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
    visit_expr(root, &mut |expr| ids.push(expr.id().cloned()));
    let no_parameters = HashMap::new();
    let records = ids
        .into_iter()
        .map(|id| {
            let id = id.map_err(|error| diagnostic(error.to_string()))?;
            let record = facts
                .get(&id)
                .map_err(|error| diagnostic(error.to_string()))?;
            let scope = record.nat_parameters.as_deref().unwrap_or(&no_parameters);
            let record = specialize_record(
                record,
                dag,
                tir,
                &FactSubstitution::Nat { scope, bindings },
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
        facts.executable_value(root.id()?)?;
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

fn evaluate_bound_nat(
    form: &crate::nat::NatPolyForm,
    scope: &HashMap<crate::syntax::type_name::GenericParamName, crate::hir::types::GenericParamId>,
    bindings: &HashMap<crate::hir::types::GenericParamId, u64>,
) -> Result<u64, BoundNatError> {
    form.evaluate_with(|name| {
        let id = scope
            .get(name)
            .ok_or_else(|| BoundNatError::MissingScope(name.clone()))?;
        bindings
            .get(id)
            .copied()
            .ok_or_else(|| BoundNatError::MissingBinding(id.clone()))
    })
}

fn bind_index_nats(
    index: &crate::registry::declared_type::IndexTypeRef,
    scope: &HashMap<crate::syntax::type_name::GenericParamName, crate::hir::types::GenericParamId>,
    bindings: &HashMap<crate::hir::types::GenericParamId, u64>,
    src: &NamedSource<Arc<String>>,
    span: crate::syntax::span::Span,
) -> Result<crate::registry::declared_type::IndexTypeRef, GraphcalError> {
    match index.finite_index_form() {
        Some(form) => {
            let cardinality = evaluate_bound_nat(&form, scope, bindings).map_err(|error| {
                GraphcalError::internal_error(
                    error.to_string(),
                    src,
                    DiagnosticAnchor::Source(span),
                )
            })?;
            let finite = crate::registry::index::FiniteIndex::try_from_u64(cardinality).map_err(
                |error| GraphcalError::EvalError {
                    message: error.to_string(),
                    src: src.clone(),
                    span: span.into(),
                },
            )?;
            Ok(crate::registry::declared_type::IndexTypeRef::from_finite_index(finite))
        }
        None => Ok(index.clone()),
    }
}

fn bind_type_nats(
    ty: &DeclaredType,
    scope: &HashMap<crate::syntax::type_name::GenericParamName, crate::hir::types::GenericParamId>,
    bindings: &HashMap<crate::hir::types::GenericParamId, u64>,
    src: &NamedSource<Arc<String>>,
    span: crate::syntax::span::Span,
) -> Result<DeclaredType, GraphcalError> {
    use crate::registry::declared_type::{DeclaredGenericArg, IndexTypeRef};
    let evaluate = |form: &crate::nat::NatPolyForm| {
        evaluate_bound_nat(form, scope, bindings).map_err(|error: BoundNatError| {
            GraphcalError::internal_error(error.to_string(), src, DiagnosticAnchor::Source(span))
        })
    };
    let bind_index = |index: &IndexTypeRef| bind_index_nats(index, scope, bindings, src, span);
    let recurse = |ty: &DeclaredType| bind_type_nats(ty, scope, bindings, src, span);
    Ok(match ty {
        DeclaredType::IndexArg(index) => DeclaredType::IndexArg(bind_index(index)?),
        DeclaredType::Key(index) => DeclaredType::Key(bind_index(index)?),
        DeclaredType::Indexed { element, index } => DeclaredType::Indexed {
            element: Box::new(recurse(element)?),
            index: bind_index(index)?,
        },
        DeclaredType::Struct(identity, args) => DeclaredType::Struct(
            identity.clone(),
            args.iter()
                .map(|arg| {
                    Ok(match arg {
                        DeclaredGenericArg::Nat(form) => DeclaredGenericArg::Nat(
                            crate::nat::NatPolyForm::from_constant(evaluate(form)?),
                        ),
                        DeclaredGenericArg::Type(ty) => DeclaredGenericArg::Type(recurse(ty)?),
                        DeclaredGenericArg::Index(index) => {
                            DeclaredGenericArg::Index(bind_index(index)?)
                        }
                        DeclaredGenericArg::Dim(_) => arg.clone(),
                    })
                })
                .collect::<Result<_, GraphcalError>>()?,
        ),
        DeclaredType::Quantity(_)
        | DeclaredType::Complex(_)
        | DeclaredType::Bool
        | DeclaredType::Int
        | DeclaredType::Datetime(_) => ty.clone(),
    })
}

fn check_retained_reconciliations(
    dag: &crate::tir::typed::model::DagTIR,
    declaration: &crate::syntax::decl_name::ResolvedDeclName,
    observations: &[crate::tir::expression_facts::NominalObservation],
) -> Result<(), GraphcalError> {
    use crate::tir::expression_facts::NominalObservation;
    use crate::tir::typed::model::ResolvedOverrideTarget;
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
                        ResolvedOverrideTarget::Type {
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
                        ResolvedOverrideTarget::Index {
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
                            || replacement.matches_ref(identity))
                        .then(|| (overridden.to_string(), "index", detail))
                    }
                };
                if let Some((overridden, kind, detail)) = matched {
                    return Err(GraphcalError::IncludeMustReconcileOverride {
                        overridden,
                        overridden_kind: kind.to_string(),
                        orphan_decl: reconciliation.orphan_decl.to_string(),
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
    substitution: &crate::ir::instance::StaticSubstitution,
) -> Result<(), GraphcalError> {
    for entry in &ctx.dag.params {
        ctx.checkpoint()?;
        let Some(default) = &entry.default else {
            continue;
        };
        let id = default.expr.id().map_err(|error| {
            GraphcalError::internal_error(
                error.to_string(),
                ctx.src,
                DiagnosticAnchor::Source(default.expr.span),
            )
        })?;
        // Compare this parameter's authoritative default, not every body root.
        let template_declaration = template.require_bound_decl_identity(
            &entry.name,
            ctx.src,
            DiagnosticAnchor::Source(entry.span),
        )?;
        let inherited = template
            .runtime_expr(&template_declaration)
            .map(crate::hir::expr::Expr::id)
            .transpose()
            .map_err(|error| {
                GraphcalError::internal_error(
                    error.to_string(),
                    ctx.src,
                    DiagnosticAnchor::Source(default.expr.span),
                )
            })?
            == Some(id);
        if !inherited {
            check_decl_expr_type(
                &ctx.for_body(default.src.resolve(ctx.src)),
                &entry.name,
                &entry.type_ann.span,
                entry.type_src.resolve(ctx.src),
            )?;
            ctx.expression_facts
                .record_contextual(&default.expr, default.src.resolve(ctx.src))?;
            continue;
        }
        let record = facts.get(id).map_err(|error| {
            GraphcalError::internal_error(
                error.to_string(),
                ctx.src,
                DiagnosticAnchor::Source(default.expr.span),
            )
        })?;
        let declaration = ctx.dag.require_bound_decl_identity(
            &entry.name,
            ctx.src,
            DiagnosticAnchor::Source(entry.span),
        )?;
        check_retained_reconciliations(ctx.dag, &declaration, record.nominal_observations())?;
        let ExpressionFact::Value { checked_type, .. } = &record.fact else {
            return Err(GraphcalError::internal_error(
                "parameter default has no value checking result",
                ctx.src,
                DiagnosticAnchor::Source(default.expr.span),
            ));
        };
        let specialized = specialize_expression_type(checked_type, substitution, ctx.tir, ctx.src)?;
        let expected = ctx.declared_types.get(&entry.name).ok_or_else(|| {
            GraphcalError::internal_error(
                "instance parameter has no declared type",
                ctx.src,
                DiagnosticAnchor::Source(entry.span),
            )
        })?;
        if &specialized != expected {
            return Err(GraphcalError::DimensionMismatchInAnnotation {
                declared: expected.format(&ctx.registry.dimensions),
                inferred: specialized.format(&ctx.registry.dimensions),
                src: default.src.resolve(ctx.src).clone(),
                span: default.expr.span.into(),
            });
        }
    }
    Ok(())
}

enum FactSubstitution<'a> {
    Static(&'a crate::ir::instance::StaticSubstitution),
    Nat {
        scope: &'a HashMap<
            crate::syntax::type_name::GenericParamName,
            crate::hir::types::GenericParamId,
        >,
        bindings: &'a HashMap<crate::hir::types::GenericParamId, u64>,
    },
}

impl FactSubstitution<'_> {
    fn value_type(
        &self,
        ty: &DeclaredType,
        tir: &TIR,
        src: &NamedSource<Arc<String>>,
        span: crate::syntax::span::Span,
    ) -> Result<DeclaredType, GraphcalError> {
        match self {
            Self::Static(substitution) => specialize_expression_type(ty, substitution, tir, src),
            Self::Nat { scope, bindings } => bind_type_nats(ty, scope, bindings, src, span),
        }
    }

    fn index(
        &self,
        index: &crate::registry::declared_type::IndexTypeRef,
        src: &NamedSource<Arc<String>>,
        span: crate::syntax::span::Span,
    ) -> Result<crate::registry::declared_type::IndexTypeRef, GraphcalError> {
        match self {
            Self::Static(substitution) => Ok(specialize_index_ref(index, substitution)),
            Self::Nat { scope, bindings } => bind_index_nats(index, scope, bindings, src, span),
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
    let fact = match &record.fact {
        ExpressionFact::Contextual(kind) => ExpressionFact::Contextual(*kind),
        ExpressionFact::Value {
            checked_type,
            constructor,
            ..
        } => {
            let checked_type = substitution.value_type(checked_type, tir, src, span)?;
            let shape = checked_expression_shape(&checked_type, tir, src, span)?;
            let constructor = constructor
                .as_ref()
                .map(|application| {
                    let DeclaredType::Struct(_, args) = &checked_type else {
                        return Err(GraphcalError::internal_error(
                            "constructor specialization has no nominal result",
                            src,
                            DiagnosticAnchor::Source(span),
                        ));
                    };
                    Ok(Box::new(ConstructorApplication {
                        definition: application.definition.clone(),
                        runtime_type: dag.runtime_struct_type_identity(&application.definition),
                        constructor: application.constructor.clone(),
                        generic_args: args.clone(),
                        required_constraints: application.required_constraints.clone(),
                    }))
                })
                .transpose()?;
            ExpressionFact::Value {
                checked_type,
                shape,
                constructor,
            }
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
        nat_parameters: record.nat_parameters.clone(),
    }))
}

pub(super) fn install_instance_expression_facts(
    tir: &mut TIR,
    src: &NamedSource<Arc<String>>,
    cancellation: &CancellationToken,
) -> Result<(), GraphcalError> {
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
        let declared_types = dag.build_declared_types(src)?;
        let collector = infer::hir::ExpressionFactCollector::new(dag);
        let ctx = DimCheckContext {
            cancellation,
            expression_facts: &collector,
            declared_types: &declared_types,
            dag,
            tir,
            registry: &tir.registry,
            builtin_fns: crate::registry::builtins::builtin_functions(),
            src,
        };
        check_instance_defaults(&ctx, template, facts, &specialization.substitution)?;
        let mut records = collector.finish();
        let environment =
            CheckingEnvironment::new(dag.dag_id().clone(), dag.body_revision().clone());
        let mut inventory = Vec::new();
        dag.owned_expression_roots().for_each(|root| {
            visit_expr(root, &mut |expr| inventory.push(expr));
        });
        for expr in inventory {
            let span = expr.span;
            let id = expr.id().map_err(|error| {
                GraphcalError::internal_error(
                    error.to_string(),
                    src,
                    DiagnosticAnchor::Source(span),
                )
            })?;
            if records.contains_key(id) {
                continue;
            }
            let record = facts.get(id).map_err(|error| {
                GraphcalError::internal_error(
                    error.to_string(),
                    src,
                    DiagnosticAnchor::Source(span),
                )
            })?;
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
            .get_mut(&owner)
            .ok_or_else(|| {
                GraphcalError::internal_error(
                    "instance disappeared during publication",
                    src,
                    DiagnosticAnchor::WholeFile,
                )
            })?
            .semantic
            .expression_facts = Some(published);
    }
    Ok(())
}
