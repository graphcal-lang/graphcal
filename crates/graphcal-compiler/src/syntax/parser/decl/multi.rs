//! Parser for the multi-declaration surface form (issue #481).
//!
//! A multi-decl introduces N parallel `param` / `node` / `const node`
//! declarations that share the row axis of a table literal:
//!
//! ```text
//! param power_consumption: Power[Component],
//! param n_installed:       Int[Component]
//!   = table[Component, (_, _)] {
//!       :           _,       _;
//!       ComponentA: 10.0 W,  1;
//!       ComponentB: 12.0 W,  2;
//!   };
//! ```
//!
//! The parser keeps the surface form as one [`MultiDecl`](crate::syntax::ast::MultiDecl)
//! (built through [`MultiDeclBuilder`], which lays each slice header out against the
//! slots); the desugar pass later expands it into N ordinary declarations.

use crate::syntax::ast::{
    BindableVisibility, DeclKind, Declaration, MapEntryKey, MultiDeclBuilder, MultiDeclLayoutError,
    MultiDeclRowWidthError, MultiDeclSharedAxes, MultiDeclSlot, MultiHeaderCell, MultiSlotAxis,
    SlotKind, TableIndexSpec, TypeExpr, Visibility,
};
use crate::syntax::decl_name::DeclName;
use crate::syntax::fin_position::FinPosition;
use crate::syntax::index_name::{IndexEntryKey, IndexVariantName};
use crate::syntax::non_empty::AtLeastTwo;
use crate::syntax::span::Span;
use crate::syntax::span::Spanned;
use crate::syntax::token::{ContextualKeyword, Token};

use super::super::{
    Expected, Found, InvalidNumberReason, ParseError, ParseErrorKind, Parser,
    UnsupportedMultiDeclShape,
};

/// A parsed slot header: `[pub|pub(bind)] [const] (param|node) IDENT: TypeExpr`.
#[derive(Debug, Clone)]
pub(super) struct SlotHeader {
    pub kind: SlotKind,
    /// Span covering the kind keyword(s).
    pub kind_span: Span,
    pub name: Spanned<DeclName>,
    pub type_ann: TypeExpr,
    /// Span from kind keyword through end of type annotation.
    pub header_span: Span,
}

impl SlotHeader {
    fn into_slot(self, axis: MultiSlotAxis) -> MultiDeclSlot {
        MultiDeclSlot {
            kind: self.kind,
            name: self.name,
            type_ann: self.type_ann,
            axis,
            header_span: self.header_span,
        }
    }
}

impl Parser<'_> {
    /// Parse a `param` / `node` / `const node` kind keyword sequence and
    /// combine it with the already-parsed visibility prefix, returning the
    /// slot kind and the keywords' span.
    ///
    /// `param` declares a named input port and rejects any visibility
    /// annotation; `node` / `const node` are computed values and reject
    /// `pub(bind)`.
    pub(super) fn parse_slot_kind(
        &mut self,
        visibility: BindableVisibility,
        visibility_span: Option<Span>,
    ) -> Result<(SlotKind, Span), ParseError> {
        match self.lexer.peek() {
            Some(Token::Param) => {
                let (_, span) = self.advance()?;
                Self::reject_param_visibility(visibility, visibility_span)?;
                Ok((SlotKind::Param, span))
            }
            Some(Token::Node) => {
                let (_, span) = self.advance()?;
                let visibility = Self::node_visibility(visibility, visibility_span)?;
                Ok((SlotKind::Node(visibility), span))
            }
            Some(Token::Const) => {
                let (_, const_span) = self.advance()?;
                let (_, node_span) = self.expect(Token::Node)?;
                let visibility = Self::node_visibility(visibility, visibility_span)?;
                Ok((SlotKind::ConstNode(visibility), const_span.merge(node_span)))
            }
            Some(_) | None => Err(self.unexpected_next(Expected::MultiDeclSlotKind)),
        }
    }

    fn reject_param_visibility(
        visibility: BindableVisibility,
        visibility_span: Option<Span>,
    ) -> Result<(), ParseError> {
        let found = match visibility {
            BindableVisibility::Private => return Ok(()),
            BindableVisibility::Public => Found::Pub,
            BindableVisibility::PublicBind => Found::PubBind,
        };
        visibility_span.map_or(Ok(()), |vis_span| {
            Err(Self::unexpected_token(
                Expected::ParamWithoutVisibility,
                found,
                vis_span,
            ))
        })
    }

    const fn node_visibility(
        visibility: BindableVisibility,
        visibility_span: Option<Span>,
    ) -> Result<Visibility, ParseError> {
        match (visibility, visibility_span) {
            (BindableVisibility::PublicBind, Some(vis_span)) => Err(Self::unexpected_token(
                Expected::NodeVisibility,
                Found::PubBind,
                vis_span,
            )),
            (visibility, _) => Ok(super::visibility_without_bindability(visibility)),
        }
    }

    /// Parse the tail of a slot header: `IDENT : TypeExpr` given that the
    /// visibility prefix and kind keyword(s) have already been consumed.
    pub(super) fn parse_slot_header_tail(
        &mut self,
        kind: SlotKind,
        kind_span: Span,
    ) -> Result<SlotHeader, ParseError> {
        let name: Spanned<DeclName> = self.parse_any_ident()?.classify();
        self.expect(Token::Colon)?;
        let type_ann = self.parse_type_expr()?;
        let header_span = kind_span.merge(type_ann.span);
        Ok(SlotHeader {
            kind,
            kind_span,
            name,
            type_ann,
            header_span,
        })
    }

    /// Parse `, [pub|pub(bind)] (param|node|const node) IDENT : TypeExpr`.
    fn parse_next_slot_header(&mut self) -> Result<SlotHeader, ParseError> {
        self.expect(Token::Comma)?;
        let (visibility, visibility_span) = self.parse_visibility_prefix()?;
        let (kind, kind_span) = self.parse_slot_kind(visibility, visibility_span)?;
        self.parse_slot_header_tail(kind, kind_span)
    }

    /// Parse the remainder of a multi-decl given the first slot header, the
    /// leading `,` already peeked but not consumed. The first slot's
    /// visibility prefix was consumed (and checked against its kind) by
    /// `parse_declaration`. Returns a single `Declaration` wrapping a
    /// [`MultiDecl`](crate::syntax::ast::MultiDecl); the desugar pass expands it into N flat
    /// declarations.
    pub(super) fn parse_multi_decl_rest(
        &mut self,
        first_slot: SlotHeader,
    ) -> Result<Declaration, ParseError> {
        // Full multi-decl surface span starts at the first slot's kind keyword.
        let first_kind_span = first_slot.kind_span;
        let second_slot = self.parse_next_slot_header()?;
        let mut slots = AtLeastTwo::new(first_slot, second_slot);
        while self.lexer.peek() == Some(&Token::Comma) {
            slots.push(self.parse_next_slot_header()?);
        }

        self.expect(Token::Eq)?;

        // Parse the multi-table expression.
        let (_, table_span) = self.expect(Token::Table)?;
        self.expect(Token::LBracket)?;

        // Shared axes: one or more, then a comma and the trailing tuple of slot axes.
        let mut shared_axes: Vec<TableIndexSpec> = Vec::new();
        if self.lexer.peek() != Some(&Token::LParen) {
            loop {
                shared_axes.push(self.parse_table_index_spec_for_multi()?);
                self.expect(Token::Comma)?;
                if self.lexer.peek() == Some(&Token::LParen) {
                    break;
                }
            }
        }

        let (slot_axes, tuple_span) = self.parse_slot_tuple()?;
        let slot_count = slots.len();
        let slots = match slots.zip_exact(slot_axes) {
            Ok(slots) => slots.map(|(header, axis)| header.into_slot(axis)),
            Err(slot_axes) => {
                return Err(ParseError::new(
                    ParseErrorKind::MultiDeclTupleArity {
                        slot_count,
                        tuple_count: slot_axes.len(),
                    },
                    tuple_span,
                ));
            }
        };

        let (_, rbracket_span) = self.expect(Token::RBracket)?;

        let Ok(shared_axes) = MultiDeclSharedAxes::try_from_vec(shared_axes) else {
            return Err(ParseError::new(
                ParseErrorKind::MultiDeclNoSharedAxis,
                table_span.merge(rbracket_span),
            ));
        };

        // v2: at most one extra-axis slot. This covers the mixed 1-D / 2-D
        // motivating example; v3 relaxes to multiple extra-axis slots, with
        // grouping disambiguated by axis lookup.
        if let Some(second_extra_span) = slots
            .iter()
            .filter_map(|slot| match &slot.axis {
                MultiSlotAxis::Axis(spanned) => Some(spanned.span),
                MultiSlotAxis::Underscore => None,
            })
            .nth(1)
        {
            return Err(unsupported_shape(
                UnsupportedMultiDeclShape::MultipleExtraAxisSlots,
                second_extra_span,
            ));
        }

        // Parse the table body. Supports one shared axis (single body,
        // `{ header; rows }`) or more (slice sections, `{ [slice] header; rows …}`).
        self.expect(Token::LBrace)?;

        let slice_axis_specs = shared_axes.slice_axes().to_vec();
        let row_axis_spec = shared_axes.row_axis().clone();
        let mut builder = MultiDeclBuilder::new(slots, shared_axes);

        if slice_axis_specs.is_empty() {
            // v1/v2 shape: one body with no slice labels.
            self.parse_multi_slice_body(&mut builder, Vec::new(), &row_axis_spec)?;
        } else {
            // v3 shape: one or more `[slice_labels] header; rows;` sections.
            while self.lexer.peek() == Some(&Token::LBracket) {
                self.lexer.next_token(); // consume `[`
                let slice_prefix = self.parse_slice_labels(&slice_axis_specs)?;
                self.expect(Token::RBracket)?;
                self.parse_multi_slice_body(&mut builder, slice_prefix, &row_axis_spec)?;
            }
            if builder.slice_count() == 0 {
                return Err(unsupported_shape(
                    UnsupportedMultiDeclShape::MissingSliceSection,
                    self.lexer.peek_with_span().map_or(table_span, |(_, s)| s),
                ));
            }
        }

        let (_, rbrace_span) = self.expect(Token::RBrace)?;
        let (_, semi_span) = self.expect(Token::Semicolon)?;

        // Full multi-decl surface span: from the first slot's kind keyword
        // through the closing `;`.
        let surface_span = first_kind_span.merge(semi_span);
        let multi = builder.finish(surface_span, table_span.merge(rbrace_span));

        Ok(Declaration {
            doc: None,
            attributes: vec![],
            kind: DeclKind::Sugar(crate::syntax::ast::RawDeclSugar::Multi(multi)),
            span: surface_span,
        })
    }

    /// Parse the slice-section prefix `[A.a1, B.b1, …]` for multi-decls
    /// with more than one shared axis. The labels cover every shared axis
    /// except the last (the row axis), in declared order.
    fn parse_slice_labels(
        &mut self,
        slice_axis_specs: &[TableIndexSpec],
    ) -> Result<Vec<MapEntryKey>, ParseError> {
        let mut keys: Vec<MapEntryKey> = Vec::with_capacity(slice_axis_specs.len());
        for (idx, axis_spec) in slice_axis_specs.iter().enumerate() {
            if idx > 0 {
                self.expect(Token::Comma)?;
            }
            match axis_spec {
                TableIndexSpec::Named(axis) => {
                    let (label_axis, variant, _) = self.parse_index_variant_path()?;
                    if label_axis.value != axis.value {
                        return Err(unsupported_shape(
                            UnsupportedMultiDeclShape::SliceLabelAxis {
                                label_axis: label_axis.value,
                                shared_axis: axis.value.clone(),
                            },
                            label_axis.span,
                        ));
                    }
                    keys.push(MapEntryKey::Named {
                        index: label_axis,
                        additional_index_spans: vec![axis.span],
                        variant,
                    });
                }
                TableIndexSpec::Finite { cardinality, span } => {
                    let (_, hash_span) = self.expect(Token::Hash)?;
                    let (_, num_span) = self.expect(Token::Number)?;
                    let text = self.lexer.slice_at(num_span).replace('_', "");
                    let value: u64 = text.parse().map_err(|_| {
                        Self::invalid_number(InvalidNumberReason::SliceLabel, num_span)
                    })?;
                    let position = Self::slice_position(*cardinality, value, num_span)?;
                    keys.push(MapEntryKey::Finite {
                        axis_span: *span,
                        position: Spanned::new(position, hash_span.merge(num_span)),
                    });
                }
            }
        }
        Ok(keys)
    }

    /// Parse one header + data rows block for a single slice of a multi-decl
    /// and add it to `builder`.
    ///
    /// For v1/v2 (single shared axis), `prefix_keys` is empty. For v3
    /// (multi-shared-axis), `prefix_keys` carries the slice labels.
    fn parse_multi_slice_body(
        &mut self,
        builder: &mut MultiDeclBuilder,
        prefix_keys: Vec<MapEntryKey>,
        row_axis: &TableIndexSpec,
    ) -> Result<(), ParseError> {
        let (header_cells, header_span) = self.parse_multi_header_row()?;
        let mut slice = builder
            .begin_slice(prefix_keys, header_cells)
            .map_err(|error| multi_decl_layout_error(error, header_span))?;

        // Rows past a `Fin(N)` row axis have no key and are not stored, so the
        // row-count diagnostic counts every parsed row here.
        let mut rows_seen: usize = 0;
        while self.lexer.peek() != Some(&Token::RBrace)
            && self.lexer.peek() != Some(&Token::LBracket)
        {
            let (row_key, row_label, label_span) = match row_axis {
                TableIndexSpec::Named(axis) => {
                    let label = self.parse_any_ident()?;
                    let label_span = label.span;
                    let named_label: Spanned<IndexVariantName> = label.classify();
                    self.expect(Token::Colon)?;
                    let row_label = IndexEntryKey::named(named_label.value.clone());
                    (
                        Some(MapEntryKey::named(axis.clone(), named_label)),
                        row_label,
                        label_span,
                    )
                }
                TableIndexSpec::Finite { cardinality, span } => {
                    let label_span = self
                        .lexer
                        .peek_with_span()
                        .map_or(header_span, |(_, span)| span);
                    let position = u64::try_from(rows_seen).map_err(|_| {
                        Self::invalid_number(InvalidNumberReason::FiniteRowPosition, label_span)
                    })?;
                    // A row past `Fin(cardinality)` has no key; it is still
                    // parsed and reported by the row-count check below.
                    let row_key =
                        FinPosition::try_new(*cardinality, position)
                            .ok()
                            .map(|position| MapEntryKey::Finite {
                                axis_span: *span,
                                position: Spanned::new(position, label_span),
                            });
                    (row_key, IndexEntryKey::position(position), label_span)
                }
            };
            let mut values = Vec::with_capacity(slice.header_count());
            loop {
                let value = self.parse_expr()?;
                values.push(value);
                if self.lexer.peek() == Some(&Token::Comma) {
                    self.lexer.next_token();
                } else {
                    break;
                }
            }
            let row_end_span = self.lexer.peek_with_span().map_or(label_span, |(_, s)| s);
            let row_span = label_span.merge(row_end_span);
            self.expect(Token::Semicolon)?;

            rows_seen += 1;
            let row_width_error = |error: MultiDeclRowWidthError| {
                ParseError::new(
                    ParseErrorKind::MultiDeclRowArity {
                        expected_count: error.header_count,
                        got: error.value_count,
                        row_label,
                    },
                    row_span,
                )
            };
            match row_key {
                Some(row_key) => slice.push_row(row_key, values).map_err(row_width_error)?,
                None if values.len() != slice.header_count() => {
                    return Err(row_width_error(MultiDeclRowWidthError {
                        header_count: slice.header_count(),
                        value_count: values.len(),
                    }));
                }
                None => {}
            }
        }

        if let TableIndexSpec::Finite { cardinality, .. } = row_axis
            && u64::try_from(rows_seen) != Ok(*cardinality)
        {
            return Err(ParseError::new(
                ParseErrorKind::TableRowLengthMismatch {
                    expected: *cardinality,
                    got: Self::table_count_from_len(rows_seen, header_span)?,
                },
                header_span,
            ));
        }

        slice.finish();
        Ok(())
    }

    /// Parse a table index spec inside a multi-decl's shared-axis prefix.
    ///
    /// Same shape as the single-decl `parse_table_index_spec`, but split out
    /// so the multi-decl parser can stop at the opening paren of the slot tuple
    /// without also advancing past a comma. Named axes retain their full path.
    fn parse_table_index_spec_for_multi(&mut self) -> Result<TableIndexSpec, ParseError> {
        self.reject_obsolete_structural_range()?;
        if self.lexer.peek() == Some(&Token::ContextualKeyword(ContextualKeyword::Fin))
            && self.lexer.peek_second() == Some(&Token::LParen)
        {
            let (_, start_span) = self.advance()?;
            self.expect(Token::LParen)?;
            let (_, cardinality_span) = self.expect(Token::Number)?;
            let text = self.lexer.slice_at(cardinality_span).replace('_', "");
            let cardinality = text.parse().map_err(|_| {
                Self::invalid_number(InvalidNumberReason::TableFinCardinality, cardinality_span)
            })?;
            let (_, end_span) = self.expect(Token::RParen)?;
            return Ok(TableIndexSpec::Finite {
                cardinality,
                span: start_span.merge(end_span),
            });
        }
        match self.lexer.peek() {
            Some(Token::Number) => {
                let (_, span) = self.advance()?;
                let expression = self.lexer.slice_at(span).to_string();
                Err(ParseError::new(
                    ParseErrorKind::ExpectedIndexFoundNat { expression },
                    span,
                ))
            }
            Some(token) if token.is_identifier() => Ok(TableIndexSpec::Named(
                self.parse_ident_path()?.into_spanned_name_path(),
            )),
            _ => {
                let (tok, span) = self.advance()?;
                Err(Self::unexpected(Expected::TableAxis, tok, span))
            }
        }
    }

    /// Parse a slot tuple: `( slot_axes { , slot_axes } [,] )`.
    ///
    /// Each entry is either `_` (no extra axis) or an identifier path naming
    /// the slot's extra axis. finite-index extras are not supported in v1.
    fn parse_slot_tuple(&mut self) -> Result<(Vec<MultiSlotAxis>, Span), ParseError> {
        let (_, lparen_span) = self.expect(Token::LParen)?;
        let mut entries = Vec::new();
        loop {
            if self.lexer.peek() == Some(&Token::RParen) {
                break;
            }
            entries.push(self.parse_slot_axis_entry()?);
            match self.lexer.peek() {
                Some(Token::Comma) => {
                    self.lexer.next_token();
                }
                _ => break,
            }
        }
        let (_, rparen_span) = self.expect(Token::RParen)?;
        Ok((entries, lparen_span.merge(rparen_span)))
    }

    fn parse_slot_axis_entry(&mut self) -> Result<MultiSlotAxis, ParseError> {
        match self.lexer.peek() {
            Some(Token::Underscore) => {
                self.advance()?;
                Ok(MultiSlotAxis::Underscore)
            }
            Some(token) if token.is_identifier() => Ok(MultiSlotAxis::Axis(
                self.parse_ident_path()?.into_spanned_name_path(),
            )),
            _ => {
                let (tok, span) = self.advance()?;
                Err(Self::unexpected(Expected::SlotTupleEntry, tok, span))
            }
        }
    }

    /// Parse the multi-decl header row: `: header_cell { , header_cell } ;`.
    fn parse_multi_header_row(&mut self) -> Result<(Vec<MultiHeaderCell>, Span), ParseError> {
        let (_, colon_span) = self.expect(Token::Colon)?;
        let mut cells = Vec::new();
        loop {
            cells.push(self.parse_header_cell()?);
            match self.lexer.peek() {
                Some(Token::Comma) => {
                    self.lexer.next_token();
                }
                _ => break,
            }
        }
        let (_, semi_span) = self.expect(Token::Semicolon)?;
        Ok((cells, colon_span.merge(semi_span)))
    }

    fn parse_header_cell(&mut self) -> Result<MultiHeaderCell, ParseError> {
        match self.lexer.peek() {
            Some(Token::Underscore) => {
                let (_, span) = self.advance()?;
                Ok(MultiHeaderCell::Underscore { span })
            }
            Some(token) if token.is_identifier() => {
                let variant: Spanned<IndexVariantName> = self.parse_any_ident()?.classify();
                Ok(MultiHeaderCell::Variant {
                    span: variant.span,
                    variant,
                })
            }
            _ => {
                let (tok, span) = self.advance()?;
                Err(Self::unexpected(Expected::HeaderCell, tok, span))
            }
        }
    }
}

const fn unsupported_shape(shape: UnsupportedMultiDeclShape, span: Span) -> ParseError {
    ParseError::new(ParseErrorKind::MultiDeclUnsupportedShape { shape }, span)
}

fn multi_decl_layout_error(error: MultiDeclLayoutError, header_span: Span) -> ParseError {
    match error {
        MultiDeclLayoutError::VariantCellForScalarSlot { span, slot_name } => unsupported_shape(
            UnsupportedMultiDeclShape::UnderscoreHeaderRequired { slot: slot_name },
            span,
        ),
        MultiDeclLayoutError::HeaderArity {
            slot_count,
            header_count,
        } => ParseError::new(
            ParseErrorKind::MultiDeclHeaderArity {
                slot_count,
                header_count,
            },
            header_span,
        ),
        MultiDeclLayoutError::NotEnoughCells { slot_name, span } => unsupported_shape(
            UnsupportedMultiDeclShape::MissingVariantCells { slot: slot_name },
            span,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::ast;
    use crate::syntax::ast::{SlotKind, Visibility};
    use crate::syntax::parser::Parser;

    /// Extract the `MultiDecl` from a file that contains exactly one
    /// multi-decl surface form.
    fn sole_multi_decl(file: &ast::File) -> &ast::MultiDecl {
        file.declarations
            .iter()
            .find_map(|d| match &d.kind {
                DeclKind::Sugar(crate::syntax::ast::RawDeclSugar::Multi(m)) => Some(m),
                _ => None,
            })
            .expect("file has one multi-decl")
    }

    #[test]
    fn multi_decl_supports_finite_shared_row_axis() {
        let source = r"
param x: Dimensionless[Fin(2)],
param y: Dimensionless[Fin(2)]
  = table[Fin(2), (_, _)] {
      : _, _;
      1.0, 2.0;
      3.0, 4.0;
  };
";
        let file = Parser::new(source).parse_file().unwrap();
        let multi = sole_multi_decl(&file);
        assert!(multi.shared_axes().row_axis().is_finite_index());
        assert_eq!(
            multi.slices()[0]
                .rows()
                .iter()
                .map(|row| row.row_key().entry_key())
                .collect::<Vec<_>>(),
            vec![IndexEntryKey::Position(0), IndexEntryKey::Position(1)]
        );
    }

    #[test]
    fn huge_finite_multi_row_cardinality_preserves_u64_diagnostic_count() {
        let source = r"
param x: Dimensionless[Fin(18446744073709551615)],
param y: Dimensionless[Fin(18446744073709551615)]
  = table[Fin(18446744073709551615), (_, _)] {
      : _, _;
      1.0, 2.0;
  };
";
        let error = Parser::new(source).parse_file().unwrap_err();
        match error.kind {
            ParseErrorKind::TableRowLengthMismatch { expected, got } => {
                let expected: u64 = expected;
                let got: u64 = got;
                assert_eq!(expected, u64::MAX);
                assert_eq!(got, 1);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn multi_decl_homogeneous_1d() {
        let source = r"
index Component = { ComponentA, ComponentB };

param power_consumption: Power[Component],
param n_installed:       Int[Component]
  = table[Component, (_, _)] {
      :           _,       _;
      ComponentA: 10.0 W,  1;
      ComponentB: 12.0 W,  2;
  };
";
        let file = Parser::new(source).parse_file().unwrap();
        // Parser emits: [index, multi-decl] — 2 top-level declarations.
        assert_eq!(file.declarations.len(), 2);

        let multi = sole_multi_decl(&file);
        assert_eq!(multi.slots().len(), 2);
        assert_eq!(multi.slots()[0].name.value.as_str(), "power_consumption");
        assert_eq!(multi.slots()[1].name.value.as_str(), "n_installed");
        assert_eq!(multi.slots().len(), 2);
        assert!(matches!(
            multi.slots()[0].axis,
            ast::MultiSlotAxis::Underscore
        ));
        assert!(matches!(
            multi.slots()[1].axis,
            ast::MultiSlotAxis::Underscore
        ));
        assert_eq!(multi.slices().len(), 1);
        assert_eq!(multi.slices()[0].rows().len(), 2);
    }

    #[test]
    fn multi_decl_mixed_kinds_param_node_const_node() {
        let source = r"
index Component = { ComponentA, ComponentB };

param      power_consumption: Power[Component],
node       installed_mass:    Mass[Component],
const node mass_per_unit:     Mass[Component]
  = table[Component, (_, _, _)] {
      :           _,       _,      _;
      ComponentA: 10.0 W,  2.5 kg, 1.2 kg;
      ComponentB: 12.0 W,  3.1 kg, 1.5 kg;
  };
";
        let file = Parser::new(source).parse_file().unwrap();
        let multi = sole_multi_decl(&file);
        assert_eq!(multi.slots().len(), 3);
        assert_eq!(multi.slots()[0].kind, SlotKind::Param);
        assert_eq!(multi.slots()[1].kind, SlotKind::Node(Visibility::Private));
        assert_eq!(
            multi.slots()[2].kind,
            SlotKind::ConstNode(Visibility::Private)
        );
    }

    #[test]
    fn multi_decl_requires_comma_before_slot_tuple() {
        let source = r"
param a: Int[I], param b: Int[I]
  = table[I (_, _)] {
      : _, _;
      X: 1, 2;
  };
";
        let err = Parser::new(source).parse_file().unwrap_err();
        assert!(
            matches!(
                err.kind,
                ParseErrorKind::UnexpectedToken {
                    expected: Expected::Token(Token::Comma),
                    found: Found::Token(Token::LParen),
                }
            ),
            "expected a missing-comma diagnostic, got {err:?}"
        );

        let valid_source = source.replace("table[I (", "table[I, (");
        Parser::new(&valid_source).parse_file().unwrap();
    }

    #[test]
    fn multi_decl_tuple_arity_mismatch() {
        let source = r"
param a: Int[Component], param b: Int[Component]
  = table[Component, (_,)] {
      : _, _;
      X: 1, 2;
  };
";
        let err = Parser::new(source).parse_file().unwrap_err();
        assert!(
            matches!(
                err.kind,
                ParseErrorKind::MultiDeclTupleArity {
                    slot_count: 2,
                    tuple_count: 1,
                    ..
                }
            ),
            "expected MultiDeclTupleArity, got {err:?}",
        );
    }

    #[test]
    fn multi_decl_row_arity_mismatch_names_slot() {
        let source = r"
param a: Int[Component], param b: Int[Component]
  = table[Component, (_, _)] {
      : _, _;
      X: 1;
  };
";
        let err = Parser::new(source).parse_file().unwrap_err();
        match err.kind {
            ParseErrorKind::MultiDeclRowArity {
                expected_count,
                got,
                row_label,
            } => {
                assert_eq!(expected_count, 2);
                assert_eq!(got, 1);
                assert_eq!(row_label.to_string(), "X");
            }
            other => panic!("expected MultiDeclRowArity, got {other:?}"),
        }
    }

    #[test]
    fn multi_decl_rejects_attributes() {
        let source = r"
#[hidden]
param a: Int[Component], param b: Int[Component]
  = table[Component, (_, _)] {
      : _, _;
      X: 1, 2;
  };
";
        let err = Parser::new(source).parse_file().unwrap_err();
        assert!(
            matches!(err.kind, ParseErrorKind::UnexpectedToken { .. }),
            "expected UnexpectedToken (attributes forbidden), got {err:?}",
        );
    }

    #[test]
    fn multi_decl_first_slot_pub_param_still_rejected() {
        // `pub` on `param` is invalid per-slot — params are implicitly
        // visible+bindable and never carry an annotation. The leading `pub`
        // here applies to the first slot, so the error fires with that slot's
        // kind even though the multi-decl shape isn't recognized until the
        // first comma.
        let source = r"
pub param a: Int[Component], param b: Int[Component]
  = table[Component, (_, _)] {
      : _, _;
      X: 1, 2;
  };
";
        let err = Parser::new(source).parse_file().unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::UnexpectedToken { .. }));
    }

    #[test]
    fn multi_decl_first_slot_pub_node_accepted() {
        // The leading `pub` becomes the first slot's visibility; the second
        // slot stays private.
        let source = r"
index Component = { ComponentA, ComponentB };

pub node a: Int[Component], node b: Int[Component]
  = table[Component, (_, _)] {
      :           _, _;
      ComponentA: 1, 2;
      ComponentB: 3, 4;
  };
";
        let file = Parser::new(source).parse_file().unwrap();
        let multi = sole_multi_decl(&file);
        assert_eq!(multi.slots()[0].kind, SlotKind::Node(Visibility::Public));
        assert_eq!(multi.slots()[1].kind, SlotKind::Node(Visibility::Private));
    }

    #[test]
    fn multi_decl_per_slot_visibility_mixed() {
        // First private, second pub via per-slot prefix after the comma.
        let source = r"
index Component = { ComponentA, ComponentB };

node a: Int[Component], pub node b: Int[Component]
  = table[Component, (_, _)] {
      :           _, _;
      ComponentA: 1, 2;
      ComponentB: 3, 4;
  };
";
        let file = Parser::new(source).parse_file().unwrap();
        let multi = sole_multi_decl(&file);
        assert_eq!(multi.slots()[0].kind, SlotKind::Node(Visibility::Private));
        assert_eq!(multi.slots()[1].kind, SlotKind::Node(Visibility::Public));
    }

    #[test]
    fn multi_decl_per_slot_pub_param_still_rejected() {
        // The per-slot rule fires for non-first slots too.
        let source = r"
index Component = { ComponentA, ComponentB };

node a: Int[Component], pub param b: Int[Component]
  = table[Component, (_, _)] {
      :           _, _;
      ComponentA: 1, 2;
      ComponentB: 3, 4;
  };
";
        let err = Parser::new(source).parse_file().unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::UnexpectedToken { .. }));
    }

    #[test]
    fn multi_decl_per_slot_pub_bind_node_rejected() {
        // pub(bind) is meaningless on node / const node — same per-slot.
        let source = r"
index Component = { ComponentA, ComponentB };

node a: Int[Component], pub(bind) node b: Int[Component]
  = table[Component, (_, _)] {
      :           _, _;
      ComponentA: 1, 2;
      ComponentB: 3, 4;
  };
";
        let err = Parser::new(source).parse_file().unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::UnexpectedToken { .. }));
    }

    #[test]
    fn multi_decl_v2_heterogeneous_accepted() {
        // v2: one extra-axis slot alongside multiple 1-D slots.
        let source = r"
index Component = { ComponentA, ComponentB };
index OperationMode = { Safe, Nominal };

param      power_consumption: Power[Component],
param      n_installed:       Int[Component],
const node mass_per_unit:     Mass[Component],
param      power_mode:        Bool[Component, OperationMode]
  = table[Component, (_, _, _, OperationMode)] {
      :            _,       _, _,      Safe,  Nominal;
      ComponentA:  10.0 W,  1, 2.5 kg,                true,                   true;
      ComponentB:  12.0 W,  2, 3.1 kg,               false,                   true;
  };
";
        let file = Parser::new(source).parse_file().unwrap();
        let multi = sole_multi_decl(&file);
        assert_eq!(multi.slots().len(), 4);
        assert!(matches!(multi.slots()[3].axis, ast::MultiSlotAxis::Axis(_)));
    }

    #[test]
    fn multi_decl_v3_two_extra_axis_slots_rejected() {
        // v2 supports at most one extra-axis slot; two adjacent extra-axis
        // slots are v3 territory and must be rejected with a clear error.
        let source = r"
param a: Bool[Component, OperationMode],
param b: Bool[Component, OperationMode]
  = table[Component, (OperationMode, OperationMode)] {
      :           Safe, Nominal, Safe, Nominal;
      ComponentA:               true,                 false,               false,                  true;
  };
";
        let err = Parser::new(source).parse_file().unwrap_err();
        assert!(
            matches!(err.kind, ParseErrorKind::MultiDeclUnsupportedShape { .. }),
            "expected MultiDeclUnsupportedShape for two extra-axis slots, got {err:?}",
        );
    }

    #[test]
    fn multi_decl_v3_sliced_shared_axes() {
        let source = r"
index Phase = { Launch, Cruise };
index Component = { ComponentA };

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
";
        let file = Parser::new(source).parse_file().unwrap();
        let multi = sole_multi_decl(&file);
        assert_eq!(multi.shared_axes().len(), 2);
        assert_eq!(multi.slices().len(), 2);
        assert_eq!(multi.slices()[0].prefix_keys().len(), 1);
        assert_eq!(multi.slices()[0].prefix_keys()[0].axis_text(), "Phase");
    }

    #[test]
    fn multi_decl_v3_slice_axis_mismatch() {
        let source = r"
param p: Int[Phase, Component],
param q: Int[Phase, Component]
  = table[Phase, Component, (_, _)] {
      [Foo#Launch]
      :           _, _;
      ComponentA: 1, 2;
  };
";
        let err = Parser::new(source).parse_file().unwrap_err();
        assert!(
            matches!(err.kind, ParseErrorKind::MultiDeclUnsupportedShape { .. }),
            "expected MultiDeclUnsupportedShape for wrong slice axis, got {err:?}",
        );
    }

    #[test]
    fn multi_decl_v2_contextual_bare_header_cells_accepted() {
        // The slot's declared extra axis determines each bare label owner.
        let source = r"
index Component = { ComponentA };
index OpMode = { Safe, Nominal };

param p: Power[Component],
param m: Bool[Component, OpMode]
  = table[Component, (_, OpMode)] {
      :           _,      Safe, Nominal;
      ComponentA: 10.0 W, true,         false;
  };
";
        let file = Parser::new(source).parse_file().unwrap();
        // 2 index decls + 1 multi-decl.
        assert_eq!(file.declarations.len(), 3);
        let multi = sole_multi_decl(&file);
        assert_eq!(multi.slots().len(), 2);
    }

    #[test]
    fn multi_decl_v2_qualified_header_cells_rejected() {
        let source = r"
param p: Power[Component],
param m: Bool[Component, OpMode]
  = table[Component, (_, OpMode)] {
      :           _,      OpMode#Safe, OpMode#Nominal;
      ComponentA: 10.0 W, true, false;
  };
";
        assert!(Parser::new(source).parse_file().is_err());
    }

    #[test]
    fn multi_decl_preserves_qualified_axis_and_label_paths() {
        let source = r"
param p: Int[mission::Phase, mission::Component],
param m: Bool[mission::Phase, mission::Component, mission::Mode]
  = table[mission::Phase, mission::Component, (_, mission::Mode)] {
      [mission::Phase#Launch]
      :           _, Safe;
      ComponentA: 1, true;
  };
";
        let file = Parser::new(source).parse_file().unwrap();
        let multi = sole_multi_decl(&file);

        let TableIndexSpec::Named(slice_axis) = &multi.shared_axes()[0] else {
            panic!("expected named slice axis")
        };
        assert_eq!(slice_axis.value.display_path(), "mission::Phase");
        let TableIndexSpec::Named(row_axis) = &multi.shared_axes()[1] else {
            panic!("expected named row axis")
        };
        assert_eq!(row_axis.value.display_path(), "mission::Component");
        let ast::MultiSlotAxis::Axis(slot_axis) = &multi.slots()[1].axis else {
            panic!("expected named slot axis")
        };
        assert_eq!(slot_axis.value.display_path(), "mission::Mode");
        let ast::MultiHeaderCell::Variant { variant, .. } = &multi.slices()[0].header_cells()[1]
        else {
            panic!("expected header variant")
        };
        assert_eq!(variant.value.as_str(), "Safe");
        let ast::MultiSlotColumnSpan::Range { extra_axis, .. } =
            &multi.slices()[0].column_layout()[1]
        else {
            panic!("expected the variant column to belong to the extra-axis slot")
        };
        assert_eq!(extra_axis.value.display_path(), "mission::Mode");
        assert_eq!(
            multi.slices()[0].prefix_keys()[0].axis_text(),
            "mission::Phase"
        );
    }
}
