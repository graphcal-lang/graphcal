//! Type inference for collection/indexed expressions:
//! ForComp, MapLiteral, TableLiteral, IndexAccess, Scan, Unfold,
//! FieldAccess, StructConstruction.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::syntax::ast::{BinOp, Expr, ExprKind, ForBinding, ForBindingIndex, IndexArg, NatExpr};
use crate::syntax::names::{FieldName, GenericParamName, IndexName, StructTypeName};
use crate::tir::typed::NatLinearForm;

use crate::registry::error::GraphcalError;
use crate::registry::types::Registry;

use super::super::helpers::{
    cartesian_product, declared_to_inferred, format_inferred_type, resolve_field_type,
};
use super::super::{DeclaredType, InferredType};
use super::infer_type;

/// Get the index name for a for binding.
fn for_binding_index_name(index: &ForBindingIndex) -> IndexName {
    match index {
        ForBindingIndex::Named(spanned) => spanned.value.clone(),
        ForBindingIndex::Range { arg, .. } => IndexName::new(nat_expr_to_index_name_str(arg)),
    }
}

/// Convert a `NatExpr` to a canonical synthetic index name string.
///
/// For literals, produces `__nat_range_3`.
/// For variables and compound expressions, produces symbolic names like
/// `__nat_range_N` or `__nat_range_N + 1`.
fn nat_expr_to_index_name_str(expr: &NatExpr) -> String {
    match expr {
        NatExpr::Literal(n, _) => crate::registry::types::nat_range_index_name(*n),
        NatExpr::Var(ident) => format!("__nat_range_{}", ident.name),
        NatExpr::Add(_, _, _) | NatExpr::Mul(_, _, _) => {
            // Normalize to polynomial form for a canonical representation.
            // During generic function body checking, we use symbolic names.
            format!("__nat_range_{expr}")
        }
    }
}

/// Normalize a `NatExpr` to `NatLinearForm` without requiring nat param validation.
///
/// This is a lenient version used during type inference where the nat params
/// in scope are not directly available. Variable validation is done elsewhere.
fn normalize_nat_expr_lenient(expr: &NatExpr) -> NatLinearForm {
    match expr {
        NatExpr::Literal(n, _) => NatLinearForm::from_constant(*n),
        NatExpr::Var(ident) => NatLinearForm::from_var(GenericParamName::new(&ident.name)),
        NatExpr::Add(lhs, rhs, _) => {
            normalize_nat_expr_lenient(lhs).add(&normalize_nat_expr_lenient(rhs))
        }
        NatExpr::Mul(lhs, rhs, _) => {
            normalize_nat_expr_lenient(lhs).mul(&normalize_nat_expr_lenient(rhs))
        }
    }
}

/// Infer the type of a for comprehension.
pub(super) fn infer_for_comp(
    bindings: &[ForBinding],
    body: &Expr,
    declared_types: &HashMap<String, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    // Add loop variables to local_types, infer body type, wrap in Indexed layers
    let mut inner_locals = local_types.clone();
    for binding in bindings {
        let var_type = match &binding.index {
            ForBindingIndex::Named(spanned_idx) => {
                let idx_name = spanned_idx.value.as_str();
                let idx_def = registry.indexes.get_index(idx_name).ok_or_else(|| {
                    GraphcalError::UnknownIndex {
                        name: spanned_idx.value.clone(),
                        src: src.clone(),
                        span: spanned_idx.span.into(),
                    }
                })?;
                match &idx_def.kind {
                    crate::registry::types::IndexKind::Named { .. }
                    | crate::registry::types::IndexKind::RequiredNamed => {
                        InferredType::Label(spanned_idx.value.clone())
                    }
                    crate::registry::types::IndexKind::Range { dimension, .. }
                    | crate::registry::types::IndexKind::RequiredRange { dimension } => {
                        InferredType::Scalar(dimension.clone())
                    }
                    crate::registry::types::IndexKind::NatRange { size } => {
                        InferredType::Fin(NatLinearForm::from_constant(*size))
                    }
                }
            }
            ForBindingIndex::Range { arg, .. } => {
                // `for i: range(N)` — loop variable is Fin(N)
                InferredType::Fin(normalize_nat_expr_lenient(arg))
            }
        };
        inner_locals.insert(binding.var.name.clone(), var_type);
    }
    let body_type = infer_type(
        body,
        declared_types,
        &inner_locals,
        registry,
        builtin_fns,
        src,
    )?;
    // Wrap body type with index layers (outermost binding first)
    let mut result = body_type;
    for binding in bindings.iter().rev() {
        let idx_name = for_binding_index_name(&binding.index);
        result = InferredType::Indexed {
            element: Box::new(result),
            index: idx_name,
        };
    }
    Ok(result)
}

/// Infer the type of a map literal or table literal.
#[expect(
    clippy::too_many_lines,
    reason = "exhaustive validation of map/table literal entries"
)]
pub(super) fn infer_map_or_table_literal(
    expr: &Expr,
    entries: &[crate::syntax::ast::MapEntry],
    declared_types: &HashMap<String, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    if entries.is_empty() {
        return Err(GraphcalError::EvalError {
            message: "empty map literal".to_string(),
            src: src.clone(),
            span: expr.span.into(),
        });
    }
    let arity = entries[0].keys.len();
    if arity == 0 {
        return Err(GraphcalError::EvalError {
            message: "map literal entry has no keys".to_string(),
            src: src.clone(),
            span: expr.span.into(),
        });
    }
    // Validate all entries have the same arity
    for entry in &entries[1..] {
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
    // Validate index names: all entries must use the same indexes in the same order
    let index_names: Vec<&IndexName> = entries[0].keys.iter().map(|k| &k.index.value).collect();
    for entry in &entries[1..] {
        for (i, key) in entry.keys.iter().enumerate() {
            if key.index.value != *index_names[i] {
                return Err(GraphcalError::IndexMismatch {
                    expected: index_names[i].clone(),
                    found: key.index.value.clone(),
                    src: src.clone(),
                    span: key.index.span.into(),
                });
            }
        }
    }
    // Validate each index exists, reject range indexes as keys, and collect variant lists
    let mut axes_variants: Vec<Vec<crate::syntax::names::VariantName>> = Vec::new();
    for key in &entries[0].keys {
        let idx_def = registry
            .indexes
            .get_index(key.index.value.as_str())
            .ok_or_else(|| GraphcalError::UnknownIndex {
                name: key.index.value.clone(),
                src: src.clone(),
                span: key.index.span.into(),
            })?;
        if idx_def.is_range() {
            return Err(GraphcalError::EvalError {
                message: format!(
                    "range index `{}` cannot be used as a map/table literal key; use a `for` comprehension instead",
                    key.index.value
                ),
                src: src.clone(),
                span: key.index.span.into(),
            });
        }
        axes_variants.push(idx_def.variants());
    }
    // Check totality over the Cartesian product
    let mut expected_tuples: std::collections::HashSet<Vec<&str>> =
        std::collections::HashSet::new();
    cartesian_product(&axes_variants, &mut Vec::new(), &mut expected_tuples);
    let mut provided_tuples: std::collections::HashSet<Vec<&str>> =
        std::collections::HashSet::new();
    for entry in entries {
        let tuple: Vec<&str> = entry
            .keys
            .iter()
            .map(|k| k.variant.value.as_str())
            .collect();
        if !provided_tuples.insert(tuple.clone()) {
            return Err(GraphcalError::EvalError {
                message: format!(
                    "duplicate map literal entry for key tuple ({})",
                    entry
                        .keys
                        .iter()
                        .map(|k| format!("{}::{}", k.index.value, k.variant.value))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                src: src.clone(),
                span: expr.span.into(),
            });
        }
        // For multi-axis, validate each variant exists in its respective index.
        // For single-axis, skip this check — extra/missing set difference
        // handles it with more specific error types (ExtraVariants/MissingVariants).
        if arity > 1 {
            for (i, key) in entry.keys.iter().enumerate() {
                if !axes_variants[i]
                    .iter()
                    .any(|v| v.as_str() == key.variant.value.as_str())
                {
                    return Err(GraphcalError::UnknownVariant {
                        index_name: key.index.value.clone(),
                        variant_name: key.variant.value.clone(),
                        src: src.clone(),
                        span: key.variant.span.into(),
                    });
                }
            }
        }
    }
    // Check for extra variants (provided but not in expected set)
    let extra: Vec<Vec<&str>> = provided_tuples
        .difference(&expected_tuples)
        .cloned()
        .collect();
    if !extra.is_empty() {
        if arity == 1 {
            let extra_variants: Vec<crate::syntax::names::VariantName> = extra
                .iter()
                .map(|t| crate::syntax::names::VariantName::new(t[0]))
                .collect();
            return Err(GraphcalError::ExtraVariants {
                index_name: index_names[0].clone(),
                extra: extra_variants,
                src: src.clone(),
                span: expr.span.into(),
            });
        }
        let extra_strs: Vec<String> = extra
            .iter()
            .map(|t| {
                t.iter()
                    .enumerate()
                    .map(|(i, v)| format!("{}::{v}", index_names[i]))
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
    // Check for missing tuples
    let missing: Vec<Vec<&str>> = expected_tuples
        .difference(&provided_tuples)
        .cloned()
        .collect();
    if !missing.is_empty() {
        if arity == 1 {
            let missing_variants: Vec<crate::syntax::names::VariantName> = missing
                .iter()
                .map(|t| crate::syntax::names::VariantName::new(t[0]))
                .collect();
            return Err(GraphcalError::MissingVariants {
                index_name: index_names[0].clone(),
                missing: missing_variants,
                src: src.clone(),
                span: expr.span.into(),
            });
        }
        let missing_strs: Vec<String> = missing
            .iter()
            .map(|t| {
                t.iter()
                    .enumerate()
                    .map(|(i, v)| format!("{}::{v}", index_names[i]))
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
    // Infer element type from first entry, check all entries match
    let first_type = infer_type(
        &entries[0].value,
        declared_types,
        local_types,
        registry,
        builtin_fns,
        src,
    )?;
    // Reject nested Indexed when the inner index is a label (named) index.
    // Label-indexed elements should use tuple keys instead: { (I::A, J::B): expr, ... }.
    // Allow when the inner index is a range index, enabling mixed-index construction:
    //   { LabelIndex::Variant: for t: RangeIndex { ... }, ... }
    if let InferredType::Indexed { index, .. } = &first_type {
        let inner_is_label = registry
            .indexes
            .get_index(index.as_str())
            .is_some_and(|def| !def.is_range());
        if inner_is_label {
            return Err(GraphcalError::EvalError {
                message: "map literal element type must be a value type, not an indexed type; use tuple keys for multi-axis map literals".to_string(),
                src: src.clone(),
                span: entries[0].value.span.into(),
            });
        }
    }
    for entry in &entries[1..] {
        let entry_type = infer_type(
            &entry.value,
            declared_types,
            local_types,
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
    // Wrap in nested Indexed layers (reverse order, matching `for` comprehension)
    let mut result = first_type;
    for idx_name in index_names.iter().rev() {
        result = InferredType::Indexed {
            element: Box::new(result),
            index: (*idx_name).clone(),
        };
    }
    Ok(result)
}

/// Infer the type of an index access expression.
pub(super) fn infer_index_access(
    expr: &Expr,
    inner: &Expr,
    args: &[IndexArg],
    declared_types: &HashMap<String, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let inner_type = infer_type(
        inner,
        declared_types,
        local_types,
        registry,
        builtin_fns,
        src,
    )?;
    // Peel off one index layer per argument
    let mut current = inner_type;
    for arg in args {
        let InferredType::Indexed {
            element,
            index: idx_name,
        } = current
        else {
            return Err(GraphcalError::EvalError {
                message: "indexing a non-indexed value".to_string(),
                src: src.clone(),
                span: expr.span.into(),
            });
        };
        // Validate the argument matches the index
        match arg {
            IndexArg::Variant { index, variant } => {
                if index.value.as_str() != idx_name.as_str() {
                    return Err(GraphcalError::IndexMismatch {
                        expected: idx_name,
                        found: index.value.clone(),
                        src: src.clone(),
                        span: index.span.into(),
                    });
                }
                // Validate variant exists
                let idx_def = registry
                    .indexes
                    .get_index(idx_name.as_str())
                    .ok_or_else(|| GraphcalError::UnknownIndex {
                        name: idx_name.clone(),
                        src: src.clone(),
                        span: index.span.into(),
                    })?;
                if !idx_def
                    .variants()
                    .iter()
                    .any(|v| v.as_str() == variant.value.as_str())
                {
                    return Err(GraphcalError::UnknownVariant {
                        index_name: idx_name,
                        variant_name: variant.value.clone(),
                        src: src.clone(),
                        span: variant.span.into(),
                    });
                }
            }
            IndexArg::Var(ident) => {
                // Must be a loop variable with matching index
                let var_type =
                    local_types
                        .get(&ident.name)
                        .ok_or_else(|| GraphcalError::UnknownLocalRef {
                            name: ident.name.clone(),
                            src: src.clone(),
                            span: ident.span.into(),
                        })?;
                match var_type {
                    InferredType::Label(label_index) => {
                        if label_index.as_str() != idx_name.as_str() {
                            return Err(GraphcalError::IndexMismatch {
                                expected: idx_name,
                                found: label_index.clone(),
                                src: src.clone(),
                                span: ident.span.into(),
                            });
                        }
                    }
                    InferredType::Struct(type_name, args) => {
                        if type_name.as_str() != idx_name.as_str() || !args.is_empty() {
                            return Err(GraphcalError::IndexMismatch {
                                expected: idx_name,
                                found: IndexName::new(type_name.as_str()),
                                src: src.clone(),
                                span: ident.span.into(),
                            });
                        }
                    }
                    InferredType::Scalar(_) => {
                        // Allow scalar locals to be used as index args
                        // for range indexes (e.g. prev_i, i in Unfold)
                        let idx_def =
                            registry
                                .indexes
                                .get_index(idx_name.as_str())
                                .ok_or_else(|| GraphcalError::UnknownIndex {
                                    name: idx_name.clone(),
                                    src: src.clone(),
                                    span: ident.span.into(),
                                })?;
                        if !idx_def.is_range() {
                            return Err(GraphcalError::EvalError {
                                message: format!("`{}` is not a loop variable", ident.name),
                                src: src.clone(),
                                span: ident.span.into(),
                            });
                        }
                    }
                    InferredType::Int => {
                        // Allow Int locals to be used as index args for nat range indexes
                        // (e.g. `for i: range(3) { v[i] }`)
                        if let Some(idx_def) = registry.indexes.get_index(idx_name.as_str())
                            && !idx_def.is_nat_range()
                        {
                            return Err(GraphcalError::EvalError {
                                message: format!(
                                    "`{}` (Int) cannot index into non-nat-range index `{}`",
                                    ident.name, idx_name
                                ),
                                src: src.clone(),
                                span: ident.span.into(),
                            });
                        }
                        // Int has no static bound — no bounds checking possible.
                        // If the index is not in registry (generic nat param),
                        // allow it — it will be checked at call site.
                    }
                    InferredType::Fin(fin_bound) => {
                        // Fin(N) can index into nat-range indexes with bounds checking.
                        // Extract the index size as a NatLinearForm and check: fin_bound <= size.
                        let index_form = if let Some(idx_def) =
                            registry.indexes.get_index(idx_name.as_str())
                        {
                            if !idx_def.is_nat_range() {
                                return Err(GraphcalError::EvalError {
                                    message: format!(
                                        "`{}` (Fin({})) cannot index into non-nat-range index `{}`",
                                        ident.name,
                                        fin_bound.format(),
                                        idx_name
                                    ),
                                    src: src.clone(),
                                    span: ident.span.into(),
                                });
                            }
                            idx_def.nat_range_size().map(NatLinearForm::from_constant)
                        } else {
                            // Index not in registry: symbolic nat range (generic param).
                            NatLinearForm::from_index_name(idx_name.as_str())
                        };
                        if let Some(index_form) = &index_form
                            && !fin_bound.is_leq(index_form)
                        {
                            return Err(GraphcalError::EvalError {
                                message: format!(
                                    "index out of bounds: `{}` has type Fin({}) but array has size {}",
                                    ident.name,
                                    fin_bound.format(),
                                    index_form.format(),
                                ),
                                src: src.clone(),
                                span: ident.span.into(),
                            });
                        }
                        // If we can't determine the index size, allow it —
                        // the check will happen at the call site.
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
            IndexArg::Expr(index_expr) => {
                // Infer the type of the expression; must be int-like.
                let expr_type = infer_type(
                    index_expr,
                    declared_types,
                    local_types,
                    registry,
                    builtin_fns,
                    src,
                )?;
                if !expr_type.is_int_like() {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "index expression must be an integer type, got {}",
                            format_inferred_type(&expr_type, registry),
                        ),
                        src: src.clone(),
                        span: index_expr.span.into(),
                    });
                }
                // Check that the indexed type is a nat-range index.
                if let Some(idx_def) = registry.indexes.get_index(idx_name.as_str())
                    && !idx_def.is_nat_range()
                {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "expression index cannot be used with non-nat-range index `{idx_name}`",
                        ),
                        src: src.clone(),
                        span: index_expr.span.into(),
                    });
                }
                // Try to compute a static Fin bound for bounds checking.
                if let Some(fin_bound) = compute_index_fin_bound(index_expr, local_types) {
                    let index_form = registry.indexes.get_index(idx_name.as_str()).map_or_else(
                        // Symbolic nat range from generic param.
                        || NatLinearForm::from_index_name(idx_name.as_str()),
                        |idx_def| idx_def.nat_range_size().map(NatLinearForm::from_constant),
                    );
                    if let Some(index_form) = &index_form
                        && !fin_bound.is_leq(index_form)
                    {
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
            }
        }
        current = *element;
    }
    Ok(current)
}

/// Try to compute a `Fin(N)` upper bound for an expression used as an index.
///
/// Returns `Some(bound)` where `bound` is a `NatLinearForm` such that the expression
/// value is guaranteed to be `< bound` (i.e., the expression has type `Fin(bound)`).
///
/// Supports:
/// - `Fin(N)` variables → bound `N`
/// - `Fin(N) + literal(k)` → bound `N + k`
/// - `literal(k) + Fin(N)` → bound `N + k`
///
/// Returns `None` for expressions whose bounds cannot be statically determined
/// (e.g., subtraction, multiplication, arbitrary `Int` values).
fn compute_index_fin_bound(
    expr: &Expr,
    local_types: &HashMap<String, InferredType>,
) -> Option<NatLinearForm> {
    match &expr.kind {
        ExprKind::LocalRef(ident) => match local_types.get(&ident.name)? {
            InferredType::Fin(bound) => Some(bound.clone()),
            _ => None,
        },
        ExprKind::BinOp {
            op: BinOp::Add,
            lhs,
            rhs,
        } => {
            // Fin(N) + literal(k) → Fin(N + k)
            // literal(k) + Fin(N) → Fin(N + k)
            fin_plus_literal(lhs, rhs, local_types)
                .or_else(|| fin_plus_literal(rhs, lhs, local_types))
        }
        _ => None,
    }
}

/// Helper: if `fin_expr` has type `Fin(N)` and `lit_expr` is an integer literal `k`,
/// return `N + k` as the combined bound.
///
/// `Fin(N) + k` has max value `(N-1) + k = N + k - 1`, so it fits in `Fin(N + k)`.
fn fin_plus_literal(
    fin_expr: &Expr,
    lit_expr: &Expr,
    local_types: &HashMap<String, InferredType>,
) -> Option<NatLinearForm> {
    let fin_bound = compute_index_fin_bound(fin_expr, local_types)?;
    let ExprKind::Integer(k) = &lit_expr.kind else {
        return None;
    };
    if *k < 0 {
        return None; // Negative offsets can't be statically bounded
    }
    #[expect(clippy::cast_sign_loss, reason = "checked non-negative above")]
    Some(fin_bound.add(&NatLinearForm::from_constant(*k as u64)))
}

/// Infer the type of a scan expression.
pub(super) fn infer_scan(
    source: &Expr,
    init: &Expr,
    acc_name: &crate::syntax::ast::Ident,
    val_name: &crate::syntax::ast::Ident,
    body: &Expr,
    declared_types: &HashMap<String, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    // source must be indexed, init must be scalar matching element type
    let source_type = infer_type(
        source,
        declared_types,
        local_types,
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
    let init_type = infer_type(
        init,
        declared_types,
        local_types,
        registry,
        builtin_fns,
        src,
    )?;
    // init and element must have the same type
    if init_type != *element {
        return Err(GraphcalError::DimensionMismatch {
            expected: format_inferred_type(&element, registry),
            found: format_inferred_type(&init_type, registry),
            src: src.clone(),
            span: init.span.into(),
            help: "scan init value must match element type of source".to_string(),
        });
    }
    // Bind acc and val as locals with element type
    let mut scan_locals = local_types.clone();
    scan_locals.insert(acc_name.name.clone(), *element.clone());
    scan_locals.insert(val_name.name.clone(), *element.clone());
    let body_type = infer_type(
        body,
        declared_types,
        &scan_locals,
        registry,
        builtin_fns,
        src,
    )?;
    if body_type != *element {
        return Err(GraphcalError::DimensionMismatch {
            expected: format_inferred_type(&element, registry),
            found: format_inferred_type(&body_type, registry),
            src: src.clone(),
            span: body.span.into(),
            help: "scan body must return the same type as the accumulator".to_string(),
        });
    }
    // scan produces an indexed result with the same index
    Ok(InferredType::Indexed { element, index })
}

/// Infer the type of an unfold expression.
///
/// `owner_decl_name` is the name of the node/const/param that contains this
/// unfold expression. It is used to look up the correct range index from the
/// owning declaration's type, rather than scanning all declared types.
pub(super) fn infer_unfold(
    init: &Expr,
    prev_name: &crate::syntax::ast::Ident,
    curr_name: &crate::syntax::ast::Ident,
    body: &Expr,
    owner_decl_name: Option<&str>,
    declared_types: &HashMap<String, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let init_type = infer_type(
        init,
        declared_types,
        local_types,
        registry,
        builtin_fns,
        src,
    )?;

    // Look up the owning declaration's type to find the range index and its
    // dimension. This is precise — it uses the specific node's declared type
    // rather than scanning all declared types (which would pick an arbitrary
    // range index if multiple exist).
    let mut scan_locals = local_types.clone();
    let owner_range_index = owner_decl_name.and_then(|name| {
        let dt = declared_types.get(name)?;
        if let DeclaredType::Indexed { index, .. } = dt {
            let idx_def = registry.indexes.get_index(index.as_str())?;
            if idx_def.is_range() {
                return Some((index.clone(), idx_def));
            }
        }
        None
    });

    if let Some((_index_name, idx_def)) = &owner_range_index {
        let dimension = match &idx_def.kind {
            crate::registry::types::IndexKind::Range { dimension, .. }
            | crate::registry::types::IndexKind::RequiredRange { dimension } => Some(dimension),
            _ => None,
        };
        if let Some(dimension) = dimension {
            scan_locals.insert(
                prev_name.name.clone(),
                InferredType::Scalar(dimension.clone()),
            );
            scan_locals.insert(
                curr_name.name.clone(),
                InferredType::Scalar(dimension.clone()),
            );
        }
    } else {
        // Fallback: dimensionless when owner is unknown or not an indexed range type
        scan_locals.insert(
            prev_name.name.clone(),
            InferredType::Scalar(crate::syntax::dimension::Dimension::dimensionless()),
        );
        scan_locals.insert(
            curr_name.name.clone(),
            InferredType::Scalar(crate::syntax::dimension::Dimension::dimensionless()),
        );
    }

    let body_type = infer_type(
        body,
        declared_types,
        &scan_locals,
        registry,
        builtin_fns,
        src,
    )?;
    if body_type != init_type {
        return Err(GraphcalError::DimensionMismatch {
            expected: format_inferred_type(&init_type, registry),
            found: format_inferred_type(&body_type, registry),
            src: src.clone(),
            span: body.span.into(),
            help: "time scan body must return the same type as the init value".to_string(),
        });
    }

    // The result type is Indexed { element: init_type, index: <range_index> }
    if let Some((index_name, _)) = owner_range_index {
        return Ok(InferredType::Indexed {
            element: Box::new(init_type),
            index: index_name,
        });
    }

    // Fallback: return init_type (will fail annotation check if declared as indexed)
    Ok(init_type)
}

/// Infer the type of a field access expression.
pub(super) fn infer_field_access(
    inner: &Expr,
    field: &crate::syntax::names::Spanned<FieldName>,
    declared_types: &HashMap<String, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let inner_type = infer_type(
        inner,
        declared_types,
        local_types,
        registry,
        builtin_fns,
        src,
    )?;
    match &inner_type {
        InferredType::Struct(type_name, type_args) => {
            let type_def = registry.types.get_type(type_name.as_str()).ok_or_else(|| {
                GraphcalError::UnknownStructType {
                    name: type_name.clone(),
                    src: src.clone(),
                    span: inner.span.into(),
                }
            })?;
            // Field access is only allowed on record types (not union or unit types)
            if type_def.is_union() {
                return Err(GraphcalError::NotAStruct {
                    name: format!("union type `{type_name}` (use `match` to access fields)"),
                    src: src.clone(),
                    span: inner.span.into(),
                });
            }
            if type_def.is_unit() {
                return Err(GraphcalError::NotAStruct {
                    name: format!("unit type `{type_name}` has no fields"),
                    src: src.clone(),
                    span: inner.span.into(),
                });
            }
            let field_def = type_def
                .fields()
                .iter()
                .find(|f| f.name.as_str() == field.value.as_str())
                .ok_or_else(|| GraphcalError::UnknownField {
                    type_name: type_name.clone(),
                    field_name: field.value.clone(),
                    src: src.clone(),
                    span: field.span.into(),
                })?;
            resolve_field_type(&field_def.type_ann, type_def, type_args, registry, src)
        }
        _ => Err(GraphcalError::NotAStruct {
            name: format_inferred_type(&inner_type, registry),
            src: src.clone(),
            span: inner.span.into(),
        }),
    }
}

/// Infer the type of a struct construction expression.
#[expect(
    clippy::too_many_lines,
    reason = "exhaustive validation of struct construction"
)]
pub(super) fn infer_struct_construction(
    expr: &Expr,
    type_name: &crate::syntax::names::Spanned<StructTypeName>,
    constructor_type_args: &[crate::syntax::ast::TypeExpr],
    fields: &[crate::syntax::ast::FieldInit],
    declared_types: &HashMap<String, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    // Look up by type name — must be a record or unit type (not a union)
    let type_def = registry
        .types
        .get_type(type_name.value.as_str())
        .ok_or_else(|| GraphcalError::UnknownStructType {
            name: type_name.value.clone(),
            src: src.clone(),
            span: type_name.span.into(),
        })?;
    if type_def.is_union() {
        return Err(GraphcalError::UnknownStructType {
            name: type_name.value.clone(),
            src: src.clone(),
            span: type_name.span.into(),
        });
    }
    let owning_type_name = type_def.name.clone();

    // Resolve constructor type args for generic structs
    let resolved_type_args: Vec<InferredType> = if constructor_type_args.is_empty()
        && type_def.generic_params.is_empty()
    {
        vec![]
    } else if !type_def.generic_params.is_empty() {
        let total_params = type_def.generic_params.len();
        let required_count = type_def
            .generic_params
            .iter()
            .take_while(|p| p.default.is_none())
            .count();
        if constructor_type_args.len() < required_count
            || constructor_type_args.len() > total_params
        {
            let hint = if required_count == total_params {
                format!("{total_params}")
            } else {
                format!("{required_count}..{total_params}")
            };
            return Err(GraphcalError::EvalError {
                message: format!(
                    "type `{}` expects {hint} type argument(s), got {}",
                    type_name.value,
                    constructor_type_args.len()
                ),
                src: src.clone(),
                span: type_name.span.into(),
            });
        }
        let no_dim_params: &[GenericParamName] = &[];
        let no_index_params: &[GenericParamName] = &[];
        let no_nat_params: &[GenericParamName] = &[];
        let mut args = Vec::with_capacity(total_params);
        for arg in constructor_type_args {
            let resolved = crate::tir::typed::resolve_type_expr(
                arg,
                registry,
                no_dim_params,
                no_index_params,
                no_nat_params,
                src,
            )?;
            let dt = crate::tir::typed::resolved_to_declared_type(&resolved, src)?;
            args.push(declared_to_inferred(&dt));
        }
        // Fill in defaults for remaining params
        for param in type_def
            .generic_params
            .iter()
            .skip(constructor_type_args.len())
        {
            let default_expr = param
                .default
                .as_ref()
                .ok_or_else(|| GraphcalError::EvalError {
                    message: format!(
                        "internal: generic parameter `{}` has no default",
                        param.name
                    ),
                    src: src.clone(),
                    span: type_name.span.into(),
                })?;
            let resolved = crate::tir::typed::resolve_type_expr(
                default_expr,
                registry,
                no_dim_params,
                no_index_params,
                no_nat_params,
                src,
            )?;
            let dt = crate::tir::typed::resolved_to_declared_type(&resolved, src)?;
            args.push(declared_to_inferred(&dt));
        }
        args
    } else {
        vec![]
    };

    // Check for extra fields
    let def_field_names: std::collections::HashSet<&str> =
        type_def.fields().iter().map(|f| f.name.as_str()).collect();
    let provided_names: Vec<&str> = fields.iter().map(|f| f.name.value.as_str()).collect();
    let extra: Vec<FieldName> = provided_names
        .iter()
        .filter(|n| !def_field_names.contains(**n))
        .map(|n| FieldName::new(*n))
        .collect();
    if !extra.is_empty() {
        return Err(GraphcalError::ExtraFields {
            type_name: type_name.value.clone(),
            extra,
            src: src.clone(),
            span: expr.span.into(),
        });
    }

    // Check for missing fields
    let provided_set: std::collections::HashSet<&str> = provided_names.iter().copied().collect();
    let missing: Vec<FieldName> = type_def
        .fields()
        .iter()
        .filter(|f| !provided_set.contains(f.name.as_str()))
        .map(|f| f.name.clone())
        .collect();
    if !missing.is_empty() {
        return Err(GraphcalError::MissingFields {
            type_name: type_name.value.clone(),
            missing,
            src: src.clone(),
            span: expr.span.into(),
        });
    }

    // Type-check each field's value
    for field_init in fields {
        let field_def = type_def
            .fields()
            .iter()
            .find(|f| f.name.as_str() == field_init.name.value.as_str())
            .ok_or_else(|| GraphcalError::EvalError {
                message: format!(
                    "internal: unknown field `{}` in struct `{}`",
                    field_init.name.value, type_name.value
                ),
                src: src.clone(),
                span: field_init.name.span.into(),
            })?;

        let value_type = if let Some(value_expr) = &field_init.value {
            infer_type(
                value_expr,
                declared_types,
                local_types,
                registry,
                builtin_fns,
                src,
            )?
        } else {
            // Shorthand: look up the local variable with the same name
            local_types
                .get(field_init.name.value.as_str())
                .cloned()
                .ok_or_else(|| GraphcalError::UnknownLocalRef {
                    name: field_init.name.value.to_string(),
                    src: src.clone(),
                    span: field_init.name.span.into(),
                })?
        };

        let expected_field_type = resolve_field_type(
            &field_def.type_ann,
            type_def,
            &resolved_type_args,
            registry,
            src,
        )?;
        if value_type != expected_field_type {
            return Err(GraphcalError::FieldDimensionMismatch {
                type_name: type_name.value.clone(),
                field_name: field_init.name.value.clone(),
                expected: format_inferred_type(&expected_field_type, registry),
                found: format_inferred_type(&value_type, registry),
                src: src.clone(),
                span: field_init.name.span.into(),
            });
        }
    }

    Ok(InferredType::Struct(owning_type_name, resolved_type_args))
}
