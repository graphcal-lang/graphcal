//! Type inference for collection/indexed expressions:
//! ForComp, MapLiteral, TableLiteral, IndexAccess, Scan, Unfold,
//! FieldAccess, StructConstruction.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use graphcal_syntax::ast::{Expr, ForBinding, IndexArg};
use graphcal_syntax::names::{FieldName, FnName, GenericParamName, IndexName, StructTypeName};

use crate::tir::ResolvedFnSig;
use graphcal_registry::error::GraphcalError;
use graphcal_registry::registry::Registry;

use super::super::helpers::{
    cartesian_product, declared_to_inferred, format_inferred_type, resolve_field_type,
};
use super::super::{DeclaredType, InferredType};
use super::infer_type;

/// Infer the type of a for comprehension.
pub(super) fn infer_for_comp(
    bindings: &[ForBinding],
    body: &Expr,
    declared_types: &HashMap<String, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, graphcal_registry::builtins::BuiltinFunction>,
    resolved_fn_sigs: &HashMap<FnName, ResolvedFnSig>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    // Add loop variables to local_types, infer body type, wrap in Indexed layers
    let mut inner_locals = local_types.clone();
    for binding in bindings {
        let idx_name = binding.index.value.as_str();
        let idx_def =
            registry
                .indexes
                .get_index(idx_name)
                .ok_or_else(|| GraphcalError::UnknownIndex {
                    name: binding.index.value.clone(),
                    src: src.clone(),
                    span: binding.index.span.into(),
                })?;
        inner_locals.insert(
            binding.var.name.clone(),
            match &idx_def.kind {
                graphcal_registry::registry::IndexKind::Named { .. }
                | graphcal_registry::registry::IndexKind::RequiredNamed => {
                    InferredType::Label(binding.index.value.clone())
                }
                graphcal_registry::registry::IndexKind::Range { dimension, .. }
                | graphcal_registry::registry::IndexKind::RequiredRange { dimension } => {
                    InferredType::Scalar(dimension.clone())
                }
            },
        );
    }
    let body_type = infer_type(
        body,
        declared_types,
        &inner_locals,
        registry,
        builtin_fns,
        resolved_fn_sigs,
        src,
    )?;
    // Wrap body type with index layers (outermost binding first)
    let mut result = body_type;
    for binding in bindings.iter().rev() {
        result = InferredType::Indexed {
            element: Box::new(result),
            index: binding.index.value.clone(),
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
    entries: &[graphcal_syntax::ast::MapEntry],
    declared_types: &HashMap<String, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, graphcal_registry::builtins::BuiltinFunction>,
    resolved_fn_sigs: &HashMap<FnName, ResolvedFnSig>,
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
    let mut axes_variants: Vec<Vec<graphcal_syntax::names::VariantName>> = Vec::new();
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
            let extra_variants: Vec<graphcal_syntax::names::VariantName> = extra
                .iter()
                .map(|t| graphcal_syntax::names::VariantName::new(t[0]))
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
            let missing_variants: Vec<graphcal_syntax::names::VariantName> = missing
                .iter()
                .map(|t| graphcal_syntax::names::VariantName::new(t[0]))
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
        resolved_fn_sigs,
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
            resolved_fn_sigs,
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
    builtin_fns: &HashMap<&str, graphcal_registry::builtins::BuiltinFunction>,
    resolved_fn_sigs: &HashMap<FnName, ResolvedFnSig>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let inner_type = infer_type(
        inner,
        declared_types,
        local_types,
        registry,
        builtin_fns,
        resolved_fn_sigs,
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
                    _ => {
                        return Err(GraphcalError::EvalError {
                            message: format!("`{}` is not a loop variable", ident.name),
                            src: src.clone(),
                            span: ident.span.into(),
                        });
                    }
                }
            }
        }
        current = *element;
    }
    Ok(current)
}

/// Infer the type of a scan expression.
pub(super) fn infer_scan(
    source: &Expr,
    init: &Expr,
    acc_name: &graphcal_syntax::ast::Ident,
    val_name: &graphcal_syntax::ast::Ident,
    body: &Expr,
    declared_types: &HashMap<String, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, graphcal_registry::builtins::BuiltinFunction>,
    resolved_fn_sigs: &HashMap<FnName, ResolvedFnSig>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    // source must be indexed, init must be scalar matching element type
    let source_type = infer_type(
        source,
        declared_types,
        local_types,
        registry,
        builtin_fns,
        resolved_fn_sigs,
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
        resolved_fn_sigs,
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
        resolved_fn_sigs,
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
    prev_name: &graphcal_syntax::ast::Ident,
    curr_name: &graphcal_syntax::ast::Ident,
    body: &Expr,
    owner_decl_name: Option<&str>,
    declared_types: &HashMap<String, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, graphcal_registry::builtins::BuiltinFunction>,
    resolved_fn_sigs: &HashMap<FnName, ResolvedFnSig>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let init_type = infer_type(
        init,
        declared_types,
        local_types,
        registry,
        builtin_fns,
        resolved_fn_sigs,
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
            graphcal_registry::registry::IndexKind::Range { dimension, .. }
            | graphcal_registry::registry::IndexKind::RequiredRange { dimension } => {
                Some(dimension)
            }
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
            InferredType::Scalar(graphcal_syntax::dimension::Dimension::dimensionless()),
        );
        scan_locals.insert(
            curr_name.name.clone(),
            InferredType::Scalar(graphcal_syntax::dimension::Dimension::dimensionless()),
        );
    }

    let body_type = infer_type(
        body,
        declared_types,
        &scan_locals,
        registry,
        builtin_fns,
        resolved_fn_sigs,
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
    field: &graphcal_syntax::names::Spanned<FieldName>,
    declared_types: &HashMap<String, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, graphcal_registry::builtins::BuiltinFunction>,
    resolved_fn_sigs: &HashMap<FnName, ResolvedFnSig>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let inner_type = infer_type(
        inner,
        declared_types,
        local_types,
        registry,
        builtin_fns,
        resolved_fn_sigs,
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
            // Field access is only allowed on single-variant (struct sugar) types
            if !type_def.is_single_variant() {
                return Err(GraphcalError::NotAStruct {
                    name: format!(
                        "multi-variant type `{type_name}` (use `match` to access fields)"
                    ),
                    src: src.clone(),
                    span: inner.span.into(),
                });
            }
            let variant = type_def
                .variants
                .first()
                .ok_or_else(|| GraphcalError::NotAStruct {
                    name: type_name.to_string(),
                    src: src.clone(),
                    span: inner.span.into(),
                })?;
            let field_def = variant
                .fields
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
    type_name: &graphcal_syntax::names::Spanned<StructTypeName>,
    constructor_type_args: &[graphcal_syntax::ast::TypeExpr],
    fields: &[graphcal_syntax::ast::FieldInit],
    declared_types: &HashMap<String, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, graphcal_registry::builtins::BuiltinFunction>,
    resolved_fn_sigs: &HashMap<FnName, ResolvedFnSig>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    // Look up by type name first (single-variant / struct sugar),
    // then by variant name (multi-variant tagged union)
    let (type_def, variant_def) =
        if let Some(type_def) = registry.types.get_type(type_name.value.as_str()) {
            // Single-variant: type_name == variant_name
            let variant =
                type_def
                    .variants
                    .first()
                    .ok_or_else(|| GraphcalError::UnknownStructType {
                        name: type_name.value.clone(),
                        src: src.clone(),
                        span: type_name.span.into(),
                    })?;
            (type_def, variant)
        } else if let Some((type_def, variant)) =
            registry.types.get_type_by_variant(type_name.value.as_str())
        {
            (type_def, variant)
        } else {
            return Err(GraphcalError::UnknownStructType {
                name: type_name.value.clone(),
                src: src.clone(),
                span: type_name.span.into(),
            });
        };
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
        let mut args = Vec::with_capacity(total_params);
        for arg in constructor_type_args {
            let resolved =
                crate::tir::resolve_type_expr(arg, registry, no_dim_params, no_index_params, src)?;
            let dt = crate::tir::resolved_to_declared_type(&resolved, src)?;
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
            let resolved = crate::tir::resolve_type_expr(
                default_expr,
                registry,
                no_dim_params,
                no_index_params,
                src,
            )?;
            let dt = crate::tir::resolved_to_declared_type(&resolved, src)?;
            args.push(declared_to_inferred(&dt));
        }
        args
    } else {
        vec![]
    };

    // Check for extra fields
    let def_field_names: std::collections::HashSet<&str> =
        variant_def.fields.iter().map(|f| f.name.as_str()).collect();
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
    let missing: Vec<FieldName> = variant_def
        .fields
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
        let field_def = variant_def
            .fields
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
                resolved_fn_sigs,
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
