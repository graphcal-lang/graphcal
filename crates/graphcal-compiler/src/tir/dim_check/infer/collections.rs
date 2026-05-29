//! Type inference for collection/indexed expressions:
//! ForComp, MapLiteral, TableLiteral, IndexAccess, Scan, Unfold,
//! FieldAccess, ConstructorCall.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::resolved_ast::{
    BinOp, Expr, ExprKind, ForBinding, ForBindingIndex, GenericArg, IndexArg, NatExpr,
};
use crate::syntax::names::{
    FieldName, GenericParamName, IndexName, IndexVariantName, NamePath, ResolvedIndexVariant,
    ScopedName,
};
use crate::syntax::span::Span;
use crate::tir::typed::NatLinearForm;

use crate::registry::error::GraphcalError;
use crate::registry::types::{Registry, TypeGenericConstraint};

use super::super::helpers::{
    cartesian_product, format_inferred_type, resolve_field_type, struct_type_def_for_inferred,
};
use super::super::{DeclaredType, InferredIndex, InferredStructType, InferredType};
use super::infer_type;

fn legacy_index_name_from_path(path: &NamePath) -> IndexName {
    IndexName::from(path.leaf().clone())
}

/// Get the index name for a for binding.
fn for_binding_index_name(index: &ForBindingIndex) -> IndexName {
    match index {
        ForBindingIndex::Named(spanned) => legacy_index_name_from_path(&spanned.value),
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

fn validate_index_path_module_scope(
    path: &crate::syntax::names::NamePath,
    tir: &crate::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<(), GraphcalError> {
    let Some((qualifier, _)) = path.qualifier_and_leaf() else {
        return Ok(());
    };
    let Some(alias) = qualifier.first() else {
        return Ok(());
    };
    if tir.module_aliases.contains_key(alias.as_str()) {
        Ok(())
    } else {
        Err(GraphcalError::EvalError {
            message: format!("module alias `{alias}` is not in scope for index path `{path}`"),
            src: src.clone(),
            span: span.into(),
        })
    }
}

fn resolved_collection_refs(
    dag: Option<&crate::tir::typed::DagTIR>,
) -> Option<&crate::tir::typed::ResolvedCollectionRefs> {
    dag.and_then(|dag| dag.resolved_collection_refs.as_ref())
}

fn inferred_index_for_path(
    path: &NamePath,
    span: Span,
    dag: Option<&crate::tir::typed::DagTIR>,
) -> InferredIndex {
    resolved_collection_refs(dag)
        .and_then(|refs| refs.for_binding_indexes.get(&span))
        .cloned()
        .map_or_else(
            || InferredIndex::legacy(legacy_index_name_from_path(path)),
            InferredIndex::from_resolved,
        )
}

fn resolved_index_variant_for_arg(
    index_span: Span,
    variant_span: Span,
    dag: Option<&crate::tir::typed::DagTIR>,
) -> Option<&crate::syntax::names::ResolvedIndexVariant> {
    let span = index_span.merge(variant_span);
    resolved_collection_refs(dag).and_then(|refs| refs.index_access_variants.get(&span))
}

fn index_def_for_inferred<'a>(
    index: &InferredIndex,
    dag: Option<&'a crate::tir::typed::DagTIR>,
    registry: &'a Registry,
) -> Option<&'a crate::registry::types::IndexDef> {
    index
        .resolved()
        .and_then(|resolved| {
            resolved_collection_refs(dag).and_then(|refs| refs.index_defs.get(resolved))
        })
        .or_else(|| registry.indexes.get_index(index.name().as_str()))
}

fn resolved_map_entry_variant_for_key<'a>(
    key: &crate::desugar::resolved_ast::MapEntryKey,
    dag: Option<&'a crate::tir::typed::DagTIR>,
) -> Option<&'a ResolvedIndexVariant> {
    let span = key.index.span.merge(key.variant.span);
    resolved_collection_refs(dag).and_then(|refs| refs.map_entry_variants.get(&span))
}

fn inferred_index_for_map_entry_key(
    key: &crate::desugar::resolved_ast::MapEntryKey,
    dag: Option<&crate::tir::typed::DagTIR>,
) -> InferredIndex {
    resolved_map_entry_variant_for_key(key, dag).map_or_else(
        || InferredIndex::legacy(key.index.value.registry_name()),
        |variant| InferredIndex::from_resolved(variant.index().clone()),
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum MapLiteralVariantKey {
    Resolved(ResolvedIndexVariant),
    Legacy {
        index: IndexName,
        variant: IndexVariantName,
    },
}

impl MapLiteralVariantKey {
    fn variant(&self) -> &IndexVariantName {
        match self {
            Self::Resolved(resolved) => resolved.variant(),
            Self::Legacy { variant, .. } => variant,
        }
    }

    fn display_index<'a>(&'a self, fallback: &'a IndexName) -> &'a IndexName {
        match self {
            Self::Resolved(_) => fallback,
            Self::Legacy { index, .. } => index,
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
        match self.index.resolved() {
            Some(index) => {
                MapLiteralVariantKey::Resolved(ResolvedIndexVariant::new(index.clone(), variant))
            }
            None => MapLiteralVariantKey::Legacy {
                index: self.index.name().clone(),
                variant,
            },
        }
    }
}

/// Infer the type of a for comprehension.
pub(super) fn infer_for_comp(
    bindings: &[ForBinding],
    body: &Expr,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    dag: Option<&crate::tir::typed::DagTIR>,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    // Add loop variables to local_types, infer body type, wrap in Indexed layers
    let mut inner_locals = local_types.clone();
    for binding in bindings {
        let var_type =
            match &binding.index {
                ForBindingIndex::Named(spanned_idx) => {
                    if resolved_collection_refs(dag)
                        .and_then(|refs| refs.for_binding_indexes.get(&spanned_idx.span))
                        .is_none()
                    {
                        validate_index_path_module_scope(
                            &spanned_idx.value,
                            tir,
                            src,
                            spanned_idx.span,
                        )?;
                    }
                    let index_identity =
                        inferred_index_for_path(&spanned_idx.value, spanned_idx.span, dag);
                    let idx_def = index_def_for_inferred(&index_identity, dag, registry)
                        .ok_or_else(|| GraphcalError::UnknownIndex {
                            name: index_identity.name().clone(),
                            src: src.clone(),
                            span: spanned_idx.span.into(),
                        })?;
                    match &idx_def.kind {
                        crate::registry::types::IndexKind::Named { .. }
                        | crate::registry::types::IndexKind::RequiredNamed => {
                            InferredType::Label(index_identity)
                        }
                        crate::registry::types::IndexKind::Range(
                            crate::registry::types::RangeIndexData { dimension, .. },
                        )
                        | crate::registry::types::IndexKind::RequiredRange { dimension } => {
                            InferredType::Scalar(dimension.clone())
                        }
                        crate::registry::types::IndexKind::NatRange { size } => {
                            InferredType::Fin(NatLinearForm::from_constant(*size as u64))
                        }
                    }
                }
                ForBindingIndex::Range { arg, .. } => {
                    // `for i: range(N)` — loop variable is Fin(N)
                    InferredType::Fin(normalize_nat_expr_lenient(arg))
                }
            };
        inner_locals.insert(binding.var.value.as_str().to_owned(), var_type);
    }
    let body_type = infer_type(
        body,
        declared_types,
        &inner_locals,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    // Wrap body type with index layers (outermost binding first)
    let mut result = body_type;
    for binding in bindings.iter().rev() {
        let index = match &binding.index {
            ForBindingIndex::Named(spanned_idx) => {
                inferred_index_for_path(&spanned_idx.value, spanned_idx.span, dag)
            }
            ForBindingIndex::Range { .. } => {
                InferredIndex::legacy(for_binding_index_name(&binding.index))
            }
        };
        result = InferredType::Indexed {
            element: Box::new(result),
            index,
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
    entries: &[crate::desugar::resolved_ast::MapEntry],
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    dag: Option<&crate::tir::typed::DagTIR>,
    tir: &crate::tir::typed::TIR,
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
    for entry in entries {
        for key in &entry.keys {
            if let crate::syntax::ast::MapEntryIndex::Named(path) = &key.index.value
                && resolved_map_entry_variant_for_key(key, dag).is_none()
            {
                validate_index_path_module_scope(path, tir, src, key.index.span)?;
            }
        }
    }

    // Validate index identities: all entries must use the same indexes in the same order.
    let mut axes = Vec::with_capacity(arity);
    for key in &entries[0].keys {
        let index = inferred_index_for_map_entry_key(key, dag);
        let idx_def = index_def_for_inferred(&index, dag, registry).ok_or_else(|| {
            GraphcalError::UnknownIndex {
                name: index.name().clone(),
                src: src.clone(),
                span: key.index.span.into(),
            }
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
        axes.push(MapLiteralAxis {
            index,
            variants: idx_def.variants(),
        });
    }
    for entry in &entries[1..] {
        for (i, key) in entry.keys.iter().enumerate() {
            let key_index = inferred_index_for_map_entry_key(key, dag);
            if key_index != axes[i].index {
                return Err(GraphcalError::IndexMismatch {
                    expected: axes[i].index.name().clone(),
                    found: key_index.name().clone(),
                    src: src.clone(),
                    span: key.index.span.into(),
                });
            }
        }
    }

    // Check totality over the Cartesian product, preserving resolved index owners where known.
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
    let mut expected_tuples: std::collections::HashSet<Vec<MapLiteralVariantKey>> =
        std::collections::HashSet::new();
    cartesian_product(&axes_variant_keys, &mut Vec::new(), &mut expected_tuples);
    let mut provided_tuples: std::collections::HashSet<Vec<MapLiteralVariantKey>> =
        std::collections::HashSet::new();
    for entry in entries {
        let tuple: Vec<MapLiteralVariantKey> = entry
            .keys
            .iter()
            .enumerate()
            .map(|(i, key)| {
                resolved_map_entry_variant_for_key(key, dag)
                    .cloned()
                    .map(MapLiteralVariantKey::Resolved)
                    .unwrap_or_else(|| axes[i].variant_key(key.variant.value.clone()))
            })
            .collect();
        if !provided_tuples.insert(tuple.clone()) {
            return Err(GraphcalError::EvalError {
                message: format!(
                    "duplicate map literal entry for key tuple ({})",
                    entry
                        .keys
                        .iter()
                        .enumerate()
                        .map(|(i, k)| k
                            .variant
                            .value
                            .qualified_by(axes[i].index.name())
                            .to_string())
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
                let key_variant = resolved_map_entry_variant_for_key(key, dag)
                    .map_or(&key.variant.value, ResolvedIndexVariant::variant);
                if !axes[i].variants.iter().any(|v| v == key_variant) {
                    return Err(GraphcalError::UnknownVariant {
                        index_name: axes[i].index.name().clone(),
                        variant_name: key_variant.clone(),
                        src: src.clone(),
                        span: key.variant.span.into(),
                    });
                }
            }
        }
    }
    // Check for extra variants (provided but not in expected set)
    let extra: Vec<Vec<MapLiteralVariantKey>> = provided_tuples
        .difference(&expected_tuples)
        .cloned()
        .collect();
    if !extra.is_empty() {
        if arity == 1 {
            let extra_variants: Vec<IndexVariantName> =
                extra.iter().map(|t| t[0].variant().clone()).collect();
            return Err(GraphcalError::ExtraVariants {
                index_name: axes[0].index.name().clone(),
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
                    .map(|(i, v)| {
                        v.variant()
                            .qualified_by(v.display_index(axes[i].index.name()))
                            .to_string()
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
    // Check for missing tuples
    let missing: Vec<Vec<MapLiteralVariantKey>> = expected_tuples
        .difference(&provided_tuples)
        .cloned()
        .collect();
    if !missing.is_empty() {
        if arity == 1 {
            let missing_variants: Vec<IndexVariantName> =
                missing.iter().map(|t| t[0].variant().clone()).collect();
            return Err(GraphcalError::MissingVariants {
                index_name: axes[0].index.name().clone(),
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
                    .map(|(i, v)| {
                        v.variant()
                            .qualified_by(v.display_index(axes[i].index.name()))
                            .to_string()
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
    // Infer element type from first entry, check all entries match
    let first_type = infer_type(
        &entries[0].value,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    // Reject nested Indexed when the inner index is a label (named) index.
    // Label-indexed elements should use tuple keys instead: { (I.A, J.B): expr, ... }.
    // Allow when the inner index is a range index, enabling mixed-index construction:
    //   { LabelIndex.Variant: for t: RangeIndex { ... }, ... }
    if let InferredType::Indexed { index, .. } = &first_type {
        let inner_is_label =
            index_def_for_inferred(index, dag, registry).is_some_and(|def| !def.is_range());
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
    // Wrap in nested Indexed layers (reverse order, matching `for` comprehension)
    let mut result = first_type;
    for axis in axes.iter().rev() {
        result = InferredType::Indexed {
            element: Box::new(result),
            index: axis.index.clone(),
        };
    }
    Ok(result)
}

/// Infer the type of an index access expression.
pub(super) fn infer_index_access(
    expr: &Expr,
    inner: &Expr,
    args: &[IndexArg],
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    dag: Option<&crate::tir::typed::DagTIR>,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let inner_type = infer_type(
        inner,
        declared_types,
        local_types,
        dag,
        tir,
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
                let resolved_variant =
                    resolved_index_variant_for_arg(index.span, variant.span, dag);
                if resolved_variant.is_none() {
                    validate_index_path_module_scope(&index.value, tir, src, index.span)?;
                }
                let arg_index = resolved_variant.map_or_else(
                    || InferredIndex::legacy(legacy_index_name_from_path(&index.value)),
                    |variant| InferredIndex::from_resolved(variant.index().clone()),
                );
                if arg_index != idx_name {
                    return Err(GraphcalError::IndexMismatch {
                        expected: idx_name.name().clone(),
                        found: arg_index.name().clone(),
                        src: src.clone(),
                        span: index.span.into(),
                    });
                }
                if resolved_variant.is_none() {
                    // Validate variant exists on the legacy leaf-keyed path only
                    // when no HIR-resolved variant sidecar is available. The
                    // HIR sidecar itself is produced by successful canonical
                    // `ResolvedIndexVariant` lookup.
                    let idx_def =
                        index_def_for_inferred(&idx_name, dag, registry).ok_or_else(|| {
                            GraphcalError::UnknownIndex {
                                name: idx_name.name().clone(),
                                src: src.clone(),
                                span: index.span.into(),
                            }
                        })?;
                    if !idx_def
                        .variants()
                        .iter()
                        .any(|v| v.as_str() == variant.value.as_str())
                    {
                        return Err(GraphcalError::UnknownVariant {
                            index_name: idx_name.name().clone(),
                            variant_name: variant.value.clone(),
                            src: src.clone(),
                            span: variant.span.into(),
                        });
                    }
                }
            }
            IndexArg::Var(ident) => {
                // Must be a loop variable with matching index
                let var_type = local_types.get(ident.name.as_str()).ok_or_else(|| {
                    GraphcalError::UnknownLocalRef {
                        name: ident.name.to_string(),
                        src: src.clone(),
                        span: ident.span.into(),
                    }
                })?;
                match var_type {
                    InferredType::Label(label_index) => {
                        if label_index != &idx_name {
                            return Err(GraphcalError::IndexMismatch {
                                expected: idx_name.name().clone(),
                                found: label_index.name().clone(),
                                src: src.clone(),
                                span: ident.span.into(),
                            });
                        }
                    }
                    InferredType::Struct(type_name, args) => {
                        if type_name.name().as_str() != idx_name.as_str() || !args.is_empty() {
                            return Err(GraphcalError::IndexMismatch {
                                expected: idx_name.name().clone(),
                                found: IndexName::new(type_name.name().as_str()),
                                src: src.clone(),
                                span: ident.span.into(),
                            });
                        }
                    }
                    InferredType::Scalar(_) => {
                        // Allow scalar locals to be used as index args
                        // for range indexes (e.g. prev_i, i in Unfold)
                        let idx_def =
                            index_def_for_inferred(&idx_name, dag, registry).ok_or_else(|| {
                                GraphcalError::UnknownIndex {
                                    name: idx_name.name().clone(),
                                    src: src.clone(),
                                    span: ident.span.into(),
                                }
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
                        if let Some(idx_def) = index_def_for_inferred(&idx_name, dag, registry)
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
                            index_def_for_inferred(&idx_name, dag, registry)
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
                    dag,
                    tir,
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
                if let Some(idx_def) = index_def_for_inferred(&idx_name, dag, registry)
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
                    let index_form = index_def_for_inferred(&idx_name, dag, registry).map_or_else(
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
        ExprKind::LocalRef(ident) => match local_types.get(ident.name.as_str())? {
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
    acc_name: &crate::syntax::span::Spanned<crate::syntax::names::LocalName>,
    val_name: &crate::syntax::span::Spanned<crate::syntax::names::LocalName>,
    body: &Expr,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    dag: Option<&crate::tir::typed::DagTIR>,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    // source must be indexed, init must be scalar matching element type
    let source_type = infer_type(
        source,
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
    let init_type = infer_type(
        init,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    // init and element must have the same type
    if init_type != *element {
        return Err(GraphcalError::DimensionMismatch {
            expected: format_inferred_type(&element, registry),
            found: format_inferred_type(&init_type, registry),
            help: "scan init value must match element type of source".to_string(),
            src: src.clone(),
            span: init.span.into(),
        });
    }
    // Bind acc and val as locals with element type
    let mut scan_locals = local_types.clone();
    scan_locals.insert(acc_name.value.as_str().to_owned(), *element.clone());
    scan_locals.insert(val_name.value.as_str().to_owned(), *element.clone());
    let body_type = infer_type(
        body,
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
    prev_name: &crate::syntax::span::Spanned<crate::syntax::names::LocalName>,
    curr_name: &crate::syntax::span::Spanned<crate::syntax::names::LocalName>,
    body: &Expr,
    owner_decl_name: Option<&str>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    dag: Option<&crate::tir::typed::DagTIR>,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let init_type = infer_type(
        init,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;

    // Look up the owning declaration's type to find the range index and its
    // dimension. This is precise — it uses the specific node's declared type
    // rather than scanning all declared types (which would pick an arbitrary
    // range index if multiple exist). The owner is a top-level decl: bare local.
    let mut scan_locals = local_types.clone();
    let owner_range_index = owner_decl_name.and_then(|name| {
        let dt = declared_types.get(&ScopedName::local(name))?;
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
            crate::registry::types::IndexKind::Range(crate::registry::types::RangeIndexData {
                dimension,
                ..
            })
            | crate::registry::types::IndexKind::RequiredRange { dimension } => Some(dimension),
            _ => None,
        };
        if let Some(dimension) = dimension {
            scan_locals.insert(
                prev_name.value.as_str().to_owned(),
                InferredType::Scalar(dimension.clone()),
            );
            scan_locals.insert(
                curr_name.value.as_str().to_owned(),
                InferredType::Scalar(dimension.clone()),
            );
        }
    } else {
        // Fallback: dimensionless when owner is unknown or not an indexed range type
        scan_locals.insert(
            prev_name.value.as_str().to_owned(),
            InferredType::Scalar(crate::syntax::dimension::Dimension::dimensionless()),
        );
        scan_locals.insert(
            curr_name.value.as_str().to_owned(),
            InferredType::Scalar(crate::syntax::dimension::Dimension::dimensionless()),
        );
    }

    let body_type = infer_type(
        body,
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

    // The result type is Indexed { element: init_type, index: <range_index> }
    if let Some((index_name, _)) = owner_range_index {
        return Ok(InferredType::Indexed {
            element: Box::new(init_type),
            index: InferredIndex::legacy(index_name),
        });
    }

    // Fallback: return init_type (will fail annotation check if declared as indexed)
    Ok(init_type)
}

/// Infer the type of a field access expression.
pub(super) fn infer_field_access(
    inner: &Expr,
    field: &crate::syntax::span::Spanned<FieldName>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    dag: Option<&crate::tir::typed::DagTIR>,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let inner_type = infer_type(
        inner,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    match &inner_type {
        InferredType::Struct(type_name, type_args) => {
            let type_def =
                struct_type_def_for_inferred(type_name, dag, registry).ok_or_else(|| {
                    GraphcalError::UnknownStructType {
                        name: type_name.to_string(),
                        src: src.clone(),
                        span: inner.span.into(),
                    }
                })?;
            // Field access is only valid on the record-shape: a single
            // -variant union whose sole constructor's name equals the
            // type's name. Multi-variant unions must be destructured
            // via `match`; required type stubs carry no fields.
            let fields = type_def.record_fields().ok_or_else(|| {
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
            let field_def = fields
                .iter()
                .find(|f| f.name.as_str() == field.value.as_str())
                .ok_or_else(|| GraphcalError::UnknownField {
                    type_name: type_name.name().clone(),
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

/// Infer the type of a constructor call expression.
#[expect(
    clippy::too_many_lines,
    reason = "exhaustive validation of constructor calls"
)]
pub(super) fn infer_constructor_call(
    expr: &Expr,
    callee: &crate::syntax::ast::IdentPath,
    constructor_generic_args: &[GenericArg],
    fields: &[crate::desugar::resolved_ast::FieldInit],
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    dag: Option<&crate::tir::typed::DagTIR>,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let resolved_target = dag
        .and_then(|dag| dag.resolved_constructor_refs.as_ref())
        .and_then(|refs| refs.constructor_calls.get(&callee.span()));

    // Resolve through the constructor namespace. With every user-defined
    // `type` stored as an n-variant union, a constructor call names a
    // constructor — not a type. The union the constructor belongs to becomes
    // the value's type.
    let (type_def, variant, constructor_name, constructor_span, owning_type_identity) =
        match resolved_target {
            Some(target) => (
                &target.type_def,
                &target.variant,
                target.variant.name.clone(),
                callee.span(),
                InferredStructType::from_resolved(target.owning_type.clone()),
            ),
            None => {
                let Some(constructor) = callee.as_bare() else {
                    return Err(GraphcalError::UnknownStructType {
                        name: callee.display_path(),
                        src: src.clone(),
                        span: callee.span().into(),
                    });
                };
                let constructor_name =
                    crate::syntax::names::ConstructorName::from_atom(constructor.name.clone());
                let (type_def, variant) = registry
                    .types
                    .lookup_ctor(&constructor_name)
                    .ok_or_else(|| GraphcalError::UnknownStructType {
                        name: constructor.name.to_string(),
                        src: src.clone(),
                        span: constructor.span.into(),
                    })?;
                (
                    type_def,
                    variant,
                    constructor_name,
                    constructor.span,
                    InferredStructType::legacy(type_def.name.clone()),
                )
            }
        };
    let owning_type_name = type_def.name.clone();
    let variant_fields: &[crate::registry::types::StructField] = &variant.fields;

    // Resolve constructor generic args for generic types.
    let resolved_type_args: Vec<InferredType> = if constructor_generic_args.is_empty()
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
                    owning_type_name,
                    constructor_generic_args.len()
                ),
                src: src.clone(),
                span: constructor_span.into(),
            });
        }
        let no_dim_params: &[GenericParamName] = &[];
        let no_index_params: &[GenericParamName] = &[];
        let no_nat_params: &[GenericParamName] = &[];
        let mut args = Vec::with_capacity(total_params);
        for (param, arg) in type_def.generic_params.iter().zip(constructor_generic_args) {
            match (param.constraint, arg) {
                (TypeGenericConstraint::Nat, GenericArg::Nat(nat_expr)) => {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "constructor generic argument `{}` for Nat parameter `{}` is not yet representable in value types",
                            nat_expr, param.name
                        ),
                        src: src.clone(),
                        span: nat_expr.span().into(),
                    });
                }
                (TypeGenericConstraint::Nat, GenericArg::Type(type_expr)) => {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "generic parameter `{}` expects a Nat argument, got a type argument",
                            param.name
                        ),
                        src: src.clone(),
                        span: type_expr.span.into(),
                    });
                }
                (_, GenericArg::Nat(nat_expr)) => {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "generic parameter `{}` expects a type argument, got Nat argument `{}`",
                            param.name, nat_expr
                        ),
                        src: src.clone(),
                        span: nat_expr.span().into(),
                    });
                }
                (_, GenericArg::Type(type_expr)) => {
                    let resolved = crate::tir::typed::resolve_type_expr(
                        type_expr,
                        registry,
                        no_dim_params,
                        no_index_params,
                        no_nat_params,
                        src,
                    )?;
                    let dt = crate::tir::typed::resolved_to_declared_type(&resolved, src)?;
                    args.push(InferredType::from(&dt));
                }
            }
        }
        // Fill in defaults for remaining params
        for param in type_def
            .generic_params
            .iter()
            .skip(constructor_generic_args.len())
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
                    span: constructor_span.into(),
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
            args.push(InferredType::from(&dt));
        }
        args
    } else {
        vec![]
    };

    // Check for extra fields
    let def_field_names: std::collections::HashSet<&str> =
        variant_fields.iter().map(|f| f.name.as_str()).collect();
    let provided_names: Vec<&str> = fields.iter().map(|f| f.name.value.as_str()).collect();
    let extra: Vec<FieldName> = provided_names
        .iter()
        .filter(|n| !def_field_names.contains(**n))
        .map(|n| FieldName::new(*n))
        .collect();
    if !extra.is_empty() {
        return Err(GraphcalError::ExtraFields {
            type_name: owning_type_name.clone(),
            extra,
            src: src.clone(),
            span: expr.span.into(),
        });
    }

    // Check for missing fields
    let provided_set: std::collections::HashSet<&str> = provided_names.iter().copied().collect();
    let missing: Vec<FieldName> = variant_fields
        .iter()
        .filter(|f| !provided_set.contains(f.name.as_str()))
        .map(|f| f.name.clone())
        .collect();
    if !missing.is_empty() {
        return Err(GraphcalError::MissingFields {
            type_name: owning_type_name.clone(),
            missing,
            src: src.clone(),
            span: expr.span.into(),
        });
    }

    // Type-check each field's value
    for field_init in fields {
        let field_def = variant_fields
            .iter()
            .find(|f| f.name.as_str() == field_init.name.value.as_str())
            .ok_or_else(|| GraphcalError::EvalError {
                message: format!(
                    "internal: unknown field `{}` in constructor `{}`",
                    field_init.name.value, constructor_name
                ),
                src: src.clone(),
                span: field_init.name.span.into(),
            })?;

        let value_type = infer_type(
            &field_init.value,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?;

        let expected_field_type = resolve_field_type(
            &field_def.type_ann,
            type_def,
            &resolved_type_args,
            registry,
            src,
        )?;
        if value_type != expected_field_type {
            return Err(GraphcalError::FieldDimensionMismatch {
                type_name: owning_type_name.clone(),
                field_name: field_init.name.value.clone(),
                expected: format_inferred_type(&expected_field_type, registry),
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
