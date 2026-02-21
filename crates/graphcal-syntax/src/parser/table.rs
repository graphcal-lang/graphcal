use crate::ast::{Expr, ExprKind, MapEntry, MapEntryKey};
use crate::names::{IndexName, Spanned, VariantName};
use crate::span::Span;
use crate::token::Token;

use super::{ParseError, Parser, is_pascal_case, is_uppercase_starting};

impl Parser<'_> {
    // --- Table expression (desugars to MapLiteral) ---

    /// Parse a table expression: `table[Index1, Index2] { ... }`
    /// Desugars to `ExprKind::MapLiteral` at parse time.
    pub(super) fn parse_table_expr(&mut self) -> Result<Expr, ParseError> {
        let (_, start_span) = self.expect(Token::Table)?;

        // Parse index list: [Index1, Index2, ...]
        self.expect(Token::LBracket)?;
        let mut indexes: Vec<Spanned<IndexName>> = Vec::new();
        loop {
            let ident = self.parse_ident_with_casing("PascalCase", is_pascal_case)?;
            indexes.push(ident.into_spanned::<IndexName>());
            if self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
            } else {
                break;
            }
        }
        self.expect(Token::RBracket)?;

        let ndim = indexes.len();

        // Parse table body: { ... }
        self.expect(Token::LBrace)?;

        let entries = if ndim == 1 {
            self.parse_table_1d(&indexes)?
        } else if self.lexer.peek() == Some(&Token::LBracket) {
            // 3D+: slice sections
            self.parse_table_sliced(&indexes)?
        } else {
            // 2D: single table (header + data rows)
            self.parse_table_single(&indexes, &[])?
        };

        let (_, end_span) = self.expect(Token::RBrace)?;
        let span = start_span.merge(end_span);

        Ok(Expr {
            kind: ExprKind::TableLiteral { indexes, entries },
            span,
        })
    }

    /// Parse a 1D table body: `Label: expr; ...`
    fn parse_table_1d(
        &mut self,
        indexes: &[Spanned<IndexName>],
    ) -> Result<Vec<MapEntry>, ParseError> {
        let mut entries = Vec::new();
        while self.lexer.peek() != Some(&Token::RBrace) {
            let label =
                self.parse_ident_with_casing("PascalCase identifier", is_uppercase_starting)?;
            self.expect(Token::Colon)?;
            let value = self.parse_expr()?;
            self.expect(Token::Semicolon)?;
            entries.push(MapEntry {
                keys: vec![MapEntryKey {
                    index: indexes[0].clone(),
                    variant: label.into_spanned::<VariantName>(),
                }],
                value,
            });
        }
        Ok(entries)
    }

    /// Parse a single 2D table (header row + data rows).
    /// `prefix_keys` are prepended to every entry (from slice labels in 3D+).
    fn parse_table_single(
        &mut self,
        indexes: &[Spanned<IndexName>],
        prefix_keys: &[MapEntryKey],
    ) -> Result<Vec<MapEntry>, ParseError> {
        // The row index is second-to-last, column index is last
        let row_index = &indexes[indexes.len() - 2];
        let col_index = &indexes[indexes.len() - 1];

        // Parse header row: ColLabel1, ColLabel2, ...;
        let mut col_labels: Vec<Spanned<VariantName>> = Vec::new();
        loop {
            let label =
                self.parse_ident_with_casing("PascalCase identifier", is_uppercase_starting)?;
            col_labels.push(label.into_spanned::<VariantName>());
            if self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
            } else {
                break;
            }
        }
        self.expect(Token::Semicolon)?;

        // Parse data rows: RowLabel: val1, val2, ...;
        let mut entries = Vec::new();
        while self.lexer.peek() != Some(&Token::RBrace)
            && self.lexer.peek() != Some(&Token::LBracket)
        {
            let row_label_ident =
                self.parse_ident_with_casing("PascalCase identifier", is_uppercase_starting)?;
            let row_label_span = row_label_ident.span;
            let row_label = row_label_ident.into_spanned::<VariantName>();
            self.expect(Token::Colon)?;

            let mut row_values = Vec::new();
            loop {
                let value = self.parse_expr()?;
                row_values.push(value);
                if self.lexer.peek() == Some(&Token::Comma) {
                    self.lexer.next_token();
                } else {
                    break;
                }
            }
            // Merge spans for the row for error reporting
            let row_end_span = self
                .lexer
                .peek_with_span()
                .map_or(row_label_span, |(_, s)| s);
            let row_span = row_label_span.merge(row_end_span);
            self.expect(Token::Semicolon)?;

            if row_values.len() != col_labels.len() {
                return Err(ParseError::TableRowLengthMismatch {
                    expected: col_labels.len(),
                    got: row_values.len(),
                    src: self.named_source(),
                    span: row_span.into(),
                });
            }

            for (col_idx, value) in row_values.into_iter().enumerate() {
                let mut keys: Vec<MapEntryKey> = prefix_keys.to_vec();
                keys.push(MapEntryKey {
                    index: row_index.clone(),
                    variant: row_label.clone(),
                });
                keys.push(MapEntryKey {
                    index: col_index.clone(),
                    variant: col_labels[col_idx].clone(),
                });
                entries.push(MapEntry { keys, value });
            }
        }

        Ok(entries)
    }

    /// Parse a 3D+ table with slice sections: `[SliceLabel1, SliceLabel2] header; rows; ...`
    fn parse_table_sliced(
        &mut self,
        indexes: &[Spanned<IndexName>],
    ) -> Result<Vec<MapEntry>, ParseError> {
        // Slice dimensions are all indexes except the last two (row and column)
        let slice_indexes = &indexes[..indexes.len() - 2];
        let mut entries = Vec::new();

        while self.lexer.peek() == Some(&Token::LBracket) {
            self.lexer.next_token(); // consume '['

            // Parse slice labels: Index::Variant, Index::Variant, ...
            let mut prefix_keys = Vec::new();
            for (i, slice_index) in slice_indexes.iter().enumerate() {
                if i > 0 {
                    self.expect(Token::Comma)?;
                }
                let index_ident =
                    self.parse_ident_with_casing("PascalCase identifier", is_uppercase_starting)?;
                self.expect(Token::ColonColon)?;
                let variant = self
                    .parse_ident_with_casing("PascalCase identifier", is_uppercase_starting)?
                    .into_spanned::<VariantName>();
                prefix_keys.push(MapEntryKey {
                    index: Spanned::new(IndexName::new(index_ident.name), index_ident.span),
                    variant,
                });
                let _ = slice_index; // The index name comes from the label itself
            }

            self.expect(Token::RBracket)?;

            // Parse the 2D table for this slice
            let slice_entries = self.parse_table_single(indexes, &prefix_keys)?;
            entries.extend(slice_entries);
        }

        Ok(entries)
    }

    // --- Map literal ---

    /// Parse a map literal after `{`, `Index`, `::`, and `Variant` have already been consumed.
    /// The `:` (colon before value) is the next token to consume.
    pub(super) fn parse_map_literal_after_first_entry(
        &mut self,
        brace_span: Span,
        first_index: Spanned<IndexName>,
        first_variant: Spanned<VariantName>,
    ) -> Result<Expr, ParseError> {
        self.expect(Token::Colon)?;
        let value = self.parse_expr()?;
        let mut entries = vec![MapEntry {
            keys: vec![MapEntryKey {
                index: first_index,
                variant: first_variant,
            }],
            value,
        }];
        // Parse remaining entries
        while self.lexer.peek() == Some(&Token::Comma) {
            self.lexer.next_token(); // consume ','
            if self.lexer.peek() == Some(&Token::RBrace) {
                break; // trailing comma
            }
            let index = self
                .parse_ident_with_casing("PascalCase", is_pascal_case)?
                .into_spanned::<IndexName>();
            self.expect(Token::ColonColon)?;
            let variant = self
                .parse_ident_with_casing("PascalCase", is_pascal_case)?
                .into_spanned::<VariantName>();
            self.expect(Token::Colon)?;
            let value = self.parse_expr()?;
            entries.push(MapEntry {
                keys: vec![MapEntryKey { index, variant }],
                value,
            });
        }
        let (_, end_span) = self.expect(Token::RBrace)?;
        let span = brace_span.merge(end_span);
        Ok(Expr {
            kind: ExprKind::MapLiteral { entries },
            span,
        })
    }

    /// Parse a tuple-key map literal after `{` has been consumed.
    ///
    /// `{ (Index1::Variant1, Index2::Variant2): expr, ... }`
    pub(super) fn parse_tuple_key_map_literal(
        &mut self,
        brace_span: Span,
    ) -> Result<Expr, ParseError> {
        let mut entries = Vec::new();
        loop {
            if self.lexer.peek() == Some(&Token::RBrace) {
                break;
            }
            self.expect(Token::LParen)?;
            let mut keys = Vec::new();
            loop {
                let index = self
                    .parse_ident_with_casing("PascalCase", is_pascal_case)?
                    .into_spanned::<IndexName>();
                self.expect(Token::ColonColon)?;
                let variant = self
                    .parse_ident_with_casing("PascalCase", is_pascal_case)?
                    .into_spanned::<VariantName>();
                keys.push(MapEntryKey { index, variant });
                if self.lexer.peek() == Some(&Token::Comma) {
                    self.lexer.next_token();
                } else {
                    break;
                }
            }
            self.expect(Token::RParen)?;
            self.expect(Token::Colon)?;
            let value = self.parse_expr()?;
            entries.push(MapEntry { keys, value });
            if self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
            } else {
                break;
            }
        }
        let (_, end_span) = self.expect(Token::RBrace)?;
        let span = brace_span.merge(end_span);
        Ok(Expr {
            kind: ExprKind::MapLiteral { entries },
            span,
        })
    }
}
