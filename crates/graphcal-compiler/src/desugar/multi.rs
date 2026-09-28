//! Multi-declaration expansion (issue #481).
//!
//! Multi-declarations — `param a: T[I], const node b: U[I, J] = table[…]{…};` —
//! are parsed as `DeclKind::Sugar(RawDeclSugar::Multi(MultiDecl))` to preserve
//! source structure for surface-aware tools (formatter, LSP). This module
//! expands one into N parallel ordinary declarations, each initialized with
//! the slot's column(s) as a map literal, so lowering, TIR, resolver, and the
//! runtime all see only single declarations.
//!
//! ## Span fidelity
//!
//! Each synthesized declaration carries:
//! - `span` — the slot header merged with the whole multi-decl surface
//!   (so errors referencing the whole decl still land on the surface).
//! - `name.span` — pointing at the slot's name identifier in the source.
//! - `type_ann.span` — pointing at the slot's type annotation in the source.
//! - `value` — a `MapLiteral` whose span covers the original `table[…] {…}`
//!   body; each entry's value carries the span of the source cell it came
//!   from.

use std::iter;

use crate::node_definition::NodeDefinition;
use crate::syntax::ast::{
    ConstNodeDecl, DeclKind, Declaration, Expr, ExprKind, MapEntry, MapEntryIndex, MapEntryKey,
    MultiDataRow, MultiDecl, MultiDeclSlice, MultiHeaderCell, MultiSlotColumnSpan, NodeDecl,
    ParamDecl, SlotKind, TableIndexSpec,
};
use crate::syntax::comments::DocComment;
use crate::syntax::index_name::IndexEntryKey;
use crate::syntax::names::NamePath;
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::phase::Desugared;
use crate::syntax::span::Spanned;

/// Expand a multi-decl into one declaration per slot, in slot order. The doc
/// block above the multi-decl documents every expanded slot.
pub(super) fn expand_multi_decl(
    multi: &MultiDecl,
    doc: Option<&DocComment>,
) -> Vec<Declaration<Desugared>> {
    let row_index = match multi.shared_axes().row_axis() {
        TableIndexSpec::Named(axis) => {
            Spanned::new(MapEntryIndex::Named(axis.value.clone()), axis.span)
        }
        TableIndexSpec::Finite { cardinality, span } => {
            Spanned::new(MapEntryIndex::Finite(*cardinality), *span)
        }
    };

    multi
        .slots()
        .iter()
        .enumerate()
        .map(|(slot_index, slot)| {
            let entries = multi
                .slices()
                .iter()
                .flat_map(|slice| slot_entries(slice, slot_index, &row_index))
                .collect();
            let value = Expr::new(ExprKind::MapLiteral { entries }, multi.table_expr_span);
            let name = slot.name.clone();
            let type_ann = slot.type_ann.clone().into();
            let kind = match slot.kind {
                SlotKind::Param => DeclKind::Param(ParamDecl {
                    name,
                    type_ann,
                    value: Some(value),
                }),
                SlotKind::Node(visibility) => DeclKind::Node(NodeDecl {
                    visibility,
                    name,
                    type_ann,
                    definition: NodeDefinition::Formula(value),
                }),
                SlotKind::ConstNode(visibility) => DeclKind::ConstNode(ConstNodeDecl {
                    visibility,
                    name,
                    type_ann,
                    value,
                }),
            };
            Declaration {
                attributes: vec![],
                kind,
                // The slot header merged with the whole multi-decl surface
                // (first slot keyword through the closing `;`), so
                // diagnostics land on the source surface.
                span: slot.header_span.merge(multi.span),
                doc: doc.cloned(),
            }
        })
        .collect()
}

/// The map entries one slice contributes to the slot at `slot_index`.
fn slot_entries(
    slice: &MultiDeclSlice,
    slot_index: usize,
    row_index: &Spanned<MapEntryIndex>,
) -> Vec<MapEntry<Desugared>> {
    let row_key = |row: &MultiDataRow| MapEntryKey {
        index: row_index.clone(),
        additional_index_spans: Vec::new(),
        variant: row.label().clone(),
    };
    match &slice.column_layout()[slot_index] {
        MultiSlotColumnSpan::Single(column) => slice
            .rows()
            .iter()
            .map(|row| MapEntry {
                keys: entry_keys(slice.prefix_keys(), row_key(row), None),
                value: row.values()[*column].clone().into(),
            })
            .collect(),
        MultiSlotColumnSpan::Range {
            start,
            end,
            extra_axis,
        } => slice
            .rows()
            .iter()
            .flat_map(|row| {
                (*start..*end).filter_map(move |column| {
                    let MultiHeaderCell::Variant { variant, .. } = &slice.header_cells()[column]
                    else {
                        return None;
                    };
                    Some(MapEntry {
                        keys: entry_keys(
                            slice.prefix_keys(),
                            row_key(row),
                            Some(extra_axis_key(extra_axis, variant)),
                        ),
                        value: row.values()[column].clone().into(),
                    })
                })
            })
            .collect(),
    }
}

fn extra_axis_key(
    extra_axis: &Spanned<NamePath>,
    variant: &Spanned<crate::syntax::index_name::IndexVariantName>,
) -> MapEntryKey {
    MapEntryKey {
        index: Spanned::new(
            MapEntryIndex::Named(extra_axis.value.clone()),
            extra_axis.span,
        ),
        additional_index_spans: vec![extra_axis.span],
        variant: Spanned::new(IndexEntryKey::named(variant.value.clone()), variant.span),
    }
}

/// Slice-label keys, then the row key, then the extra-axis key (if any).
fn entry_keys(
    prefix: &[MapEntryKey],
    row: MapEntryKey,
    extra: Option<MapEntryKey>,
) -> NonEmpty<MapEntryKey> {
    match prefix.split_first() {
        None => NonEmpty::new(row, extra.into_iter().collect()),
        Some((first, rest)) => NonEmpty::new(
            first.clone(),
            rest.iter()
                .cloned()
                .chain(iter::once(row))
                .chain(extra)
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::ast::{File, RawDeclSugar, Visibility};
    use crate::syntax::parser::Parser;
    use crate::syntax::phase::Raw;
    use crate::syntax::span::Span;

    fn expand(source: &str) -> Vec<Declaration<Desugared>> {
        let file: File<Raw> = Parser::new(source).parse_file().unwrap();
        let multi = file
            .declarations
            .iter()
            .find_map(|d| match &d.kind {
                DeclKind::Sugar(RawDeclSugar::Multi(m)) => Some(m),
                _ => None,
            })
            .expect("file has one multi-decl");
        expand_multi_decl(multi, None)
    }

    fn map_entries(expr: &Expr<Desugared>) -> &[MapEntry<Desugared>] {
        match &expr.kind {
            ExprKind::MapLiteral { entries } => entries,
            other => panic!("expected MapLiteral, got {other:?}"),
        }
    }

    fn param_entries(decl: &Declaration<Desugared>) -> &[MapEntry<Desugared>] {
        match &decl.kind {
            DeclKind::Param(p) => map_entries(p.value.as_ref().expect("param has a value")),
            other => panic!("expected Param, got {other:?}"),
        }
    }

    fn key_indexes(entry: &MapEntry<Desugared>) -> Vec<String> {
        entry
            .keys
            .iter()
            .map(|key| key.index.value.to_string())
            .collect()
    }

    fn key_labels(entry: &MapEntry<Desugared>) -> String {
        entry
            .keys
            .iter()
            .map(|key| key.variant.value.to_string())
            .collect::<Vec<_>>()
            .join("/")
    }

    fn number(expr: &Expr<Desugared>) -> f64 {
        match expr.kind {
            ExprKind::Number(n) => n,
            ref other => panic!("expected number, got {other:?}"),
        }
    }

    fn numbers(entries: &[MapEntry<Desugared>]) -> Vec<f64> {
        entries.iter().map(|e| number(&e.value)).collect()
    }

    #[test]
    fn homogeneous_1d_slots_become_one_map_literal_each() {
        let decls = expand(
            r"
param a: Dimensionless[Component],
param b: Dimensionless[Component]
  = table[Component, (_, _)] {
      :           _,   _;
      ComponentA: 1.0, 2.0;
      ComponentB: 3.0, 4.0;
  };
",
        );
        assert_eq!(decls.len(), 2);
        let a = param_entries(&decls[0]);
        let b = param_entries(&decls[1]);
        assert_eq!(numbers(a), [1.0, 3.0]);
        assert_eq!(numbers(b), [2.0, 4.0]);
        assert_eq!(key_indexes(&a[0]), ["Component"]);
        assert_eq!(key_labels(&a[1]), "ComponentB");
    }

    #[test]
    fn slot_kinds_and_visibility_carry_over() {
        let decls = expand(
            r"
pub node a: Int[Component], const node b: Int[Component], pub const node c: Int[Component]
  = table[Component, (_, _, _)] {
      :           _, _, _;
      ComponentA: 1, 2, 3;
  };
",
        );
        assert!(matches!(
            &decls[0].kind,
            DeclKind::Node(NodeDecl {
                visibility: Visibility::Public,
                definition: NodeDefinition::Formula(_),
                ..
            })
        ));
        assert!(matches!(
            &decls[1].kind,
            DeclKind::ConstNode(ConstNodeDecl {
                visibility: Visibility::Private,
                ..
            })
        ));
        assert!(matches!(
            &decls[2].kind,
            DeclKind::ConstNode(ConstNodeDecl {
                visibility: Visibility::Public,
                ..
            })
        ));
    }

    #[test]
    fn extra_axis_slot_is_keyed_by_row_and_header_variant() {
        let decls = expand(
            r"
param p: Power[Component],
param m: Dimensionless[Component, OperationMode]
  = table[Component, (_, OperationMode)] {
      :           _,      Safe, Nominal;
      ComponentA: 10.0 W, 1.0,  2.0;
      ComponentB: 12.0 W, 3.0,  4.0;
  };
",
        );
        let entries = param_entries(&decls[1]);
        assert_eq!(key_indexes(&entries[0]), ["Component", "OperationMode"]);
        assert_eq!(
            entries.iter().map(key_labels).collect::<Vec<_>>(),
            [
                "ComponentA/Safe",
                "ComponentA/Nominal",
                "ComponentB/Safe",
                "ComponentB/Nominal"
            ]
        );
        assert_eq!(numbers(entries), [1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn slice_labels_prefix_every_key() {
        let decls = expand(
            r"
param p: Int[Phase, Component],
param q: Int[Phase, Component]
  = table[Phase, Component, (_, _)] {
      [Phase#Launch]
      :           _, _;
      ComponentA: 1, 2;

      [Phase#Cruise]
      :           _, _;
      ComponentA: 3, 4;
  };
",
        );
        let entries = param_entries(&decls[0]);
        assert_eq!(entries.len(), 2);
        for entry in entries {
            assert_eq!(key_indexes(entry), ["Phase", "Component"]);
        }
        assert_eq!(key_labels(&entries[1]), "Cruise/ComponentA");
    }

    #[test]
    fn finite_row_axis_keys_rows_by_position_and_spans_cover_the_surface() {
        let source = r"
param x: Dimensionless[Fin(2)],
param y: Dimensionless[Fin(2)]
  = table[Fin(2), (_, _)] {
      : _, _;
      1.0, 2.0;
      3.0, 4.0;
  };
";
        let decls = expand(source);
        let entries = param_entries(&decls[1]);
        assert_eq!(
            entries
                .iter()
                .map(|e| e.keys.first().variant.value.clone())
                .collect::<Vec<_>>(),
            [IndexEntryKey::Position(0), IndexEntryKey::Position(1)]
        );
        let text = |span: Span| &source[span.offset()..span.offset() + span.len()];
        // The slot header merged with the whole multi-decl surface.
        assert!(text(decls[1].span).starts_with("param x"));
        assert!(text(decls[1].span).ends_with("};"));
        assert_eq!(text(entries[1].value.span), "4.0");
        let DeclKind::Param(y) = &decls[1].kind else {
            panic!("expected Param")
        };
        assert!(text(y.value.as_ref().unwrap().span).starts_with("table[Fin(2)"));
    }
}
