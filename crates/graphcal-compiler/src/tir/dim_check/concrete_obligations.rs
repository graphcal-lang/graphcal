//! Discharge concrete nominal applications using retained bound proofs.
//! This pass follows expression publication; it never infers bound source HIR.

use super::generic_substitution::{concrete_generic_substitutions, recorded_nominal};
use crate::cancellation::CancellationToken;
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::outcome::Outcome;
use crate::semantic::checked_type::{
    CheckedGenericArg, CheckedType, IndexTypeRef, StructTypeRef, Symbolic,
};
use crate::semantic_error::SemanticError;
use crate::semantic_error::domain::DomainError;
use crate::semantic_error::evaluation::EvaluationError;
use crate::semantic_error::index::IndexError;
use crate::semantic_error::structure::StructError;
use crate::source_id::SourceId;
use crate::syntax::span::Span;
use crate::tir::texpr::{CheckedBody, TBody, TNodeRef, visit_tnodes};
use crate::tir::typed::model::DagTIR;
use crate::tir::typed::program::TirRead;
use crate::tir::typed::{NominalFieldSemantics, NominalMember};

#[derive(Clone, PartialEq, Eq)]
struct Application {
    identity: StructTypeRef,
    arguments: Vec<CheckedGenericArg<Symbolic>>,
}

struct Context<'a> {
    dag: &'a DagTIR,
    tir: &'a dyn TirRead,
    src: SourceId,
    span: Span,
    cancellation: &'a CancellationToken,
}

pub(super) fn validate_concrete_type_obligations(
    inferred: &CheckedType<Symbolic>,
    dag: &DagTIR,
    tir: &dyn TirRead,
    src: SourceId,
    span: Span,
    cancellation: &CancellationToken,
) -> Result<(), Outcome<SemanticError>> {
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
    checking: &crate::tir::typed::checking_tir::CheckingTir<'_>,
    src: SourceId,
    cancellation: &CancellationToken,
) -> Result<(), Outcome<SemanticError>> {
    let tir: &dyn TirRead = checking;
    for (dag, bodies) in checking.tir.dags.with_local_facts(checking.bodies) {
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
) -> Result<(), Outcome<SemanticError>> {
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
                .ok_or_else(|| {
                    SemanticError::located(
                        ctx.src,
                        ctx.span,
                        StructError::UnknownStructType {
                            name: identity.to_string(),
                        },
                    )
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
                return Err(SemanticError::located(ctx.src, ctx.span, EvaluationError::Failed { message: format!(
                        "recursive generic type `{identity}` changes its arguments; concrete field obligations cannot be discharged finitely"
                    ) }).into());
            }
            stack.push(application);
            // A type whose constructors have no fields needs no field
            // semantics.
            let has_fields = definition
                .union_members()
                .into_iter()
                .flatten()
                .any(|member| !member.fields().is_empty());
            if has_fields {
                let nominal = recorded_nominal(ctx.dag, identity.resolved(), ctx.src, ctx.span)?;
                for field in nominal.members().flat_map(NominalMember::fields) {
                    ctx.cancellation.checkpoint()?;
                    let ty =
                        substitutions.field_type(field.semantics().resolved_type(), ctx.src)?;
                    for bound in field.semantics().domain_bounds().into_iter().flatten() {
                        check_bound(field, bound, &ty, substitutions.nats(), ctx)?;
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
        CheckedType::Key(index) => Ok(validate_index(index, ctx)?),
        CheckedType::Quantity(_)
        | CheckedType::Complex(_)
        | CheckedType::Bool
        | CheckedType::Int
        | CheckedType::Datetime(_) => Ok(()),
    }
}

fn validate_index(index: &IndexTypeRef<Symbolic>, ctx: &Context<'_>) -> Result<(), SemanticError> {
    match index.to_concrete() {
        Some(_) => Ok(()),
        None => Err(SemanticError::located(
            ctx.src,
            ctx.span,
            IndexError::UnresolvedFiniteIndexObligation {
                index: index.display_name(),
            },
        )),
    }
}

fn check_bound(
    field: NominalFieldSemantics<'_>,
    bound: &crate::tir::typed::ResolvedDomainBound,
    target: &CheckedType,
    nats: &std::collections::HashMap<crate::hir::types::GenericParamId, u64>,
    ctx: &Context<'_>,
) -> Result<(), SemanticError> {
    let expected =
        super::domain_bound_type::expected_bound_from_inferred(target).ok_or_else(|| {
            SemanticError::located(
                bound.src,
                bound.span,
                DomainError::InvalidDomainTarget {
                    type_kind: super::format_checked_type(target, ctx.tir.registry()),
                },
            )
        })?;
    let display = field.display_name();
    let owning_dag = field.member().nominal().identity().owner();
    let (Some(owner), Some(bodies)) = (ctx.tir.dag(owning_dag), ctx.tir.checked_bodies(owning_dag))
    else {
        return Err(SemanticError::internal_error(
            "field-constraint owner has no checked DAG",
            bound.src,
            DiagnosticAnchor::Source(bound.span),
        ));
    };
    let tree = super::body_specialization::specialize_bound_body(
        ctx.tir,
        owner,
        bodies,
        &bound.value,
        nats,
        bound.src,
    )?;
    super::domain_bound_type::check_one_bound_with_display_name(
        &display,
        bound,
        tree.ty(),
        &expected,
        ctx.tir.registry(),
        bound.src,
    )
}
