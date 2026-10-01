//! Option A template-body closure validation.

use super::{DimCheckContext, check_decl_expr_type, check_hir_assert_body, infer};
use crate::declaration_kind::DeclarationKind;
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::graphcal_error::GraphcalError;
use crate::outcome::Outcome;
use crate::resolved_name::{ResolvedDeclName, ResolvedStructTypeName};
use crate::semantic::checked_type::{CheckedType, Symbolic};
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
) -> Option<&'a crate::hir::source_interface::StaticPort> {
    dag.static_ports().iter().find(|port| {
        port.role == StaticRole::OptionalInput
            && matches!(
                &port.identity,
                crate::hir::source_interface::StaticPortIdentity::Type(port_identity)
                    if port_identity == identity
            )
    })
}

fn infer_operand(
    ctx: &DimCheckContext<'_>,
    owner: Option<&ResolvedDeclName>,
    expr: &crate::hir::expr::Expr,
) -> Result<CheckedType<Symbolic>, Outcome<GraphcalError>> {
    ctx.infer_hir(expr, owner)
}

fn emit_violation(
    ctx: &DimCheckContext<'_>,
    body: &TemplateBodyIdentity,
    port: &crate::hir::source_interface::StaticPort,
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

/// Report a checked root's first use of an optional type port's definition.
///
/// The uses are the ones the body's own checking pass observed; the body is
/// not inferred again.
fn check_expr(
    ctx: &DimCheckContext<'_>,
    body: &TemplateBodyIdentity,
    expr: &crate::hir::expr::Expr,
) -> Result<(), Outcome<GraphcalError>> {
    let dependencies = ctx.observations.type_definition_dependencies(expr.id());
    dependencies
        .into_iter()
        .try_for_each(|dependency| {
            optional_type_port(ctx.env.dag, dependency.identity()).map_or(Ok(()), |port| {
                emit_violation(ctx, body, port, dependency.span())
            })
        })
        .map_err(Outcome::Failed)
}

/// The declaration identity when this DAG authored it.
fn local_owner(
    ctx: &DimCheckContext<'_>,
    declaration: ResolvedDeclName,
) -> Option<ResolvedDeclName> {
    (declaration.owner() == ctx.env.dag.dag_id()).then_some(declaration)
}

/// What a body that fails to check in a rigid view reports.
#[derive(Clone, Copy)]
enum RigidFailure<'a> {
    /// The body depends on this optional port's default (V007).
    Violation(&'a crate::hir::source_interface::StaticPort),
    /// The closure check already accepted every body; report the error as is.
    Propagate,
}

fn rigid_dimension_error(
    ctx: &DimCheckContext<'_>,
    body: &TemplateBodyIdentity,
    failure: RigidFailure<'_>,
    span: Span,
    result: Result<(), Outcome<GraphcalError>>,
) -> Result<(), Outcome<GraphcalError>> {
    match (result, failure) {
        (Ok(()), _) => Ok(()),
        (Err(error @ Outcome::Cancelled), _) | (Err(error), RigidFailure::Propagate) => Err(error),
        (Err(Outcome::Failed(_)), RigidFailure::Violation(port)) => {
            Ok(emit_violation(ctx, body, port, span)?)
        }
    }
}

fn check_rigid_plot_field(
    ctx: &DimCheckContext<'_>,
    owner: &ResolvedDeclName,
    body: &TemplateBodyIdentity,
    failure: RigidFailure<'_>,
    field: &crate::ir::model::LoweredPlotField,
) -> Result<(), Outcome<GraphcalError>> {
    let (property, expected) = match &field.property {
        crate::ir::model::LoweredPlotProperty::Mark(property) => {
            (property.name(), property.value_type())
        }
        crate::ir::model::LoweredPlotProperty::Plot(property) => {
            (property.name(), property.value_type())
        }
        crate::ir::model::LoweredPlotProperty::Composition(property) => {
            (property.name(), property.value_type())
        }
        crate::ir::model::LoweredPlotProperty::Unknown(property) => {
            return Err(GraphcalError::internal_error(
                format!("unchecked plot property `{property}` reached rigid validation"),
                ctx.env.src,
                DiagnosticAnchor::Source(field.name_span),
            )
            .into());
        }
    };
    rigid_dimension_error(
        ctx,
        body,
        failure,
        field.value.span,
        super::plot::check_property_value(ctx, owner, property, expected, field),
    )
}

fn check_rigid_value_bodies(
    ctx: &DimCheckContext<'_>,
    failure: RigidFailure<'_>,
) -> Result<(), Outcome<GraphcalError>> {
    for (kind, name, declaration, annotation, body_span) in ctx
        .env
        .dag
        .consts()
        .map(|entry| {
            (
                DeclarationKind::ConstNode,
                entry.name(),
                entry.identity(),
                &entry.type_ann,
                entry.expr.span,
            )
        })
        .chain(ctx.env.dag.nodes().filter_map(|entry| {
            entry.definition.formula().map(|expression| {
                (
                    DeclarationKind::Node,
                    entry.name(),
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
            failure,
            body_span,
            check_decl_expr_type(ctx, name, &declaration, annotation),
        )?;
    }
    Ok(())
}

fn check_rigid_assertion_bodies(
    ctx: &DimCheckContext<'_>,
    failure: RigidFailure<'_>,
) -> Result<(), Outcome<GraphcalError>> {
    for entry in ctx.env.dag.asserts() {
        let Some(owner) = local_owner(ctx, entry.identity()) else {
            continue;
        };
        let assertion = ctx.hir_assert_body(entry.name(), &owner, entry.span)?;
        let body = TemplateBodyIdentity {
            kind: DeclarationKind::Assert,
            name: entry.name().atom().clone(),
        };
        rigid_dimension_error(
            ctx,
            &body,
            failure,
            entry.span,
            check_hir_assert_body(ctx, &owner, assertion, entry.span).map(|_| ()),
        )?;
    }
    Ok(())
}

fn check_rigid_plot_bodies(
    ctx: &DimCheckContext<'_>,
    failure: RigidFailure<'_>,
) -> Result<(), Outcome<GraphcalError>> {
    for entry in ctx.env.dag.plots() {
        let Some(owner) = local_owner(ctx, entry.identity()) else {
            continue;
        };
        let body = TemplateBodyIdentity {
            kind: DeclarationKind::Plot,
            name: entry.name().atom().clone(),
        };
        for (_, expression) in &entry.body.encodings {
            if matches!(
                expression.kind(),
                crate::hir::expr::ExprKind::StringLiteral(_)
            ) {
                // A contextual string channel has no type to depend on a port.
                continue;
            }
            rigid_dimension_error(
                ctx,
                &body,
                failure,
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
            check_rigid_plot_field(ctx, &owner, &body, failure, field)?;
        }
    }
    Ok(())
}

fn check_rigid_composition_bodies(
    ctx: &DimCheckContext<'_>,
    failure: RigidFailure<'_>,
) -> Result<(), Outcome<GraphcalError>> {
    for (kind, name, declaration, fields) in ctx
        .env
        .dag
        .figures()
        .map(|entry| {
            (
                DeclarationKind::Figure,
                entry.name(),
                entry.identity(),
                entry.fields.as_slice(),
            )
        })
        .chain(ctx.env.dag.layers().map(|entry| {
            (
                DeclarationKind::Layer,
                entry.name(),
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
            check_rigid_plot_field(ctx, &owner, &body, failure, field)?;
        }
    }
    Ok(())
}

fn check_rigid_unit_bodies(
    ctx: &DimCheckContext<'_>,
    failure: RigidFailure<'_>,
) -> Result<(), Outcome<GraphcalError>> {
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
            failure,
            entry.expr.span,
            super::check_dynamic_unit_scale_type(ctx, entry),
        )?;
    }
    Ok(())
}

/// Check every source-authored executable body of `template` in the view
/// where the optional dimension `ports` are rigid, checking the template's
/// plots with `plots`. Returns `plots`'s result and what the rigid inference
/// recorded.
fn check_in_rigid_view<R>(
    tir: &crate::tir::typed::UncheckedTir,
    template: &crate::tir::typed::DagTIR,
    ports: &[crate::resolved_name::ResolvedDimName],
    failure: RigidFailure<'_>,
    src: &miette::NamedSource<std::sync::Arc<String>>,
    cancellation: &crate::cancellation::CancellationToken,
    plots: impl FnOnce(&DimCheckContext<'_>) -> Result<R, Outcome<GraphcalError>>,
) -> Result<(R, infer::hir::FinishedObservations), Outcome<GraphcalError>> {
    let rigid_tir = crate::tir::typed::rigid_dimension_view(tir, template.dag_id(), ports, src)?;
    let rigid_dag = rigid_tir.dags.get(template.dag_id()).ok_or_else(|| {
        GraphcalError::internal_error(
            format!("rigid template DAG `{}` is unavailable", template.dag_id()),
            src,
            DiagnosticAnchor::WholeFile,
        )
    })?;
    let observations = infer::hir::BodyObservations::default();
    let rigid_ctx = DimCheckContext {
        env: infer::hir::InferEnv {
            dag: rigid_dag,
            tir: &rigid_tir,
            registry: rigid_tir.registry(),
            src,
        },
        assembly: &rigid_tir,
        cancellation,
        observations: &observations,
    };
    check_rigid_value_bodies(&rigid_ctx, failure)?;
    check_rigid_assertion_bodies(&rigid_ctx, failure)?;
    let result = plots(&rigid_ctx)?;
    check_rigid_composition_bodies(&rigid_ctx, failure)?;
    check_rigid_unit_bodies(&rigid_ctx, failure)?;
    Ok((result, observations.finish()))
}

fn check_rigid_dimension_port(
    ctx: &DimCheckContext<'_>,
    port: &crate::hir::source_interface::StaticPort,
    dimension: &crate::resolved_name::ResolvedDimName,
) -> Result<(), Outcome<GraphcalError>> {
    let failure = RigidFailure::Violation(port);
    check_in_rigid_view(
        ctx.assembly,
        ctx.env.dag,
        std::slice::from_ref(dimension),
        failure,
        ctx.env.src,
        ctx.cancellation,
        |rigid| check_rigid_plot_bodies(rigid, failure),
    )
    .map(drop)
}

/// A template's checked trees in the view where the optional dimension
/// ports an instance binds are rigid.
pub(super) struct PortGenericTrees {
    /// Typed trees of the bodies the rigid inference checked, by root.
    pub(super) trees: super::body_specialization::DerivedTrees,
    /// Channel shapes of the template's own plots.
    pub(super) plot_channels: super::plot::CheckedPlotChannelShapes,
}

/// Trees of `template` in the view where the bound optional dimension
/// `ports` are rigid: what an instance binding these ports specializes,
/// since the template's own trees saw their defaults.
///
/// The closure check has accepted every body with each port rigid, so any
/// failure here is reported unchanged.
pub(super) fn port_generic_trees(
    tir: &crate::tir::typed::UncheckedTir,
    template: &crate::tir::typed::DagTIR,
    ports: &[crate::resolved_name::ResolvedDimName],
    src: &miette::NamedSource<std::sync::Arc<String>>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<PortGenericTrees, Outcome<GraphcalError>> {
    let (plot_channels, finished) = check_in_rigid_view(
        tir,
        template,
        ports,
        RigidFailure::Propagate,
        src,
        cancellation,
        |rigid| {
            rigid
                .env
                .dag
                .plots()
                .filter(|entry| local_owner(rigid, entry.identity()).is_some())
                .map(|entry| super::plot::check_plot_entry(rigid, entry))
                .collect()
        },
    )?;
    let (bodies, calls) = finished.typed.into_roots();
    Ok(PortGenericTrees {
        trees: super::body_specialization::DerivedTrees {
            bodies,
            nominal_uses: finished.nominal_uses,
            calls,
        },
        plot_channels,
    })
}

fn check_rigid_dimensions(ctx: &DimCheckContext<'_>) -> Result<(), Outcome<GraphcalError>> {
    ctx.env
        .dag
        .static_ports()
        .iter()
        .filter(|port| port.role == StaticRole::OptionalInput)
        .filter_map(|port| match &port.identity {
            crate::hir::source_interface::StaticPortIdentity::Dimension(identity) => {
                Some((port, identity))
            }
            crate::hir::source_interface::StaticPortIdentity::Type(_)
            | crate::hir::source_interface::StaticPortIdentity::Index(_) => None,
        })
        .try_for_each(|(port, identity)| check_rigid_dimension_port(ctx, port, identity))
}

fn check_template_value_bodies(ctx: &DimCheckContext<'_>) -> Result<(), Outcome<GraphcalError>> {
    for (kind, name, declaration, expr) in ctx
        .env
        .dag
        .consts()
        .map(|entry| {
            (
                DeclarationKind::ConstNode,
                entry.name(),
                entry.identity(),
                &entry.expr,
            )
        })
        .chain(ctx.env.dag.nodes().filter_map(|entry| {
            entry.definition.formula().map(|expression| {
                (
                    DeclarationKind::Node,
                    entry.name(),
                    entry.identity(),
                    expression,
                )
            })
        }))
    {
        ctx.checkpoint()?;
        if local_owner(ctx, declaration).is_none() {
            continue;
        }
        let identity = TemplateBodyIdentity {
            kind,
            name: name.atom().clone(),
        };
        check_expr(ctx, &identity, expr)?;
    }
    Ok(())
}

fn check_template_assertion_bodies(
    ctx: &DimCheckContext<'_>,
) -> Result<(), Outcome<GraphcalError>> {
    for entry in ctx.env.dag.asserts() {
        ctx.checkpoint()?;
        if local_owner(ctx, entry.identity()).is_none() {
            continue;
        }
        let identity = TemplateBodyIdentity {
            kind: DeclarationKind::Assert,
            name: entry.name().atom().clone(),
        };
        match &*entry.body {
            crate::hir::expr::AssertBody::Expr(expr) => check_expr(ctx, &identity, expr)?,
            crate::hir::expr::AssertBody::Tolerance {
                actual,
                expected,
                tolerance,
            } => {
                check_expr(ctx, &identity, actual)?;
                check_expr(ctx, &identity, expected)?;
                check_expr(ctx, &identity, tolerance)?;
            }
        }
    }
    Ok(())
}

fn check_template_plot_bodies(ctx: &DimCheckContext<'_>) -> Result<(), Outcome<GraphcalError>> {
    for entry in ctx.env.dag.plots() {
        if local_owner(ctx, entry.identity()).is_none() {
            continue;
        }
        let identity = TemplateBodyIdentity {
            kind: DeclarationKind::Plot,
            name: entry.name().atom().clone(),
        };
        for expr in entry
            .body
            .encodings
            .iter()
            .map(|(_, expr)| expr)
            .chain(entry.body.mark_properties.iter().map(|field| &field.value))
            .chain(entry.body.properties.iter().map(|field| &field.value))
        {
            check_expr(ctx, &identity, expr)?;
        }
    }
    Ok(())
}

fn check_template_composition_bodies(
    ctx: &DimCheckContext<'_>,
) -> Result<(), Outcome<GraphcalError>> {
    for (kind, name, declaration, fields) in ctx
        .env
        .dag
        .figures()
        .map(|entry| {
            (
                DeclarationKind::Figure,
                entry.name(),
                entry.identity(),
                entry.fields.as_slice(),
            )
        })
        .chain(ctx.env.dag.layers().map(|entry| {
            (
                DeclarationKind::Layer,
                entry.name(),
                entry.identity(),
                entry.fields.as_slice(),
            )
        }))
    {
        if local_owner(ctx, declaration).is_none() {
            continue;
        }
        let identity = TemplateBodyIdentity {
            kind,
            name: name.atom().clone(),
        };
        for field in fields {
            check_expr(ctx, &identity, &field.value)?;
        }
    }
    Ok(())
}

fn check_template_unit_bodies(ctx: &DimCheckContext<'_>) -> Result<(), Outcome<GraphcalError>> {
    for entry in ctx.env.dag.semantic.dynamic_unit_scales.values() {
        if entry.unit.owner() != ctx.env.dag.dag_id() {
            continue;
        }
        let identity = TemplateBodyIdentity {
            kind: DeclarationKind::Unit,
            name: entry.unit.atom().clone(),
        };
        check_expr(ctx, &identity, &entry.expr)?;
    }
    Ok(())
}

/// Validate every source-authored executable body in one reusable DAG.
pub(super) fn check_template_body_closure(
    ctx: &DimCheckContext<'_>,
) -> Result<(), Outcome<GraphcalError>> {
    check_rigid_dimensions(ctx)?;
    check_template_value_bodies(ctx)?;
    check_template_assertion_bodies(ctx)?;
    check_template_plot_bodies(ctx)?;
    check_template_composition_bodies(ctx)?;
    check_template_unit_bodies(ctx)
}
