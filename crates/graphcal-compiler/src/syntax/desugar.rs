//! Syntactic sugar desugaring pass (issue #481).
//!
//! Multi-declarations — `param a: T[I], const node b: U[I, J] = table[…]{…};` —
//! are parsed as `DeclKind::Multi(MultiDecl)` to preserve source structure
//! for surface-aware tools (formatter, etc.). This module expands them into
//! N parallel ordinary declarations before semantic analysis, so lowering,
//! TIR, resolver, and LSP all see only single declarations.
//!
//! The desugar pass is called at the top of
//! [`crate::syntax::name_resolve::resolve_name_refs`]; everything downstream
//! can assume `DeclKind::Multi` does not appear in the AST.
//!
//! ## Span fidelity
//!
//! Each synthesized declaration carries:
//! - `span` — from the slot header keyword through the closing `;` of the
//!   multi-decl (so errors referencing the whole decl still land on the
//!   surface).
//! - `name.span` — pointing at the slot's name identifier in the source.
//! - `type_ann.span` — pointing at the slot's type annotation in the source.
//! - `value` — a synthesized `TableLiteral` whose span covers the original
//!   `table[…] {…}` body; each entry's value carries the span of the source
//!   cell it came from.

/// Panic used in post-desugar exhaustive matches over `DeclKind`. Marks
/// the invariant that [`desugar_multi_decls_in_file`] has already run.
#[cold]
#[track_caller]
#[inline(never)]
#[expect(
    clippy::panic,
    reason = "indicates a broken invariant — multi-decls must be desugared before this pass"
)]
pub fn unreachable_post_desugar() -> ! {
    panic!(
        "DeclKind::Multi should have been removed by syntax::desugar::desugar_multi_decls_in_file"
    )
}

use crate::registry::types::nat_range_index_name;
use crate::syntax::ast::{
    ConstNodeDecl, DagDecl, DeclKind, Declaration, Expr, ExprKind, File, MapEntry, MapEntryKey,
    MultiDecl, MultiHeaderCell, MultiSlotColumnSpan, MultiSlotKind, NodeDecl, ParamDecl,
    TableIndexSpec, Visibility,
};
use crate::syntax::names::{IndexName, Spanned, VariantName};

/// Expand every `DeclKind::Multi` declaration in `file` into its N
/// constituent ordinary declarations (in source order). Recurses into
/// `DagDecl` bodies.
pub fn desugar_multi_decls_in_file(file: &mut File) {
    file.declarations = file.declarations.drain(..).flat_map(desugar_decl).collect();
}

fn desugar_decl(decl: Declaration) -> Vec<Declaration> {
    let Declaration {
        attributes,
        visibility,
        kind,
        span,
    } = decl;
    match kind {
        DeclKind::Multi(multi) => expand_multi_decl(&multi),
        DeclKind::Dag(mut dag) => {
            desugar_multi_decls_in_dag(&mut dag);
            vec![Declaration {
                attributes,
                visibility,
                kind: DeclKind::Dag(dag),
                span,
            }]
        }
        other => vec![Declaration {
            attributes,
            visibility,
            kind: other,
            span,
        }],
    }
}

fn desugar_multi_decls_in_dag(dag: &mut DagDecl) {
    let body = std::mem::take(&mut dag.body);
    dag.body = body.into_iter().flat_map(desugar_decl).collect();
}

/// Expand a single `MultiDecl` into its N constituent declarations.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "single cohesive routine for multi-decl expansion"
)]
pub fn expand_multi_decl(multi: &MultiDecl) -> Vec<Declaration> {
    // Parser guarantees at least one shared axis; the `MultiDeclNoSharedAxis`
    // error rejects the alternative at parse time.
    let Some(row_index_spec) = multi.shared_axes.last().cloned() else {
        return Vec::new();
    };
    let slice_axis_specs: &[TableIndexSpec] = &multi.shared_axes[..multi.shared_axes.len() - 1];

    let row_index_name = match &row_index_spec {
        TableIndexSpec::Named(s) => s.clone(),
        TableIndexSpec::NatRange(n, sp) => {
            Spanned::new(IndexName::new(nat_range_index_name(*n)), *sp)
        }
    };

    let mut out: Vec<Declaration> = Vec::with_capacity(multi.slots.len());
    for (slot_idx, slot) in multi.slots.iter().enumerate() {
        let mut slot_entries: Vec<MapEntry> = Vec::new();
        let mut slot_indexes: Vec<TableIndexSpec> = slice_axis_specs.to_vec();
        slot_indexes.push(row_index_spec.clone());
        let mut extra_axis_name: Option<Spanned<IndexName>> = None;

        for slice in &multi.slices {
            let col_span = &slice.column_layout[slot_idx];
            match col_span {
                MultiSlotColumnSpan::Single(col_idx) => {
                    for row in &slice.rows {
                        let mut keys = slice.prefix_keys.clone();
                        keys.push(MapEntryKey {
                            index: row_index_name.clone(),
                            variant: row.label.clone(),
                        });
                        slot_entries.push(MapEntry {
                            keys,
                            value: row.values[*col_idx].clone(),
                        });
                    }
                }
                MultiSlotColumnSpan::Range {
                    start,
                    end,
                    extra_axis,
                } => {
                    if extra_axis_name.is_none() {
                        extra_axis_name = Some(extra_axis.clone());
                    }
                    let extra_index_name = Spanned::new(extra_axis.value.clone(), extra_axis.span);
                    let col_variants: Vec<Spanned<VariantName>> = slice.header_cells[*start..*end]
                        .iter()
                        .filter_map(|c| match c {
                            MultiHeaderCell::Variant { variant, .. } => Some(variant.clone()),
                            MultiHeaderCell::Underscore { .. } => None,
                        })
                        .collect();
                    for row in &slice.rows {
                        for (local_col, col_variant) in col_variants.iter().enumerate() {
                            let global_col = start + local_col;
                            let mut keys = slice.prefix_keys.clone();
                            keys.push(MapEntryKey {
                                index: row_index_name.clone(),
                                variant: row.label.clone(),
                            });
                            keys.push(MapEntryKey {
                                index: extra_index_name.clone(),
                                variant: col_variant.clone(),
                            });
                            slot_entries.push(MapEntry {
                                keys,
                                value: row.values[global_col].clone(),
                            });
                        }
                    }
                }
            }
        }

        if let Some(extra) = extra_axis_name {
            slot_indexes.push(TableIndexSpec::Named(extra));
        }

        let table_expr = Expr {
            kind: ExprKind::TableLiteral {
                indexes: slot_indexes,
                entries: slot_entries,
            },
            span: multi.table_expr_span,
        };

        let kind = match slot.kind {
            MultiSlotKind::Param => DeclKind::Param(ParamDecl {
                name: slot.name.clone(),
                type_ann: slot.type_ann.clone(),
                value: Some(table_expr),
            }),
            MultiSlotKind::Node => DeclKind::Node(NodeDecl {
                name: slot.name.clone(),
                type_ann: slot.type_ann.clone(),
                value: table_expr,
            }),
            MultiSlotKind::ConstNode => DeclKind::ConstNode(ConstNodeDecl {
                name: slot.name.clone(),
                type_ann: slot.type_ann.clone(),
                value: table_expr,
            }),
        };

        // `span` covers the slot header through the closing `;` of the
        // whole multi-decl so diagnostics land on the source surface.
        let decl_span = slot.header_span.merge(multi.span);

        out.push(Declaration {
            attributes: vec![],
            visibility: Visibility::Private,
            kind,
            span: decl_span,
        });
    }

    out
}
