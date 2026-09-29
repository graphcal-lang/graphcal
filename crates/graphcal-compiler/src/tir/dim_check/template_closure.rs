//! Option A template-body closure validation.

use super::{DimCheckContext, InferredType, check_decl_expr_type, check_hir_assert_body, infer};
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::hir;
use crate::registry::error::GraphcalError;
use crate::registry::resolve_types::DeclarationKind;
use crate::resolved_name::{ResolvedDeclName, ResolvedStructTypeName};
use crate::static_interface::StaticRole;
use crate::syntax::names::NameAtom;
use crate::syntax::span::Span;
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

fn infer_operand(
    ctx: &DimCheckContext<'_>,
    owner: Option<&ResolvedDeclName>,
    expr: &hir::Expr,
) -> Result<InferredType, GraphcalError> {
    ctx.infer_hir(expr, owner)
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
                src: ctx.env.src.clone(),
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
    let dependencies =
        ctx.env
            .collect_type_definition_dependencies(expr, owner, ctx.cancellation)?;
    dependencies.into_iter().try_for_each(|dependency| {
        optional_type_port(ctx.env.dag, dependency.identity()).map_or(Ok(()), |port| {
            emit_violation(ctx, body, port, dependency.span())
        })
    })
}

/// The declaration identity when this DAG authored it.
fn local_owner(
    ctx: &DimCheckContext<'_>,
    declaration: ResolvedDeclName,
) -> Option<ResolvedDeclName> {
    (declaration.owner() == ctx.env.dag.dag_id()).then_some(declaration)
}

fn rigid_dimension_error(
    ctx: &DimCheckContext<'_>,
    body: &TemplateBodyIdentity,
    port: &crate::hir::StaticPort,
    span: Span,
    result: Result<(), GraphcalError>,
) -> Result<(), GraphcalError> {
    match result {
        Ok(()) => Ok(()),
        Err(error @ GraphcalError::Cancelled(_)) => Err(error),
        Err(_) => emit_violation(ctx, body, port, span),
    }
}

fn check_rigid_plot_field(
    ctx: &DimCheckContext<'_>,
    owner: &ResolvedDeclName,
    body: &TemplateBodyIdentity,
    port: &crate::hir::StaticPort,
    field: &crate::ir::lower::LoweredPlotField,
) -> Result<(), GraphcalError> {
    let (property, expected) = match &field.property {
        crate::ir::lower::LoweredPlotProperty::Mark(property) => {
            (property.name(), property.value_type())
        }
        crate::ir::lower::LoweredPlotProperty::Plot(property) => {
            (property.name(), property.value_type())
        }
        crate::ir::lower::LoweredPlotProperty::Composition(property) => {
            (property.name(), property.value_type())
        }
        crate::ir::lower::LoweredPlotProperty::Unknown(property) => {
            return Err(GraphcalError::internal_error(
                format!("unchecked plot property `{property}` reached rigid validation"),
                ctx.env.src,
                DiagnosticAnchor::Source(field.name_span),
            ));
        }
    };
    rigid_dimension_error(
        ctx,
        body,
        port,
        field.value.span,
        super::plot::check_property_value(ctx, owner, property, expected, field),
    )
}

fn check_rigid_value_bodies(
    ctx: &DimCheckContext<'_>,
    port: &crate::hir::StaticPort,
) -> Result<(), GraphcalError> {
    for (kind, name, declaration, annotation, body_span) in ctx
        .env
        .dag
        .consts()
        .map(|entry| {
            (
                DeclarationKind::ConstNode,
                &entry.name,
                entry.identity(),
                &entry.type_ann,
                entry.expr.span,
            )
        })
        .chain(ctx.env.dag.nodes().filter_map(|entry| {
            entry.definition.formula().map(|expression| {
                (
                    DeclarationKind::Node,
                    &entry.name,
                    entry.identity(),
                    &entry.type_ann,
                    expression.span,
                )
            })
        }))
    {
        let Some(declaration) = local_owner(ctx, declaration) else {
            continue;
        };
        let body = TemplateBodyIdentity {
            kind,
            name: name.atom().clone(),
        };
        rigid_dimension_error(
            ctx,
            &body,
            port,
            body_span,
            check_decl_expr_type(ctx, name, &declaration, annotation),
        )?;
    }
    Ok(())
}

fn check_rigid_assertion_bodies(
    ctx: &DimCheckContext<'_>,
    port: &crate::hir::StaticPort,
) -> Result<(), GraphcalError> {
    for entry in ctx.env.dag.asserts() {
        let Some(owner) = local_owner(ctx, entry.identity()) else {
            continue;
        };
        let assertion = ctx.hir_assert_body(&entry.name, &owner, entry.span)?;
        let body = TemplateBodyIdentity {
            kind: DeclarationKind::Assert,
            name: entry.name.atom().clone(),
        };
        rigid_dimension_error(
            ctx,
            &body,
            port,
            entry.span,
            check_hir_assert_body(ctx, &owner, assertion, entry.span).map(|_| ()),
        )?;
    }
    Ok(())
}

fn check_rigid_plot_bodies(
    ctx: &DimCheckContext<'_>,
    port: &crate::hir::StaticPort,
) -> Result<(), GraphcalError> {
    for entry in ctx.env.dag.plots() {
        let Some(owner) = local_owner(ctx, entry.identity()) else {
            continue;
        };
        let body = TemplateBodyIdentity {
            kind: DeclarationKind::Plot,
            name: entry.name.atom().clone(),
        };
        for (_, expression) in &entry.body.encodings {
            rigid_dimension_error(
                ctx,
                &body,
                port,
                expression.span,
                infer_operand(ctx, Some(&owner), expression).map(|_| ()),
            )?;
        }
        for field in entry
            .body
            .mark_properties
            .iter()
            .chain(&entry.body.properties)
        {
            check_rigid_plot_field(ctx, &owner, &body, port, field)?;
        }
    }
    Ok(())
}

fn check_rigid_composition_bodies(
    ctx: &DimCheckContext<'_>,
    port: &crate::hir::StaticPort,
) -> Result<(), GraphcalError> {
    for (kind, name, declaration, fields) in ctx
        .env
        .dag
        .figures()
        .map(|entry| {
            (
                DeclarationKind::Figure,
                &entry.name,
                entry.identity(),
                entry.fields.as_slice(),
            )
        })
        .chain(ctx.env.dag.layers().map(|entry| {
            (
                DeclarationKind::Layer,
                &entry.name,
                entry.identity(),
                entry.fields.as_slice(),
            )
        }))
    {
        let Some(owner) = local_owner(ctx, declaration) else {
            continue;
        };
        let body = TemplateBodyIdentity {
            kind,
            name: name.atom().clone(),
        };
        for field in fields {
            check_rigid_plot_field(ctx, &owner, &body, port, field)?;
        }
    }
    Ok(())
}

fn check_rigid_unit_bodies(
    ctx: &DimCheckContext<'_>,
    port: &crate::hir::StaticPort,
) -> Result<(), GraphcalError> {
    for entry in ctx.env.dag.semantic.dynamic_unit_scales.values() {
        if entry.unit.owner() != ctx.env.dag.dag_id() {
            continue;
        }
        let body = TemplateBodyIdentity {
            kind: DeclarationKind::Unit,
            name: entry.unit.atom().clone(),
        };
        rigid_dimension_error(
            ctx,
            &body,
            port,
            entry.expr.span,
            super::check_dynamic_unit_scale_type(ctx, entry),
        )?;
    }
    Ok(())
}

fn check_rigid_dimension_port(
    ctx: &DimCheckContext<'_>,
    port: &crate::hir::StaticPort,
    dimension: &crate::resolved_name::ResolvedDimName,
) -> Result<(), GraphcalError> {
    let rigid_tir = crate::tir::typed::rigid_dimension_view(
        ctx.env.tir,
        ctx.env.dag.dag_id(),
        dimension,
        ctx.env.src,
    )?;
    let rigid_dag = rigid_tir.dags.get(ctx.env.dag.dag_id()).ok_or_else(|| {
        GraphcalError::internal_error(
            format!(
                "rigid template DAG `{}` is unavailable",
                ctx.env.dag.dag_id()
            ),
            ctx.env.src,
            DiagnosticAnchor::WholeFile,
        )
    })?;
    let expression_facts = infer::hir::ExpressionFactCollector::new(rigid_dag);
    let rigid_ctx = DimCheckContext {
        env: infer::hir::InferEnv {
            dag: rigid_dag,
            tir: &rigid_tir,
            registry: &rigid_tir.registry,
            src: ctx.env.src,
        },
        cancellation: ctx.cancellation,
        expression_facts: &expression_facts,
    };
    check_rigid_value_bodies(&rigid_ctx, port)?;
    check_rigid_assertion_bodies(&rigid_ctx, port)?;
    check_rigid_plot_bodies(&rigid_ctx, port)?;
    check_rigid_composition_bodies(&rigid_ctx, port)?;
    check_rigid_unit_bodies(&rigid_ctx, port)
}

fn check_rigid_dimensions(ctx: &DimCheckContext<'_>) -> Result<(), GraphcalError> {
    ctx.env
        .dag
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

fn check_template_value_bodies(ctx: &DimCheckContext<'_>) -> Result<(), GraphcalError> {
    for (kind, name, declaration, expr) in ctx
        .env
        .dag
        .consts()
        .map(|entry| {
            (
                DeclarationKind::ConstNode,
                &entry.name,
                entry.identity(),
                &entry.expr,
            )
        })
        .chain(ctx.env.dag.nodes().filter_map(|entry| {
            entry.definition.formula().map(|expression| {
                (
                    DeclarationKind::Node,
                    &entry.name,
                    entry.identity(),
                    expression,
                )
            })
        }))
    {
        ctx.checkpoint()?;
        let Some(owner) = local_owner(ctx, declaration) else {
            continue;
        };
        let identity = TemplateBodyIdentity {
            kind,
            name: name.atom().clone(),
        };
        check_expr(ctx, Some(&owner), &identity, expr)?;
    }
    Ok(())
}

fn check_template_assertion_bodies(ctx: &DimCheckContext<'_>) -> Result<(), GraphcalError> {
    for entry in ctx.env.dag.asserts() {
        ctx.checkpoint()?;
        let Some(owner) = local_owner(ctx, entry.identity()) else {
            continue;
        };
        let identity = TemplateBodyIdentity {
            kind: DeclarationKind::Assert,
            name: entry.name.atom().clone(),
        };
        match &*entry.body {
            hir::AssertBody::Expr(expr) => check_expr(ctx, Some(&owner), &identity, expr)?,
            hir::AssertBody::Tolerance {
                actual,
                expected,
                tolerance,
            } => {
                check_expr(ctx, Some(&owner), &identity, actual)?;
                check_expr(ctx, Some(&owner), &identity, expected)?;
                check_expr(ctx, Some(&owner), &identity, tolerance)?;
            }
        }
    }
    Ok(())
}

fn check_template_plot_bodies(ctx: &DimCheckContext<'_>) -> Result<(), GraphcalError> {
    for entry in ctx.env.dag.plots() {
        let Some(owner) = local_owner(ctx, entry.identity()) else {
            continue;
        };
        let identity = TemplateBodyIdentity {
            kind: DeclarationKind::Plot,
            name: entry.name.atom().clone(),
        };
        for expr in entry
            .body
            .encodings
            .iter()
            .map(|(_, expr)| expr)
            .chain(entry.body.mark_properties.iter().map(|field| &field.value))
            .chain(entry.body.properties.iter().map(|field| &field.value))
        {
            check_expr(ctx, Some(&owner), &identity, expr)?;
        }
    }
    Ok(())
}

fn check_template_composition_bodies(ctx: &DimCheckContext<'_>) -> Result<(), GraphcalError> {
    for (kind, name, declaration, fields) in ctx
        .env
        .dag
        .figures()
        .map(|entry| {
            (
                DeclarationKind::Figure,
                &entry.name,
                entry.identity(),
                entry.fields.as_slice(),
            )
        })
        .chain(ctx.env.dag.layers().map(|entry| {
            (
                DeclarationKind::Layer,
                &entry.name,
                entry.identity(),
                entry.fields.as_slice(),
            )
        }))
    {
        let Some(owner) = local_owner(ctx, declaration) else {
            continue;
        };
        let identity = TemplateBodyIdentity {
            kind,
            name: name.atom().clone(),
        };
        for field in fields {
            check_expr(ctx, Some(&owner), &identity, &field.value)?;
        }
    }
    Ok(())
}

fn check_template_unit_bodies(ctx: &DimCheckContext<'_>) -> Result<(), GraphcalError> {
    for entry in ctx.env.dag.semantic.dynamic_unit_scales.values() {
        if entry.unit.owner() != ctx.env.dag.dag_id() {
            continue;
        }
        let identity = TemplateBodyIdentity {
            kind: DeclarationKind::Unit,
            name: entry.unit.atom().clone(),
        };
        check_expr(ctx, None, &identity, &entry.expr)?;
    }
    Ok(())
}

/// Validate every source-authored executable body in one reusable DAG.
pub(super) fn check_template_body_closure(ctx: &DimCheckContext<'_>) -> Result<(), GraphcalError> {
    check_rigid_dimensions(ctx)?;
    check_template_value_bodies(ctx)?;
    check_template_assertion_bodies(ctx)?;
    check_template_plot_bodies(ctx)?;
    check_template_composition_bodies(ctx)?;
    check_template_unit_bodies(ctx)
}
