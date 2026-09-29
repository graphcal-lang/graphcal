//! Discharge concrete nominal applications using retained bound proofs.
//! This pass follows expression publication; it never infers bound source HIR.

use super::infer::hir::concrete_generic_substitutions;
use crate::cancellation::CancellationToken;
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::hir::nominal::{NominalConstructor, NominalTypeDef};
use crate::registry::checked_type::{
    CheckedGenericArg, CheckedType, IndexTypeRef, StructTypeRef, Symbolic,
};
use crate::registry::error::GraphcalError;
use crate::syntax::span::Span;
use crate::tir::expression_facts::{ExpressionFact, ValueFact};
use crate::tir::typed::model::{DagTIR, ResolvedStructFieldTypeKey, TIR};
use miette::NamedSource;
use std::sync::Arc;

#[derive(Clone, PartialEq, Eq)]
struct Application {
    identity: StructTypeRef,
    arguments: Vec<CheckedGenericArg<Symbolic>>,
}

struct Context<'a> {
    dag: &'a DagTIR,
    tir: &'a TIR,
    src: &'a NamedSource<Arc<String>>,
    span: Span,
    cancellation: &'a CancellationToken,
}

pub(super) fn validate_concrete_type_obligations(
    inferred: &CheckedType<Symbolic>,
    dag: &DagTIR,
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
    span: Span,
    cancellation: &CancellationToken,
) -> Result<(), GraphcalError> {
    validate(
        inferred,
        &Context {
            dag,
            tir,
            src,
            span,
            cancellation,
        },
        &mut Vec::new(),
    )
}

pub(super) fn validate_project(
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
    cancellation: &CancellationToken,
) -> Result<(), GraphcalError> {
    for (_, dag) in tir.local_dags() {
        for (_, annotation) in dag.value_decl_types() {
            validate_concrete_type_obligations(
                &annotation.checked().declared().to_symbolic(),
                dag,
                tir,
                src,
                annotation.span,
                cancellation,
            )?;
        }
        let facts = dag.expression_facts().map_err(|error| {
            GraphcalError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
        })?;
        for (id, record) in facts.records() {
            if let Some(value) = record.fact.symbolic_value()
                && value.constructor.is_some()
            {
                let span = facts.span(id).map_err(|error| {
                    GraphcalError::internal_error(
                        error.to_string(),
                        src,
                        DiagnosticAnchor::WholeFile,
                    )
                })?;
                validate_concrete_type_obligations(
                    &value.checked_type,
                    dag,
                    tir,
                    src,
                    span,
                    cancellation,
                )?;
            }
        }
    }
    Ok(())
}

fn validate(
    inferred: &CheckedType<Symbolic>,
    ctx: &Context<'_>,
    stack: &mut Vec<Application>,
) -> Result<(), GraphcalError> {
    ctx.cancellation.checkpoint()?;
    super::expression_axes::check_materializable(inferred, ctx.tir, ctx.src, ctx.span)?;
    match inferred {
        CheckedType::Struct(identity, arguments) => {
            for arg in arguments {
                if let CheckedGenericArg::Type(ty) = arg {
                    validate(ty, ctx, stack)?;
                }
            }
            let definition = ctx
                .tir
                .struct_type_def(identity.resolved())
                .ok_or_else(|| GraphcalError::UnknownStructType {
                    name: identity.to_string(),
                    src: ctx.src.clone(),
                    span: ctx.span.into(),
                })?;
            let application = Application {
                identity: identity.clone(),
                arguments: arguments.clone(),
            };
            let substitutions =
                concrete_generic_substitutions(definition, arguments, ctx.src, ctx.span)?;
            if let Some(ancestor) = stack.iter().find(|ancestor| ancestor.identity == *identity) {
                if ancestor == &application {
                    return Ok(());
                }
                return Err(GraphcalError::EvalError {
                    message: format!(
                        "recursive generic type `{identity}` changes its arguments; concrete field obligations cannot be discharged finitely"
                    ),
                    src: ctx.src.clone(),
                    span: ctx.span.into(),
                });
            }
            stack.push(application);
            for member in definition.union_members().into_iter().flatten() {
                for field in member.fields() {
                    ctx.cancellation.checkpoint()?;
                    let key = ResolvedStructFieldTypeKey {
                        owning_type: identity.resolved().clone(),
                        constructor: member.name(),
                        field: field.name().clone(),
                    };
                    let semantics = ctx.dag.semantic.type_defs.field(&key).ok_or_else(|| {
                        GraphcalError::internal_error(
                            format!(
                                "semantic type metadata missing field `{}.{}`",
                                member.name(),
                                field.name()
                            ),
                            ctx.src,
                            DiagnosticAnchor::Source(ctx.span),
                        )
                    })?;
                    let ty = substitutions.field_type(semantics.resolved_type(), ctx.src)?;
                    for bound in semantics.domain_bounds() {
                        check_bound(
                            &key,
                            definition,
                            member,
                            bound,
                            &ty,
                            substitutions.nats(),
                            ctx,
                        )?;
                    }
                    validate(&ty.to_symbolic(), ctx, stack)?;
                }
            }
            stack.pop();
            Ok(())
        }
        CheckedType::Indexed { element, index } => {
            validate_index(index, ctx)?;
            validate(element, ctx, stack)
        }
        CheckedType::Key(index) => validate_index(index, ctx),
        CheckedType::Quantity(_)
        | CheckedType::Complex(_)
        | CheckedType::Bool
        | CheckedType::Int
        | CheckedType::Datetime(_) => Ok(()),
    }
}

fn validate_index(index: &IndexTypeRef<Symbolic>, ctx: &Context<'_>) -> Result<(), GraphcalError> {
    match index.to_concrete() {
        Some(_) => Ok(()),
        None => Err(GraphcalError::EvalError {
            message: format!("unresolved finite-index obligation `{index}`"),
            src: ctx.src.clone(),
            span: ctx.span.into(),
        }),
    }
}

fn check_bound(
    key: &ResolvedStructFieldTypeKey,
    definition: &NominalTypeDef,
    member: &NominalConstructor,
    bound: &crate::tir::typed::ResolvedDomainBound,
    target: &CheckedType,
    nats: &std::collections::HashMap<crate::hir::types::GenericParamId, u64>,
    ctx: &Context<'_>,
) -> Result<(), GraphcalError> {
    let expected = super::expected_bound_from_inferred(target).ok_or_else(|| {
        GraphcalError::InvalidDomainTarget {
            type_kind: super::format_checked_type(target, &ctx.tir.registry),
            src: bound.src.clone(),
            span: bound.span.into(),
        }
    })?;
    let display = if member.name().as_str() == definition.name().as_str() {
        format!("{}.{}", definition.name(), key.field)
    } else {
        format!("{}.{}.{}", definition.name(), member.name(), key.field)
    };
    let owner = ctx
        .tir
        .dag_registry()
        .get(key.owning_type.owner())
        .ok_or_else(|| {
            GraphcalError::internal_error(
                "field-constraint owner has no checked DAG",
                &bound.src,
                DiagnosticAnchor::Source(bound.span),
            )
        })?;
    let facts = super::expression_facts::specialize_bound_expression_facts(
        ctx.tir,
        owner,
        &bound.value,
        nats,
        &bound.src,
    )?;
    let id = bound.value.id();
    let record = facts.executable_value(id).map_err(|error| {
        GraphcalError::internal_error(
            error.to_string(),
            &bound.src,
            DiagnosticAnchor::Source(bound.span),
        )
    })?;
    let ExpressionFact::Executable(ValueFact { checked_type, .. }) = &record.fact else {
        return Err(GraphcalError::internal_error(
            "bound has no retained value type",
            &bound.src,
            DiagnosticAnchor::Source(bound.span),
        ));
    };
    super::check_one_bound_with_display_name(
        &display,
        bound,
        checked_type,
        &expected,
        &ctx.tir.registry,
        &bound.src,
    )
}
