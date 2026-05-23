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
//! v1 supports homogeneous 1-D slots only — every slot is typed
//! `T[SharedAxis]`, every tuple entry is `_`, every header cell is `_`.
//! Later versions relax these restrictions (see the design doc at
//! `.local/2026-04-23_issue-481-dataframe-table-literal-proposals.md`).
//!
//! Multi-decls are **pure syntactic sugar**: this parser desugars them
//! into N separate [`Declaration`] values, each carrying its own
//! synthesized `TableLiteral` initializer. Downstream compiler passes
//! see N ordinary declarations.

use crate::syntax::ast::{
    self as ast, DeclKind, Declaration, Expr, MapEntryIndex, MapEntryKey, TableIndexSpec, TypeExpr,
    Visibility,
};
use crate::syntax::names::{DeclName, IndexName, IndexVariantName};
use crate::syntax::span::Span;
use crate::syntax::span::Spanned;
use crate::syntax::token::Token;

use super::super::{ParseError, Parser};

/// Kind of a value-decl slot: `param`, `node`, or `const node`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SlotKind {
    Param,
    Node,
    ConstNode,
}

/// A parsed slot header: `[pub|pub(bind)] [const] (param|node) IDENT: TypeExpr`.
#[derive(Debug, Clone)]
pub(super) struct SlotHeader {
    pub visibility: Visibility,
    pub kind: SlotKind,
    /// Span covering the kind keyword(s).
    pub kind_span: Span,
    pub name: Spanned<DeclName>,
    pub type_ann: TypeExpr,
    /// Span from kind keyword through end of type annotation.
    pub header_span: Span,
}

impl Parser<'_> {
    /// Validate that `visibility` is legal on a value-decl slot of `kind`.
    /// Mirrors the per-decl checks at the top of `parse_declaration`:
    /// `param` rejects any visibility annotation; `node` / `const node`
    /// reject `pub(bind)`. Called once per multi-decl slot.
    pub(super) fn check_value_decl_visibility(
        &self,
        visibility: Visibility,
        visibility_span: Option<Span>,
        kind: SlotKind,
    ) -> Result<(), ParseError> {
        let Some(vis_span) = visibility_span else {
            return Ok(());
        };
        match (kind, visibility) {
            (SlotKind::Param, Visibility::Public) => Err(self.unexpected_token(
                "no visibility annotation (params are always visible and bindable)",
                "`pub`",
                vis_span,
            )),
            (SlotKind::Param, Visibility::PublicBind) => Err(self.unexpected_token(
                "no visibility annotation (params are always visible and bindable)",
                "`pub(bind)`",
                vis_span,
            )),
            (SlotKind::Node | SlotKind::ConstNode, Visibility::PublicBind) => {
                Err(self.unexpected_token(
                    "`pub` (nodes are computed values — `pub(bind)` is not meaningful; use `param` to declare a bindable input)",
                    "`pub(bind)`",
                    vis_span,
                ))
            }
            _ => Ok(()),
        }
    }

    /// Parse the tail of a slot header: `IDENT : TypeExpr` given that the
    /// kind keyword(s) have already been consumed and their span captured.
    /// Visibility is supplied by the caller (the leading `pub`/`pub(bind)`
    /// prefix is parsed before the kind keyword).
    pub(super) fn parse_slot_header_tail(
        &mut self,
        visibility: Visibility,
        kind: SlotKind,
        kind_span: Span,
    ) -> Result<SlotHeader, ParseError> {
        let name = self.parse_any_ident()?.into_spanned::<DeclName>();
        self.expect(Token::Colon)?;
        let type_ann = self.parse_type_expr()?;
        let header_span = kind_span.merge(type_ann.span);
        Ok(SlotHeader {
            visibility,
            kind,
            kind_span,
            name,
            type_ann,
            header_span,
        })
    }

    /// Parse the remainder of a multi-decl given the first slot header,
    /// the leading `,` already peeked but not consumed. The first slot's
    /// visibility was consumed by `parse_declaration` before the multi-decl
    /// was recognized, and is supplied via `first_visibility`/
    /// `first_visibility_span` so per-slot validation runs once for every
    /// slot. Returns a single `Declaration` wrapping a [`ast::MultiDecl`].
    /// Expansion to N flat declarations happens later in
    /// [`crate::syntax::desugar::desugar_multi_decls_in_file`].
    #[expect(
        clippy::too_many_lines,
        reason = "single cohesive routine for the multi-decl body parse"
    )]
    pub(super) fn parse_multi_decl_rest(
        &mut self,
        first_slot: SlotHeader,
        first_visibility: Visibility,
        first_visibility_span: Option<Span>,
    ) -> Result<Declaration, ParseError> {
        // Validate the first slot's visibility against its kind. The
        // single-decl path applies the same rules inline before reaching
        // here; for multi-decl we run them per-slot so a leading `pub`
        // followed by a comma still gets validated against slot 0's kind.
        self.check_value_decl_visibility(first_visibility, first_visibility_span, first_slot.kind)?;
        let mut slots: Vec<SlotHeader> = vec![first_slot];

        // Parse remaining slots: `, [pub|pub(bind)] (param|node|const node) IDENT : TypeExpr`.
        while self.lexer.peek() == Some(&Token::Comma) {
            self.lexer.next_token(); // consume ','
            let (visibility, visibility_span) = self.parse_visibility_prefix()?;
            let (kind, kind_span) = self.parse_slot_kind()?;
            self.check_value_decl_visibility(visibility, visibility_span, kind)?;
            let header = self.parse_slot_header_tail(visibility, kind, kind_span)?;
            slots.push(header);
        }

        if slots.len() < 2 {
            // Parser can't actually reach this branch because the caller only
            // invokes us after a comma, but guard against refactors.
            let span = slots[0].header_span;
            return Err(ParseError::MultiDeclSingleSlot {
                src: self.named_source(),
                span: span.into(),
            });
        }

        self.expect(Token::Eq)?;

        // Parse the multi-table expression.
        let (_, table_span) = self.expect(Token::Table)?;
        self.expect(Token::LBracket)?;

        // Shared axes: one or more, then a trailing tuple of slot axes.
        let mut shared_axes: Vec<TableIndexSpec> = Vec::new();
        loop {
            // Detect the tuple: next token is `(`.
            if self.lexer.peek() == Some(&Token::LParen) {
                break;
            }
            shared_axes.push(self.parse_table_index_spec_for_multi()?);
            match self.lexer.peek() {
                Some(Token::Comma) => {
                    self.lexer.next_token();
                }
                _ => break,
            }
        }

        let (slot_axes, tuple_span) = self.parse_slot_tuple()?;

        if slot_axes.len() != slots.len() {
            return Err(ParseError::MultiDeclTupleArity {
                slot_count: slots.len(),
                tuple_count: slot_axes.len(),
                src: self.named_source(),
                span: tuple_span.into(),
            });
        }

        let (_, rbracket_span) = self.expect(Token::RBracket)?;

        if shared_axes.is_empty() {
            return Err(ParseError::MultiDeclNoSharedAxis {
                src: self.named_source(),
                span: table_span.merge(rbracket_span).into(),
            });
        }

        // v2: at most one extra-axis slot. This covers the mixed 1-D / 2-D
        // motivating example; v3 relaxes to multiple extra-axis slots, with
        // grouping disambiguated by axis lookup.
        let extra_axis_slot_count = slot_axes
            .iter()
            .filter(|a| matches!(a, SlotAxis::Axis(_)))
            .count();
        if extra_axis_slot_count > 1 {
            let second_extra_span = slot_axes
                .iter()
                .filter_map(|a| match a {
                    SlotAxis::Axis(spanned) => Some(spanned.span),
                    SlotAxis::Underscore => None,
                })
                .nth(1)
                .unwrap_or(tuple_span);
            return Err(ParseError::MultiDeclUnsupportedShape {
                reason: "multi-decl with more than one extra-axis slot is not yet supported (v3)"
                    .to_string(),
                src: self.named_source(),
                span: second_extra_span.into(),
            });
        }

        // Parse the table body. Supports one shared axis (single body,
        // `{ header; rows }`) or more (slice sections, `{ [slice] header; rows …}`).
        self.expect(Token::LBrace)?;

        let row_index_spec = shared_axes[shared_axes.len() - 1].clone();
        let slice_axis_specs: Vec<TableIndexSpec> = shared_axes[..shared_axes.len() - 1].to_vec();

        // Collect: per slice, (slice_prefix_keys, header_cells, row_values).
        // For single-shared-axis multi-decls, there is exactly one slice with
        // empty prefix keys.
        let mut slices: Vec<MultiSlice> = Vec::new();

        if slice_axis_specs.is_empty() {
            // v1/v2 shape: one body with no slice labels.
            let slice = self.parse_multi_slice_body(&[], &slot_axes, &slots)?;
            slices.push(slice);
        } else {
            // v3 shape: one or more `[slice_labels] header; rows;` sections.
            while self.lexer.peek() == Some(&Token::LBracket) {
                self.lexer.next_token(); // consume `[`
                let slice_prefix = self.parse_slice_labels(&slice_axis_specs)?;
                self.expect(Token::RBracket)?;
                let slice = self.parse_multi_slice_body(&slice_prefix, &slot_axes, &slots)?;
                slices.push(slice);
            }
            if slices.is_empty() {
                return Err(ParseError::MultiDeclUnsupportedShape {
                    reason:
                        "multi-decl with multiple shared axes requires at least one `[slice]` section"
                            .to_string(),
                    src: self.named_source(),
                    span: self
                        .lexer
                        .peek_with_span()
                        .map_or(table_span, |(_, s)| s)
                        .into(),
                });
            }
        }

        let (_, rbrace_span) = self.expect(Token::RBrace)?;
        let (_, semi_span) = self.expect(Token::Semicolon)?;

        let table_total_span = table_span.merge(rbrace_span);

        // Full multi-decl surface span: from the first slot's kind keyword
        // through the closing `;`.
        let surface_span = slots[0].kind_span.merge(semi_span);

        // Convert the parser's internal structured forms into AST types.
        let ast_slots: Vec<ast::MultiDeclSlot> = slots
            .iter()
            .map(|s| ast::MultiDeclSlot {
                visibility: s.visibility,
                kind: match s.kind {
                    SlotKind::Param => ast::MultiSlotKind::Param,
                    SlotKind::Node => ast::MultiSlotKind::Node,
                    SlotKind::ConstNode => ast::MultiSlotKind::ConstNode,
                },
                kind_span: s.kind_span,
                name: s.name.clone(),
                type_ann: s.type_ann.clone(),
                header_span: s.header_span,
            })
            .collect();

        let ast_slot_axes: Vec<ast::MultiSlotAxis> = slot_axes
            .iter()
            .map(|a| match a {
                SlotAxis::Underscore => ast::MultiSlotAxis::Underscore,
                SlotAxis::Axis(spanned) => ast::MultiSlotAxis::Axis(spanned.clone()),
            })
            .collect();

        let ast_slices: Vec<ast::MultiDeclSlice> = slices
            .iter()
            .map(|slice| ast::MultiDeclSlice {
                prefix_keys: slice.prefix_keys.clone(),
                header_cells: slice
                    .header_cells
                    .iter()
                    .map(|c| match c {
                        HeaderCell::Underscore(sp) => {
                            ast::MultiHeaderCell::Underscore { span: *sp }
                        }
                        HeaderCell::Variant {
                            axis,
                            variant,
                            span,
                        } => ast::MultiHeaderCell::Variant {
                            axis: axis.clone(),
                            variant: variant.clone(),
                            span: *span,
                        },
                    })
                    .collect(),
                header_span: slice.header_span,
                column_layout: slice
                    .column_layout
                    .iter()
                    .map(|span| match span {
                        SlotColumnSpan::Single(idx) => ast::MultiSlotColumnSpan::Single(*idx),
                        SlotColumnSpan::Range {
                            start,
                            end,
                            extra_axis,
                        } => ast::MultiSlotColumnSpan::Range {
                            start: *start,
                            end: *end,
                            extra_axis: extra_axis.clone(),
                        },
                    })
                    .collect(),
                rows: slice
                    .row_values
                    .iter()
                    .map(|(label, values, row_span)| ast::MultiDataRow {
                        label: label.clone(),
                        values: values.clone(),
                        span: *row_span,
                    })
                    .collect(),
            })
            .collect();

        // Silence "used" lints: these locals were needed in the old
        // expansion loop; they're preserved in the captured AST instead.
        let _ = (&row_index_spec, &slice_axis_specs, table_total_span);

        let multi = ast::MultiDecl {
            slots: ast_slots,
            shared_axes: ast::MultiDeclSharedAxes::try_from_vec(shared_axes.clone())
                .expect("multi-decl parser rejects an empty shared-axis list"),
            slot_axes: ast_slot_axes,
            slices: ast_slices,
            span: surface_span,
            table_expr_span: table_total_span,
        };

        Ok(Declaration {
            attributes: vec![],
            visibility: Visibility::Private,
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
                    let axis_ident = self.parse_any_ident()?;
                    self.expect(Token::Dot)?;
                    let variant_ident = self.parse_any_ident()?;
                    if axis_ident.name != axis.value.as_str() {
                        return Err(ParseError::MultiDeclUnsupportedShape {
                            reason: format!(
                                "slice label qualifies axis `{}`, but the shared axis at this position is `{}`",
                                axis_ident.name,
                                axis.value.as_str(),
                            ),
                            src: self.named_source(),
                            span: axis_ident.span.into(),
                        });
                    }
                    keys.push(MapEntryKey {
                        index: Spanned::new(MapEntryIndex::Named(axis.value.clone()), axis.span),
                        variant: variant_ident.into_spanned::<IndexVariantName>(),
                    });
                }
                TableIndexSpec::NatRange(n, sp) => {
                    let (_, hash_span) = self.expect(Token::Hash)?;
                    let (_, num_span) = self.expect(Token::Number)?;
                    let text = self.lexer.slice_at(num_span).replace('_', "");
                    let value: u64 = text.parse().map_err(|_| ParseError::InvalidNumber {
                        reason: "expected non-negative integer in slice label".to_string(),
                        src: self.named_source(),
                        span: num_span.into(),
                    })?;
                    if value >= *n {
                        return Err(ParseError::InvalidNumber {
                            reason: format!(
                                "slice index #{value} out of range for axis of size {n}"
                            ),
                            src: self.named_source(),
                            span: num_span.into(),
                        });
                    }
                    let variant_span = hash_span.merge(num_span);
                    keys.push(MapEntryKey {
                        index: Spanned::new(MapEntryIndex::NatRange(*n), *sp),
                        variant: Spanned::new(IndexVariantName::range_step(value), variant_span),
                    });
                }
            }
        }
        Ok(keys)
    }

    /// Parse one header + data rows block for a single slice of a multi-decl.
    ///
    /// For v1/v2 (single shared axis), `prefix_keys` is empty. For v3
    /// (multi-shared-axis), `prefix_keys` carries the slice labels.
    fn parse_multi_slice_body(
        &mut self,
        prefix_keys: &[MapEntryKey],
        slot_axes: &[SlotAxis],
        slots: &[SlotHeader],
    ) -> Result<MultiSlice, ParseError> {
        let (header_cells, header_span) = self.parse_multi_header_row()?;
        let column_layout = build_column_layout(slot_axes, &header_cells, header_span, slots)
            .map_err(|e| e.into_parse_error(&self.named_source()))?;

        let mut row_values: Vec<(Spanned<IndexVariantName>, Vec<Expr>, Span)> = Vec::new();
        while self.lexer.peek() != Some(&Token::RBrace)
            && self.lexer.peek() != Some(&Token::LBracket)
        {
            let label = self.parse_any_ident()?;
            let label_span = label.span;
            let row_label = label.into_spanned::<IndexVariantName>();
            self.expect(Token::Colon)?;
            let mut values = Vec::with_capacity(header_cells.len());
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

            if values.len() != header_cells.len() {
                return Err(ParseError::MultiDeclRowArity {
                    slot_count: header_cells.len(),
                    got: values.len(),
                    row_label: row_label.value.as_str().to_string(),
                    src: self.named_source(),
                    span: row_span.into(),
                });
            }
            row_values.push((row_label, values, row_span));
        }

        Ok(MultiSlice {
            prefix_keys: prefix_keys.to_vec(),
            header_cells,
            header_span,
            column_layout,
            row_values,
        })
    }

    /// Parse a `(param|node|const node)` kind keyword sequence, returning
    /// the kind and its span.
    fn parse_slot_kind(&mut self) -> Result<(SlotKind, Span), ParseError> {
        match self.lexer.peek() {
            Some(Token::Param) => {
                let (_, span) = self.advance()?;
                Ok((SlotKind::Param, span))
            }
            Some(Token::Node) => {
                let (_, span) = self.advance()?;
                Ok((SlotKind::Node, span))
            }
            Some(Token::Const) => {
                let (_, const_span) = self.advance()?;
                let (_, node_span) = self.expect(Token::Node)?;
                Ok((SlotKind::ConstNode, const_span.merge(node_span)))
            }
            Some(_) => {
                let (tok, span) = self.advance()?;
                Err(self.unexpected_token(
                    "`param`, `node`, or `const node` for next multi-decl slot",
                    &tok.to_string(),
                    span,
                ))
            }
            None => Err(self.unexpected_eof("`param`, `node`, or `const node`")),
        }
    }

    /// Parse a table index spec inside a multi-decl's shared-axis prefix.
    ///
    /// Same shape as the single-decl `parse_table_index_spec`, but split
    /// out so the multi-decl parser can stop at the opening paren of the
    /// slot tuple without also advancing past a comma.
    fn parse_table_index_spec_for_multi(&mut self) -> Result<TableIndexSpec, ParseError> {
        match self.lexer.peek() {
            Some(Token::Number) => {
                let (_, span) = self.advance()?;
                let text = self.lexer.slice_at(span).replace('_', "");
                let value: u64 = text.parse().map_err(|_| ParseError::InvalidNumber {
                    reason: "expected non-negative integer in table index position".to_string(),
                    src: self.named_source(),
                    span: span.into(),
                })?;
                Ok(TableIndexSpec::NatRange(value, span))
            }
            Some(Token::Ident) => {
                let ident = self.parse_any_ident()?;
                Ok(TableIndexSpec::Named(ident.into_spanned::<IndexName>()))
            }
            _ => {
                let (tok, span) = self.advance()?;
                Err(self.unexpected_token("index name or integer literal", &tok.to_string(), span))
            }
        }
    }

    /// Parse a slot tuple: `( slot_axes { , slot_axes } [,] )`.
    ///
    /// Each entry is either `_` (no extra axis) or an identifier naming
    /// the slot's extra axis. Nat-range extras are not supported in v1.
    fn parse_slot_tuple(&mut self) -> Result<(Vec<SlotAxis>, Span), ParseError> {
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

    fn parse_slot_axis_entry(&mut self) -> Result<SlotAxis, ParseError> {
        match self.lexer.peek() {
            Some(Token::Underscore) => {
                self.advance()?;
                Ok(SlotAxis::Underscore)
            }
            Some(Token::Ident) => {
                let ident = self.parse_any_ident()?;
                Ok(SlotAxis::Axis(ident.into_spanned::<IndexName>()))
            }
            _ => {
                let (tok, span) = self.advance()?;
                Err(self.unexpected_token(
                    "`_` or an axis identifier in slot tuple",
                    &tok.to_string(),
                    span,
                ))
            }
        }
    }

    /// Parse the multi-decl header row: `: header_cell { , header_cell } ;`.
    fn parse_multi_header_row(&mut self) -> Result<(Vec<HeaderCell>, Span), ParseError> {
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

    fn parse_header_cell(&mut self) -> Result<HeaderCell, ParseError> {
        match self.lexer.peek() {
            Some(Token::Underscore) => {
                let (_, span) = self.advance()?;
                Ok(HeaderCell::Underscore(span))
            }
            Some(Token::Ident) => {
                let ident = self.parse_any_ident()?;
                if self.lexer.peek() == Some(&Token::Dot) {
                    self.lexer.next_token();
                    let variant = self.parse_any_ident()?;
                    let span = ident.span.merge(variant.span);
                    return Ok(HeaderCell::Variant {
                        axis: Some(Spanned::new(IndexName::new(&ident.name), ident.span)),
                        variant: variant.into_spanned::<IndexVariantName>(),
                        span,
                    });
                }
                let span = ident.span;
                Ok(HeaderCell::Variant {
                    axis: None,
                    variant: ident.into_spanned::<IndexVariantName>(),
                    span,
                })
            }
            _ => {
                let (tok, span) = self.advance()?;
                Err(self.unexpected_token(
                    "`_` or a variant identifier in header row",
                    &tok.to_string(),
                    span,
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::ast::{ExprKind, MultiSlotKind, Visibility};
    use crate::syntax::desugar::expand_multi_decl;
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
        assert_eq!(multi.slots.len(), 2);
        assert_eq!(multi.slots[0].name.value.as_str(), "power_consumption");
        assert_eq!(multi.slots[1].name.value.as_str(), "n_installed");
        assert_eq!(multi.slot_axes.len(), 2);
        assert!(matches!(multi.slot_axes[0], ast::MultiSlotAxis::Underscore));
        assert!(matches!(multi.slot_axes[1], ast::MultiSlotAxis::Underscore));
        assert_eq!(multi.slices.len(), 1);
        assert_eq!(multi.slices[0].rows.len(), 2);

        // The desugar pass expands this to two separate Param decls each
        // carrying a 1-D TableLiteral.
        let desugared = expand_multi_decl(multi);
        assert_eq!(desugared.len(), 2);
        let DeclKind::Param(first) = &desugared[0].kind else {
            panic!("expected Param")
        };
        match &first.value.as_ref().unwrap().kind {
            ExprKind::Sugar(crate::syntax::ast::RawExprSugar::TableLiteral {
                indexes,
                entries,
            }) => {
                assert_eq!(indexes.len(), 1);
                assert_eq!(entries.len(), 2);
            }
            other => panic!("expected TableLiteral, got {other:?}"),
        }
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
        assert_eq!(multi.slots.len(), 3);
        assert_eq!(multi.slots[0].kind, MultiSlotKind::Param);
        assert_eq!(multi.slots[1].kind, MultiSlotKind::Node);
        assert_eq!(multi.slots[2].kind, MultiSlotKind::ConstNode);
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
                err,
                ParseError::MultiDeclTupleArity {
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
        match err {
            ParseError::MultiDeclRowArity {
                slot_count,
                got,
                row_label,
                ..
            } => {
                assert_eq!(slot_count, 2);
                assert_eq!(got, 1);
                assert_eq!(row_label, "X");
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
            matches!(err, ParseError::UnexpectedToken { .. }),
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
        assert!(matches!(err, ParseError::UnexpectedToken { .. }));
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
        assert_eq!(multi.slots[0].visibility, Visibility::Public);
        assert_eq!(multi.slots[1].visibility, Visibility::Private);

        let desugared = expand_multi_decl(multi);
        assert_eq!(desugared[0].visibility, Visibility::Public);
        assert_eq!(desugared[1].visibility, Visibility::Private);
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
        assert_eq!(multi.slots[0].visibility, Visibility::Private);
        assert_eq!(multi.slots[1].visibility, Visibility::Public);

        let desugared = expand_multi_decl(multi);
        assert_eq!(desugared[0].visibility, Visibility::Private);
        assert_eq!(desugared[1].visibility, Visibility::Public);
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
        assert!(matches!(err, ParseError::UnexpectedToken { .. }));
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
        assert!(matches!(err, ParseError::UnexpectedToken { .. }));
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
      ComponentA:  10.0 W,  1, 2.5 kg, true,  true;
      ComponentB:  12.0 W,  2, 3.1 kg, false, true;
  };
";
        let file = Parser::new(source).parse_file().unwrap();
        let multi = sole_multi_decl(&file);
        assert_eq!(multi.slots.len(), 4);
        assert!(matches!(multi.slot_axes[3], ast::MultiSlotAxis::Axis(_)));

        // After desugar, the 4th slot (`power_mode`) becomes a Param with a
        // 2-D TableLiteral over Component × OperationMode.
        let desugared = expand_multi_decl(multi);
        assert_eq!(desugared.len(), 4);
        match &desugared[3].kind {
            DeclKind::Param(p) => match &p.value.as_ref().unwrap().kind {
                ExprKind::Sugar(crate::syntax::ast::RawExprSugar::TableLiteral {
                    indexes,
                    entries,
                }) => {
                    assert_eq!(indexes.len(), 2);
                    assert_eq!(entries.len(), 4); // 2 components × 2 modes
                    assert_eq!(entries[0].keys[0].index.value.to_string(), "Component");
                    assert_eq!(entries[0].keys[1].index.value.to_string(), "OperationMode");
                }
                other => panic!("expected TableLiteral, got {other:?}"),
            },
            other => panic!("expected Param, got {other:?}"),
        }
    }

    #[test]
    fn multi_decl_v3_two_extra_axis_slots_rejected() {
        // v2 supports at most one extra-axis slot; two adjacent extra-axis
        // slots are v3 territory and must be rejected with a clear error.
        let source = r"
param a: Bool[Component, OperationMode],
param b: Bool[Component, OperationMode]
  = table[Component, (OperationMode, OperationMode)] {
      :           Safe, Nominal, OpMode.Safe, OpMode.Nominal;
      ComponentA: true, false,   false,        true;
  };
";
        let err = Parser::new(source).parse_file().unwrap_err();
        assert!(
            matches!(err, ParseError::MultiDeclUnsupportedShape { .. }),
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
      [Phase.Launch]
      :           _, _;
      ComponentA: 1, 2;

      [Phase.Cruise]
      :           _, _;
      ComponentA: 3, 4;
  };
";
        let file = Parser::new(source).parse_file().unwrap();
        let multi = sole_multi_decl(&file);
        assert_eq!(multi.shared_axes.len(), 2);
        assert_eq!(multi.slices.len(), 2);
        assert_eq!(multi.slices[0].prefix_keys.len(), 1);
        assert_eq!(
            multi.slices[0].prefix_keys[0].index.value.to_string(),
            "Phase"
        );

        // Desugared: each slot becomes a Param with a 2-D TableLiteral
        // keyed by (Phase, Component).
        let desugared = expand_multi_decl(multi);
        assert_eq!(desugared.len(), 2);
        match &desugared[0].kind {
            DeclKind::Param(p) => match &p.value.as_ref().unwrap().kind {
                ExprKind::Sugar(crate::syntax::ast::RawExprSugar::TableLiteral {
                    indexes,
                    entries,
                }) => {
                    assert_eq!(indexes.len(), 2);
                    assert_eq!(entries.len(), 2); // 2 phases × 1 component
                    for e in entries {
                        assert_eq!(e.keys.len(), 2);
                        assert_eq!(e.keys[0].index.value.to_string(), "Phase");
                        assert_eq!(e.keys[1].index.value.to_string(), "Component");
                    }
                }
                other => panic!("expected TableLiteral, got {other:?}"),
            },
            other => panic!("expected Param, got {other:?}"),
        }
    }

    #[test]
    fn multi_decl_v3_slice_axis_mismatch() {
        let source = r"
param p: Int[Phase, Component],
param q: Int[Phase, Component]
  = table[Phase, Component, (_, _)] {
      [Foo.Launch]
      :           _, _;
      ComponentA: 1, 2;
  };
";
        let err = Parser::new(source).parse_file().unwrap_err();
        assert!(
            matches!(err, ParseError::MultiDeclUnsupportedShape { .. }),
            "expected MultiDeclUnsupportedShape for wrong slice axis, got {err:?}",
        );
    }

    #[test]
    fn multi_decl_v2_qualified_header_cells_accepted() {
        // Author may qualify header cells for readability. The parser accepts
        // and uses the bare variant name.
        let source = r"
index Component = { ComponentA };
index OpMode = { Safe, Nominal };

param p: Power[Component],
param m: Bool[Component, OpMode]
  = table[Component, (_, OpMode)] {
      :           _,      OpMode.Safe, OpMode.Nominal;
      ComponentA: 10.0 W, true,         false;
  };
";
        let file = Parser::new(source).parse_file().unwrap();
        // 2 index decls + 1 multi-decl.
        assert_eq!(file.declarations.len(), 3);
        let multi = sole_multi_decl(&file);
        assert_eq!(multi.slots.len(), 2);
    }
}

/// A parsed entry in a slot tuple: either `_` or a named extra axis.
#[derive(Debug, Clone)]
pub(super) enum SlotAxis {
    /// `_` — slot has no extra axis (1-D, shares only the row axis).
    Underscore,
    /// Identifier — slot has a single extra axis (heterogeneous, 2-D).
    Axis(Spanned<IndexName>),
}

/// A parsed header-row cell: `_`, a bare variant, or a qualified variant.
#[derive(Debug, Clone)]
pub(super) enum HeaderCell {
    Underscore(Span),
    Variant {
        /// Axis qualifier, if the author wrote `Axis.Variant`.
        axis: Option<Spanned<IndexName>>,
        variant: Spanned<IndexVariantName>,
        span: Span,
    },
}

/// One parsed slice of a multi-decl body: a prefix of shared-axis keys
/// (empty for single-shared-axis multi-decls) followed by a header row
/// and the associated data rows.
#[derive(Debug)]
pub(super) struct MultiSlice {
    pub prefix_keys: Vec<MapEntryKey>,
    pub header_cells: Vec<HeaderCell>,
    pub header_span: Span,
    pub column_layout: Vec<SlotColumnSpan>,
    pub row_values: Vec<(Spanned<IndexVariantName>, Vec<Expr>, Span)>,
}

/// Where each slot's cells live within the parsed header row.
#[derive(Debug, Clone)]
pub(super) enum SlotColumnSpan {
    /// 1-D slot — a single column at `col_idx`.
    Single(usize),
    /// Extra-axis slot — columns `start..end`, with the slot's extra axis.
    Range {
        start: usize,
        end: usize,
        extra_axis: Spanned<IndexName>,
    },
}

/// Internal error from layout validation; converted to `ParseError` by the caller.
pub(super) enum LayoutError {
    HeaderCellKind {
        span: Span,
        slot_name: String,
        expected_underscore: bool,
    },
    HeaderArity {
        slot_count: usize,
        header_count: usize,
        span: Span,
    },
    AxisMismatch {
        span: Span,
        slot_name: String,
        expected_axis: String,
        got_axis: String,
    },
    NotEnoughCells {
        slot_name: String,
        span: Span,
    },
}

impl LayoutError {
    pub(super) fn into_parse_error(
        self,
        src: &miette::NamedSource<std::sync::Arc<String>>,
    ) -> ParseError {
        match self {
            Self::HeaderCellKind {
                span,
                slot_name,
                expected_underscore,
            } => ParseError::MultiDeclUnsupportedShape {
                reason: if expected_underscore {
                    format!("header cell for 1-D slot `{slot_name}` must be `_`")
                } else {
                    format!(
                        "header cell for extra-axis slot `{slot_name}` must be a variant label, not `_`"
                    )
                },
                src: src.clone(),
                span: span.into(),
            },
            Self::HeaderArity {
                slot_count,
                header_count,
                span,
            } => ParseError::MultiDeclHeaderArity {
                slot_count,
                header_count,
                src: src.clone(),
                span: span.into(),
            },
            Self::AxisMismatch {
                span,
                slot_name,
                expected_axis,
                got_axis,
            } => ParseError::MultiDeclUnsupportedShape {
                reason: format!(
                    "header cell for slot `{slot_name}` is qualified with `{got_axis}.…`, but the slot's extra axis is `{expected_axis}`",
                ),
                src: src.clone(),
                span: span.into(),
            },
            Self::NotEnoughCells { slot_name, span } => ParseError::MultiDeclUnsupportedShape {
                reason: format!(
                    "slot `{slot_name}` is declared with an extra axis but has zero variant cells in the header row",
                ),
                src: src.clone(),
                span: span.into(),
            },
        }
    }
}

/// Map header cells to slots.
///
/// For each tuple entry:
/// - `Underscore` → consume exactly one header cell, which must be `_`.
/// - `Axis(name)` → consume all contiguous non-`_` cells until the next `_`
///   (or end of row). Qualified cells must match the axis name.
///
/// The last rule assumes **at most one extra-axis slot** in v2; v3 will
/// disambiguate adjacent extra-axis slots by axis lookup.
pub(super) fn build_column_layout(
    slot_axes: &[SlotAxis],
    header_cells: &[HeaderCell],
    header_span: Span,
    slots: &[SlotHeader],
) -> Result<Vec<SlotColumnSpan>, LayoutError> {
    let mut layout = Vec::with_capacity(slot_axes.len());
    let mut cursor = 0usize;

    for (slot_idx, slot_axis) in slot_axes.iter().enumerate() {
        let slot_name = slots[slot_idx].name.value.as_str().to_string();
        match slot_axis {
            SlotAxis::Underscore => {
                if cursor >= header_cells.len() {
                    return Err(LayoutError::HeaderArity {
                        slot_count: slot_axes.len(),
                        header_count: header_cells.len(),
                        span: header_span,
                    });
                }
                match &header_cells[cursor] {
                    HeaderCell::Underscore(_) => {}
                    HeaderCell::Variant { span, .. } => {
                        return Err(LayoutError::HeaderCellKind {
                            span: *span,
                            slot_name,
                            expected_underscore: true,
                        });
                    }
                }
                layout.push(SlotColumnSpan::Single(cursor));
                cursor += 1;
            }
            SlotAxis::Axis(extra_axis) => {
                let start = cursor;
                while cursor < header_cells.len() {
                    match &header_cells[cursor] {
                        HeaderCell::Underscore(_) => break,
                        HeaderCell::Variant { axis, span, .. } => {
                            if let Some(axis) = axis
                                && axis.value != extra_axis.value
                            {
                                return Err(LayoutError::AxisMismatch {
                                    span: *span,
                                    slot_name,
                                    expected_axis: extra_axis.value.as_str().to_string(),
                                    got_axis: axis.value.as_str().to_string(),
                                });
                            }
                            cursor += 1;
                        }
                    }
                }
                if cursor == start {
                    return Err(LayoutError::NotEnoughCells {
                        slot_name,
                        span: extra_axis.span,
                    });
                }
                layout.push(SlotColumnSpan::Range {
                    start,
                    end: cursor,
                    extra_axis: extra_axis.clone(),
                });
            }
        }
    }

    if cursor != header_cells.len() {
        return Err(LayoutError::HeaderArity {
            slot_count: slot_axes.len(),
            header_count: header_cells.len(),
            span: header_span,
        });
    }

    Ok(layout)
}
