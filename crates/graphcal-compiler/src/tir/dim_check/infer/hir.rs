//! HIR-backed expression type inference.
//!
//! This is an incremental semantic consumer for module-aware declaration
//! expressions. It returns `Ok(None)` for expression forms that still need the
//! syntax-AST inference path, but the forms it does accept consume HIR
//! references directly: canonical declaration/index refs, lexical `LocalId`s,
//! and typed built-in function variants.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::hir::{self, BuiltinFnName, ConstRef, FunctionRef};
use crate::registry::error::GraphcalError;
use crate::registry::types::Registry;
use crate::syntax::dimension::Dimension;
use crate::syntax::names::{GenericParamName, ResolvedName, ScopedName, UnitName, namespace};
use crate::syntax::span::Span;
use crate::tir::typed::NatLinearForm;

use super::super::builtins::infer_fn_dim;
use super::super::helpers::{expect_scalar, format_inferred_type};
use super::super::{DeclaredType, InferredIndex, InferredType};

type HirLocalTypes = HashMap<hir::LocalId, InferredType>;

type ResolvedDeclKey = ResolvedName<namespace::Decl>;

/// Infer a HIR expression if this incremental consumer supports every form in it.
pub(in crate::tir::dim_check) fn infer_hir_type_with_owner(
    expr: &hir::Expr,
    owner_decl_name: Option<&str>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<Option<InferredType>, GraphcalError> {
    if !hir_dimcheck_supported(expr) {
        return Ok(None);
    }
    let locals = HirLocalTypes::new();
    infer_hir_type(
        expr,
        owner_decl_name,
        declared_types,
        &locals,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )
}

fn hir_dimcheck_supported(expr: &hir::Expr) -> bool {
    match &expr.kind {
        hir::ExprKind::Number(_)
        | hir::ExprKind::Integer(_)
        | hir::ExprKind::Bool(_)
        | hir::ExprKind::StringLiteral(_)
        | hir::ExprKind::TypeSystemRef(_)
        | hir::ExprKind::UnitLiteral { .. }
        | hir::ExprKind::GraphRef(_)
        | hir::ExprKind::LocalRef(_)
        | hir::ExprKind::VariantLiteral(_) => true,
        hir::ExprKind::ConstRef(target) => matches!(
            &target.value,
            ConstRef::Decl(_)
                | ConstRef::IndexVariant(_)
                | ConstRef::Builtin(_)
                | ConstRef::TimeScale(_)
        ),
        hir::ExprKind::FnCall { callee, args, .. } => {
            matches!(&callee.value, FunctionRef::Builtin(BuiltinFnName::Sqrt))
                && args.iter().all(hir_dimcheck_supported)
        }
        hir::ExprKind::ForComp { body, .. } => hir_dimcheck_supported(body),
        hir::ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            hir_dimcheck_supported(condition)
                && hir_dimcheck_supported(then_branch)
                && hir_dimcheck_supported(else_branch)
        }
        hir::ExprKind::UnaryOp { operand, .. } => hir_dimcheck_supported(operand),
        hir::ExprKind::BinOp { op, lhs, rhs } => {
            matches!(
                op,
                crate::desugar::resolved_ast::BinOp::Eq | crate::desugar::resolved_ast::BinOp::Ne
            ) && hir_dimcheck_supported(lhs)
                && hir_dimcheck_supported(rhs)
        }
        hir::ExprKind::IndexAccess { .. }
        | hir::ExprKind::ConstructorCall { .. }
        | hir::ExprKind::MapLiteral { .. }
        | hir::ExprKind::Scan { .. }
        | hir::ExprKind::Unfold { .. }
        | hir::ExprKind::Match { .. }
        | hir::ExprKind::FieldAccess { .. }
        | hir::ExprKind::InlineDagRef { .. }
        | hir::ExprKind::Convert { .. }
        | hir::ExprKind::DisplayTimezone { .. } => false,
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "mirrors syntax inference context"
)]
fn infer_hir_type(
    expr: &hir::Expr,
    owner_decl_name: Option<&str>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<Option<InferredType>, GraphcalError> {
    let inferred = match &expr.kind {
        hir::ExprKind::Number(_) => InferredType::Scalar(Dimension::dimensionless()),
        hir::ExprKind::Integer(_) => InferredType::Int,
        hir::ExprKind::Bool(_) => InferredType::Bool,
        hir::ExprKind::StringLiteral(_) => {
            return Err(GraphcalError::DimensionMismatch {
                expected: "a numeric or boolean expression".to_string(),
                found: "string literal".to_string(),
                help: "string literals can only be used as arguments to datetime() or epoch()"
                    .to_string(),
                src: src.clone(),
                span: expr.span.into(),
            });
        }
        hir::ExprKind::TypeSystemRef(name) => {
            return Err(GraphcalError::DimensionMismatch {
                expected: "a value expression".to_string(),
                found: format!("type-system name `{:?}`", name.value),
                help: "type-system names can only be used in include/import bindings".to_string(),
                src: src.clone(),
                span: name.span.into(),
            });
        }
        hir::ExprKind::UnitLiteral { unit, .. } => infer_hir_unit_literal(unit, registry, src)?,
        hir::ExprKind::VariantLiteral(variant) => {
            InferredType::Label(InferredIndex::from_resolved(variant.value.index().clone()))
        }
        hir::ExprKind::GraphRef(target) => {
            infer_resolved_decl_ref_type(&target.value, declared_types, dag, src)?
        }
        hir::ExprKind::ConstRef(target) => infer_hir_const_ref(target, declared_types, dag, src)?,
        hir::ExprKind::LocalRef(local) => {
            local_types.get(&local.value).cloned().ok_or_else(|| {
                GraphcalError::UnknownLocalRef {
                    name: format!("#{}", local.value.index()),
                    src: src.clone(),
                    span: local.span.into(),
                }
            })?
        }
        hir::ExprKind::FnCall { callee, args, .. } => infer_hir_fn_call(
            callee,
            args,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?,
        hir::ExprKind::ForComp { bindings, body } => infer_hir_for_comp(
            bindings,
            body,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?,
        hir::ExprKind::IndexAccess { expr: inner, args } => infer_hir_index_access(
            expr,
            inner,
            args,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?,
        hir::ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => infer_hir_if(
            condition,
            then_branch,
            else_branch,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?,
        hir::ExprKind::UnaryOp { op, operand } => infer_hir_unary(
            *op,
            operand,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?,
        hir::ExprKind::BinOp { op, lhs, rhs } => infer_hir_binop(
            expr.span,
            *op,
            lhs,
            rhs,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?,
        // These forms still rely on richer syntax inference helpers while this
        // chunk migrates the core reference/local/built-in path.
        hir::ExprKind::ConstructorCall { .. }
        | hir::ExprKind::MapLiteral { .. }
        | hir::ExprKind::Scan { .. }
        | hir::ExprKind::Unfold { .. }
        | hir::ExprKind::Match { .. }
        | hir::ExprKind::FieldAccess { .. }
        | hir::ExprKind::InlineDagRef { .. }
        | hir::ExprKind::Convert { .. }
        | hir::ExprKind::DisplayTimezone { .. } => return Ok(None),
    };
    Ok(Some(inferred))
}

fn infer_hir_unit_literal(
    unit: &crate::desugar::resolved_ast::UnitExpr,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let dim = registry
        .units
        .resolve_unit_dimension(unit)
        .map_err(|_| GraphcalError::DimensionOverflow {
            src: src.clone(),
            span: unit.span.into(),
        })?
        .ok_or_else(|| {
            for item in &unit.terms {
                if registry.units.get_unit(item.name.value.as_str()).is_none() {
                    return GraphcalError::UnknownUnit {
                        name: item.name.value.clone(),
                        src: src.clone(),
                        span: item.name.span.into(),
                    };
                }
            }
            GraphcalError::UnknownUnit {
                name: UnitName::new("unknown"),
                src: src.clone(),
                span: unit.span.into(),
            }
        })?;
    Ok(InferredType::Scalar(dim))
}

fn infer_resolved_decl_ref_type(
    target: &ResolvedDeclKey,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    dag: &crate::tir::typed::DagTIR,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let local_name = ScopedName::local(target.as_str());

    if target.owner() == &dag.dag_id
        && let Some(inferred) = infer_bound_decl_type(&local_name, declared_types, dag, src)?
    {
        return Ok(inferred);
    }

    for name in dag
        .semantic
        .decl_bindings
        .iter()
        .filter_map(|(name, resolved)| (resolved == target).then_some(name))
    {
        if let Some(inferred) = infer_bound_decl_type(name, declared_types, dag, src)? {
            return Ok(inferred);
        }
    }

    for name in dag
        .imported_value_sources
        .iter()
        .filter_map(|(name, source)| {
            imported_source_matches_resolved(source, target).then_some(name)
        })
    {
        if let Some(inferred) = infer_bound_decl_type(name, declared_types, dag, src)? {
            return Ok(inferred);
        }
    }

    Err(GraphcalError::UnknownGraphRef {
        name: local_name,
        src: src.clone(),
        span: crate::syntax::span::Span::new(0, 0).into(),
    })
}

fn infer_bound_decl_type(
    name: &ScopedName,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    dag: &crate::tir::typed::DagTIR,
    src: &NamedSource<Arc<String>>,
) -> Result<Option<InferredType>, GraphcalError> {
    if let Some(resolved_type) = dag.resolved_decl_types.get(name) {
        let dim_sub = HashMap::new();
        let index_sub =
            HashMap::<GenericParamName, crate::registry::declared_type::IndexTypeRef>::new();
        let nat_sub = HashMap::new();
        return crate::tir::typed::substitute_resolved_type(
            resolved_type,
            &dim_sub,
            &index_sub,
            &nat_sub,
            src,
        )
        .map(Some);
    }

    Ok(declared_types.get(name).map(InferredType::from))
}

fn imported_source_matches_resolved(
    source: &crate::ir::lower::ImportedValueSource,
    target: &ResolvedDeclKey,
) -> bool {
    source.dag_id == *target.owner() && source.source_name.as_str() == target.as_str()
}

fn infer_hir_const_ref(
    target: &crate::syntax::span::Spanned<ConstRef>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    dag: &crate::tir::typed::DagTIR,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    match &target.value {
        ConstRef::Decl(resolved) => {
            infer_resolved_decl_ref_type(resolved, declared_types, dag, src)
        }
        ConstRef::IndexVariant(variant) => Ok(InferredType::Label(InferredIndex::from_resolved(
            variant.index().clone(),
        ))),
        ConstRef::Builtin(_) => Ok(InferredType::Scalar(Dimension::dimensionless())),
        ConstRef::TimeScale(_) => Err(GraphcalError::DimensionMismatch {
            expected: "value expression".to_string(),
            found: "time scale".to_string(),
            help: "time scales can only be used as the second argument to epoch()".to_string(),
            src: src.clone(),
            span: target.span.into(),
        }),
        ConstRef::Constructor(_) | ConstRef::GenericNatParam(_) => Err(GraphcalError::EvalError {
            message: "unsupported HIR const-like reference in dim-check".to_string(),
            src: src.clone(),
            span: target.span.into(),
        }),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "mirrors syntax inference context"
)]
fn infer_arg(
    arg: &hir::Expr,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<Option<InferredType>, GraphcalError> {
    infer_hir_type(
        arg,
        None,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )
}

#[expect(clippy::too_many_arguments, reason = "function-call context")]
fn infer_hir_fn_call(
    callee: &crate::syntax::span::Spanned<FunctionRef>,
    args: &[hir::Expr],
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let FunctionRef::Builtin(name) = &callee.value else {
        return Err(GraphcalError::UnknownFunction {
            name: "user function".to_string(),
            src: src.clone(),
            span: callee.span.into(),
        });
    };
    let name = *name;
    match name.special_kind() {
        Some(crate::registry::resolve_types::SpecialFnKind::Aggregation(kind))
            if args.len() == 1 =>
        {
            let Some(arg_type) = infer_arg(
                &args[0],
                declared_types,
                local_types,
                dag,
                tir,
                registry,
                builtin_fns,
                src,
            )?
            else {
                return Err(unsupported_expr(&args[0], src));
            };
            if let InferredType::Indexed { element, .. } = arg_type {
                return Ok(match kind {
                    crate::registry::resolve_types::AggregationFn::Count => {
                        InferredType::Scalar(Dimension::dimensionless())
                    }
                    crate::registry::resolve_types::AggregationFn::Sum
                    | crate::registry::resolve_types::AggregationFn::Min
                    | crate::registry::resolve_types::AggregationFn::Max
                    | crate::registry::resolve_types::AggregationFn::Mean => *element,
                });
            }
            infer_hir_builtin_fn(
                name,
                args,
                declared_types,
                local_types,
                dag,
                tir,
                registry,
                builtin_fns,
                src,
            )
        }
        Some(crate::registry::resolve_types::SpecialFnKind::TypeConversion(kind)) => {
            infer_hir_type_conversion(
                kind,
                callee.span,
                args,
                declared_types,
                local_types,
                dag,
                tir,
                registry,
                builtin_fns,
                src,
            )
        }
        Some(crate::registry::resolve_types::SpecialFnKind::TimeScaleConversion(scale)) => {
            infer_hir_timescale_conversion(
                name,
                scale,
                callee.span,
                args,
                declared_types,
                local_types,
                dag,
                tir,
                registry,
                builtin_fns,
                src,
            )
        }
        Some(crate::registry::resolve_types::SpecialFnKind::Constructor(kind)) => {
            infer_hir_datetime_constructor(
                kind,
                callee.span,
                args,
                declared_types,
                local_types,
                dag,
                tir,
                registry,
                builtin_fns,
                src,
            )
        }
        Some(crate::registry::resolve_types::SpecialFnKind::DatetimeExtract(_)) => {
            infer_hir_datetime_unary(
                name,
                callee.span,
                args,
                declared_types,
                local_types,
                dag,
                tir,
                registry,
                builtin_fns,
                src,
                InferredType::Int,
            )
        }
        Some(crate::registry::resolve_types::SpecialFnKind::DatetimeFrom(_)) => {
            let Some(arg_type) = infer_arg(
                &args[0],
                declared_types,
                local_types,
                dag,
                tir,
                registry,
                builtin_fns,
                src,
            )?
            else {
                return Err(unsupported_expr(&args[0], src));
            };
            match &arg_type {
                InferredType::Scalar(dim) if dim.is_dimensionless() => {}
                t if t.is_int_like() => {}
                _ => {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "Dimensionless or Int".to_string(),
                        found: format_inferred_type(&arg_type, registry),
                        help: format!(
                            "{}() requires a dimensionless numeric argument",
                            name.as_str()
                        ),
                        src: src.clone(),
                        span: args[0].span.into(),
                    });
                }
            }
            Ok(InferredType::Datetime(
                crate::registry::time_scale::TimeScale::UTC,
            ))
        }
        Some(crate::registry::resolve_types::SpecialFnKind::DatetimeTo(_)) => {
            infer_hir_datetime_unary(
                name,
                callee.span,
                args,
                declared_types,
                local_types,
                dag,
                tir,
                registry,
                builtin_fns,
                src,
                InferredType::Scalar(Dimension::dimensionless()),
            )
        }
        None | Some(crate::registry::resolve_types::SpecialFnKind::Aggregation(_)) => {
            infer_hir_builtin_fn(
                name,
                args,
                declared_types,
                local_types,
                dag,
                tir,
                registry,
                builtin_fns,
                src,
            )
        }
    }
}

#[expect(clippy::too_many_arguments, reason = "function-call context")]
fn infer_hir_builtin_fn(
    name: BuiltinFnName,
    args: &[hir::Expr],
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let Some(func) = builtin_fns.get(name.as_str()) else {
        return Err(GraphcalError::UnknownFunction {
            name: name.as_str().to_string(),
            src: src.clone(),
            span: args.first().map_or_else(
                || crate::syntax::span::Span::new(0, 0).into(),
                |arg| arg.span.into(),
            ),
        });
    };
    let arg_dims: Vec<Dimension> = args
        .iter()
        .map(|arg| {
            let Some(t) = infer_arg(
                arg,
                declared_types,
                local_types,
                dag,
                tir,
                registry,
                builtin_fns,
                src,
            )?
            else {
                return Err(unsupported_expr(arg, src));
            };
            expect_scalar(&t, registry, src, arg.span)
        })
        .collect::<Result<_, _>>()?;
    infer_fn_dim(name.as_str(), &func.dim_sig, &arg_dims, &[], registry, src)
        .map(InferredType::Scalar)
}

#[expect(clippy::too_many_arguments, reason = "function-call context")]
fn infer_hir_type_conversion(
    kind: crate::registry::resolve_types::TypeConversionFn,
    span: crate::syntax::span::Span,
    args: &[hir::Expr],
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let expected_arity = 1;
    if args.len() != expected_arity {
        return Err(GraphcalError::WrongArity {
            name: crate::syntax::names::FnName::new(kind.as_str()),
            expected: expected_arity,
            got: args.len(),
            src: src.clone(),
            span: span.into(),
        });
    }
    let Some(arg_type) = infer_arg(
        &args[0],
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?
    else {
        return Err(unsupported_expr(&args[0], src));
    };
    match kind {
        crate::registry::resolve_types::TypeConversionFn::ToFloat => {
            if !arg_type.is_int_like() {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Int".to_string(),
                    found: format_inferred_type(&arg_type, registry),
                    help: "to_float() requires an Int argument".to_string(),
                    src: src.clone(),
                    span: args[0].span.into(),
                });
            }
            Ok(InferredType::Scalar(Dimension::dimensionless()))
        }
        crate::registry::resolve_types::TypeConversionFn::ToInt => {
            let dim = expect_scalar(&arg_type, registry, src, args[0].span)?;
            if !dim.is_dimensionless() {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Dimensionless".to_string(),
                    found: registry.dimensions.format_dimension(&dim),
                    help: "to_int() requires a Dimensionless argument".to_string(),
                    src: src.clone(),
                    span: args[0].span.into(),
                });
            }
            Ok(InferredType::Int)
        }
    }
}

#[expect(clippy::too_many_arguments, reason = "function-call context")]
fn infer_hir_timescale_conversion(
    name: BuiltinFnName,
    scale: crate::registry::time_scale::TimeScale,
    span: crate::syntax::span::Span,
    args: &[hir::Expr],
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    if args.len() != 1 {
        return Err(GraphcalError::WrongArity {
            name: crate::syntax::names::FnName::new(name.as_str()),
            expected: 1,
            got: args.len(),
            src: src.clone(),
            span: span.into(),
        });
    }
    let Some(arg_type) = infer_arg(
        &args[0],
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?
    else {
        return Err(unsupported_expr(&args[0], src));
    };
    if !matches!(arg_type, InferredType::Datetime(_)) {
        return Err(GraphcalError::DimensionMismatch {
            expected: "Datetime".to_string(),
            found: format_inferred_type(&arg_type, registry),
            help: format!("{}() requires a Datetime argument", name.as_str()),
            src: src.clone(),
            span: args[0].span.into(),
        });
    }
    Ok(InferredType::Datetime(scale))
}

#[expect(clippy::too_many_arguments, reason = "function-call context")]
fn infer_hir_datetime_constructor(
    kind: crate::registry::resolve_types::ConstructorFn,
    span: crate::syntax::span::Span,
    args: &[hir::Expr],
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    match kind {
        crate::registry::resolve_types::ConstructorFn::Datetime => {
            if args.is_empty() || args.len() > 2 {
                return Err(GraphcalError::EvalError {
                    message: format!("datetime() expects 1 or 2 arguments, got {}", args.len()),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            if !matches!(args[0].kind, hir::ExprKind::StringLiteral(_)) {
                let Some(found) = infer_arg(
                    &args[0],
                    declared_types,
                    local_types,
                    dag,
                    tir,
                    registry,
                    builtin_fns,
                    src,
                )?
                else {
                    return Err(unsupported_expr(&args[0], src));
                };
                return Err(GraphcalError::DimensionMismatch {
                    expected: "string literal".to_string(),
                    found: format_inferred_type(&found, registry),
                    help: "datetime() requires a string literal argument".to_string(),
                    src: src.clone(),
                    span: args[0].span.into(),
                });
            }
            if args.len() == 2 && !matches!(args[1].kind, hir::ExprKind::StringLiteral(_)) {
                let Some(found) = infer_arg(
                    &args[1],
                    declared_types,
                    local_types,
                    dag,
                    tir,
                    registry,
                    builtin_fns,
                    src,
                )?
                else {
                    return Err(unsupported_expr(&args[1], src));
                };
                return Err(GraphcalError::DimensionMismatch {
                    expected: "string literal (IANA timezone)".to_string(),
                    found: format_inferred_type(&found, registry),
                    help: "datetime() second argument must be a timezone string literal"
                        .to_string(),
                    src: src.clone(),
                    span: args[1].span.into(),
                });
            }
            Ok(InferredType::Datetime(
                crate::registry::time_scale::TimeScale::UTC,
            ))
        }
        crate::registry::resolve_types::ConstructorFn::Epoch => {
            if args.len() != 2 {
                return Err(GraphcalError::WrongArity {
                    name: crate::syntax::names::FnName::new("epoch"),
                    expected: 2,
                    got: args.len(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            if !matches!(args[0].kind, hir::ExprKind::StringLiteral(_)) {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "string literal".to_string(),
                    found: "non-string".to_string(),
                    help: "epoch() requires a string literal as its first argument".to_string(),
                    src: src.clone(),
                    span: args[0].span.into(),
                });
            }
            let hir::ExprKind::ConstRef(scale_ref) = &args[1].kind else {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "time scale".to_string(),
                    found: "value".to_string(),
                    help: "epoch() requires a time scale identifier as its second argument"
                        .to_string(),
                    src: src.clone(),
                    span: args[1].span.into(),
                });
            };
            let ConstRef::TimeScale(scale) = scale_ref.value else {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "time scale".to_string(),
                    found: "value".to_string(),
                    help: "epoch() requires a time scale identifier as its second argument"
                        .to_string(),
                    src: src.clone(),
                    span: args[1].span.into(),
                });
            };
            Ok(InferredType::Datetime(scale))
        }
    }
}

#[expect(clippy::too_many_arguments, reason = "function-call context")]
fn infer_hir_datetime_unary(
    name: BuiltinFnName,
    span: crate::syntax::span::Span,
    args: &[hir::Expr],
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
    result: InferredType,
) -> Result<InferredType, GraphcalError> {
    if args.len() != 1 {
        return Err(GraphcalError::WrongArity {
            name: crate::syntax::names::FnName::new(name.as_str()),
            expected: 1,
            got: args.len(),
            src: src.clone(),
            span: span.into(),
        });
    }
    let Some(arg_type) = infer_arg(
        &args[0],
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?
    else {
        return Err(unsupported_expr(&args[0], src));
    };
    if !matches!(arg_type, InferredType::Datetime(_)) {
        return Err(GraphcalError::DimensionMismatch {
            expected: "Datetime".to_string(),
            found: format_inferred_type(&arg_type, registry),
            help: format!("{}() requires a Datetime argument", name.as_str()),
            src: src.clone(),
            span: args[0].span.into(),
        });
    }
    Ok(result)
}

#[expect(clippy::too_many_arguments, reason = "if expression context")]
fn infer_hir_if(
    condition: &hir::Expr,
    then_branch: &hir::Expr,
    else_branch: &hir::Expr,
    owner_decl_name: Option<&str>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let Some(cond_type) = infer_hir_type(
        condition,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?
    else {
        return Err(unsupported_expr(condition, src));
    };
    if cond_type != InferredType::Bool {
        return Err(GraphcalError::DimensionMismatch {
            expected: "Bool".to_string(),
            found: format_inferred_type(&cond_type, registry),
            help: "if/else condition must be Bool".to_string(),
            src: src.clone(),
            span: condition.span.into(),
        });
    }
    let Some(then_type) = infer_hir_type(
        then_branch,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?
    else {
        return Err(unsupported_expr(then_branch, src));
    };
    let Some(else_type) = infer_hir_type(
        else_branch,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?
    else {
        return Err(unsupported_expr(else_branch, src));
    };
    if then_type != else_type {
        return Err(GraphcalError::DimensionMismatch {
            expected: format_inferred_type(&then_type, registry),
            found: format_inferred_type(&else_type, registry),
            help: "both branches of if/else must have the same dimension".to_string(),
            src: src.clone(),
            span: else_branch.span.into(),
        });
    }
    Ok(then_type)
}

#[expect(clippy::too_many_arguments, reason = "unary expression context")]
fn infer_hir_unary(
    op: crate::desugar::resolved_ast::UnaryOp,
    operand: &hir::Expr,
    owner_decl_name: Option<&str>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let Some(operand_type) = infer_hir_type(
        operand,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?
    else {
        return Err(unsupported_expr(operand, src));
    };
    match op {
        crate::desugar::resolved_ast::UnaryOp::Not if operand_type == InferredType::Bool => {
            Ok(InferredType::Bool)
        }
        crate::desugar::resolved_ast::UnaryOp::Not => Err(GraphcalError::DimensionMismatch {
            expected: "Bool".to_string(),
            found: format_inferred_type(&operand_type, registry),
            help: "logical NOT requires a Bool operand".to_string(),
            src: src.clone(),
            span: operand.span.into(),
        }),
        crate::desugar::resolved_ast::UnaryOp::Neg => Ok(operand_type),
    }
}

#[expect(clippy::too_many_arguments, reason = "binary expression context")]
fn infer_hir_binop(
    span: crate::syntax::span::Span,
    op: crate::desugar::resolved_ast::BinOp,
    lhs: &hir::Expr,
    rhs: &hir::Expr,
    owner_decl_name: Option<&str>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    use crate::desugar::resolved_ast::BinOp;
    let Some(lhs_type) = infer_hir_type(
        lhs,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?
    else {
        return Err(unsupported_expr(lhs, src));
    };
    let Some(rhs_type) = infer_hir_type(
        rhs,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?
    else {
        return Err(unsupported_expr(rhs, src));
    };
    match op {
        BinOp::And | BinOp::Or => {
            if lhs_type == InferredType::Bool && rhs_type == InferredType::Bool {
                Ok(InferredType::Bool)
            } else {
                Err(GraphcalError::DimensionMismatch {
                    expected: "Bool".to_string(),
                    found: format_inferred_type(&lhs_type, registry),
                    help: "boolean operators require Bool operands".to_string(),
                    src: src.clone(),
                    span: span.into(),
                })
            }
        }
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            Ok(InferredType::Bool)
        }
        BinOp::Add | BinOp::Sub => {
            if lhs_type.is_int_like() && rhs_type.is_int_like() {
                return Ok(InferredType::Int);
            }
            let lhs_dim = expect_scalar(&lhs_type, registry, src, lhs.span)?;
            let rhs_dim = expect_scalar(&rhs_type, registry, src, rhs.span)?;
            if lhs_dim != rhs_dim {
                return Err(GraphcalError::DimensionMismatch {
                    expected: registry.dimensions.format_dimension(&lhs_dim),
                    found: registry.dimensions.format_dimension(&rhs_dim),
                    help: "operands of addition and subtraction must have the same dimension"
                        .to_string(),
                    src: src.clone(),
                    span: rhs.span.into(),
                });
            }
            Ok(InferredType::Scalar(lhs_dim))
        }
        BinOp::Mul => {
            if lhs_type.is_int_like() && rhs_type.is_int_like() {
                return Ok(InferredType::Int);
            }
            let dim = (expect_scalar(&lhs_type, registry, src, lhs.span)?
                * expect_scalar(&rhs_type, registry, src, rhs.span)?)
            .map_err(|_| GraphcalError::DimensionOverflow {
                src: src.clone(),
                span: span.into(),
            })?;
            Ok(InferredType::Scalar(dim))
        }
        BinOp::Div => {
            if lhs_type.is_int_like() && rhs_type.is_int_like() {
                return Ok(InferredType::Int);
            }
            let dim = (expect_scalar(&lhs_type, registry, src, lhs.span)?
                / expect_scalar(&rhs_type, registry, src, rhs.span)?)
            .map_err(|_| GraphcalError::DimensionOverflow {
                src: src.clone(),
                span: span.into(),
            })?;
            Ok(InferredType::Scalar(dim))
        }
        BinOp::Mod => Ok(InferredType::Int),
        BinOp::Pow => Ok(InferredType::Scalar(expect_scalar(
            &lhs_type, registry, src, lhs.span,
        )?)),
    }
}

fn hir_nat_to_linear_form(expr: &hir::NatExpr) -> NatLinearForm {
    match expr {
        hir::NatExpr::Literal(n, _) => NatLinearForm::from_constant(*n),
        hir::NatExpr::Param(param) => NatLinearForm::from_var(param.value.name.clone()),
        hir::NatExpr::Add(lhs, rhs, _) => {
            hir_nat_to_linear_form(lhs).add(&hir_nat_to_linear_form(rhs))
        }
        hir::NatExpr::Mul(lhs, rhs, _) => {
            hir_nat_to_linear_form(lhs).mul(&hir_nat_to_linear_form(rhs))
        }
    }
}

fn nat_range_error(
    err: crate::registry::types::NatRangeIndexError,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> GraphcalError {
    GraphcalError::EvalError {
        message: err.to_string(),
        src: src.clone(),
        span: span.into(),
    }
}

fn index_def_for_inferred<'a>(
    index: &InferredIndex,
    dag: &'a crate::tir::typed::DagTIR,
    registry: &'a Registry,
) -> Option<&'a crate::registry::types::IndexDef> {
    if let Some(nat_range) = index.concrete_nat_range() {
        return registry.indexes.get_nat_range(nat_range);
    }
    dag.semantic
        .collection_refs
        .index_defs
        .get(index.declared_resolved()?)
}

#[expect(clippy::too_many_arguments, reason = "for-comprehension context")]
fn infer_hir_for_comp(
    bindings: &[hir::expr::ForBinding],
    body: &hir::Expr,
    owner_decl_name: Option<&str>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let mut inner_locals = local_types.clone();
    for binding in bindings {
        let var_type =
            match &binding.index {
                hir::expr::ForBindingIndex::Named(index) => {
                    let index_identity = InferredIndex::from_resolved(index.value.clone());
                    let idx_def = index_def_for_inferred(&index_identity, dag, registry)
                        .ok_or_else(|| GraphcalError::UnknownIndex {
                            name: index_identity.name(),
                            src: src.clone(),
                            span: index.span.into(),
                        })?;
                    match &idx_def.kind {
                        crate::registry::types::IndexKind::Named { .. }
                        | crate::registry::types::IndexKind::RequiredNamed => {
                            InferredType::Label(index_identity)
                        }
                        crate::registry::types::IndexKind::Range(data) => {
                            InferredType::Scalar(data.dimension.clone())
                        }
                        crate::registry::types::IndexKind::RequiredRange { dimension } => {
                            InferredType::Scalar(dimension.clone())
                        }
                        crate::registry::types::IndexKind::NatRange { size } => {
                            InferredType::Fin(NatLinearForm::from_constant(size.get() as u64))
                        }
                    }
                }
                hir::expr::ForBindingIndex::Range { arg, .. } => {
                    InferredType::Fin(hir_nat_to_linear_form(arg))
                }
            };
        inner_locals.insert(binding.local.id, var_type);
    }
    let Some(mut result) = infer_hir_type(
        body,
        owner_decl_name,
        declared_types,
        &inner_locals,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?
    else {
        return Err(unsupported_expr(body, src));
    };
    for binding in bindings.iter().rev() {
        let index = match &binding.index {
            hir::expr::ForBindingIndex::Named(index) => {
                InferredIndex::from_resolved(index.value.clone())
            }
            hir::expr::ForBindingIndex::Range { arg, span } => {
                InferredIndex::from_nat_range_form(hir_nat_to_linear_form(arg))
                    .map_err(|err| nat_range_error(err, src, *span))?
            }
        };
        result = InferredType::Indexed {
            element: Box::new(result),
            index,
        };
    }
    Ok(result)
}

#[expect(clippy::too_many_arguments, reason = "index-access context")]
fn infer_hir_index_access(
    expr: &hir::Expr,
    inner: &hir::Expr,
    args: &[hir::expr::IndexArg],
    owner_decl_name: Option<&str>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let Some(mut current) = infer_hir_type(
        inner,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?
    else {
        return Err(unsupported_expr(inner, src));
    };
    for arg in args {
        let InferredType::Indexed { element, index } = current else {
            return Err(GraphcalError::EvalError {
                message: "indexing a non-indexed value".to_string(),
                src: src.clone(),
                span: expr.span.into(),
            });
        };
        match arg {
            hir::expr::IndexArg::Variant(variant) => {
                let arg_index = InferredIndex::from_resolved(variant.value.index().clone());
                if arg_index != index {
                    return Err(GraphcalError::IndexMismatch {
                        expected: index.name(),
                        found: arg_index.name(),
                        src: src.clone(),
                        span: variant.span.into(),
                    });
                }
            }
            hir::expr::IndexArg::Var(local) => {
                let Some(var_type) = local_types.get(&local.value) else {
                    return Err(GraphcalError::UnknownLocalRef {
                        name: format!("#{}", local.value.index()),
                        src: src.clone(),
                        span: local.span.into(),
                    });
                };
                if let InferredType::Label(label_index) = var_type
                    && label_index != &index
                {
                    return Err(GraphcalError::IndexMismatch {
                        expected: index.name(),
                        found: label_index.name(),
                        src: src.clone(),
                        span: local.span.into(),
                    });
                }
            }
            hir::expr::IndexArg::Expr(index_expr) => {
                let Some(expr_type) = infer_hir_type(
                    index_expr,
                    owner_decl_name,
                    declared_types,
                    local_types,
                    dag,
                    tir,
                    registry,
                    builtin_fns,
                    src,
                )?
                else {
                    return Err(unsupported_expr(index_expr, src));
                };
                if !expr_type.is_int_like() {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "index expression must be an integer type, got {}",
                            format_inferred_type(&expr_type, registry)
                        ),
                        src: src.clone(),
                        span: index_expr.span.into(),
                    });
                }
            }
        }
        current = *element;
    }
    Ok(current)
}

fn unsupported_expr(expr: &hir::Expr, src: &NamedSource<Arc<String>>) -> GraphcalError {
    GraphcalError::InternalError {
        message: "HIR expression inference fell back after entering a supported expression"
            .to_string(),
        src: src.clone(),
        span: expr.span.into(),
    }
}
