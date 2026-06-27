//! Syntactic sugar desugaring pass (issue #481).
//!
//! Multi-declarations — `param a: T[I], const node b: U[I, J] = table[…]{…};` —
//! are parsed as `DeclKind::Sugar(RawDeclSugar::Multi(MultiDecl))` to preserve
//! source structure for surface-aware tools (formatter, LSP). This module
//! expands them into N parallel ordinary declarations before semantic
//! analysis, so lowering, TIR, resolver, and the runtime all see only
//! single declarations.
//!
//! The desugar pass is invoked at the top of
//! HIR lowering; everything downstream
//! can assume `DeclKind::Sugar` does not appear in the AST.
//!
//! Note: today this pass mutates a `File<Raw>` in place, eliminating sugar
//! variants by walking + flattening. A future commit will switch the API
//! to `File<Raw> -> File<Desugared>` (the [`From`] impl in
//! [`crate::desugar::convert`] is the engine for that transition) and
//! pin all consumers to `File<Desugared>`, replacing the runtime
//! [`unreachable_post_desugar`] panics with [`crate::syntax::phase::never`]
//! on the [`Infallible`](core::convert::Infallible) `Sugar` payload.
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
        "DeclKind::Sugar should have been removed by syntax::desugar::desugar_multi_decls_in_file"
    )
}

use crate::syntax::ast::{
    ConstNodeDecl, DeclKind, Declaration, Expr, ExprKind, File, MapEntry, MapEntryIndex,
    MapEntryKey, MultiDecl, MultiHeaderCell, MultiSlotColumnSpan, MultiSlotKind, NodeDecl,
    ParamDecl, TableIndexSpec,
};
use crate::syntax::index_name::{IndexName, IndexVariantName};
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::phase::{Desugared, Raw};
use crate::syntax::span::Spanned;

fn multi_entry_keys(
    mut prefix: Vec<MapEntryKey>,
    row: MapEntryKey,
    extra: Option<MapEntryKey>,
) -> NonEmpty<MapEntryKey> {
    if prefix.is_empty() {
        let rest = extra.into_iter().collect();
        NonEmpty::new(row, rest)
    } else {
        let first = prefix.remove(0);
        prefix.push(row);
        prefix.extend(extra);
        NonEmpty::new(first, prefix)
    }
}

/// Expand every multi-decl in `file` into its N constituent ordinary
/// declarations and return the result as [`File<Desugared>`].
///
/// Consumes the raw file by value because the phase split is a type-level
/// transformation: `File<Raw>` and `File<Desugared>` are distinct types and
/// cannot share storage. The actual conversion logic lives in
/// [`crate::desugar::convert`] (the `From<File<Raw>> for File<Desugared>`
/// impl), which dispatches the multi-decl `Sugar` arm to
/// [`expand_multi_decl`].
#[must_use]
pub fn desugar_multi_decls_in_file(file: File<Raw>) -> File<Desugared> {
    file.into()
}

/// A declaration produced by multi-decl expansion.
///
/// Expansion can only yield the three slot kinds, so this enum makes the
/// "never `Sugar`" invariant a type instead of a convention the desugar
/// pass had to re-assert with a panic.
#[derive(Debug)]
pub enum ExpandedSlotDecl {
    Param(ParamDecl, crate::syntax::span::Span),
    Node(NodeDecl, crate::syntax::span::Span),
    ConstNode(ConstNodeDecl, crate::syntax::span::Span),
}

impl ExpandedSlotDecl {
    /// Re-wrap as a generic [`Declaration`] (used by tests that inspect the
    /// expansion through the ordinary AST surface).
    #[must_use]
    pub fn into_declaration(self) -> Declaration {
        let (kind, span) = match self {
            Self::Param(p, span) => (DeclKind::Param(p), span),
            Self::Node(n, span) => (DeclKind::Node(n), span),
            Self::ConstNode(c, span) => (DeclKind::ConstNode(c), span),
        };
        Declaration {
            attributes: vec![],
            kind,
            span,
        }
    }
}

/// Expand a single `MultiDecl` into its N constituent declarations.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "single cohesive routine for multi-decl expansion"
)]
pub fn expand_multi_decl(multi: &MultiDecl) -> Vec<ExpandedSlotDecl> {
    let row_index_spec = multi.shared_axes.row_axis().clone();
    let slice_axis_specs: &[TableIndexSpec] = multi.shared_axes.slice_axes();

    let row_index_name = match &row_index_spec {
        TableIndexSpec::Named(s) => Spanned::new(MapEntryIndex::Named(s.value.clone()), s.span),
        TableIndexSpec::NatRange(n, sp) => Spanned::new(MapEntryIndex::NatRange(*n), *sp),
    };

    let mut out: Vec<ExpandedSlotDecl> = Vec::with_capacity(multi.slots.len());
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
                        let row_key = MapEntryKey {
                            index: row_index_name.clone(),
                            variant: row.label.clone(),
                        };
                        slot_entries.push(MapEntry {
                            keys: multi_entry_keys(slice.prefix_keys.clone(), row_key, None),
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
                    let extra_index_name = Spanned::new(
                        MapEntryIndex::Named(extra_axis.value.clone().into()),
                        extra_axis.span,
                    );
                    let col_variants: Vec<Spanned<IndexVariantName>> = slice.header_cells
                        [*start..*end]
                        .iter()
                        .filter_map(|c| match c {
                            MultiHeaderCell::Variant { variant, .. } => Some(variant.clone()),
                            MultiHeaderCell::Underscore { .. } => None,
                        })
                        .collect();
                    for row in &slice.rows {
                        for (local_col, col_variant) in col_variants.iter().enumerate() {
                            let global_col = start + local_col;
                            let row_key = MapEntryKey {
                                index: row_index_name.clone(),
                                variant: row.label.clone(),
                            };
                            let extra_key = MapEntryKey {
                                index: extra_index_name.clone(),
                                variant: col_variant.clone(),
                            };
                            slot_entries.push(MapEntry {
                                keys: multi_entry_keys(
                                    slice.prefix_keys.clone(),
                                    row_key,
                                    Some(extra_key),
                                ),
                                value: row.values[global_col].clone(),
                            });
                        }
                    }
                }
            }
        }

        if let Some(extra) = extra_axis_name {
            slot_indexes.push(TableIndexSpec::Named(Spanned::new(
                extra.value.into(),
                extra.span,
            )));
        }

        let table_expr = Expr::new(
            ExprKind::Sugar(crate::syntax::ast::RawExprSugar::TableLiteral {
                indexes: slot_indexes,
                entries: slot_entries,
            }),
            multi.table_expr_span,
        );

        // `span` covers the slot header through the closing `;` of the
        // whole multi-decl so diagnostics land on the source surface.
        let decl_span = slot.header_span.merge(multi.span);

        out.push(match slot.kind {
            MultiSlotKind::Param => ExpandedSlotDecl::Param(
                ParamDecl {
                    name: slot.name.clone(),
                    type_ann: slot.type_ann.clone(),
                    value: Some(table_expr),
                },
                decl_span,
            ),
            MultiSlotKind::Node => ExpandedSlotDecl::Node(
                NodeDecl {
                    visibility: slot.visibility,
                    name: slot.name.clone(),
                    type_ann: slot.type_ann.clone(),
                    value: table_expr,
                },
                decl_span,
            ),
            MultiSlotKind::ConstNode => ExpandedSlotDecl::ConstNode(
                ConstNodeDecl {
                    visibility: slot.visibility,
                    name: slot.name.clone(),
                    type_ann: slot.type_ann.clone(),
                    value: table_expr,
                },
                decl_span,
            ),
        });
    }

    out
}
