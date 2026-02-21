use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;
use miette::NamedSource;

use graphcal_syntax::ast::{BinOp, Expr, ExprKind, FnBody, MapEntry, UnaryOp};
use graphcal_syntax::names::{FieldName, IndexName, StructTypeName, VariantName};
use graphcal_syntax::span::Span;

use crate::builtins::BuiltinFunction;
use crate::error::GraphcalError;
use crate::registry::Registry;

/// A runtime value: either a scalar (f64 in SI units), a bool, a struct, or an indexed collection.
#[derive(Debug, Clone)]
pub enum RuntimeValue {
    Scalar(f64),
    Bool(bool),
    Int(i64),
    Struct {
        type_name: StructTypeName,
        variant: VariantName,
        fields: IndexMap<FieldName, Self>,
    },
    /// An indexed collection: maps variant names to values, preserving declaration order.
    Indexed {
        index_name: IndexName,
        entries: IndexMap<VariantName, Self>,
    },
    /// A range index label during `Unfold` iteration.
    /// Carries the step index and SI value (for arithmetic like `t - prev_t`).
    RangeLabel {
        step_index: usize,
        value: f64,
    },
}

impl RuntimeValue {
    /// Extract scalar value, returning an error message if this is not a scalar.
    /// (Type mismatches should be caught by `dim_check`; this is defense-in-depth.)
    fn expect_scalar(&self, context: &str) -> Result<f64, String> {
        match self {
            Self::Scalar(v) => Ok(*v),
            Self::Bool(_) => Err(format!("expected scalar for {context}, got Bool")),
            Self::Int(i) => Err(format!("expected scalar for {context}, got Int({i})")),
            Self::Struct { type_name, .. } => Err(format!(
                "expected scalar for {context}, got struct `{type_name}`"
            )),
            Self::Indexed { index_name, .. } => Err(format!(
                "expected scalar for {context}, got indexed value `{index_name}[...]`"
            )),
            Self::RangeLabel { value, .. } => Ok(*value),
        }
    }

    /// Extract boolean value, returning an error message if this is not a Bool.
    /// (Type mismatches should be caught by `dim_check`; this is defense-in-depth.)
    fn expect_bool(&self, context: &str) -> Result<bool, String> {
        match self {
            Self::Bool(b) => Ok(*b),
            other => Err(format!("expected Bool for {context}, got {other:?}")),
        }
    }
}

/// Evaluate an expression given a set of resolved values and built-in functions.
/// Returns a `RuntimeValue` (scalar or struct).
///
/// # Errors
///
/// Returns a [`GraphcalError`] if the expression references an undefined variable,
/// constant, or function.
#[expect(
    clippy::too_many_lines,
    reason = "single match over all ExprKind variants"
)]
pub fn eval_expr(
    expr: &Expr,
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, BuiltinFunction>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    match &expr.kind {
        ExprKind::Number(n) => Ok(RuntimeValue::Scalar(*n)),
        ExprKind::Integer(n) => Ok(RuntimeValue::Int(*n)),
        ExprKind::UnitLiteral { value, unit } => {
            let (_dim, scale) =
                registry
                    .units
                    .resolve_unit_expr(unit)
                    .ok_or_else(|| GraphcalError::EvalError {
                        message: "unknown unit in literal".to_string(),
                        src: src.clone(),
                        span: unit.span.into(),
                    })?;
            Ok(RuntimeValue::Scalar(*value * scale))
        }
        ExprKind::Bool(b) => Ok(RuntimeValue::Bool(*b)),
        ExprKind::VariantLiteral { index, variant } => Ok(RuntimeValue::Struct {
            type_name: StructTypeName::new(index.value.as_str()),
            variant: variant.value.clone(),
            fields: IndexMap::new(),
        }),
        ExprKind::GraphRef(ident) | ExprKind::QualifiedGraphRef { name: ident, .. } => values
            .get(ident.value.as_str())
            .cloned()
            .ok_or_else(|| GraphcalError::EvalError {
                message: format!("undefined graph reference `@{}`", ident.value),
                src: src.clone(),
                span: expr.span.into(),
            }),
        ExprKind::ConstRef(ident) | ExprKind::QualifiedConstRef { name: ident, .. } => values
            .get(ident.value.as_str())
            .cloned()
            .or_else(|| {
                builtin_consts
                    .get(ident.value.as_str())
                    .map(|v| RuntimeValue::Scalar(*v))
            })
            .ok_or_else(|| GraphcalError::EvalError {
                message: format!("undefined constant `{}`", ident.value),
                src: src.clone(),
                span: expr.span.into(),
            }),
        ExprKind::LocalRef(ident) => {
            local_values
                .get(ident.name.as_str())
                .cloned()
                .ok_or_else(|| GraphcalError::EvalError {
                    message: format!("undefined local variable `{}`", ident.name),
                    src: src.clone(),
                    span: expr.span.into(),
                })
        }
        ExprKind::BinOp { op, lhs, rhs } => match op {
            // Logical operators: Bool operands, Bool result
            BinOp::And => {
                let l = eval_expr(
                    lhs,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?
                .expect_bool("AND operand")
                .map_err(|msg| GraphcalError::EvalError {
                    message: msg,
                    src: src.clone(),
                    span: expr.span.into(),
                })?;
                let r = eval_expr(
                    rhs,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?
                .expect_bool("AND operand")
                .map_err(|msg| GraphcalError::EvalError {
                    message: msg,
                    src: src.clone(),
                    span: expr.span.into(),
                })?;
                Ok(RuntimeValue::Bool(l && r))
            }
            BinOp::Or => {
                let l = eval_expr(
                    lhs,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?
                .expect_bool("OR operand")
                .map_err(|msg| GraphcalError::EvalError {
                    message: msg,
                    src: src.clone(),
                    span: expr.span.into(),
                })?;
                let r = eval_expr(
                    rhs,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?
                .expect_bool("OR operand")
                .map_err(|msg| GraphcalError::EvalError {
                    message: msg,
                    src: src.clone(),
                    span: expr.span.into(),
                })?;
                Ok(RuntimeValue::Bool(l || r))
            }
            // Equality: compare same-typed value-level entities.
            BinOp::Eq | BinOp::Ne => {
                let l = eval_expr(
                    lhs,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?;
                let r = eval_expr(
                    rhs,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?;
                let is_eq = *op == BinOp::Eq;
                match (&l, &r) {
                    (RuntimeValue::Bool(lb), RuntimeValue::Bool(rb)) => {
                        Ok(RuntimeValue::Bool(if is_eq { lb == rb } else { lb != rb }))
                    }
                    (RuntimeValue::Int(li), RuntimeValue::Int(ri)) => {
                        Ok(RuntimeValue::Bool(if is_eq { li == ri } else { li != ri }))
                    }
                    (RuntimeValue::Struct { .. }, RuntimeValue::Struct { .. }) => {
                        let eq = runtime_value_equals(&l, &r);
                        Ok(RuntimeValue::Bool(if is_eq { eq } else { !eq }))
                    }
                    _ => {
                        let lv = l.expect_scalar("comparison operand").map_err(|msg| {
                            GraphcalError::EvalError {
                                message: msg,
                                src: src.clone(),
                                span: expr.span.into(),
                            }
                        })?;
                        let rv = r.expect_scalar("comparison operand").map_err(|msg| {
                            GraphcalError::EvalError {
                                message: msg,
                                src: src.clone(),
                                span: expr.span.into(),
                            }
                        })?;
                        Ok(RuntimeValue::Bool(eval_comparison(
                            *op, lv, rv, src, expr.span,
                        )?))
                    }
                }
            }
            // Ordering comparisons: Int or Scalar operands, Bool result
            BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
                let l = eval_expr(
                    lhs,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?;
                let r = eval_expr(
                    rhs,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?;
                if let (RuntimeValue::Int(li), RuntimeValue::Int(ri)) = (&l, &r) {
                    let result = match op {
                        BinOp::Lt => li < ri,
                        BinOp::Gt => li > ri,
                        BinOp::Le => li <= ri,
                        BinOp::Ge => li >= ri,
                        _ => {
                            return Err(GraphcalError::EvalError {
                                message: format!(
                                    "internal: unexpected operator {op:?} in integer comparison"
                                ),
                                src: src.clone(),
                                span: expr.span.into(),
                            });
                        }
                    };
                    return Ok(RuntimeValue::Bool(result));
                }
                let lv = l.expect_scalar("comparison operand").map_err(|msg| {
                    GraphcalError::EvalError {
                        message: msg,
                        src: src.clone(),
                        span: expr.span.into(),
                    }
                })?;
                let rv = r.expect_scalar("comparison operand").map_err(|msg| {
                    GraphcalError::EvalError {
                        message: msg,
                        src: src.clone(),
                        span: expr.span.into(),
                    }
                })?;
                Ok(RuntimeValue::Bool(eval_comparison(
                    *op, lv, rv, src, expr.span,
                )?))
            }
            // Arithmetic operators: Int, Scalar, or derived struct operands
            _ => {
                let l = eval_expr(
                    lhs,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?;
                let r = eval_expr(
                    rhs,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?;
                if let (RuntimeValue::Int(li), RuntimeValue::Int(ri)) = (&l, &r) {
                    return Ok(RuntimeValue::Int(eval_int_binop(
                        *op, *li, *ri, src, expr.span,
                    )?));
                }
                // Component-wise struct operations for derive(Add)/derive(Sub)
                if let (
                    RuntimeValue::Struct {
                        type_name,
                        variant,
                        fields: lhs_fields,
                    },
                    RuntimeValue::Struct {
                        fields: rhs_fields, ..
                    },
                ) = (&l, &r)
                {
                    return eval_struct_binop(
                        *op, type_name, variant, lhs_fields, rhs_fields, src, expr.span,
                    );
                }
                let lv =
                    l.expect_scalar("binary operand")
                        .map_err(|msg| GraphcalError::EvalError {
                            message: msg,
                            src: src.clone(),
                            span: expr.span.into(),
                        })?;
                let rv =
                    r.expect_scalar("binary operand")
                        .map_err(|msg| GraphcalError::EvalError {
                            message: msg,
                            src: src.clone(),
                            span: expr.span.into(),
                        })?;
                Ok(RuntimeValue::Scalar(eval_binop(
                    *op, lv, rv, src, expr.span,
                )?))
            }
        },
        ExprKind::UnaryOp { op, operand } => match op {
            UnaryOp::Neg => {
                let v = eval_expr(
                    operand,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?;
                match v {
                    RuntimeValue::Int(i) => {
                        let negated = i.checked_neg().ok_or_else(|| GraphcalError::EvalError {
                            message: "integer negation overflow".to_string(),
                            src: src.clone(),
                            span: expr.span.into(),
                        })?;
                        Ok(RuntimeValue::Int(negated))
                    }
                    RuntimeValue::Struct {
                        type_name,
                        variant,
                        fields,
                    } => eval_struct_neg(&type_name, &variant, &fields, src, expr.span),
                    _ => Ok(RuntimeValue::Scalar(
                        -v.expect_scalar("unary negation").map_err(|msg| {
                            GraphcalError::EvalError {
                                message: msg,
                                src: src.clone(),
                                span: expr.span.into(),
                            }
                        })?,
                    )),
                }
            }
            UnaryOp::Not => {
                let v = eval_expr(
                    operand,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?
                .expect_bool("logical NOT")
                .map_err(|msg| GraphcalError::EvalError {
                    message: msg,
                    src: src.clone(),
                    span: expr.span.into(),
                })?;
                Ok(RuntimeValue::Bool(!v))
            }
        },
        ExprKind::FnCall { name, args } | ExprKind::QualifiedFnCall { name, args, .. } => {
            // Aggregation functions over indexed values: sum, min, max, mean, count
            if matches!(
                name.value.as_str(),
                "sum" | "min" | "max" | "mean" | "count"
            ) && args.len() == 1
            {
                let arg_val = eval_expr(
                    &args[0],
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?;
                if let RuntimeValue::Indexed { entries, .. } = arg_val {
                    let type_err = |msg: String| GraphcalError::EvalError {
                        message: msg,
                        src: src.clone(),
                        span: expr.span.into(),
                    };
                    return Ok(match name.value.as_str() {
                        "sum" => {
                            let mut total = 0.0_f64;
                            for v in entries.values() {
                                total += v.expect_scalar("sum element").map_err(&type_err)?;
                            }
                            RuntimeValue::Scalar(total)
                        }
                        "min" => {
                            let mut min = f64::INFINITY;
                            for v in entries.values() {
                                min = min.min(v.expect_scalar("min element").map_err(&type_err)?);
                            }
                            RuntimeValue::Scalar(min)
                        }
                        "max" => {
                            let mut max = f64::NEG_INFINITY;
                            for v in entries.values() {
                                max = max.max(v.expect_scalar("max element").map_err(&type_err)?);
                            }
                            RuntimeValue::Scalar(max)
                        }
                        "mean" => {
                            #[expect(
                                clippy::cast_precision_loss,
                                reason = "indexed collection length fits in f64"
                            )]
                            let n = entries.len() as f64;
                            let mut total = 0.0_f64;
                            for v in entries.values() {
                                total += v.expect_scalar("mean element").map_err(&type_err)?;
                            }
                            RuntimeValue::Scalar(total / n)
                        }
                        "count" => {
                            #[expect(
                                clippy::cast_precision_loss,
                                reason = "indexed collection length fits in f64"
                            )]
                            let n = entries.len() as f64;
                            RuntimeValue::Scalar(n)
                        }
                        _ => {
                            return Err(GraphcalError::EvalError {
                                message: format!(
                                    "internal: unexpected aggregate function `{}`",
                                    name.value
                                ),
                                src: src.clone(),
                                span: expr.span.into(),
                            });
                        }
                    });
                }
                // If not indexed, fall through to builtins (min/max are 2-arg builtins)
            }

            // Conversion builtins: to_float and to_int
            if name.value.as_str() == "to_float" {
                let arg = eval_expr(
                    &args[0],
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?;
                let RuntimeValue::Int(i) = arg else {
                    return Err(GraphcalError::EvalError {
                        message: "internal: to_float() received non-Int argument (should have been caught by dim_check)".to_string(),
                        src: src.clone(),
                        span: args[0].span.into(),
                    });
                };
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "explicit conversion from Int to float"
                )]
                return Ok(RuntimeValue::Scalar(i as f64));
            }
            if name.value.as_str() == "to_int" {
                let arg = eval_expr(
                    &args[0],
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?;
                let f = arg.expect_scalar("to_int argument").map_err(|msg| {
                    GraphcalError::EvalError {
                        message: msg,
                        src: src.clone(),
                        span: expr.span.into(),
                    }
                })?;
                if !f.is_finite() {
                    return Err(GraphcalError::EvalError {
                        message: format!("to_int() requires a finite value, got {f}"),
                        src: src.clone(),
                        span: expr.span.into(),
                    });
                }
                // i64 range: -9_223_372_036_854_775_808 ..= 9_223_372_036_854_775_807
                // The casts below round to the nearest f64: i64::MIN rounds exactly,
                // i64::MAX rounds up to 9.223372036854776e18. This makes the check
                // slightly conservative (rejects a few borderline values), which is
                // the safe direction for engineering use.
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "intentional: boundary rounds to safe side, rejecting borderline values"
                )]
                if f < (i64::MIN as f64) || f > (i64::MAX as f64) {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "to_int() argument {f} is outside the representable integer range ({}..={})",
                            i64::MIN,
                            i64::MAX,
                        ),
                        src: src.clone(),
                        span: expr.span.into(),
                    });
                }
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "range-checked truncating conversion from float to Int"
                )]
                return Ok(RuntimeValue::Int(f as i64));
            }

            // Try builtin first
            if let Some(builtin) = builtin_fns.get(name.value.as_str()) {
                let arg_values: Vec<f64> = args
                    .iter()
                    .map(|a| {
                        let rv = eval_expr(
                            a,
                            values,
                            local_values,
                            builtin_consts,
                            builtin_fns,
                            registry,
                            src,
                        )?;
                        rv.expect_scalar("function argument").map_err(|msg| {
                            GraphcalError::EvalError {
                                message: msg,
                                src: src.clone(),
                                span: a.span.into(),
                            }
                        })
                    })
                    .collect::<Result<_, _>>()?;
                let result = (builtin.eval)(&arg_values);
                return Ok(RuntimeValue::Scalar(check_finite(
                    result,
                    name.value.as_str(),
                    src,
                    expr.span,
                )?));
            }

            // Try user-defined function
            let fn_def = registry
                .functions
                .get_function(name.value.as_str())
                .ok_or_else(|| GraphcalError::EvalError {
                    message: format!("unknown function `{}`", name.value),
                    src: src.clone(),
                    span: name.span.into(),
                })?;

            // Evaluate arguments
            let arg_values: Vec<RuntimeValue> = args
                .iter()
                .map(|a| {
                    eval_expr(
                        a,
                        values,
                        local_values,
                        builtin_consts,
                        builtin_fns,
                        registry,
                        src,
                    )
                })
                .collect::<Result<_, _>>()?;

            // Build fn_locals from param names + arg values
            let mut fn_locals: HashMap<String, RuntimeValue> = HashMap::new();
            for (param, val) in fn_def.params.iter().zip(arg_values) {
                fn_locals.insert(param.name.clone(), val);
            }

            // Evaluate body: pass `values` for ConstRef access (user consts like GM_EARTH).
            // Purity is enforced by the resolver's @ prohibition -- no GraphRef nodes
            // exist in function bodies, so passing values is safe.
            let body = fn_def.body.clone();
            match &body {
                FnBody::Short(expr) => eval_expr(
                    expr,
                    values,
                    &fn_locals,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                ),
                FnBody::Block { stmts, expr } => {
                    let mut block_locals = fn_locals;
                    for binding in stmts {
                        let val = eval_expr(
                            &binding.value,
                            values,
                            &block_locals,
                            builtin_consts,
                            builtin_fns,
                            registry,
                            src,
                        )?;
                        block_locals.insert(binding.name.name.clone(), val);
                    }
                    eval_expr(
                        expr,
                        values,
                        &block_locals,
                        builtin_consts,
                        builtin_fns,
                        registry,
                        src,
                    )
                }
            }
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            let cond = eval_expr(
                condition,
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )?
            .expect_bool("if condition")
            .map_err(|msg| GraphcalError::EvalError {
                message: msg,
                src: src.clone(),
                span: expr.span.into(),
            })?;
            if cond {
                eval_expr(
                    then_branch,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )
            } else {
                eval_expr(
                    else_branch,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )
            }
        }
        ExprKind::Convert { expr: inner, .. } | ExprKind::AsCast { expr: inner, .. } => eval_expr(
            inner,
            values,
            local_values,
            builtin_consts,
            builtin_fns,
            registry,
            src,
        ),
        ExprKind::Block { stmts, expr: body } => {
            let mut block_locals = local_values.clone();
            for binding in stmts {
                let val = eval_expr(
                    &binding.value,
                    values,
                    &block_locals,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?;
                block_locals.insert(binding.name.name.clone(), val);
            }
            eval_expr(
                body,
                values,
                &block_locals,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )
        }
        ExprKind::FieldAccess { expr: inner, field } => {
            let inner_val = eval_expr(
                inner,
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )?;
            match inner_val {
                RuntimeValue::Struct { fields, .. } => fields
                    .get(field.value.as_str())
                    .cloned()
                    .ok_or_else(|| GraphcalError::EvalError {
                        message: format!("no field `{}` on struct", field.value),
                        src: src.clone(),
                        span: field.span.into(),
                    }),
                _ => Err(GraphcalError::EvalError {
                    message: "field access on non-struct value".to_string(),
                    src: src.clone(),
                    span: inner.span.into(),
                }),
            }
        }
        ExprKind::StructConstruction {
            type_name, fields, ..
        } => {
            let mut field_map = IndexMap::new();
            for field_init in fields {
                let val = if let Some(value_expr) = &field_init.value {
                    eval_expr(
                        value_expr,
                        values,
                        local_values,
                        builtin_consts,
                        builtin_fns,
                        registry,
                        src,
                    )?
                } else {
                    // Shorthand: look up name in local scope, then graph scope
                    local_values
                        .get(field_init.name.value.as_str())
                        .or_else(|| values.get(field_init.name.value.as_str()))
                        .cloned()
                        .ok_or_else(|| GraphcalError::EvalError {
                            message: format!(
                                "undefined variable `{}` for shorthand field",
                                field_init.name.value
                            ),
                            src: src.clone(),
                            span: field_init.name.span.into(),
                        })?
                };
                field_map.insert(field_init.name.value.clone(), val);
            }
            // Resolve owning type and variant names
            let (owning_type, variant_name) =
                if registry.types.get_type(type_name.value.as_str()).is_some() {
                    // Single-variant: type_name == variant_name
                    (
                        type_name.value.clone(),
                        VariantName::new(type_name.value.as_str()),
                    )
                } else if let Some((type_def, _)) =
                    registry.types.get_type_by_variant(type_name.value.as_str())
                {
                    (
                        type_def.name.clone(),
                        VariantName::new(type_name.value.as_str()),
                    )
                } else {
                    return Err(GraphcalError::EvalError {
                        message: format!("unknown type or variant `{}`", type_name.value),
                        src: src.clone(),
                        span: type_name.span.into(),
                    });
                };
            Ok(RuntimeValue::Struct {
                type_name: owning_type,
                variant: variant_name,
                fields: field_map,
            })
        }

        ExprKind::MapLiteral { entries } | ExprKind::TableLiteral { entries, .. } => {
            eval_map_literal(
                entries,
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )
        }

        ExprKind::ForComp { bindings, body } => {
            // Evaluate body for each combination of index variants
            eval_for_comp(
                bindings,
                body,
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )
        }

        ExprKind::IndexAccess { expr: inner, args } => {
            let mut current = eval_expr(
                inner,
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )?;
            for arg in args {
                let RuntimeValue::Indexed { entries, .. } = current else {
                    return Err(GraphcalError::EvalError {
                        message: "indexing a non-indexed value".to_string(),
                        src: src.clone(),
                        span: expr.span.into(),
                    });
                };
                let variant_name: VariantName = match arg {
                    graphcal_syntax::ast::IndexArg::Variant { variant, .. } => {
                        variant.value.clone()
                    }
                    graphcal_syntax::ast::IndexArg::Var(ident) => {
                        let var_val = local_values.get(&ident.name).ok_or_else(|| {
                            GraphcalError::EvalError {
                                message: format!("undefined loop variable `{}`", ident.name),
                                src: src.clone(),
                                span: ident.span.into(),
                            }
                        })?;
                        match var_val {
                            RuntimeValue::Struct { variant, .. } => variant.clone(),
                            RuntimeValue::RangeLabel { step_index, .. } => {
                                VariantName::new(format!("#{step_index}"))
                            }
                            _ => {
                                return Err(GraphcalError::EvalError {
                                    message: format!("`{}` is not a loop variable", ident.name),
                                    src: src.clone(),
                                    span: ident.span.into(),
                                });
                            }
                        }
                    }
                };
                current = entries.get(variant_name.as_str()).cloned().ok_or_else(|| {
                    GraphcalError::EvalError {
                        message: format!("variant `{variant_name}` not found"),
                        src: src.clone(),
                        span: expr.span.into(),
                    }
                })?;
            }
            Ok(current)
        }

        ExprKind::Scan {
            source,
            init,
            acc_name,
            val_name,
            body,
        } => {
            let source_val = eval_expr(
                source,
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )?;
            let RuntimeValue::Indexed {
                index_name,
                entries: source_entries,
            } = source_val
            else {
                return Err(GraphcalError::EvalError {
                    message: "scan source must be an indexed value".to_string(),
                    src: src.clone(),
                    span: source.span.into(),
                });
            };
            let init_val = eval_expr(
                init,
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )?;

            let mut acc = init_val;
            let mut result_entries = IndexMap::new();
            for (variant, val) in &source_entries {
                let mut scan_locals = local_values.clone();
                scan_locals.insert(acc_name.name.clone(), acc);
                scan_locals.insert(val_name.name.clone(), val.clone());
                let body_val = eval_expr(
                    body,
                    values,
                    &scan_locals,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?;
                result_entries.insert(variant.clone(), body_val.clone());
                acc = body_val;
            }
            Ok(RuntimeValue::Indexed {
                index_name,
                entries: result_entries,
            })
        }

        ExprKind::Unfold { .. } => {
            // Unfold is evaluated at a higher level (evaluate_plan in eval.rs)
            // because it needs to insert partial results into the values map
            // for self-referencing via @node_name[prev_i].
            Err(GraphcalError::EvalError {
                message: "Unfold must be evaluated by evaluate_plan, not eval_expr".to_string(),
                src: src.clone(),
                span: expr.span.into(),
            })
        }

        ExprKind::Match { scrutinee, arms } => {
            let scrutinee_val = eval_expr(
                scrutinee,
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )?;

            match &scrutinee_val {
                RuntimeValue::Struct {
                    variant,
                    fields: scrutinee_fields,
                    ..
                } => {
                    // Tagged union match
                    let matched_arm = arms
                        .iter()
                        .find(|arm| arm.pattern.variant_name.value.as_str() == variant.as_str())
                        .ok_or_else(|| GraphcalError::EvalError {
                            message: format!("no match arm for variant `{variant}`"),
                            src: src.clone(),
                            span: expr.span.into(),
                        })?;

                    // Bind pattern variables
                    let mut arm_locals = local_values.clone();
                    for binding in &matched_arm.pattern.bindings {
                        match binding {
                            graphcal_syntax::ast::PatternBinding::Bind { field, var } => {
                                let field_val = scrutinee_fields
                                    .get(field.value.as_str())
                                    .ok_or_else(|| GraphcalError::EvalError {
                                        message: format!(
                                            "no field `{}` on variant `{variant}`",
                                            field.value
                                        ),
                                        src: src.clone(),
                                        span: field.span.into(),
                                    })?;
                                arm_locals.insert(var.name.clone(), field_val.clone());
                            }
                            graphcal_syntax::ast::PatternBinding::Wildcard { .. } => {}
                        }
                    }

                    eval_expr(
                        &matched_arm.body,
                        values,
                        &arm_locals,
                        builtin_consts,
                        builtin_fns,
                        registry,
                        src,
                    )
                }
                _ => Err(GraphcalError::EvalError {
                    message: "match scrutinee must be a struct/tagged union".to_string(),
                    src: src.clone(),
                    span: scrutinee.span.into(),
                }),
            }
        }
    }
}

/// Evaluate a `for` comprehension by iterating over index variants.
///
/// For single binding `for m: Maneuver { body }`, iterates over Maneuver variants
/// and collects results into `Indexed`.
/// For multi-binding, produces nested `Indexed` values.
/// Evaluate a map literal, handling both single-axis and multi-axis (tuple-key) entries.
///
/// For single-axis (`keys.len() == 1`), builds a flat `Indexed`.
/// For multi-axis, groups entries by the first key's variant and recursively
/// builds nested `Indexed` values from the remaining keys.
fn eval_map_literal(
    entries: &[MapEntry],
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, BuiltinFunction>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    let arity = entries[0].keys.len();
    let idx_name = entries[0].keys[0].index.value.clone();

    if arity == 1 {
        // Single-axis: flat Indexed
        let mut result = IndexMap::new();
        for entry in entries {
            let val = eval_expr(
                &entry.value,
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )?;
            result.insert(entry.keys[0].variant.value.clone(), val);
        }
        return Ok(RuntimeValue::Indexed {
            index_name: idx_name,
            entries: result,
        });
    }

    // Multi-axis: group by first key, recurse on remaining keys
    let idx_def = registry
        .indexes
        .get_index(idx_name.as_str())
        .ok_or_else(|| GraphcalError::EvalError {
            message: format!(
                "internal: unknown index `{idx_name}` (should have been caught by dim_check)"
            ),
            src: src.clone(),
            span: entries[0].keys[0].index.span.into(),
        })?;
    let variants = idx_def.variants();

    let mut outer = IndexMap::new();
    for variant in &variants {
        // Collect entries whose first key matches this variant, stripping the first key
        let sub_entries: Vec<MapEntry> = entries
            .iter()
            .filter(|e| e.keys[0].variant.value.as_str() == variant.as_str())
            .map(|e| MapEntry {
                keys: e.keys[1..].to_vec(),
                value: e.value.clone(),
            })
            .collect();

        let inner = eval_map_literal(
            &sub_entries,
            values,
            local_values,
            builtin_consts,
            builtin_fns,
            registry,
            src,
        )?;
        outer.insert(variant.clone(), inner);
    }
    Ok(RuntimeValue::Indexed {
        index_name: idx_name,
        entries: outer,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "passes through evaluation context to recursive calls"
)]
fn eval_for_comp(
    bindings: &[graphcal_syntax::ast::ForBinding],
    body: &Expr,
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, BuiltinFunction>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    let binding = &bindings[0];
    let idx_name = binding.index.value.clone();
    let idx_def = registry
        .indexes
        .get_index(idx_name.as_str())
        .ok_or_else(|| GraphcalError::EvalError {
            message: format!(
                "internal: unknown index `{idx_name}` (should have been caught by dim_check)"
            ),
            src: src.clone(),
            span: binding.index.span.into(),
        })?;
    let remaining = &bindings[1..];

    let variants = idx_def.variants();
    let mut entries = IndexMap::new();
    for variant in &variants {
        let mut inner_locals = local_values.clone();
        let binding_value = match &idx_def.kind {
            crate::registry::IndexKind::Named { .. } => RuntimeValue::Struct {
                type_name: StructTypeName::new(idx_name.as_str()),
                variant: variant.clone(),
                fields: IndexMap::new(),
            },
            crate::registry::IndexKind::Range { .. } => {
                let step_index = variant
                    .as_str()
                    .strip_prefix('#')
                    .and_then(|s| s.parse::<usize>().ok())
                    .ok_or_else(|| GraphcalError::EvalError {
                        message: format!(
                            "internal: range variant `{variant}` has unexpected format (expected #N)"
                        ),
                        src: src.clone(),
                        span: binding.index.span.into(),
                    })?;
                RuntimeValue::RangeLabel {
                    step_index,
                    value: idx_def.step_value(step_index).map_err(|e| {
                        GraphcalError::EvalError {
                            message: format!(
                                "internal: range index step {step_index} out of bounds: {e}"
                            ),
                            src: src.clone(),
                            span: binding.index.span.into(),
                        }
                    })?,
                }
            }
        };
        inner_locals.insert(binding.var.name.clone(), binding_value);
        let val = if remaining.is_empty() {
            eval_expr(
                body,
                values,
                &inner_locals,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )?
        } else {
            eval_for_comp(
                remaining,
                body,
                values,
                &inner_locals,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )?
        };
        entries.insert(variant.clone(), val);
    }
    Ok(RuntimeValue::Indexed {
        index_name: idx_name,
        entries,
    })
}

fn runtime_value_equals(lhs: &RuntimeValue, rhs: &RuntimeValue) -> bool {
    match (lhs, rhs) {
        (
            RuntimeValue::Struct {
                type_name: lt,
                variant: lv,
                fields: lf,
            },
            RuntimeValue::Struct {
                type_name: rt,
                variant: rv,
                fields: rf,
            },
        ) => {
            lt == rt
                && lv == rv
                && lf.len() == rf.len()
                && lf
                    .iter()
                    .all(|(k, lvf)| rf.get(k).is_some_and(|rvf| runtime_value_equals(lvf, rvf)))
        }
        _ => false,
    }
}

/// Validate that a computed value is finite, returning an `EvalError` if it is NaN or infinite.
fn check_finite(
    value: f64,
    context: &str,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<f64, GraphcalError> {
    if value.is_nan() {
        Err(GraphcalError::EvalError {
            message: format!("invalid argument for {context} (result is NaN)"),
            src: src.clone(),
            span: span.into(),
        })
    } else if value.is_infinite() {
        Err(GraphcalError::EvalError {
            message: format!("{context} produced infinite result"),
            src: src.clone(),
            span: span.into(),
        })
    } else {
        Ok(value)
    }
}

/// Evaluate a comparison operator on two f64 values.
#[expect(clippy::float_cmp, reason = "DSL equality uses exact comparison")]
fn eval_comparison(
    op: BinOp,
    l: f64,
    r: f64,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<bool, GraphcalError> {
    match op {
        BinOp::Eq => Ok(l == r),
        BinOp::Ne => Ok(l != r),
        BinOp::Lt => Ok(l < r),
        BinOp::Gt => Ok(l > r),
        BinOp::Le => Ok(l <= r),
        BinOp::Ge => Ok(l >= r),
        _ => Err(GraphcalError::EvalError {
            message: format!("internal: unexpected operator {op:?} in comparison"),
            src: src.clone(),
            span: span.into(),
        }),
    }
}

/// Evaluate an arithmetic binary operator on two i64 values with checked arithmetic.
fn eval_int_binop(
    op: BinOp,
    l: i64,
    r: i64,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<i64, GraphcalError> {
    match op {
        BinOp::Add => l.checked_add(r),
        BinOp::Sub => l.checked_sub(r),
        BinOp::Mul => l.checked_mul(r),
        BinOp::Div => {
            if r == 0 {
                return Err(GraphcalError::EvalError {
                    message: "integer division by zero".to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            l.checked_div(r)
        }
        BinOp::Mod => {
            if r == 0 {
                return Err(GraphcalError::EvalError {
                    message: "integer modulo by zero".to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            l.checked_rem(r)
        }
        BinOp::Pow => {
            if r < 0 {
                return Err(GraphcalError::EvalError {
                    message: "integer exponent must be non-negative".to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            let exp = u32::try_from(r).map_err(|_| GraphcalError::EvalError {
                message: "integer exponent too large".to_string(),
                src: src.clone(),
                span: span.into(),
            })?;
            l.checked_pow(exp)
        }
        _ => {
            return Err(GraphcalError::EvalError {
                message: format!("internal: unexpected operator {op:?} in integer arithmetic"),
                src: src.clone(),
                span: span.into(),
            });
        }
    }
    .ok_or_else(|| GraphcalError::EvalError {
        message: "integer arithmetic overflow".to_string(),
        src: src.clone(),
        span: span.into(),
    })
}

/// Evaluate an arithmetic binary operator on two f64 values.
fn eval_binop(
    op: BinOp,
    l: f64,
    r: f64,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<f64, GraphcalError> {
    let result = match op {
        BinOp::Add => l + r,
        BinOp::Sub => l - r,
        BinOp::Mul => l * r,
        BinOp::Div => {
            if r == 0.0 {
                return Err(GraphcalError::EvalError {
                    message: "division by zero".to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            l / r
        }
        BinOp::Pow => l.powf(r),
        _ => {
            return Err(GraphcalError::EvalError {
                message: format!("internal: unexpected operator {op:?} in arithmetic"),
                src: src.clone(),
                span: span.into(),
            });
        }
    };

    // Post-check: if inputs were finite but result is not, report an error.
    if l.is_finite() && r.is_finite() && !result.is_finite() {
        if result.is_nan() {
            Err(GraphcalError::EvalError {
                message: "invalid arithmetic operation (result is NaN)".to_string(),
                src: src.clone(),
                span: span.into(),
            })
        } else {
            Err(GraphcalError::EvalError {
                message: "arithmetic overflow (result is infinite)".to_string(),
                src: src.clone(),
                span: span.into(),
            })
        }
    } else {
        Ok(result)
    }
}

/// Component-wise binary operation on two struct values (for derive(Add)/derive(Sub)).
/// Type checking has already verified that both operands are the same struct type
/// and that the struct derives the appropriate operator.
fn eval_struct_binop(
    op: BinOp,
    type_name: &StructTypeName,
    variant: &VariantName,
    lhs_fields: &IndexMap<FieldName, RuntimeValue>,
    rhs_fields: &IndexMap<FieldName, RuntimeValue>,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<RuntimeValue, GraphcalError> {
    let mut result_fields = IndexMap::with_capacity(lhs_fields.len());
    for (field_name, lhs_val) in lhs_fields {
        let rhs_val = &rhs_fields[field_name];
        let result_val = match (lhs_val, rhs_val) {
            (RuntimeValue::Scalar(l), RuntimeValue::Scalar(r)) => {
                RuntimeValue::Scalar(eval_binop(op, *l, *r, src, span)?)
            }
            (RuntimeValue::Int(l), RuntimeValue::Int(r)) => {
                RuntimeValue::Int(eval_int_binop(op, *l, *r, src, span)?)
            }
            _ => {
                return Err(GraphcalError::EvalError {
                    message: format!(
                        "field `{field_name}` of struct `{type_name}` has unsupported type for component-wise operation"
                    ),
                    src: src.clone(),
                    span: span.into(),
                });
            }
        };
        result_fields.insert(field_name.clone(), result_val);
    }
    Ok(RuntimeValue::Struct {
        type_name: type_name.clone(),
        variant: variant.clone(),
        fields: result_fields,
    })
}

/// Component-wise negation of a struct value (for derive(Neg)).
fn eval_struct_neg(
    type_name: &StructTypeName,
    variant: &VariantName,
    fields: &IndexMap<FieldName, RuntimeValue>,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<RuntimeValue, GraphcalError> {
    let mut result_fields = IndexMap::with_capacity(fields.len());
    for (field_name, val) in fields {
        let result_val = match val {
            RuntimeValue::Scalar(v) => RuntimeValue::Scalar(-v),
            RuntimeValue::Int(i) => {
                RuntimeValue::Int(i.checked_neg().ok_or_else(|| GraphcalError::EvalError {
                    message: "integer negation overflow".to_string(),
                    src: src.clone(),
                    span: span.into(),
                })?)
            }
            _ => {
                return Err(GraphcalError::EvalError {
                    message: format!(
                        "field `{field_name}` of struct `{type_name}` has unsupported type for component-wise negation"
                    ),
                    src: src.clone(),
                    span: span.into(),
                });
            }
        };
        result_fields.insert(field_name.clone(), result_val);
    }
    Ok(RuntimeValue::Struct {
        type_name: type_name.clone(),
        variant: variant.clone(),
        fields: result_fields,
    })
}
