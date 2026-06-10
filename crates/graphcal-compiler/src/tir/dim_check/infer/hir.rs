//! HIR-backed expression type inference.
//!
//! This is the semantic expression type checker for module-aware declaration and
//! assertion bodies. It consumes HIR references directly: canonical declaration,
//! index, constructor, inline-DAG refs, lexical `LocalId`s, and typed built-in
//! function variants. It must not fall back to source/syntax-AST inference.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::hir::{self, BuiltinFnName, ConstRef, FunctionRef};
use crate::registry::declared_type::IndexTypeRef;
use crate::registry::error::GraphcalError;
use crate::registry::types::{Registry, TypeDef, TypeGenericConstraint, UnionMemberDef};
use crate::syntax::ast::UnaryOp;
use crate::syntax::dimension::{Dimension, Rational};
use crate::syntax::names::{
    FieldName, GenericParamName, IndexName, IndexVariantName, ResolvedIndexVariant, ResolvedName,
    ScopedName, namespace,
};
use crate::syntax::span::Span;
use crate::tir::typed::NatLinearForm;

use super::super::builtins::infer_fn_dim_from_spans;
use super::super::helpers::{
    cartesian_product, expect_scalar, format_inferred_type, resolved_type_matches_inferred,
    struct_type_def_for_inferred,
};
use super::super::{DeclaredType, InferredIndex, InferredStructType, InferredType};

type HirLocalTypes = HashMap<hir::LocalId, InferredType>;

type ResolvedDeclKey = ResolvedName<namespace::Decl>;

/// Infer a HIR expression.
pub(in crate::tir::dim_check) fn infer_hir_type_with_owner(
    expr: &hir::Expr,
    owner_decl_name: Option<&str>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
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
) -> Result<InferredType, GraphcalError> {
    // Recursion choke point: inference recurses once per tree level
    // (unbounded for left-nested operator chains).
    crate::stack::with_stack_growth(|| {
        infer_hir_type_inner(
            expr,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "mirrors infer_hir_type's signature"
)]
fn infer_hir_type_inner(
    expr: &hir::Expr,
    owner_decl_name: Option<&str>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
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
            infer_resolved_decl_ref_type(&target.value, target.span, declared_types, dag, src)?
        }
        hir::ExprKind::ConstRef(target) => {
            infer_hir_const_ref(target, declared_types, dag, registry, src)?
        }
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
        hir::ExprKind::Convert {
            expr: inner,
            target,
        } => infer_hir_convert(
            inner,
            target,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?,
        hir::ExprKind::DisplayTimezone {
            expr: inner,
            timezone,
        } => infer_hir_display_timezone(
            expr,
            inner,
            timezone,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?,
        hir::ExprKind::FieldAccess { expr: inner, field } => infer_hir_field_access(
            inner,
            field,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?,
        hir::ExprKind::ConstructorCall {
            callee,
            generic_args,
            fields,
        } => infer_hir_constructor_call(
            expr,
            callee,
            generic_args,
            fields,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?,
        hir::ExprKind::MapLiteral { entries } => infer_hir_map_literal(
            expr,
            entries,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?,
        hir::ExprKind::Scan {
            source,
            init,
            acc,
            val,
            body,
        } => infer_hir_scan(
            source,
            init,
            acc,
            val,
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
        hir::ExprKind::Unfold {
            init,
            prev,
            curr,
            body,
        } => infer_hir_unfold(
            init,
            prev,
            curr,
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
        hir::ExprKind::Match { scrutinee, arms } => infer_hir_match(
            expr,
            scrutinee,
            arms,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?,
        hir::ExprKind::InlineDagRef {
            target,
            args,
            output,
        } => infer_hir_inline_dag_ref(
            expr,
            target,
            args,
            output,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?,
    };
    Ok(inferred)
}

fn infer_hir_unit_literal(
    unit: &crate::desugar::resolved_ast::UnitExpr,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let dim = rules::resolve_unit_dimension_or_diagnose(unit, registry, src)?;
    Ok(InferredType::Scalar(dim))
}

fn infer_resolved_decl_ref_type(
    target: &ResolvedDeclKey,
    span: Span,
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
        span: span.into(),
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
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    match &target.value {
        ConstRef::Decl(resolved) => {
            infer_resolved_decl_ref_type(resolved, target.span, declared_types, dag, src)
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
        ConstRef::Constructor(constructor) => {
            let target_def = dag
                .semantic
                .constructor_refs
                .constructor_defs
                .get(constructor)
                .ok_or_else(|| GraphcalError::InternalError {
                    message: format!(
                        "semantic constructor metadata missing for nullary constructor `{constructor}`"
                    ),
                    src: src.clone(),
                    span: target.span.into(),
                })?;
            if !target_def.variant.fields.is_empty() {
                return Err(GraphcalError::EvalError {
                    message: format!(
                        "constructor `{}` requires field arguments",
                        target_def.variant.name
                    ),
                    src: src.clone(),
                    span: target.span.into(),
                });
            }
            let type_args = resolve_constructor_generic_args(
                &target_def.owning_type,
                &target_def.type_def,
                &[],
                dag,
                registry,
                src,
                target.span,
            )?;
            Ok(InferredType::Struct(
                InferredStructType::from_resolved(target_def.owning_type.clone()),
                type_args,
            ))
        }
        ConstRef::GenericNatParam(param) => Err(GraphcalError::EvalError {
            message: format!(
                "generic Nat parameter `{}` is type-level only and is not a runtime value",
                param.name
            ),
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
) -> Result<InferredType, GraphcalError> {
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
    let FunctionRef::Builtin(name) = callee.value;
    match name.special_kind() {
        Some(crate::registry::resolve_types::SpecialFnKind::Aggregation(kind))
            if args.len() == 1 =>
        {
            let arg_type = infer_arg(
                &args[0],
                declared_types,
                local_types,
                dag,
                tir,
                registry,
                builtin_fns,
                src,
            )?;
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
            if args.len() != 1 {
                return Err(GraphcalError::WrongArity {
                    name: crate::syntax::names::FnName::new(name.as_str()),
                    expected: 1,
                    got: args.len(),
                    src: src.clone(),
                    span: callee.span.into(),
                });
            }
            let arg_type = infer_arg(
                &args[0],
                declared_types,
                local_types,
                dag,
                tir,
                registry,
                builtin_fns,
                src,
            )?;
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
    }
}

#[expect(clippy::too_many_arguments, reason = "function-call context")]
fn infer_hir_builtin_fn(
    name: BuiltinFnName,
    callee_span: Span,
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
            span: callee_span.into(),
        });
    };
    if args.len() != func.dim_sig.params.len() {
        return Err(GraphcalError::WrongArity {
            name: crate::syntax::names::FnName::new(name.as_str()),
            expected: func.dim_sig.params.len(),
            got: args.len(),
            src: src.clone(),
            span: callee_span.into(),
        });
    }
    let arg_dims: Vec<Dimension> = args
        .iter()
        .map(|arg| {
            let t = infer_arg(
                arg,
                declared_types,
                local_types,
                dag,
                tir,
                registry,
                builtin_fns,
                src,
            )?;
            expect_scalar(&t, registry, src, arg.span)
        })
        .collect::<Result<_, _>>()?;
    let arg_spans: Vec<Span> = args.iter().map(|arg| arg.span).collect();
    infer_fn_dim_from_spans(
        name.as_str(),
        &func.dim_sig,
        &arg_dims,
        &arg_spans,
        registry,
        src,
    )
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
    let arg_type = infer_arg(
        &args[0],
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
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
    let arg_type = infer_arg(
        &args[0],
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
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
                let found = infer_arg(
                    &args[0],
                    declared_types,
                    local_types,
                    dag,
                    tir,
                    registry,
                    builtin_fns,
                    src,
                )?;
                return Err(GraphcalError::DimensionMismatch {
                    expected: "string literal".to_string(),
                    found: format_inferred_type(&found, registry),
                    help: "datetime() requires a string literal argument".to_string(),
                    src: src.clone(),
                    span: args[0].span.into(),
                });
            }
            if args.len() == 2 && !matches!(args[1].kind, hir::ExprKind::StringLiteral(_)) {
                let found = infer_arg(
                    &args[1],
                    declared_types,
                    local_types,
                    dag,
                    tir,
                    registry,
                    builtin_fns,
                    src,
                )?;
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
    let arg_type = infer_arg(
        &args[0],
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
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
    let infer = |expr: &hir::Expr| {
        infer_hir_type(
            expr,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )
    };
    let cond_type = infer(condition)?;
    let then_type = infer(then_branch)?;
    let else_type = infer(else_branch)?;
    rules::if_rule(
        &Operand {
            ty: cond_type,
            span: condition.span,
        },
        &Operand {
            ty: then_type,
            span: then_branch.span,
        },
        &Operand {
            ty: else_type,
            span: else_branch.span,
        },
        registry,
        src,
    )
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
    let operand_type = infer_hir_type(
        operand,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    rules::unary_rule(
        op,
        &Operand {
            ty: operand_type,
            span: operand.span,
        },
        registry,
        src,
    )
}

use super::rules::{self, LiteralExponent, Operand};

fn hir_literal_exponent(expr: &hir::Expr) -> Option<LiteralExponent> {
    match &expr.kind {
        hir::ExprKind::Integer(n) => Some(LiteralExponent::Int(*n)),
        hir::ExprKind::Number(n) => Some(LiteralExponent::Float(*n)),
        hir::ExprKind::UnaryOp {
            op: UnaryOp::Neg,
            operand,
        } => match &operand.kind {
            hir::ExprKind::Integer(n) => Some(LiteralExponent::Int(n.wrapping_neg())),
            hir::ExprKind::Number(n) => Some(LiteralExponent::Float(-*n)),
            _ => None,
        },
        _ => None,
    }
}

fn try_const_int(expr: &hir::Expr) -> Option<i64> {
    use crate::desugar::resolved_ast::BinOp;
    match &expr.kind {
        hir::ExprKind::Integer(n) => Some(*n),
        hir::ExprKind::UnaryOp {
            op: UnaryOp::Neg,
            operand,
        } => try_const_int(operand)?.checked_neg(),
        hir::ExprKind::BinOp { op, lhs, rhs } => {
            let l = try_const_int(lhs)?;
            let r = try_const_int(rhs)?;
            match op {
                BinOp::Add => l.checked_add(r),
                BinOp::Sub => l.checked_sub(r),
                BinOp::Mul => l.checked_mul(r),
                BinOp::Div if r != 0 => l.checked_div(r),
                BinOp::Mod if r != 0 => l.checked_rem(r),
                BinOp::Pow if r >= 0 => u32::try_from(r).ok().and_then(|e| l.checked_pow(e)),
                _ => None,
            }
        }
        _ => None,
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
    let lhs_type = infer_hir_type(
        lhs,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    let rhs_type = infer_hir_type(
        rhs,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    // Only the `^` rule reads the exponent shape; `x ^ -2` is structurally
    // `Unary(Neg, IntLit(2))`, which is still compile-time-known.
    let (rhs_lit, rhs_const_int) = if matches!(op, BinOp::Pow) {
        (hir_literal_exponent(rhs), try_const_int(rhs))
    } else {
        (None, None)
    };
    rules::binop_rule(
        span,
        op,
        &Operand {
            ty: lhs_type,
            span: lhs.span,
        },
        &Operand {
            ty: rhs_type,
            span: rhs.span,
        },
        rhs_lit,
        rhs_const_int,
        registry,
        src,
    )
}

fn hir_nat_to_linear_form(
    expr: &hir::NatExpr,
) -> Result<NatLinearForm, crate::syntax::nat::NatOverflowError> {
    match expr {
        hir::NatExpr::Literal(n, _) => Ok(NatLinearForm::from_constant(*n)),
        hir::NatExpr::Param(param) => Ok(NatLinearForm::from_var(param.value.name.clone())),
        hir::NatExpr::Add(lhs, rhs, _) => {
            hir_nat_to_linear_form(lhs)?.add(&hir_nat_to_linear_form(rhs)?)
        }
        hir::NatExpr::Mul(lhs, rhs, _) => {
            hir_nat_to_linear_form(lhs)?.mul(&hir_nat_to_linear_form(rhs)?)
        }
    }
}

fn nat_overflow_error(
    err: crate::syntax::nat::NatOverflowError,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> GraphcalError {
    GraphcalError::EvalError {
        message: err.to_string(),
        src: src.clone(),
        span: span.into(),
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
        let var_type = match &binding.index {
            hir::expr::ForBindingIndex::Named(index) => {
                let index_identity = InferredIndex::from_resolved(index.value.clone());
                let idx_def = super::collections::index_def_for_inferred(
                    &index_identity,
                    Some(dag),
                    registry,
                )
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
            hir::expr::ForBindingIndex::Range { arg, span } => InferredType::Fin(
                hir_nat_to_linear_form(arg).map_err(|err| nat_overflow_error(err, src, *span))?,
            ),
        };
        inner_locals.insert(binding.local.id, var_type);
    }
    let mut result = infer_hir_type(
        body,
        owner_decl_name,
        declared_types,
        &inner_locals,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    for binding in bindings.iter().rev() {
        let index = match &binding.index {
            hir::expr::ForBindingIndex::Named(index) => {
                InferredIndex::from_resolved(index.value.clone())
            }
            hir::expr::ForBindingIndex::Range { arg, span } => {
                let form = hir_nat_to_linear_form(arg)
                    .map_err(|err| nat_overflow_error(err, src, *span))?;
                InferredIndex::from_nat_range_form(form)
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
    let mut current = infer_hir_type(
        inner,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
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
                match var_type {
                    InferredType::Label(label_index) => {
                        if label_index != &index {
                            return Err(GraphcalError::IndexMismatch {
                                expected: index.name(),
                                found: label_index.name(),
                                src: src.clone(),
                                span: local.span.into(),
                            });
                        }
                    }
                    InferredType::Scalar(_) => {
                        let idx_def =
                            super::collections::index_def_for_inferred(&index, Some(dag), registry)
                                .ok_or_else(|| GraphcalError::UnknownIndex {
                                    name: index.name(),
                                    src: src.clone(),
                                    span: local.span.into(),
                                })?;
                        if !idx_def.is_range() {
                            return Err(GraphcalError::EvalError {
                                message: format!(
                                    "`#{}` is not a range-index loop variable",
                                    local.value.index()
                                ),
                                src: src.clone(),
                                span: local.span.into(),
                            });
                        }
                    }
                    InferredType::Int => {
                        if let Some(idx_def) =
                            super::collections::index_def_for_inferred(&index, Some(dag), registry)
                            && !idx_def.is_nat_range()
                        {
                            return Err(GraphcalError::EvalError {
                                message: format!(
                                    "Int local cannot index into non-nat-range index `{}`",
                                    index.name()
                                ),
                                src: src.clone(),
                                span: local.span.into(),
                            });
                        }
                    }
                    InferredType::Fin(fin_bound) => {
                        let index_form =
                            super::collections::index_def_for_inferred(&index, Some(dag), registry)
                                .map_or_else(
                                    || index.nat_range_form(),
                                    |idx_def| {
                                        if !idx_def.is_nat_range() {
                                            return None;
                                        }
                                        idx_def.nat_range_size().map(NatLinearForm::from_constant)
                                    },
                                );
                        let Some(index_form) = index_form else {
                            return Err(GraphcalError::EvalError {
                                message: format!(
                                    "Fin({}) local cannot index into non-nat-range index `{}`",
                                    fin_bound.format(),
                                    index.name()
                                ),
                                src: src.clone(),
                                span: local.span.into(),
                            });
                        };
                        if !fin_bound.is_leq(&index_form) {
                            return Err(GraphcalError::EvalError {
                                message: format!(
                                    "index out of bounds: local has type Fin({}) but array has size {}",
                                    fin_bound.format(),
                                    index_form.format(),
                                ),
                                src: src.clone(),
                                span: local.span.into(),
                            });
                        }
                    }
                    _ => {
                        return Err(GraphcalError::EvalError {
                            message: format!(
                                "`#{}` is not a valid index variable",
                                local.value.index()
                            ),
                            src: src.clone(),
                            span: local.span.into(),
                        });
                    }
                }
            }
            hir::expr::IndexArg::Expr(index_expr) => {
                let expr_type = infer_hir_type(
                    index_expr,
                    owner_decl_name,
                    declared_types,
                    local_types,
                    dag,
                    tir,
                    registry,
                    builtin_fns,
                    src,
                )?;
                let index_form =
                    super::collections::index_def_for_inferred(&index, Some(dag), registry)
                        .map_or_else(
                            || index.nat_range_form(),
                            |idx_def| {
                                if !idx_def.is_nat_range() {
                                    return None;
                                }
                                idx_def.nat_range_size().map(NatLinearForm::from_constant)
                            },
                        );
                let Some(index_form) = index_form else {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "integer expression cannot index into non-nat-range index `{}`",
                            index.name()
                        ),
                        src: src.clone(),
                        span: index_expr.span.into(),
                    });
                };
                match expr_type {
                    InferredType::Int => {}
                    InferredType::Fin(ref fin_bound) => {
                        if !fin_bound.is_leq(&index_form) {
                            return Err(GraphcalError::EvalError {
                                message: format!(
                                    "index out of bounds: expression has type Fin({}) but array has size {}",
                                    fin_bound.format(),
                                    index_form.format(),
                                ),
                                src: src.clone(),
                                span: index_expr.span.into(),
                            });
                        }
                    }
                    _ => {
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
        }
        current = *element;
    }
    Ok(current)
}

#[expect(clippy::too_many_arguments, reason = "conversion expression context")]
fn infer_hir_convert(
    inner: &hir::Expr,
    target: &crate::desugar::resolved_ast::UnitExpr,
    owner_decl_name: Option<&str>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let inner_type = infer_hir_type(
        inner,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    let expr_dim = expect_scalar(&inner_type, registry, src, inner.span)?;
    let target_dim = rules::resolve_unit_dimension_or_diagnose(target, registry, src)?;

    if expr_dim != target_dim {
        return Err(GraphcalError::ConversionDimensionMismatch {
            target: registry.dimensions.format_dimension(&target_dim),
            expr_dim: registry.dimensions.format_dimension(&expr_dim),
            src: src.clone(),
            span: target.span.into(),
        });
    }

    Ok(InferredType::Scalar(expr_dim))
}

#[expect(
    clippy::too_many_arguments,
    reason = "display timezone expression context"
)]
fn infer_hir_display_timezone(
    expr: &hir::Expr,
    inner: &hir::Expr,
    timezone: &str,
    owner_decl_name: Option<&str>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let inner_type = infer_hir_type(
        inner,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    if !matches!(&inner_type, InferredType::Datetime(_)) {
        return Err(GraphcalError::DimensionMismatch {
            expected: "Datetime".to_string(),
            found: format_inferred_type(&inner_type, registry),
            help: format!("timezone display `-> \"{timezone}\"` requires a Datetime expression"),
            src: src.clone(),
            span: inner.span.into(),
        });
    }
    if jiff::tz::TimeZone::get(timezone).is_err() {
        return Err(GraphcalError::InvalidTimezone {
            timezone: timezone.to_string(),
            src: src.clone(),
            span: expr.span.into(),
        });
    }
    Ok(inner_type)
}

fn resolved_type_field_key(
    owning_type: &ResolvedName<namespace::StructType>,
    constructor: &UnionMemberDef,
    field: &FieldName,
) -> crate::tir::typed::ResolvedStructFieldTypeKey {
    crate::tir::typed::ResolvedStructFieldTypeKey {
        owning_type: owning_type.clone(),
        constructor: constructor.name.clone(),
        field: field.clone(),
    }
}

fn generic_substitutions(
    type_def: &TypeDef,
    type_args: &[InferredType],
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<GenericSubstitutions, GraphcalError> {
    let mut subs = GenericSubstitutions::default();
    for (param, arg) in type_def.generic_params.iter().zip(type_args.iter()) {
        match param.constraint {
            TypeGenericConstraint::Dim => match arg {
                InferredType::Scalar(dim) => {
                    subs.dims.insert(param.name.clone(), dim.clone());
                }
                other => {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "generic parameter `{}` expects a scalar dimension, but got {}",
                            param.name,
                            format_inferred_type(other, registry)
                        ),
                        src: src.clone(),
                        span: span.into(),
                    });
                }
            },
            TypeGenericConstraint::Index => match arg {
                InferredType::Label(index) => {
                    subs.indexes
                        .insert(param.name.clone(), index.type_ref().clone());
                }
                other => {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "generic parameter `{}` expects an index type, but got {}",
                            param.name,
                            format_inferred_type(other, registry)
                        ),
                        src: src.clone(),
                        span: span.into(),
                    });
                }
            },
            TypeGenericConstraint::Nat => {}
            TypeGenericConstraint::Unconstrained => {
                subs.types.insert(param.name.clone(), arg.clone());
            }
        }
    }
    Ok(subs)
}

#[derive(Default)]
struct GenericSubstitutions {
    dims: HashMap<GenericParamName, Dimension>,
    indexes: HashMap<GenericParamName, IndexTypeRef>,
    nats: HashMap<GenericParamName, u64>,
    types: HashMap<GenericParamName, InferredType>,
}

fn substitute_resolved_type_with_type_params(
    resolved: &crate::tir::typed::ResolvedTypeExpr,
    subs: &GenericSubstitutions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    crate::tir::typed::substitute_resolved_type_with_types(
        resolved,
        &subs.dims,
        &subs.indexes,
        &subs.nats,
        &subs.types,
        src,
    )
}

fn resolved_field_type(
    owning_type: &ResolvedName<namespace::StructType>,
    constructor: &UnionMemberDef,
    field: &FieldName,
    type_def: &TypeDef,
    type_args: &[InferredType],
    dag: &crate::tir::typed::DagTIR,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<InferredType, GraphcalError> {
    let key = resolved_type_field_key(owning_type, constructor, field);
    let resolved = dag
        .semantic
        .type_defs
        .field_types
        .get(&key)
        .ok_or_else(|| GraphcalError::InternalError {
            message: format!(
                "semantic type metadata missing field type for `{}.{}`",
                constructor.name, field
            ),
            src: src.clone(),
            span: span.into(),
        })?;
    let subs = generic_substitutions(type_def, type_args, registry, src, span)?;
    substitute_resolved_type_with_type_params(resolved, &subs, src)
}

fn record_member(type_def: &TypeDef) -> Option<&UnionMemberDef> {
    let members = type_def.union_members()?;
    let [only] = members else {
        return None;
    };
    (only.name.as_str() == type_def.name.as_str()).then_some(only)
}

#[expect(clippy::too_many_arguments, reason = "field-access expression context")]
fn infer_hir_field_access(
    inner: &hir::Expr,
    field: &crate::syntax::span::Spanned<FieldName>,
    owner_decl_name: Option<&str>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let inner_type = infer_hir_type(
        inner,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    let InferredType::Struct(type_name, type_args) = &inner_type else {
        return Err(GraphcalError::NotAStruct {
            name: format_inferred_type(&inner_type, registry),
            src: src.clone(),
            span: inner.span.into(),
        });
    };
    let type_def =
        struct_type_def_for_inferred(type_name, Some(dag), registry).ok_or_else(|| {
            GraphcalError::UnknownStructType {
                name: type_name.to_string(),
                src: src.clone(),
                span: inner.span.into(),
            }
        })?;
    let member = record_member(type_def).ok_or_else(|| {
        let detail = if type_def.is_required() {
            format!("required type `{}` has no fields", type_name.name())
        } else {
            format!(
                "union type `{}` (use `match` to access fields)",
                type_name.name()
            )
        };
        GraphcalError::NotAStruct {
            name: detail,
            src: src.clone(),
            span: inner.span.into(),
        }
    })?;
    if !member
        .fields
        .iter()
        .any(|field_def| field_def.name == field.value)
    {
        return Err(GraphcalError::UnknownField {
            type_name: type_name.name().clone(),
            field_name: field.value.clone(),
            src: src.clone(),
            span: field.span.into(),
        });
    }
    resolved_field_type(
        type_name.resolved(),
        member,
        &field.value,
        type_def,
        type_args,
        dag,
        registry,
        src,
        field.span,
    )
}

fn infer_hir_generic_type_arg(
    type_expr: &hir::TypeExpr,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    match &type_expr.kind {
        hir::TypeExprKind::Builtin(hir::BuiltinType::Dimensionless) => {
            Ok(InferredType::Scalar(Dimension::dimensionless()))
        }
        hir::TypeExprKind::Builtin(hir::BuiltinType::Bool) => Ok(InferredType::Bool),
        hir::TypeExprKind::Builtin(hir::BuiltinType::Int) => Ok(InferredType::Int),
        hir::TypeExprKind::Builtin(hir::BuiltinType::Datetime(scale)) => {
            Ok(InferredType::Datetime(scale.scale()))
        }
        hir::TypeExprKind::DimExpr(dim_expr) => {
            infer_hir_dim_expr_arg(dim_expr, registry, src).map(InferredType::Scalar)
        }
        hir::TypeExprKind::Label(index) => Ok(InferredType::Label(InferredIndex::from_resolved(
            index.value.clone(),
        ))),
        hir::TypeExprKind::Struct(name) => Ok(InferredType::Struct(
            InferredStructType::from_resolved(name.value.clone()),
            vec![],
        )),
        hir::TypeExprKind::GenericTypeParam(param) => Err(GraphcalError::EvalError {
            message: format!(
                "generic type parameter `{}` is not bound at this constructor call site",
                param.value.name
            ),
            src: src.clone(),
            span: param.span.into(),
        }),
        hir::TypeExprKind::TypeApplication { name, type_args } => Ok(InferredType::Struct(
            InferredStructType::from_resolved(name.value.clone()),
            type_args
                .iter()
                .map(|arg| infer_hir_generic_type_arg(arg, registry, src))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        hir::TypeExprKind::Indexed { base, indexes } => {
            let mut result = infer_hir_generic_type_arg(base, registry, src)?;
            for index in indexes.iter().rev() {
                let inferred_index = match index {
                    hir::IndexRef::Concrete(index) => {
                        InferredIndex::from_resolved(index.value.clone())
                    }
                    hir::IndexRef::GenericParam(param) => {
                        return Err(GraphcalError::EvalError {
                            message: format!(
                                "generic index parameter `{}` is not bound at this constructor call site",
                                param.value.name
                            ),
                            src: src.clone(),
                            span: param.span.into(),
                        });
                    }
                    hir::IndexRef::NatExpr(nat) => {
                        let form = hir_nat_to_linear_form(nat)
                            .map_err(|err| nat_overflow_error(err, src, nat.span()))?;
                        InferredIndex::from_nat_range_form(form)
                            .map_err(|err| nat_range_error(err, src, nat.span()))?
                    }
                };
                result = InferredType::Indexed {
                    element: Box::new(result),
                    index: inferred_index,
                };
            }
            Ok(result)
        }
    }
}

fn infer_hir_dim_expr_arg(
    dim_expr: &hir::DimExpr,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<Dimension, GraphcalError> {
    dim_expr
        .terms
        .iter()
        .try_fold(Dimension::dimensionless(), |acc, item| {
            let (dim, power, span) = match &item.term.target {
                hir::DimTermTarget::Dimension(target) => {
                    let dim = registry
                        .dimensions
                        .get_dimension(target.value.as_str())
                        .cloned()
                        .ok_or_else(|| GraphcalError::UnknownDimension {
                            name: target.value.to_unowned_def_name(),
                            src: src.clone(),
                            span: target.span.into(),
                        })?;
                    (dim, item.term.power.unwrap_or(1), item.term.span)
                }
                hir::DimTermTarget::GenericParam(param) => {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "generic dimension parameter `{}` is not bound at this constructor call site",
                            param.value.name
                        ),
                        src: src.clone(),
                        span: param.span.into(),
                    });
                }
            };
            let powered = dim.pow(Rational::from_int(power)).map_err(|_| {
                GraphcalError::DimensionOverflow {
                    src: src.clone(),
                    span: span.into(),
                }
            })?;
            match item.op {
                crate::desugar::resolved_ast::MulDivOp::Mul => {
                    (acc * powered).map_err(|_| GraphcalError::DimensionOverflow {
                        src: src.clone(),
                        span: span.into(),
                    })
                }
                crate::desugar::resolved_ast::MulDivOp::Div => {
                    (acc / powered).map_err(|_| GraphcalError::DimensionOverflow {
                        src: src.clone(),
                        span: span.into(),
                    })
                }
            }
        })
}

#[expect(
    clippy::too_many_arguments,
    reason = "constructor-call expression context"
)]
fn infer_hir_constructor_call(
    expr: &hir::Expr,
    callee: &crate::syntax::span::Spanned<ResolvedName<namespace::Constructor>>,
    constructor_generic_args: &[hir::expr::GenericArg],
    fields: &[hir::expr::FieldInit],
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let target = dag
        .semantic
        .constructor_refs
        .constructor_calls
        .get(&callee.span)
        .ok_or_else(|| GraphcalError::InternalError {
            message: format!(
                "semantic TIR missing constructor call target for `{}`",
                callee.value
            ),
            src: src.clone(),
            span: callee.span.into(),
        })?;
    let type_def = &target.type_def;
    let variant = &target.variant;
    let owning_type_identity = InferredStructType::from_resolved(target.owning_type.clone());
    let owning_type_name = type_def.name.clone();

    let resolved_type_args = resolve_constructor_generic_args(
        &target.owning_type,
        type_def,
        constructor_generic_args,
        dag,
        registry,
        src,
        callee.span,
    )?;

    let def_field_names: std::collections::HashSet<&str> = variant
        .fields
        .iter()
        .map(|field| field.name.as_str())
        .collect();
    let provided_names: Vec<&str> = fields
        .iter()
        .map(|field| field.name.value.as_str())
        .collect();
    let mut seen_fields = std::collections::HashSet::new();
    for field in fields {
        if !seen_fields.insert(field.name.value.clone()) {
            return Err(GraphcalError::EvalError {
                message: format!(
                    "duplicate field `{}` in constructor `{}`",
                    field.name.value, variant.name
                ),
                src: src.clone(),
                span: field.name.span.into(),
            });
        }
    }
    let extra: Vec<FieldName> = provided_names
        .iter()
        .filter(|name| !def_field_names.contains(**name))
        .map(|name| FieldName::new(*name))
        .collect();
    if !extra.is_empty() {
        return Err(GraphcalError::ExtraFields {
            type_name: owning_type_name,
            extra,
            src: src.clone(),
            span: expr.span.into(),
        });
    }

    let provided_set: std::collections::HashSet<&str> = provided_names.iter().copied().collect();
    let missing: Vec<FieldName> = variant
        .fields
        .iter()
        .filter(|field| !provided_set.contains(field.name.as_str()))
        .map(|field| field.name.clone())
        .collect();
    if !missing.is_empty() {
        return Err(GraphcalError::MissingFields {
            type_name: owning_type_name,
            missing,
            src: src.clone(),
            span: expr.span.into(),
        });
    }

    for field_init in fields {
        let field_def = variant
            .fields
            .iter()
            .find(|field| field.name == field_init.name.value)
            .ok_or_else(|| GraphcalError::EvalError {
                message: format!(
                    "internal: unknown field `{}` in constructor `{}`",
                    field_init.name.value, variant.name
                ),
                src: src.clone(),
                span: field_init.name.span.into(),
            })?;
        let value_type = infer_hir_type(
            &field_init.value,
            None,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?;
        let expected = resolved_field_type(
            &target.owning_type,
            variant,
            &field_def.name,
            type_def,
            &resolved_type_args,
            dag,
            registry,
            src,
            field_init.name.span,
        )?;
        if value_type != expected {
            return Err(GraphcalError::FieldDimensionMismatch {
                type_name: owning_type_name,
                field_name: field_init.name.value.clone(),
                expected: format_inferred_type(&expected, registry),
                found: format_inferred_type(&value_type, registry),
                src: src.clone(),
                span: field_init.name.span.into(),
            });
        }
    }

    Ok(InferredType::Struct(
        owning_type_identity,
        resolved_type_args,
    ))
}

fn resolve_constructor_generic_args(
    owning_type: &ResolvedName<namespace::StructType>,
    type_def: &TypeDef,
    constructor_generic_args: &[hir::expr::GenericArg],
    dag: &crate::tir::typed::DagTIR,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<Vec<InferredType>, GraphcalError> {
    if constructor_generic_args.is_empty() && type_def.generic_params.is_empty() {
        return Ok(Vec::new());
    }
    let total_params = type_def.generic_params.len();
    let required_count = type_def
        .generic_params
        .iter()
        .take_while(|param| param.default.is_none())
        .count();
    if constructor_generic_args.len() < required_count
        || constructor_generic_args.len() > total_params
    {
        let hint = if required_count == total_params {
            format!("{total_params}")
        } else {
            format!("{required_count}..{total_params}")
        };
        return Err(GraphcalError::EvalError {
            message: format!(
                "type `{}` expects {hint} generic argument(s), got {}",
                type_def.name,
                constructor_generic_args.len()
            ),
            src: src.clone(),
            span: span.into(),
        });
    }
    let mut args = Vec::with_capacity(total_params);
    for (param, arg) in type_def.generic_params.iter().zip(constructor_generic_args) {
        match (param.constraint, arg) {
            (TypeGenericConstraint::Nat, hir::expr::GenericArg::Nat(nat_expr)) => {
                return Err(GraphcalError::EvalError {
                    message: format!(
                        "constructor generic argument `{}` for Nat parameter `{}` cannot be used in constructor value types",
                        // The argument is rejected either way; render a
                        // placeholder if its Nat arithmetic overflows.
                        hir_nat_to_linear_form(nat_expr)
                            .map_or_else(|_| "<overflow>".to_string(), |f| f.format()),
                        param.name
                    ),
                    src: src.clone(),
                    span: nat_expr.span().into(),
                });
            }
            (TypeGenericConstraint::Nat, hir::expr::GenericArg::Type(type_expr)) => {
                return Err(GraphcalError::EvalError {
                    message: format!(
                        "generic parameter `{}` expects a Nat argument, got a type argument",
                        param.name
                    ),
                    src: src.clone(),
                    span: type_expr.span.into(),
                });
            }
            (_, hir::expr::GenericArg::Nat(nat_expr)) => {
                return Err(GraphcalError::EvalError {
                    message: format!(
                        "generic parameter `{}` expects a type argument, got Nat argument `{}`",
                        param.name,
                        // The argument is rejected either way; render a
                        // placeholder if its Nat arithmetic overflows.
                        hir_nat_to_linear_form(nat_expr)
                            .map_or_else(|_| "<overflow>".to_string(), |f| f.format())
                    ),
                    src: src.clone(),
                    span: nat_expr.span().into(),
                });
            }
            (_, hir::expr::GenericArg::Type(type_expr)) => {
                args.push(infer_hir_generic_type_arg(type_expr, registry, src)?);
            }
        }
    }
    for param in type_def
        .generic_params
        .iter()
        .skip(constructor_generic_args.len())
    {
        let resolved_default = dag
            .semantic
            .type_defs
            .generic_defaults
            .get(&(owning_type.clone(), param.name.clone()))
            .ok_or_else(|| GraphcalError::EvalError {
                message: format!(
                    "internal: generic parameter `{}` has no default",
                    param.name
                ),
                src: src.clone(),
                span: span.into(),
            })?;
        let subs = generic_substitutions(type_def, &args, registry, src, span)?;
        args.push(substitute_resolved_type_with_type_params(
            resolved_default,
            &subs,
            src,
        )?);
    }
    Ok(args)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum MapLiteralVariantKey {
    Declared(ResolvedIndexVariant),
    NatRange {
        form: NatLinearForm,
        variant: IndexVariantName,
    },
}

impl MapLiteralVariantKey {
    const fn variant(&self) -> &IndexVariantName {
        match self {
            Self::Declared(resolved) => resolved.variant(),
            Self::NatRange { variant, .. } => variant,
        }
    }

    fn display_index(&self) -> IndexName {
        match self {
            Self::Declared(resolved) => resolved.index().to_unowned_def_name(),
            Self::NatRange { form, .. } => IndexName::new(format!("range({})", form.format())),
        }
    }
}

#[derive(Debug, Clone)]
struct MapLiteralAxis {
    index: InferredIndex,
    variants: Vec<IndexVariantName>,
}

impl MapLiteralAxis {
    fn variant_key(&self, variant: IndexVariantName) -> MapLiteralVariantKey {
        match self.index.type_ref() {
            IndexTypeRef::Declared(reference) => MapLiteralVariantKey::Declared(
                ResolvedIndexVariant::new(reference.resolved().clone(), variant),
            ),
            IndexTypeRef::NatRange(reference) => MapLiteralVariantKey::NatRange {
                form: reference.form(),
                variant,
            },
        }
    }
}

fn inferred_index_for_hir_map_key(
    key: &hir::expr::MapEntryKey,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredIndex, GraphcalError> {
    match key {
        hir::expr::MapEntryKey::IndexVariant(variant) => {
            Ok(InferredIndex::from_resolved(variant.value.index().clone()))
        }
        hir::expr::MapEntryKey::NatRangeVariant { size, variant } => {
            InferredIndex::from_nat_range_form(NatLinearForm::from_constant(*size))
                .map_err(|err| nat_range_error(err, src, variant.span))
        }
    }
}

fn hir_map_key_variant(key: &hir::expr::MapEntryKey) -> IndexVariantName {
    match key {
        hir::expr::MapEntryKey::IndexVariant(variant) => variant.value.variant().clone(),
        hir::expr::MapEntryKey::NatRangeVariant { variant, .. } => variant.value.clone(),
    }
}

#[expect(clippy::too_many_arguments, reason = "map literal expression context")]
#[expect(
    clippy::too_many_lines,
    reason = "exhaustive validation of map literal entries"
)]
fn infer_hir_map_literal(
    expr: &hir::Expr,
    entries: &[hir::expr::MapEntry],
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let Some(first_entry) = entries.first() else {
        return Err(GraphcalError::EvalError {
            message: "empty map literal".to_string(),
            src: src.clone(),
            span: expr.span.into(),
        });
    };
    let arity = first_entry.keys.len();
    for entry in entries.iter().skip(1) {
        if entry.keys.len() != arity {
            return Err(GraphcalError::EvalError {
                message: format!(
                    "map literal entries have inconsistent key arity: expected {arity}, found {}",
                    entry.keys.len()
                ),
                src: src.clone(),
                span: expr.span.into(),
            });
        }
    }

    let mut axes = Vec::with_capacity(arity);
    for key in &first_entry.keys {
        let index = inferred_index_for_hir_map_key(key, src)?;
        let idx_def = super::collections::index_def_for_inferred(&index, Some(dag), registry)
            .ok_or_else(|| GraphcalError::UnknownIndex {
                name: index.name(),
                src: src.clone(),
                span: expr.span.into(),
            })?;
        if idx_def.is_range() {
            return Err(GraphcalError::EvalError {
                message: format!(
                    "range index `{}` cannot be used as a map/table literal key; use a `for` comprehension instead",
                    index.name()
                ),
                src: src.clone(),
                span: expr.span.into(),
            });
        }
        axes.push(MapLiteralAxis {
            index,
            variants: idx_def.variants(),
        });
    }
    for entry in entries.iter().skip(1) {
        for (i, key) in entry.keys.iter().enumerate() {
            let key_index = inferred_index_for_hir_map_key(key, src)?;
            if key_index != axes[i].index {
                return Err(GraphcalError::IndexMismatch {
                    expected: axes[i].index.name(),
                    found: key_index.name(),
                    src: src.clone(),
                    span: expr.span.into(),
                });
            }
        }
    }

    let axes_variant_keys: Vec<Vec<MapLiteralVariantKey>> = axes
        .iter()
        .map(|axis| {
            axis.variants
                .iter()
                .cloned()
                .map(|variant| axis.variant_key(variant))
                .collect()
        })
        .collect();
    let mut expected_tuples = std::collections::HashSet::new();
    cartesian_product(&axes_variant_keys, &mut Vec::new(), &mut expected_tuples);
    let mut provided_tuples = std::collections::HashSet::new();
    for entry in entries {
        let tuple: Vec<MapLiteralVariantKey> = entry
            .keys
            .iter()
            .enumerate()
            .map(|(i, key)| match key {
                hir::expr::MapEntryKey::IndexVariant(variant) => {
                    MapLiteralVariantKey::Declared(variant.value.clone())
                }
                hir::expr::MapEntryKey::NatRangeVariant { variant, .. } => {
                    axes[i].variant_key(variant.value.clone())
                }
            })
            .collect();
        if !provided_tuples.insert(tuple.clone()) {
            return Err(GraphcalError::EvalError {
                message: "duplicate map literal entry".to_string(),
                src: src.clone(),
                span: expr.span.into(),
            });
        }
        if arity > 1 {
            for (i, key) in entry.keys.iter().enumerate() {
                let key_variant = hir_map_key_variant(key);
                if !axes[i]
                    .variants
                    .iter()
                    .any(|variant| variant == &key_variant)
                {
                    return Err(GraphcalError::UnknownVariant {
                        index_name: axes[i].index.name(),
                        variant_name: key_variant,
                        src: src.clone(),
                        span: expr.span.into(),
                    });
                }
            }
        }
    }

    let extra: Vec<Vec<MapLiteralVariantKey>> = provided_tuples
        .difference(&expected_tuples)
        .cloned()
        .collect();
    if !extra.is_empty() {
        if arity == 1 {
            return Err(GraphcalError::ExtraVariants {
                index_name: axes[0].index.name(),
                extra: extra
                    .iter()
                    .map(|tuple| tuple[0].variant().clone())
                    .collect(),
                src: src.clone(),
                span: expr.span.into(),
            });
        }
        let extra_strs: Vec<String> = extra
            .iter()
            .map(|tuple| {
                tuple
                    .iter()
                    .map(|variant| {
                        let display_index = variant.display_index();
                        variant.variant().qualified_by(&display_index).to_string()
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .collect();
        return Err(GraphcalError::EvalError {
            message: format!(
                "extra entries in map literal: ({})",
                extra_strs.join("), (")
            ),
            src: src.clone(),
            span: expr.span.into(),
        });
    }
    let missing: Vec<Vec<MapLiteralVariantKey>> = expected_tuples
        .difference(&provided_tuples)
        .cloned()
        .collect();
    if !missing.is_empty() {
        if arity == 1 {
            return Err(GraphcalError::MissingVariants {
                index_name: axes[0].index.name(),
                missing: missing
                    .iter()
                    .map(|tuple| tuple[0].variant().clone())
                    .collect(),
                src: src.clone(),
                span: expr.span.into(),
            });
        }
        let missing_strs: Vec<String> = missing
            .iter()
            .map(|tuple| {
                tuple
                    .iter()
                    .map(|variant| {
                        let display_index = variant.display_index();
                        variant.variant().qualified_by(&display_index).to_string()
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .collect();
        return Err(GraphcalError::EvalError {
            message: format!(
                "non-exhaustive map literal: missing entries for ({})",
                missing_strs.join("), (")
            ),
            src: src.clone(),
            span: expr.span.into(),
        });
    }

    let first_type = infer_hir_type(
        &first_entry.value,
        None,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    if let InferredType::Indexed { index, .. } = &first_type {
        let inner_is_label = super::collections::index_def_for_inferred(index, Some(dag), registry)
            .is_some_and(|def| !def.is_range());
        if inner_is_label {
            return Err(GraphcalError::EvalError {
                message: "map literal element type must be a value type, not an indexed type; use tuple keys for multi-axis map literals".to_string(),
                src: src.clone(),
                span: first_entry.value.span.into(),
            });
        }
    }
    for entry in entries.iter().skip(1) {
        let entry_type = infer_hir_type(
            &entry.value,
            None,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?;
        if entry_type != first_type {
            return Err(GraphcalError::DimensionMismatchInAnnotation {
                declared: format_inferred_type(&first_type, registry),
                inferred: format_inferred_type(&entry_type, registry),
                src: src.clone(),
                span: entry.value.span.into(),
            });
        }
    }
    let mut result = first_type;
    for axis in axes.iter().rev() {
        result = InferredType::Indexed {
            element: Box::new(result),
            index: axis.index.clone(),
        };
    }
    Ok(result)
}

#[expect(clippy::too_many_arguments, reason = "scan expression context")]
fn infer_hir_scan(
    source: &hir::Expr,
    init: &hir::Expr,
    acc: &hir::LocalDef,
    val: &hir::LocalDef,
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
    let source_type = infer_hir_type(
        source,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    let InferredType::Indexed { element, index } = source_type else {
        return Err(GraphcalError::EvalError {
            message: "scan source must be an indexed value".to_string(),
            src: src.clone(),
            span: source.span.into(),
        });
    };
    let init_type = infer_hir_type(
        init,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    if init_type != *element {
        return Err(GraphcalError::DimensionMismatch {
            expected: format_inferred_type(&element, registry),
            found: format_inferred_type(&init_type, registry),
            help: "scan init value must match element type of source".to_string(),
            src: src.clone(),
            span: init.span.into(),
        });
    }
    let mut scan_locals = local_types.clone();
    scan_locals.insert(acc.id, *element.clone());
    scan_locals.insert(val.id, *element.clone());
    let body_type = infer_hir_type(
        body,
        owner_decl_name,
        declared_types,
        &scan_locals,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    if body_type != *element {
        return Err(GraphcalError::DimensionMismatch {
            expected: format_inferred_type(&element, registry),
            found: format_inferred_type(&body_type, registry),
            help: "scan body must return the same type as the accumulator".to_string(),
            src: src.clone(),
            span: body.span.into(),
        });
    }
    Ok(InferredType::Indexed { element, index })
}

#[expect(clippy::too_many_arguments, reason = "unfold expression context")]
fn infer_hir_unfold(
    init: &hir::Expr,
    prev: &hir::LocalDef,
    curr: &hir::LocalDef,
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
    let init_type = infer_hir_type(
        init,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    let owner_range_index = owner_decl_name.and_then(|name| {
        let resolved = dag.resolved_decl_types.get(&ScopedName::local(name))?;
        let crate::tir::typed::ResolvedTypeExpr::Indexed { indexes, .. } = resolved else {
            return None;
        };
        let crate::tir::typed::ResolvedIndex::Concrete(index, _) = indexes.first()? else {
            return None;
        };
        let idx_def = dag.semantic.collection_refs.index_defs.get(index)?;
        idx_def
            .is_range()
            .then(|| (InferredIndex::from_resolved(index.clone()), idx_def))
    });
    let (index, idx_def) = owner_range_index.ok_or_else(|| GraphcalError::EvalError {
        message:
            "unfold expression must appear in a declaration with a concrete range-indexed type"
                .to_string(),
        src: src.clone(),
        span: body.span.into(),
    })?;
    let dimension = match &idx_def.kind {
        crate::registry::types::IndexKind::Range(data) => data.dimension.clone(),
        crate::registry::types::IndexKind::RequiredRange { dimension } => dimension.clone(),
        _ => {
            return Err(GraphcalError::EvalError {
                message: format!("unfold requires a range index, got `{}`", index.name()),
                src: src.clone(),
                span: body.span.into(),
            });
        }
    };
    let mut scan_locals = local_types.clone();
    scan_locals.insert(prev.id, InferredType::Scalar(dimension.clone()));
    scan_locals.insert(curr.id, InferredType::Scalar(dimension));
    let body_type = infer_hir_type(
        body,
        owner_decl_name,
        declared_types,
        &scan_locals,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    if body_type != init_type {
        return Err(GraphcalError::DimensionMismatch {
            expected: format_inferred_type(&init_type, registry),
            found: format_inferred_type(&body_type, registry),
            help: "time scan body must return the same type as the init value".to_string(),
            src: src.clone(),
            span: body.span.into(),
        });
    }
    Ok(InferredType::Indexed {
        element: Box::new(init_type),
        index,
    })
}

fn constructor_field_type(
    field: &crate::syntax::span::Spanned<FieldName>,
    variant: &UnionMemberDef,
    owning_type: &ResolvedName<namespace::StructType>,
    type_def: &TypeDef,
    scrutinee_type_args: &[InferredType],
    dag: &crate::tir::typed::DagTIR,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    if !variant
        .fields
        .iter()
        .any(|field_def| field_def.name == field.value)
    {
        return Err(GraphcalError::UnknownField {
            type_name: type_def.name.clone(),
            field_name: field.value.clone(),
            src: src.clone(),
            span: field.span.into(),
        });
    }
    resolved_field_type(
        owning_type,
        variant,
        &field.value,
        type_def,
        scrutinee_type_args,
        dag,
        registry,
        src,
        field.span,
    )
}

#[expect(clippy::too_many_arguments, reason = "match expression context")]
#[expect(clippy::too_many_lines, reason = "exhaustive handling of match arms")]
fn infer_hir_match(
    expr: &hir::Expr,
    scrutinee: &hir::Expr,
    arms: &[hir::expr::MatchArm],
    owner_decl_name: Option<&str>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let scrutinee_type = infer_hir_type(
        scrutinee,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    match &scrutinee_type {
        InferredType::Label(index_identity) => {
            let index_def =
                super::collections::index_def_for_inferred(index_identity, Some(dag), registry)
                    .ok_or_else(|| GraphcalError::UnknownIndex {
                        name: index_identity.name(),
                        src: src.clone(),
                        span: scrutinee.span.into(),
                    })?;
            let variants = match &index_def.kind {
                crate::registry::types::IndexKind::Named { variants } => variants.clone(),
                crate::registry::types::IndexKind::RequiredNamed => vec![],
                _ => {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "cannot match on range index `{}`; only named indexes can be matched",
                            index_identity.name()
                        ),
                        src: src.clone(),
                        span: scrutinee.span.into(),
                    });
                }
            };
            let mut covered = std::collections::HashSet::new();
            let mut arm_types = Vec::new();
            for arm in arms {
                let hir::expr::MatchPattern::IndexLabel { variant, span } = &arm.pattern else {
                    return Err(GraphcalError::EvalError {
                        message: "label match arms must use index-label patterns".to_string(),
                        src: src.clone(),
                        span: arm.span.into(),
                    });
                };
                if !index_identity.matches_resolved(variant.value.index()) {
                    return Err(GraphcalError::IndexMismatch {
                        expected: index_identity.name(),
                        found: variant.value.index().to_unowned_def_name(),
                        src: src.clone(),
                        span: (*span).into(),
                    });
                }
                let variant_name = variant.value.variant();
                if !variants.iter().any(|v| v == variant_name) {
                    return Err(GraphcalError::UnknownVariant {
                        index_name: index_identity.name(),
                        variant_name: variant_name.clone(),
                        src: src.clone(),
                        span: variant.span.into(),
                    });
                }
                if !covered.insert(variant_name.clone()) {
                    return Err(GraphcalError::EvalError {
                        message: format!("duplicate match arm for variant `{variant_name}`"),
                        src: src.clone(),
                        span: (*span).into(),
                    });
                }
                arm_types.push(infer_hir_type(
                    &arm.body,
                    owner_decl_name,
                    declared_types,
                    local_types,
                    dag,
                    tir,
                    registry,
                    builtin_fns,
                    src,
                )?);
            }
            for variant in variants {
                if !covered.contains(&variant) {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "non-exhaustive match: variant `{}` not covered",
                            variant.qualified_by(&index_identity.name())
                        ),
                        src: src.clone(),
                        span: expr.span.into(),
                    });
                }
            }
            hir_arm_types_match(&arm_types, arms, registry, src, expr)
        }
        InferredType::Struct(type_name, scrutinee_type_args) => {
            let type_def = struct_type_def_for_inferred(type_name, Some(dag), registry)
                .ok_or_else(|| GraphcalError::UnknownStructType {
                    name: type_name.to_string(),
                    src: src.clone(),
                    span: scrutinee.span.into(),
                })?;
            let mut covered = std::collections::HashSet::new();
            let mut arm_types = Vec::new();
            for arm in arms {
                let hir::expr::MatchPattern::Constructor {
                    constructor,
                    bindings,
                    span,
                } = &arm.pattern
                else {
                    return Err(GraphcalError::EvalError {
                        message: "union match arms must use constructor patterns".to_string(),
                        src: src.clone(),
                        span: arm.span.into(),
                    });
                };
                let pattern = dag
                    .semantic
                    .constructor_refs
                    .match_pattern_constructors
                    .get(&constructor.span)
                    .ok_or_else(|| GraphcalError::InternalError {
                        message: format!(
                            "semantic TIR missing constructor match target for `{}`",
                            constructor.value
                        ),
                        src: src.clone(),
                        span: constructor.span.into(),
                    })?;
                if !type_name.matches_resolved(&pattern.target.owning_type) {
                    return Err(GraphcalError::UnknownField {
                        type_name: type_name.name().clone(),
                        field_name: FieldName::new(pattern.target.variant.name.as_str()),
                        src: src.clone(),
                        span: constructor.span.into(),
                    });
                }
                if !covered.insert(pattern.target.variant.name.clone()) {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "duplicate match arm for `{}`",
                            pattern.target.variant.name
                        ),
                        src: src.clone(),
                        span: (*span).into(),
                    });
                }
                let mut arm_locals = local_types.clone();
                let mut seen_pattern_fields = std::collections::HashSet::new();
                for binding in bindings {
                    let field = match binding {
                        hir::expr::PatternBinding::Bind { field, .. }
                        | hir::expr::PatternBinding::Wildcard { field, .. } => field,
                    };
                    if !seen_pattern_fields.insert(field.value.clone()) {
                        return Err(GraphcalError::EvalError {
                            message: format!(
                                "duplicate pattern binding for field `{}` in `{}`",
                                field.value, pattern.target.variant.name
                            ),
                            src: src.clone(),
                            span: field.span.into(),
                        });
                    }
                    let field_type = constructor_field_type(
                        field,
                        &pattern.target.variant,
                        &pattern.target.owning_type,
                        &pattern.target.type_def,
                        scrutinee_type_args,
                        dag,
                        registry,
                        src,
                    )?;
                    match binding {
                        hir::expr::PatternBinding::Bind { local, .. } => {
                            arm_locals.insert(local.id, field_type);
                        }
                        hir::expr::PatternBinding::Wildcard { .. } => {}
                    }
                }
                arm_types.push(infer_hir_type(
                    &arm.body,
                    owner_decl_name,
                    declared_types,
                    &arm_locals,
                    dag,
                    tir,
                    registry,
                    builtin_fns,
                    src,
                )?);
            }
            if let Some(members) = type_def.union_members() {
                for member in members {
                    if !covered.contains(&member.name) {
                        return Err(GraphcalError::EvalError {
                            message: format!(
                                "non-exhaustive match: member `{}` not covered",
                                member.name
                            ),
                            src: src.clone(),
                            span: expr.span.into(),
                        });
                    }
                }
            }
            hir_arm_types_match(&arm_types, arms, registry, src, expr)
        }
        _ => Err(GraphcalError::EvalError {
            message: format!(
                "cannot match on type `{}`; expected a tagged union or label value",
                format_inferred_type(&scrutinee_type, registry)
            ),
            src: src.clone(),
            span: scrutinee.span.into(),
        }),
    }
}

fn hir_arm_types_match(
    arm_types: &[InferredType],
    arms: &[hir::expr::MatchArm],
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    expr: &hir::Expr,
) -> Result<InferredType, GraphcalError> {
    rules::match_arms_rule(arm_types, |i| arms[i].body.span, expr.span, registry, src)
}

#[expect(clippy::too_many_arguments, reason = "inline-DAG expression context")]
fn infer_hir_inline_dag_ref(
    expr: &hir::Expr,
    target: &crate::syntax::span::Spanned<crate::dag_id::DagId>,
    args: &[hir::expr::ParamBinding],
    output: &crate::syntax::span::Spanned<ResolvedName<namespace::Decl>>,
    owner_decl_name: Option<&str>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let display_path = target.value.to_string();
    let resolved_call = dag
        .semantic
        .inline_dag_refs
        .calls
        .get(&expr.span)
        .ok_or_else(|| GraphcalError::InternalError {
            message: format!("semantic TIR missing inline-DAG call metadata for `{display_path}`"),
            src: src.clone(),
            span: expr.span.into(),
        })?;
    if resolved_call.target != target.value {
        return Err(GraphcalError::InternalError {
            message: format!(
                "semantic inline-DAG metadata target `{}` disagrees with HIR target `{}`",
                resolved_call.target, target.value
            ),
            src: src.clone(),
            span: target.span.into(),
        });
    }
    let dag_tir = tir
        .dags
        .get(&target.value)
        .ok_or_else(|| GraphcalError::UnknownDag {
            name: display_path.clone(),
            src: src.clone(),
            span: target.span.into(),
        })?;

    let mut required_param_keys = std::collections::HashSet::new();
    let param_decl_types_by_key: HashMap<ResolvedDeclKey, &crate::tir::typed::ResolvedTypeExpr> =
        dag_tir
            .params
            .iter()
            .map(|param| {
                let key = dag_tir
                    .resolved_decl_key_for_local(&param.name)
                    .ok_or_else(|| GraphcalError::InternalError {
                        message: format!(
                            "semantic declaration key missing for inline-DAG param `{}`",
                            param.name
                        ),
                        src: src.clone(),
                        span: param.span.into(),
                    })?;
                if param.default_expr.is_none() {
                    required_param_keys.insert(key.clone());
                }
                let resolved = dag_tir
                    .resolved_decl_types
                    .get(&param.name)
                    .ok_or_else(|| GraphcalError::InternalError {
                        message: format!(
                            "semantic type missing for inline-DAG param `{}`",
                            param.name
                        ),
                        src: src.clone(),
                        span: param.type_ann.span.into(),
                    })?;
                Ok((key, resolved))
            })
            .collect::<Result<_, GraphcalError>>()?;
    let node_decl_types_by_key: HashMap<ResolvedDeclKey, &crate::tir::typed::ResolvedTypeExpr> =
        dag_tir
            .nodes
            .iter()
            .map(|node| {
                let key = dag_tir
                    .resolved_decl_key_for_local(&node.name)
                    .ok_or_else(|| GraphcalError::InternalError {
                        message: format!(
                            "semantic declaration key missing for inline-DAG node `{}`",
                            node.name
                        ),
                        src: src.clone(),
                        span: node.span.into(),
                    })?;
                let resolved = dag_tir.resolved_decl_types.get(&node.name).ok_or_else(|| {
                    GraphcalError::InternalError {
                        message: format!(
                            "semantic type missing for inline-DAG node `{}`",
                            node.name
                        ),
                        src: src.clone(),
                        span: node.type_ann.span.into(),
                    }
                })?;
                Ok((key, resolved))
            })
            .collect::<Result<_, GraphcalError>>()?;

    let mut bound_resolved_names: std::collections::HashSet<ResolvedDeclKey> =
        std::collections::HashSet::with_capacity(args.len());
    for binding in args {
        let target_key = resolved_call
            .arg_targets
            .get(&binding.target.span)
            .ok_or_else(|| GraphcalError::InternalError {
                message: format!(
                    "semantic TIR missing inline-DAG arg target for binding `{}`",
                    binding.target.value
                ),
                src: src.clone(),
                span: binding.target.span.into(),
            })?;
        bound_resolved_names.insert(target_key.clone());
        let expected = param_decl_types_by_key.get(target_key).ok_or_else(|| {
            GraphcalError::UnknownInlineDagParam {
                name: target_key.as_str().to_string(),
                dag_name: display_path.clone(),
                src: src.clone(),
                span: binding.target.span.into(),
            }
        })?;
        let found = infer_hir_type(
            &binding.value,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?;
        if !resolved_type_matches_inferred(expected, &found) {
            return Err(GraphcalError::InlineDagArgDimensionMismatch {
                param_name: target_key.as_str().to_string(),
                expected: expected.format(registry),
                found: format_inferred_type(&found, registry),
                src: src.clone(),
                span: binding.value.span.into(),
            });
        }
    }

    let mut missing: Vec<String> = required_param_keys
        .iter()
        .filter(|param| !bound_resolved_names.contains(*param))
        .map(|param| param.as_str().to_string())
        .collect();
    if !missing.is_empty() {
        missing.sort();
        return Err(GraphcalError::MissingInlineDagBindings {
            missing,
            dag_name: display_path.clone(),
            src: src.clone(),
            span: expr.span.into(),
        });
    }

    let output_key = &resolved_call.output.value;
    if output_key != &output.value {
        return Err(GraphcalError::InternalError {
            message: format!(
                "semantic inline-DAG metadata output `{}` disagrees with HIR output `{}`",
                output_key, output.value
            ),
            src: src.clone(),
            span: output.span.into(),
        });
    }
    let output_decl = node_decl_types_by_key.get(output_key).ok_or_else(|| {
        GraphcalError::UnknownInlineDagOutput {
            name: output_key.as_str().to_string(),
            dag_name: display_path.clone(),
            src: src.clone(),
            span: output.span.into(),
        }
    })?;
    let output_name = output_key.as_str();
    if !dag_tir.pub_nodes.contains(output_name) {
        return Err(GraphcalError::ImportPrivateItem {
            name: output_name.to_string(),
            file_path: display_path,
            src: src.clone(),
            span: output.span.into(),
        });
    }
    substitute_resolved_type_with_type_params(output_decl, &GenericSubstitutions::default(), src)
}
