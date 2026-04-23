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

        // v1: at least one shared axis, exactly one shared axis, all slot axes
        // entries are `_`. v2/v3 relax these constraints.
        if shared_axes.is_empty() {
            return Err(ParseError::MultiDeclNoSharedAxis {
                src: self.named_source(),
                span: table_span.merge(rbracket_span).into(),
            });
        }
        if shared_axes.len() > 1 {
            // v3 territory: N-D shared-axis prefix is not yet supported.
            return Err(ParseError::MultiDeclUnsupportedShape {
                reason: "multi-decl with multiple shared axes is not yet supported (v3)"
                    .to_string(),
                src: self.named_source(),
                span: table_span.merge(rbracket_span).into(),
            });
        }
        for (idx, slot_axis) in slot_axes.iter().enumerate() {
            if let SlotAxis::Axis(axis_name) = slot_axis {
                return Err(ParseError::MultiDeclUnsupportedShape {
                    reason: format!(
                        "multi-decl slot `{}` has an extra axis `{}`; heterogeneous slots are not yet supported (v2)",
                        slots[idx].name.value.as_str(),
                        axis_name.value.as_str(),
                    ),
                    src: self.named_source(),
                    span: axis_name.span.into(),
                });
            }
        }

        // Parse the table body.
        self.expect(Token::LBrace)?;

        // Header row: `: _, _, ..., _;`
        let (header_cells, header_span) = self.parse_multi_header_row()?;
        if header_cells.len() != slots.len() {
            return Err(ParseError::MultiDeclHeaderArity {
                slot_count: slots.len(),
                header_count: header_cells.len(),
                src: self.named_source(),
                span: header_span.into(),
            });
        }
        for (idx, cell) in header_cells.iter().enumerate() {
            if let HeaderCell::Variant { span, .. } = cell {
                return Err(ParseError::MultiDeclUnsupportedShape {
                    reason: format!(
                        "header cell for slot `{}` must be `_` in v1 (heterogeneous slots arrive in v2)",
                        slots[idx].name.value.as_str(),
                    ),
                    src: self.named_source(),
                    span: (*span).into(),
                });
            }
        }

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
            let mut values = Vec::with_capacity(slots.len());
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

            if values.len() != slots.len() {
                return Err(ParseError::MultiDeclRowArity {
                    slot_count: slots.len(),
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
        // `TableLiteral` initializer with the shared axis and that slot's column.
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
            let entries: Vec<MapEntry> = row_values
                .iter()
                .map(|(label, values, _)| MapEntry {
                    keys: vec![MapEntryKey {
                        index: row_index_name.clone(),
                        variant: label.clone(),
                    }],
                    value: values[slot_idx].clone(),
                })
                .collect();

            let table_expr = Expr {
                kind: ExprKind::TableLiteral {
                    indexes: vec![row_index_spec.clone()],
                    entries,
                },
                span: table_total_span,
            };

            // Extend the declaration span to cover `header .. ; (at multi-decl semicolon)`
            // so diagnostics pointing at the synthesized decl hit the full surface.
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
                // Optional `::Variant` qualification.
                let span = if self.lexer.peek() == Some(&Token::ColonColon) {
                    self.lexer.next_token();
                    let variant = self.parse_any_ident()?;
                    ident.span.merge(variant.span)
                } else {
                    ident.span
                };
                Ok(HeaderCell::Variant { span })
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
    fn multi_decl_v2_heterogeneous_rejected() {
        let source = r"
param a: Int[Component], param b: Int[Component, OperationMode]
  = table[Component, (_, OperationMode)] {
      : _, Safe, Nominal;
      X: 1, true, false;
  };
";
        let err = Parser::new(source).parse_file().unwrap_err();
        assert!(
            matches!(err, ParseError::MultiDeclUnsupportedShape { .. }),
            "expected MultiDeclUnsupportedShape (heterogeneous is v2), got {err:?}",
        );
    }
}

/// A parsed entry in a slot tuple: either `_` or a named axis.
///
/// v1 accepts only `Underscore`; `Axis` is preserved through the parser
/// so v2 can surface heterogeneous slots. Until then, the parser rejects
/// any `Axis` entry with a dedicated diagnostic.
#[derive(Debug, Clone)]
pub(super) enum SlotAxis {
    Underscore,
    Axis(Spanned<IndexName>),
}

/// A parsed header-row cell: `_` or `[Axis::]Variant`.
///
/// Same staging as [`SlotAxis`]: v1 accepts only `Underscore`; bare and
/// qualified variant labels parse but are rejected until v2.
#[derive(Debug, Clone)]
pub(super) enum HeaderCell {
    Underscore,
    Variant { span: Span },
}
