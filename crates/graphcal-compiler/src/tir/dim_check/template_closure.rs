//! Option A template-body closure validation.

use std::sync::Arc;

use miette::NamedSource;

use super::{DimCheckContext, InferredType, check_decl_expr_type, check_hir_assert_body, infer};
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::hir;
use crate::registry::error::GraphcalError;
use crate::registry::resolve_types::DeclarationKind;
use crate::static_interface::StaticRole;
use crate::syntax::decl_name::ResolvedDeclName;
use crate::syntax::names::NameAtom;
use crate::syntax::span::Span;
use crate::syntax::type_name::ResolvedStructTypeName;
use crate::tir::template_closure::{
    StaticDependency, StaticUseContext, TemplateClosureCheck, validate,
};

#[derive(Clone)]
struct TemplateBodyIdentity {
    kind: DeclarationKind,
    name: NameAtom,
}

fn optional_type_port<'a>(
    dag: &'a crate::tir::typed::DagTIR,
    identity: &ResolvedStructTypeName,
) -> Option<&'a crate::hir::StaticPort> {
    dag.static_ports().iter().find(|port| {
        port.role == StaticRole::OptionalInput
            && matches!(
                &port.identity,
                crate::hir::StaticPortIdentity::Type(port_identity)
                    if port_identity == identity
            )
    })
}

fn constructor_owner<'a>(
    ctx: &'a DimCheckContext<'_>,
    constructor: &crate::syntax::type_name::ResolvedConstructorName,
    span: Span,
) -> Result<&'a ResolvedStructTypeName, GraphcalError> {
    ctx.dag
        .semantic
        .constructor_refs
        .constructor_defs
        .get(constructor)
        .map(|target| &target.owning_type)
        .ok_or_else(|| GraphcalError::InternalError {
            message: format!(
                "template-closure validation is missing constructor metadata for `{constructor}`"
            ),
            src: ctx.src.clone(),
            span: span.into(),
        })
}

fn infer_operand(
    ctx: &DimCheckContext<'_>,
    owner: Option<&ResolvedDeclName>,
    expr: &hir::Expr,
) -> Result<InferredType, GraphcalError> {
    infer::hir::infer_hir_type_with_materialized_shapes_and_cancellation(
        expr,
        owner,
        ctx.declared_types,
        ctx.dag,
        ctx.tir,
        ctx.registry,
        ctx.builtin_fns,
        ctx.src,
        ctx.cancellation,
        ctx.materialized_shapes.clone(),
    )
}

fn emit_violation(
    ctx: &DimCheckContext<'_>,
    body: &TemplateBodyIdentity,
    port: &crate::hir::StaticPort,
    span: Span,
) -> Result<(), GraphcalError> {
    let check = TemplateClosureCheck {
        kind: port.identity.kind(),
        role: port.role,
        context: StaticUseContext::TemplateBody,
        dependency: StaticDependency::DefaultDefinition,
    };
    let violation =
        validate(check).map_err(
            |violation| GraphcalError::TemplateBodyDependsOnStaticDefault {
                body_kind: body.kind,
                body_name: body.name.clone(),
                port_kind: violation.kind,
                port_name: port.identity.name().clone(),
                src: ctx.src.clone(),
                span: span.into(),
            },
        );
    match violation {
        Ok(()) => Ok(()),
        Err(error) => Err(error),
    }
}

fn check_expr(
    ctx: &DimCheckContext<'_>,
    owner: Option<&ResolvedDeclName>,
    body: &TemplateBodyIdentity,
    expr: &hir::Expr,
) -> Result<(), GraphcalError> {
    let mut result = Ok(());
    hir::visit_expr(expr, &mut |candidate| {
        if result.is_err() {
            return;
        }
        let use_site = match &candidate.kind {
            hir::ExprKind::FieldAccess {
                expr: operand,
                field,
            } => match infer_operand(ctx, owner, operand) {
                Ok(InferredType::Struct(identity, _)) => {
                    optional_type_port(ctx.dag, identity.resolved()).map(|port| (port, field.span))
                }
                Ok(_) => None,
                Err(error) => {
                    result = Err(error);
                    None
                }
            },
            hir::ExprKind::ConstructorCall { callee, .. } => {
                match constructor_owner(ctx, &callee.value, callee.span) {
                    Ok(identity) => {
                        optional_type_port(ctx.dag, identity).map(|port| (port, callee.span))
                    }
                    Err(error) => {
                        result = Err(error);
                        None
                    }
                }
            }
            hir::ExprKind::ConstRef(target) => match &target.value {
                hir::ConstRef::Constructor(constructor) => {
                    match constructor_owner(ctx, constructor, target.span) {
                        Ok(identity) => {
                            optional_type_port(ctx.dag, identity).map(|port| (port, target.span))
                        }
                        Err(error) => {
                            result = Err(error);
                            None
                        }
                    }
                }
                _ => None,
            },
            hir::ExprKind::Match { arms, .. } => arms.iter().find_map(|arm| {
                let hir::expr::MatchPattern::Constructor { constructor, .. } = &arm.pattern else {
                    return None;
                };
                match constructor_owner(ctx, &constructor.value, constructor.span) {
                    Ok(identity) => {
                        optional_type_port(ctx.dag, identity).map(|port| (port, constructor.span))
                    }
                    Err(error) => {
                        result = Err(error);
                        None
                    }
                }
            }),
            _ => None,
        };
        if let Some((port, span)) = use_site {
            result = emit_violation(ctx, body, port, span);
        }
    });
    result
}

fn local_owner(
    dag: &crate::tir::typed::DagTIR,
    name: &crate::syntax::module_name::ScopedName,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<Option<ResolvedDeclName>, GraphcalError> {
    let owner = dag.require_bound_decl_identity(name, src, DiagnosticAnchor::Source(span))?;
    Ok((owner.owner() == dag.dag_id()).then_some(owner))
}

fn rigid_dimension_error(
    ctx: &DimCheckContext<'_>,
    body: TemplateBodyIdentity,
    port: &crate::hir::StaticPort,
    span: Span,
    result: Result<(), GraphcalError>,
) -> Result<(), GraphcalError> {
    match result {
        Ok(()) => Ok(()),
        Err(error @ GraphcalError::Cancelled(_)) => Err(error),
        Err(_) => emit_violation(ctx, &body, port, span),
    }
}

fn check_rigid_dimension_port(
    ctx: &DimCheckContext<'_>,
    port: &crate::hir::StaticPort,
    dimension: &crate::syntax::dimension::ResolvedDimName,
) -> Result<(), GraphcalError> {
    let rigid_tir =
        crate::tir::typed::rigid_dimension_view(ctx.tir, ctx.dag.dag_id(), dimension, ctx.src)?;
    let rigid_dag = rigid_tir.dags.get(ctx.dag.dag_id()).ok_or_else(|| {
        GraphcalError::internal_error(
            format!("rigid template DAG `{}` is unavailable", ctx.dag.dag_id()),
            ctx.src,
            DiagnosticAnchor::WholeFile,
        )
    })?;
    let declared_types = rigid_dag.build_declared_types(ctx.src)?;
    let materialized_shapes = infer::hir::MaterializedShapeCollector::default();
    let rigid_ctx = DimCheckContext {
        cancellation: ctx.cancellation,
        materialized_shapes: &materialized_shapes,
        declared_types: &declared_types,
        dag: rigid_dag,
        tir: &rigid_tir,
        registry: &rigid_tir.registry,
        builtin_fns: ctx.builtin_fns,
        src: ctx.src,
    };

    for (kind, name, annotation_span, body_span, body_src) in rigid_dag
        .consts
        .iter()
        .map(|entry| {
            (
                DeclarationKind::ConstNode,
                &entry.name,
                entry.type_ann.span,
                entry.expr.span,
                entry.body_src.resolve(ctx.src),
            )
        })
        .chain(rigid_dag.nodes.iter().map(|entry| {
            (
                DeclarationKind::Node,
                &entry.name,
                entry.type_ann.span,
                entry.expr.span,
                entry.body_src.resolve(ctx.src),
            )
        }))
    {
        let Some(_) = local_owner(rigid_dag, name, body_src, annotation_span)? else {
            continue;
        };
        let body_ctx = rigid_ctx.for_body(body_src);
        rigid_dimension_error(
            &body_ctx,
            TemplateBodyIdentity {
                kind,
                name: name.member().atom().clone(),
            },
            port,
            body_span,
            check_decl_expr_type(&body_ctx, name, &annotation_span, body_src),
        )?;
    }

    for entry in &rigid_dag.asserts {
        let body_src = entry.body_src.resolve(ctx.src);
        let Some(owner) = local_owner(rigid_dag, &entry.name, body_src, entry.span)? else {
            continue;
        };
        let body_ctx = rigid_ctx.for_body(body_src);
        let body = body_ctx.hir_assert_body(&entry.name, entry.span)?;
        rigid_dimension_error(
            &body_ctx,
            TemplateBodyIdentity {
                kind: DeclarationKind::Assert,
                name: entry.name.member().atom().clone(),
            },
            port,
            entry.span,
            check_hir_assert_body(&body_ctx, &owner, body, entry.span).map(|_| ()),
        )?;
    }

    Ok(())
}

fn check_rigid_dimensions(ctx: &DimCheckContext<'_>) -> Result<(), GraphcalError> {
    ctx.dag
        .static_ports()
        .iter()
        .filter(|port| port.role == StaticRole::OptionalInput)
        .filter_map(|port| match &port.identity {
            crate::hir::StaticPortIdentity::Dimension(identity) => Some((port, identity)),
            crate::hir::StaticPortIdentity::Type(_) | crate::hir::StaticPortIdentity::Index(_) => {
                None
            }
        })
        .try_for_each(|(port, identity)| check_rigid_dimension_port(ctx, port, identity))
}

/// Validate every source-authored executable body in one reusable DAG.
pub(super) fn check_template_body_closure(ctx: &DimCheckContext<'_>) -> Result<(), GraphcalError> {
    check_rigid_dimensions(ctx)?;
    for (kind, name, expr, body_src, span) in ctx
        .dag
        .consts
        .iter()
        .map(|entry| {
            (
                DeclarationKind::ConstNode,
                &entry.name,
                &entry.expr,
                entry.body_src.resolve(ctx.src),
                entry.span,
            )
        })
        .chain(ctx.dag.nodes.iter().map(|entry| {
            (
                DeclarationKind::Node,
                &entry.name,
                &entry.expr,
                entry.body_src.resolve(ctx.src),
                entry.span,
            )
        }))
    {
        ctx.checkpoint()?;
        let Some(owner) = local_owner(ctx.dag, name, body_src, span)? else {
            continue;
        };
        let body_ctx = ctx.for_body(body_src);
        check_expr(
            &body_ctx,
            Some(&owner),
            &TemplateBodyIdentity {
                kind,
                name: name.member().atom().clone(),
            },
            expr,
        )?;
    }

    for entry in &ctx.dag.asserts {
        ctx.checkpoint()?;
        let body_src = entry.body_src.resolve(ctx.src);
        let Some(owner) = local_owner(ctx.dag, &entry.name, body_src, entry.span)? else {
            continue;
        };
        let body_ctx = ctx.for_body(body_src);
        let identity = TemplateBodyIdentity {
            kind: DeclarationKind::Assert,
            name: entry.name.member().atom().clone(),
        };
        match &*entry.body {
            hir::AssertBody::Expr(expr) => check_expr(&body_ctx, Some(&owner), &identity, expr)?,
            hir::AssertBody::Tolerance {
                actual,
                expected,
                tolerance,
            } => {
                check_expr(&body_ctx, Some(&owner), &identity, actual)?;
                check_expr(&body_ctx, Some(&owner), &identity, expected)?;
                check_expr(&body_ctx, Some(&owner), &identity, tolerance)?;
            }
        }
    }

    for entry in &ctx.dag.plots {
        let body_src = entry.body_src.resolve(ctx.src);
        let Some(owner) = local_owner(ctx.dag, &entry.name, body_src, Span::new(0, 0))? else {
            continue;
        };
        let body_ctx = ctx.for_body(body_src);
        let identity = TemplateBodyIdentity {
            kind: DeclarationKind::Plot,
            name: entry.name.member().atom().clone(),
        };
        for expr in entry
            .body
            .encodings
            .iter()
            .map(|(_, expr)| expr)
            .chain(entry.body.mark_properties.iter().map(|field| &field.value))
            .chain(entry.body.properties.iter().map(|field| &field.value))
        {
            check_expr(&body_ctx, Some(&owner), &identity, expr)?;
        }
    }

    for (kind, entry_name, fields, body_src, span) in ctx
        .dag
        .figures
        .iter()
        .map(|entry| {
            (
                DeclarationKind::Figure,
                &entry.name,
                entry.fields.as_slice(),
                entry.body_src.resolve(ctx.src),
                Span::new(0, 0),
            )
        })
        .chain(ctx.dag.layers.iter().map(|entry| {
            (
                DeclarationKind::Layer,
                &entry.name,
                entry.fields.as_slice(),
                entry.body_src.resolve(ctx.src),
                Span::new(0, 0),
            )
        }))
    {
        let Some(owner) = local_owner(ctx.dag, entry_name, body_src, span)? else {
            continue;
        };
        let body_ctx = ctx.for_body(body_src);
        let identity = TemplateBodyIdentity {
            kind,
            name: entry_name.member().atom().clone(),
        };
        for field in fields {
            check_expr(&body_ctx, Some(&owner), &identity, &field.value)?;
        }
    }

    for entry in ctx.dag.semantic.dynamic_unit_scales.values() {
        if entry.unit.owner() != ctx.dag.dag_id() {
            continue;
        }
        let body_ctx = ctx.for_body(&entry.src);
        check_expr(
            &body_ctx,
            None,
            &TemplateBodyIdentity {
                kind: DeclarationKind::Unit,
                name: entry.unit.atom().clone(),
            },
            &entry.expr,
        )?;
    }

    Ok(())
}
