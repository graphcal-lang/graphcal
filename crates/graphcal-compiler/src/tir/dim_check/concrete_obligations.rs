//! Discharge concrete nominal applications using retained bound proofs.
//! This pass follows expression publication; it never infers bound source HIR.

use super::generic_substitution::concrete_generic_substitutions;
use crate::cancellation::CancellationToken;
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::graphcal_error::GraphcalError;
use crate::hir::nominal::{NominalConstructor, NominalTypeDef};
use crate::semantic::checked_type::{
    CheckedGenericArg, CheckedType, IndexTypeRef, StructTypeRef, Symbolic,
};
use crate::syntax::span::Span;
use crate::tir::texpr::{CheckedBody, TBody, TNodeRef, visit_tnodes};
use crate::tir::typed::model::{DagTIR, ResolvedStructFieldTypeKey};
use crate::tir::typed::program::TirRead;
use miette::NamedSource;
use std::sync::Arc;

#[derive(Clone, PartialEq, Eq)]
struct Application {
    identity: StructTypeRef,
    arguments: Vec<CheckedGenericArg<Symbolic>>,
}

struct Context<'a> {
    dag: &'a DagTIR,
    tir: &'a dyn TirRead,
    src: &'a NamedSource<Arc<String>>,
    span: Span,
    cancellation: &'a CancellationToken,
}

pub(super) fn validate_concrete_type_obligations(
    inferred: &CheckedType<Symbolic>,
    dag: &DagTIR,
    tir: &dyn TirRead,
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
    checking: &crate::tir::typed::CheckingTir<'_>,
    src: &NamedSource<Arc<String>>,
    cancellation: &CancellationToken,
) -> Result<(), GraphcalError> {
    let tir: &dyn TirRead = checking;
    for (dag_id, dag) in checking.tir.local_dags() {
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
        let bodies = checking.bodies.get(dag_id).ok_or_else(|| {
            GraphcalError::internal_error(
                format!("DAG `{dag_id}` has no published typed bodies"),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
        for (_, body) in bodies.roots() {
            for (checked_type, span) in constructor_applications(body) {
                validate_concrete_type_obligations(
                    &checked_type,
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

/// The type and span of every constructor application in a checked tree, in
/// pre-order.
fn constructor_applications(body: &CheckedBody) -> Vec<(CheckedType<Symbolic>, Span)> {
    fn collect<V: crate::tir::texpr::map::SymbolicView>(
        body: &TBody<V>,
    ) -> Vec<(CheckedType<Symbolic>, Span)> {
        let mut applications = Vec::new();
        visit_tnodes(body.as_node(), &mut |node| {
            if let TNodeRef::Value(expr) = node
                && expr.application().is_some()
            {
                applications.push((V::symbolic_type(expr.ty()).into_owned(), expr.span()));
            }
        });
        applications
    }
    match body {
        CheckedBody::Executable(body) => collect(body),
        CheckedBody::Deferred(body) => collect(body),
    }
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
    let expected =
        super::domain_bound_type::expected_bound_from_inferred(target).ok_or_else(|| {
            GraphcalError::InvalidDomainTarget {
                type_kind: super::format_checked_type(target, ctx.tir.registry()),
                src: bound.src.clone(),
                span: bound.span.into(),
            }
        })?;
    let display = if member.name().as_str() == definition.name().as_str() {
        format!("{}.{}", definition.name(), key.field)
    } else {
        format!("{}.{}.{}", definition.name(), member.name(), key.field)
    };
    let (Some(owner), Some(bodies)) = (
        ctx.tir.dag(key.owning_type.owner()),
        ctx.tir.checked_bodies(key.owning_type.owner()),
    ) else {
        return Err(GraphcalError::internal_error(
            "field-constraint owner has no checked DAG",
            &bound.src,
            DiagnosticAnchor::Source(bound.span),
        ));
    };
    let tree = super::body_specialization::specialize_bound_body(
        ctx.tir,
        owner,
        bodies,
        &bound.value,
        nats,
        &bound.src,
    )?;
    super::domain_bound_type::check_one_bound_with_display_name(
        &display,
        bound,
        tree.ty(),
        &expected,
        ctx.tir.registry(),
        &bound.src,
    )
}
