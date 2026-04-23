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
    ConstNodeDecl, DeclKind, Declaration, Expr, ExprKind, MapEntry, MapEntryKey, NodeDecl,
    ParamDecl, TableIndexSpec, TypeExpr, Visibility,
};
use crate::syntax::names::{DeclName, IndexName, Spanned, VariantName};
use crate::syntax::span::Span;
use crate::syntax::token::Token;

use super::super::{ParseError, Parser};

/// Kind of a value-decl slot: `param`, `node`, or `const node`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SlotKind {
    Param,
    Node,
    ConstNode,
}

/// A parsed slot header: `[const] (param|node) IDENT: TypeExpr`.
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

impl Parser<'_> {
    /// Parse the tail of a slot header: `IDENT : TypeExpr` given that the
    /// kind keyword(s) have already been consumed and their span captured.
    pub(super) fn parse_slot_header_tail(
        &mut self,
        kind: SlotKind,
        kind_span: Span,
    ) -> Result<SlotHeader, ParseError> {
        let name = self.parse_any_ident()?.into_spanned::<DeclName>();
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

    /// Parse the remainder of a multi-decl given the first slot header,
    /// the leading `,` already peeked but not consumed.
    ///
    /// Desugars the surface form to N separate [`Declaration`] values.
    #[expect(
        clippy::too_many_lines,
        reason = "single cohesive routine for the multi-decl body parse + desugar"
    )]
    pub(super) fn parse_multi_decl_rest(
        &mut self,
        first_slot: SlotHeader,
    ) -> Result<Vec<Declaration>, ParseError> {
        let mut slots: Vec<SlotHeader> = vec![first_slot];

        // Parse remaining slots: `, (param|node|const node) IDENT : TypeExpr`.
        while self.lexer.peek() == Some(&Token::Comma) {
            self.lexer.next_token(); // consume ','
            let (kind, kind_span) = self.parse_slot_kind()?;
            let header = self.parse_slot_header_tail(kind, kind_span)?;
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
        if shared_axes.len() > 1 {
            // v3 territory: N-D shared-axis prefix (with slices) is not yet supported.
            return Err(ParseError::MultiDeclUnsupportedShape {
                reason: "multi-decl with multiple shared axes is not yet supported (v3)"
                    .to_string(),
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

        // Parse the table body.
        self.expect(Token::LBrace)?;

        let (header_cells, header_span) = self.parse_multi_header_row()?;
        let column_layout = build_column_layout(&slot_axes, &header_cells, header_span, &slots)
            .map_err(|e| e.into_parse_error(&self.named_source()))?;

        // Data rows.
        let mut row_values: Vec<(Spanned<VariantName>, Vec<Expr>, Span)> = Vec::new();
        while self.lexer.peek() != Some(&Token::RBrace) {
            if self.lexer.peek() == Some(&Token::LBracket) {
                return Err(ParseError::MultiDeclUnsupportedShape {
                    reason: "slice sections in multi-decl tables are not yet supported (v3)"
                        .to_string(),
                    src: self.named_source(),
                    span: self
                        .lexer
                        .peek_with_span()
                        .map_or(table_span, |(_, s)| s)
                        .into(),
                });
            }
            let label = self.parse_any_ident()?;
            let label_span = label.span;
            let row_label = label.into_spanned::<VariantName>();
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

        let (_, rbrace_span) = self.expect(Token::RBrace)?;
        let (_, semi_span) = self.expect(Token::Semicolon)?;

        let table_total_span = table_span.merge(rbrace_span);

        // Desugar each slot into its own `Declaration` carrying a synthesized
        // `TableLiteral` initializer with the shared axis and (for extra-axis
        // slots) the per-column variant.
        let row_index_spec = shared_axes[0].clone();
        let row_index_name = match &row_index_spec {
            TableIndexSpec::Named(s) => s.clone(),
            TableIndexSpec::NatRange(n, sp) => {
                use crate::registry::types::nat_range_index_name;
                Spanned::new(IndexName::new(nat_range_index_name(*n)), *sp)
            }
        };

        let mut out: Vec<Declaration> = Vec::with_capacity(slots.len());
        for (slot_idx, slot) in slots.iter().enumerate() {
            let span = &column_layout[slot_idx];
            let (slot_indexes, entries): (Vec<TableIndexSpec>, Vec<MapEntry>) = match span {
                SlotColumnSpan::Single(col_idx) => {
                    let entries = row_values
                        .iter()
                        .map(|(label, values, _)| MapEntry {
                            keys: vec![MapEntryKey {
                                index: row_index_name.clone(),
                                variant: label.clone(),
                            }],
                            value: values[*col_idx].clone(),
                        })
                        .collect();
                    (vec![row_index_spec.clone()], entries)
                }
                SlotColumnSpan::Range {
                    start,
                    end,
                    extra_axis,
                } => {
                    let col_variants: Vec<Spanned<VariantName>> = header_cells[*start..*end]
                        .iter()
                        .filter_map(|c| match c {
                            HeaderCell::Variant { variant, .. } => Some(variant.clone()),
                            HeaderCell::Underscore => None,
                        })
                        .collect();
                    let extra_index_name = Spanned::new(extra_axis.value.clone(), extra_axis.span);
                    let mut entries = Vec::with_capacity(row_values.len() * col_variants.len());
                    for (label, values, _) in &row_values {
                        for (local_col, col_variant) in col_variants.iter().enumerate() {
                            let global_col = start + local_col;
                            entries.push(MapEntry {
                                keys: vec![
                                    MapEntryKey {
                                        index: row_index_name.clone(),
                                        variant: label.clone(),
                                    },
                                    MapEntryKey {
                                        index: extra_index_name.clone(),
                                        variant: col_variant.clone(),
                                    },
                                ],
                                value: values[global_col].clone(),
                            });
                        }
                    }
                    (
                        vec![
                            row_index_spec.clone(),
                            TableIndexSpec::Named(extra_axis.clone()),
                        ],
                        entries,
                    )
                }
            };

            let table_expr = Expr {
                kind: ExprKind::TableLiteral {
                    indexes: slot_indexes,
                    entries,
                },
                span: table_total_span,
            };

            let decl_span = slot.header_span.merge(semi_span);

            let kind = match slot.kind {
                SlotKind::Param => DeclKind::Param(ParamDecl {
                    name: slot.name.clone(),
                    type_ann: slot.type_ann.clone(),
                    value: Some(table_expr),
                }),
                SlotKind::Node => DeclKind::Node(NodeDecl {
                    name: slot.name.clone(),
                    type_ann: slot.type_ann.clone(),
                    value: table_expr,
                }),
                SlotKind::ConstNode => DeclKind::ConstNode(ConstNodeDecl {
                    name: slot.name.clone(),
                    type_ann: slot.type_ann.clone(),
                    value: table_expr,
                }),
            };

            out.push(Declaration {
                attributes: vec![],
                visibility: Visibility::Private,
                kind,
                span: decl_span,
            });
        }

        Ok(out)
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
                self.advance()?;
                Ok(HeaderCell::Underscore)
            }
            Some(Token::Ident) => {
                let ident = self.parse_any_ident()?;
                if self.lexer.peek() == Some(&Token::ColonColon) {
                    self.lexer.next_token();
                    let variant = self.parse_any_ident()?;
                    let span = ident.span.merge(variant.span);
                    return Ok(HeaderCell::Variant {
                        axis: Some(Spanned::new(IndexName::new(&ident.name), ident.span)),
                        variant: variant.into_spanned::<VariantName>(),
                        span,
                    });
                }
                let span = ident.span;
                Ok(HeaderCell::Variant {
                    axis: None,
                    variant: ident.into_spanned::<VariantName>(),
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
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "test code"
    )]

    use super::*;
    use crate::syntax::parser::Parser;

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
        assert_eq!(file.declarations.len(), 3);
        // First is the index decl, then two synthesized param decls.
        match &file.declarations[1].kind {
            DeclKind::Param(p) => {
                assert_eq!(p.name.value.as_str(), "power_consumption");
                match &p.value.as_ref().expect("param has default").kind {
                    ExprKind::TableLiteral { indexes, entries } => {
                        assert_eq!(indexes.len(), 1);
                        assert_eq!(entries.len(), 2);
                        assert_eq!(entries[0].keys[0].variant.value.as_str(), "ComponentA");
                    }
                    other => panic!("expected TableLiteral, got {other:?}"),
                }
            }
            other => panic!("expected Param, got {other:?}"),
        }
        match &file.declarations[2].kind {
            DeclKind::Param(p) => {
                assert_eq!(p.name.value.as_str(), "n_installed");
                match &p.value.as_ref().expect("param has default").kind {
                    ExprKind::TableLiteral {
                        indexes: _,
                        entries,
                    } => {
                        assert_eq!(entries.len(), 2);
                    }
                    other => panic!("expected TableLiteral, got {other:?}"),
                }
            }
            other => panic!("expected Param, got {other:?}"),
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
        assert_eq!(file.declarations.len(), 4);
        assert!(matches!(file.declarations[1].kind, DeclKind::Param(_)));
        assert!(matches!(file.declarations[2].kind, DeclKind::Node(_)));
        assert!(matches!(file.declarations[3].kind, DeclKind::ConstNode(_)));
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
    fn multi_decl_rejects_visibility_on_whole() {
        let source = r"
pub param a: Int[Component], param b: Int[Component]
  = table[Component, (_, _)] {
      : _, _;
      X: 1, 2;
  };
";
        let err = Parser::new(source).parse_file().unwrap_err();
        // `pub param` is already rejected earlier (params never pub).
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
        assert_eq!(file.declarations.len(), 6);
        // power_mode is the 2-D slot — its TableLiteral should have 2 indexes.
        match &file.declarations[5].kind {
            DeclKind::Param(p) => match &p.value.as_ref().unwrap().kind {
                ExprKind::TableLiteral { indexes, entries } => {
                    assert_eq!(indexes.len(), 2);
                    assert_eq!(entries.len(), 4); // 2 components × 2 modes
                    assert_eq!(entries[0].keys.len(), 2);
                    assert_eq!(entries[0].keys[0].index.value.as_str(), "Component");
                    assert_eq!(entries[0].keys[1].index.value.as_str(), "OperationMode");
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
      :           Safe, Nominal, OpMode::Safe, OpMode::Nominal;
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
    fn multi_decl_v2_qualified_header_cells_accepted() {
        // Author may qualify header cells for readability. The parser accepts
        // and uses the bare variant name.
        let source = r"
index Component = { ComponentA };
index OpMode = { Safe, Nominal };

param p: Power[Component],
param m: Bool[Component, OpMode]
  = table[Component, (_, OpMode)] {
      :           _,      OpMode::Safe, OpMode::Nominal;
      ComponentA: 10.0 W, true,         false;
  };
";
        let file = Parser::new(source).parse_file().unwrap();
        assert_eq!(file.declarations.len(), 4);
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
    Underscore,
    Variant {
        /// Axis qualifier, if the author wrote `Axis::Variant`.
        axis: Option<Spanned<IndexName>>,
        variant: Spanned<VariantName>,
        span: Span,
    },
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
                    "header cell for slot `{slot_name}` is qualified with `{got_axis}::…`, but the slot's extra axis is `{expected_axis}`",
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
                    HeaderCell::Underscore => {}
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
                        HeaderCell::Underscore => break,
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
