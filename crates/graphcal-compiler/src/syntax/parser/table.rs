use crate::syntax::ast::{Expr, ExprKind, MapEntry, MapEntryIndex, MapEntryKey, TableIndexSpec};
use crate::syntax::index_name::IndexVariantName;
use crate::syntax::names::NamePath;
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::span::Span;
use crate::syntax::span::Spanned;
use crate::syntax::token::Token;

use super::{ParseError, Parser};

fn table_entry_keys(
    mut prefix: Vec<MapEntryKey>,
    row: MapEntryKey,
    column: MapEntryKey,
) -> NonEmpty<MapEntryKey> {
    if prefix.is_empty() {
        NonEmpty::new(row, vec![column])
    } else {
        let first = prefix.remove(0);
        prefix.push(row);
        prefix.push(column);
        NonEmpty::new(first, prefix)
    }
}

impl Parser<'_> {
    // --- Table expression (desugars to MapLiteral) ---

    /// Parse a table expression: `table[Index1, Index2] { ... }`
    ///
    /// Each index is either a named identifier or an integer literal that
    /// desugars to a `range(N)` index with synthetic variants `#0..#N-1`.
    pub(super) fn parse_table_expr(&mut self) -> Result<Expr, ParseError> {
        let (_, start_span) = self.expect(Token::Table)?;

        // Parse index list: [Index1, Index2, ...]
        self.expect(Token::LBracket)?;
        let mut indexes: Vec<TableIndexSpec> = Vec::new();
        loop {
            indexes.push(self.parse_table_index_spec()?);
            if self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
                if self.lexer.peek() == Some(&Token::RBracket) {
                    break;
                }
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
        } else if ndim >= 3 {
            // 3D+: slice sections
            self.parse_table_sliced(&indexes)?
        } else {
            // 2D: single table (header + data rows)
            self.parse_table_single(&indexes, &[])?
        };

        let (_, end_span) = self.expect(Token::RBrace)?;
        let span = start_span.merge(end_span);

        Ok(Expr::new(
            ExprKind::Sugar(crate::syntax::ast::RawExprSugar::TableLiteral { indexes, entries }),
            span,
        ))
    }

    /// Parse a single index spec in `table[...]`: either an identifier or an
    /// integer literal (for `range(N)` Nat indexes).
    fn parse_table_index_spec(&mut self) -> Result<TableIndexSpec, ParseError> {
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
            Some(Token::Ident | Token::Scan | Token::Unfold | Token::Linspace | Token::Step) => {
                let ident = self.parse_any_ident()?;
                Ok(TableIndexSpec::Named(ident.into_spanned::<NamePath>()))
            }
            _ => {
                let (tok, span) = self.advance()?;
                Err(self.unexpected_token("index name or integer literal", &tok.to_string(), span))
            }
        }
    }

    fn named_index_spanned(index: &Spanned<NamePath>) -> Spanned<MapEntryIndex> {
        Spanned::new(MapEntryIndex::Named(index.value.clone()), index.span)
    }

    fn named_index_spanned_owned(index: Spanned<NamePath>) -> Spanned<MapEntryIndex> {
        Spanned::new(MapEntryIndex::Named(index.value), index.span)
    }

    /// Build the typed map-entry index used for entries on a `NatRange` axis.
    const fn nat_range_index_spanned(size: u64, span: Span) -> Spanned<MapEntryIndex> {
        Spanned::new(MapEntryIndex::NatRange(size), span)
    }

    /// Synthetic variant name `#i` for a `NatRange` axis.
    fn nat_range_variant_spanned(i: u64, span: Span) -> Spanned<IndexVariantName> {
        Spanned::new(IndexVariantName::range_step(i), span)
    }

    /// Parse a 1D table body.
    ///
    /// Named index: `Label: expr; ...`
    /// `NatRange` index: `expr; ...` (no labels, exactly N rows)
    fn parse_table_1d(&mut self, indexes: &[TableIndexSpec]) -> Result<Vec<MapEntry>, ParseError> {
        match &indexes[0] {
            TableIndexSpec::Named(name) => self.parse_table_1d_named(name),
            TableIndexSpec::NatRange(n, span) => self.parse_table_1d_nat(*n, *span),
        }
    }

    fn parse_table_1d_named(
        &mut self,
        index: &Spanned<NamePath>,
    ) -> Result<Vec<MapEntry>, ParseError> {
        let mut entries = Vec::new();
        while self.lexer.peek() != Some(&Token::RBrace) {
            let label = self.parse_any_ident()?;
            self.expect(Token::Colon)?;
            let value = self.parse_expr()?;
            self.expect(Token::Semicolon)?;
            entries.push(MapEntry {
                keys: NonEmpty::singleton(MapEntryKey {
                    index: Self::named_index_spanned(index),
                    variant: label.into_spanned::<IndexVariantName>(),
                }),
                value,
            });
        }
        Ok(entries)
    }

    fn parse_table_1d_nat(&mut self, n: u64, span: Span) -> Result<Vec<MapEntry>, ParseError> {
        let index = Self::nat_range_index_spanned(n, span);
        let mut entries = Vec::new();
        let start_offset = span.offset();
        while self.lexer.peek() != Some(&Token::RBrace) {
            let value = self.parse_expr()?;
            self.expect(Token::Semicolon)?;
            let i = entries.len() as u64;
            entries.push(MapEntry {
                keys: NonEmpty::singleton(MapEntryKey {
                    index: index.clone(),
                    variant: Self::nat_range_variant_spanned(i, value.span),
                }),
                value,
            });
        }
        if entries.len() as u64 != n {
            let end_span = self.lexer.peek_with_span().map_or(span, |(_, s)| s);
            let body_span = Span::new(
                start_offset,
                end_span.offset() + end_span.len() - start_offset,
            );
            return Err(ParseError::TableRowLengthMismatch {
                expected: usize::try_from(n).unwrap_or(usize::MAX),
                got: entries.len(),
                src: self.named_source(),
                span: body_span.into(),
            });
        }
        Ok(entries)
    }

    /// Parse a single 2D table (optional header row + data rows).
    /// `prefix_keys` are prepended to every entry (from slice labels in 3D+).
    #[expect(
        clippy::too_many_lines,
        reason = "branches over Named/NatRange axis combinations"
    )]
    fn parse_table_single(
        &mut self,
        indexes: &[TableIndexSpec],
        prefix_keys: &[MapEntryKey],
    ) -> Result<Vec<MapEntry>, ParseError> {
        let n = indexes.len();
        let row_spec = &indexes[n - 2];
        let col_spec = &indexes[n - 1];

        // Build the row/column index name used for emitted keys.
        let row_index_template: Spanned<MapEntryIndex> = match row_spec {
            TableIndexSpec::Named(s) => Self::named_index_spanned(s),
            TableIndexSpec::NatRange(n, sp) => Self::nat_range_index_spanned(*n, *sp),
        };
        let col_index_template: Spanned<MapEntryIndex> = match col_spec {
            TableIndexSpec::Named(s) => Self::named_index_spanned(s),
            TableIndexSpec::NatRange(n, sp) => Self::nat_range_index_spanned(*n, *sp),
        };

        // Parse the column header row.
        // - Named column axis: requires `: ColLabel1, ColLabel2, ...;`
        // - NatRange column axis: no header; auto-generate `#0..#(n-1)` labels.
        let col_labels: Vec<Spanned<IndexVariantName>> = match col_spec {
            TableIndexSpec::Named(_) => {
                self.expect(Token::Colon)?;
                let mut labels = Vec::new();
                loop {
                    let label = self.parse_any_ident()?;
                    labels.push(label.into_spanned::<IndexVariantName>());
                    if self.lexer.peek() == Some(&Token::Comma) {
                        self.lexer.next_token();
                    } else {
                        break;
                    }
                }
                self.expect(Token::Semicolon)?;
                labels
            }
            TableIndexSpec::NatRange(n, sp) => (0..*n)
                .map(|i| Self::nat_range_variant_spanned(i, *sp))
                .collect(),
        };

        // Parse data rows.
        let mut entries = Vec::new();
        let mut row_index_counter: u64 = 0;
        while self.lexer.peek() != Some(&Token::RBrace)
            && self.lexer.peek() != Some(&Token::LBracket)
        {
            // Determine the row label for this row.
            let (row_label, row_label_span) = match row_spec {
                TableIndexSpec::Named(_) => {
                    let row_label_ident = self.parse_any_ident()?;
                    let span = row_label_ident.span;
                    let label = row_label_ident.into_spanned::<IndexVariantName>();
                    self.expect(Token::Colon)?;
                    (label, span)
                }
                TableIndexSpec::NatRange(_, sp) => {
                    // A row index beyond the range size is reported by the
                    // row-length mismatch logic below; the row is still
                    // parsed here.
                    let label = Self::nat_range_variant_spanned(row_index_counter, *sp);
                    let span = self.lexer.peek_with_span().map_or(*sp, |(_, s)| s);
                    (label, span)
                }
            };

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
                let row_key = MapEntryKey {
                    index: row_index_template.clone(),
                    variant: row_label.clone(),
                };
                let column_key = MapEntryKey {
                    index: col_index_template.clone(),
                    variant: col_labels[col_idx].clone(),
                };
                entries.push(MapEntry {
                    keys: table_entry_keys(prefix_keys.to_vec(), row_key, column_key),
                    value,
                });
            }
            row_index_counter += 1;
        }

        // For NatRange row axis, validate row count.
        if let TableIndexSpec::NatRange(n, sp) = row_spec
            && row_index_counter != *n
        {
            return Err(ParseError::TableRowLengthMismatch {
                expected: usize::try_from(*n).unwrap_or(usize::MAX),
                got: usize::try_from(row_index_counter).unwrap_or(usize::MAX),
                src: self.named_source(),
                span: (*sp).into(),
            });
        }

        Ok(entries)
    }

    /// Parse a 3D+ table with slice sections: `[SliceLabel1, ...] header; rows; ...`
    ///
    /// Slice labels are `Index.Variant` for named axes, or `#N` for `NatRange` axes.
    fn parse_table_sliced(
        &mut self,
        indexes: &[TableIndexSpec],
    ) -> Result<Vec<MapEntry>, ParseError> {
        // Slice dimensions are all indexes except the last two (row and column)
        let slice_indexes = &indexes[..indexes.len() - 2];
        let mut entries = Vec::new();

        while self.lexer.peek() == Some(&Token::LBracket) {
            self.lexer.next_token(); // consume '['

            // Parse slice labels.
            let mut prefix_keys = Vec::new();
            for (i, slice_index) in slice_indexes.iter().enumerate() {
                if i > 0 {
                    self.expect(Token::Comma)?;
                }
                match slice_index {
                    TableIndexSpec::Named(axis) => {
                        let index_ident = self.parse_any_ident()?;
                        if index_ident.name != axis.value.leaf().as_str() {
                            return Err(self.unexpected_token(
                                &format!("slice axis `{}`", axis.value.display_path()),
                                index_ident.name.as_str(),
                                index_ident.span,
                            ));
                        }
                        self.expect(Token::Dot)?;
                        let variant = self.parse_any_ident()?.into_spanned::<IndexVariantName>();
                        prefix_keys.push(MapEntryKey {
                            index: Spanned::new(
                                MapEntryIndex::Named(NamePath::local(index_ident.name.clone())),
                                index_ident.span,
                            ),
                            variant,
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
                        prefix_keys.push(MapEntryKey {
                            index: Self::nat_range_index_spanned(*n, *sp),
                            variant: Spanned::new(
                                IndexVariantName::range_step(value),
                                variant_span,
                            ),
                        });
                    }
                }
            }

            self.expect(Token::RBracket)?;

            // Parse the 2D table for this slice
            let slice_entries = self.parse_table_single(indexes, &prefix_keys)?;
            entries.extend(slice_entries);
        }

        Ok(entries)
    }

    // --- Map literal ---

    /// Parse a map literal after `{`, `Index`, `.`, and `Variant` have already been consumed.
    /// The `:` (colon before value) is the next token to consume.
    pub(super) fn parse_map_literal_after_first_entry(
        &mut self,
        brace_span: Span,
        first_index: Spanned<NamePath>,
        first_variant: Spanned<IndexVariantName>,
    ) -> Result<Expr, ParseError> {
        self.expect(Token::Colon)?;
        let value = self.parse_expr()?;
        let mut entries = vec![MapEntry {
            keys: NonEmpty::singleton(MapEntryKey {
                index: Self::named_index_spanned_owned(first_index),
                variant: first_variant,
            }),
            value,
        }];
        // Parse remaining entries
        while self.lexer.peek() == Some(&Token::Comma) {
            self.lexer.next_token(); // consume ','
            if self.lexer.peek() == Some(&Token::RBrace) {
                break; // trailing comma
            }
            let (index, variant, _) = self.parse_index_variant_path()?;
            self.expect(Token::Colon)?;
            let value = self.parse_expr()?;
            entries.push(MapEntry {
                keys: NonEmpty::singleton(MapEntryKey {
                    index: Self::named_index_spanned_owned(index),
                    variant,
                }),
                value,
            });
        }
        let (_, end_span) = self.expect(Token::RBrace)?;
        let span = brace_span.merge(end_span);
        Ok(Expr::new(ExprKind::MapLiteral { entries }, span))
    }

    /// Parse a tuple-key map literal after `{` has been consumed.
    ///
    /// `{ (Index1.Variant1, Index2.Variant2): expr, ... }`
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
            let (index, variant, _) = self.parse_index_variant_path()?;
            let first_key = MapEntryKey {
                index: Self::named_index_spanned_owned(index),
                variant,
            };
            let mut rest_keys = Vec::new();
            while self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
                let (index, variant, _) = self.parse_index_variant_path()?;
                rest_keys.push(MapEntryKey {
                    index: Self::named_index_spanned_owned(index),
                    variant,
                });
            }
            self.expect(Token::RParen)?;
            self.expect(Token::Colon)?;
            let value = self.parse_expr()?;
            entries.push(MapEntry {
                keys: NonEmpty::new(first_key, rest_keys),
                value,
            });
            if self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
            } else {
                break;
            }
        }
        let (_, end_span) = self.expect(Token::RBrace)?;
        let span = brace_span.merge(end_span);
        Ok(Expr::new(ExprKind::MapLiteral { entries }, span))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::ast::{DeclKind, ExprKind};

    #[test]
    fn parse_map_literal() {
        let source = "param dv: Velocity[Maneuver] = { Maneuver.Departure: 2.0 km/s, Maneuver.Correction: 0.05 km/s };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.value.as_ref().unwrap().kind {
                ExprKind::MapLiteral { entries } => {
                    assert_eq!(entries.len(), 2);
                    assert_eq!(entries[0].keys[0].index.value.to_string(), "Maneuver");
                    assert_eq!(entries[0].keys[0].variant.value.as_str(), "Departure");
                    assert_eq!(entries[1].keys[0].index.value.to_string(), "Maneuver");
                    assert_eq!(entries[1].keys[0].variant.value.as_str(), "Correction");
                }
                other => panic!("expected MapLiteral, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    fn named_index_name(spec: &TableIndexSpec) -> &str {
        match spec {
            TableIndexSpec::Named(s) => s.value.leaf().as_str(),
            TableIndexSpec::NatRange(..) => panic!("expected Named spec"),
        }
    }

    fn nat_range_size(spec: &TableIndexSpec) -> u64 {
        match spec {
            TableIndexSpec::NatRange(n, _) => *n,
            TableIndexSpec::Named(_) => panic!("expected NatRange spec"),
        }
    }

    #[test]
    fn parse_table_1d() {
        let source = r"param v: Velocity[Maneuver] = table[Maneuver] {
        Departure: 2.46 km/s;
        Correction: 0.12 km/s;
        Insertion: 1.83 km/s;
    };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.value.as_ref().unwrap().kind {
                ExprKind::Sugar(crate::syntax::ast::RawExprSugar::TableLiteral {
                    indexes,
                    entries,
                }) => {
                    assert_eq!(indexes.len(), 1);
                    assert_eq!(named_index_name(&indexes[0]), "Maneuver");
                    assert_eq!(entries.len(), 3);
                    assert_eq!(entries[0].keys.len(), 1);
                    assert_eq!(entries[0].keys[0].index.value.to_string(), "Maneuver");
                    assert_eq!(entries[0].keys[0].variant.value.as_str(), "Departure");
                    assert_eq!(entries[1].keys[0].variant.value.as_str(), "Correction");
                    assert_eq!(entries[2].keys[0].variant.value.as_str(), "Insertion");
                }
                other => panic!("expected TableLiteral, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_table_1d_nat() {
        let source = r"param v: Dimensionless[3] = table[3] {
        1.0;
        2.0;
        3.0;
    };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.value.as_ref().unwrap().kind {
                ExprKind::Sugar(crate::syntax::ast::RawExprSugar::TableLiteral {
                    indexes,
                    entries,
                }) => {
                    assert_eq!(indexes.len(), 1);
                    assert_eq!(nat_range_size(&indexes[0]), 3);
                    assert_eq!(entries.len(), 3);
                    assert_eq!(entries[0].keys[0].index.value.to_string(), "range(3)");
                    assert_eq!(entries[0].keys[0].variant.value.as_str(), "#0");
                    assert_eq!(entries[1].keys[0].variant.value.as_str(), "#1");
                    assert_eq!(entries[2].keys[0].variant.value.as_str(), "#2");
                }
                other => panic!("expected TableLiteral, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_table_2d() {
        let source = r"param m: Mass[Phase, Maneuver] = table[Phase, Maneuver] {
        : Departure, Correction, Insertion;
        Launch:  5000.0 kg, 0.0 kg, 0.0 kg;
        Cruise:  0.0 kg, 4500.0 kg, 0.0 kg;
        Arrival: 0.0 kg, 0.0 kg, 4000.0 kg;
    };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.value.as_ref().unwrap().kind {
                ExprKind::Sugar(crate::syntax::ast::RawExprSugar::TableLiteral {
                    indexes,
                    entries,
                }) => {
                    assert_eq!(indexes.len(), 2);
                    assert_eq!(named_index_name(&indexes[0]), "Phase");
                    assert_eq!(named_index_name(&indexes[1]), "Maneuver");
                    assert_eq!(entries.len(), 9);
                    assert_eq!(entries[0].keys.len(), 2);
                    assert_eq!(entries[0].keys[0].index.value.to_string(), "Phase");
                    assert_eq!(entries[0].keys[0].variant.value.as_str(), "Launch");
                    assert_eq!(entries[0].keys[1].index.value.to_string(), "Maneuver");
                    assert_eq!(entries[0].keys[1].variant.value.as_str(), "Departure");
                    assert_eq!(entries[1].keys[1].variant.value.as_str(), "Correction");
                    assert_eq!(entries[8].keys[0].variant.value.as_str(), "Arrival");
                    assert_eq!(entries[8].keys[1].variant.value.as_str(), "Insertion");
                }
                other => panic!("expected TableLiteral, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_table_index_list_allows_trailing_comma() {
        let source = r"param m: Mass[Phase, Maneuver] = table[Phase, Maneuver,] {
        : Departure, Correction;
        Launch:  5000.0 kg, 0.0 kg;
        Cruise:  0.0 kg, 4500.0 kg;
    };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.value.as_ref().unwrap().kind {
                ExprKind::Sugar(crate::syntax::ast::RawExprSugar::TableLiteral {
                    indexes,
                    entries,
                }) => {
                    assert_eq!(indexes.len(), 2);
                    assert_eq!(named_index_name(&indexes[0]), "Phase");
                    assert_eq!(named_index_name(&indexes[1]), "Maneuver");
                    assert_eq!(entries.len(), 4);
                }
                other => panic!("expected TableLiteral, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_table_2d_all_nat() {
        let source = r"param m: Dimensionless[2, 3] = table[2, 3] {
        1.0, 2.0, 3.0;
        4.0, 5.0, 6.0;
    };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.value.as_ref().unwrap().kind {
                ExprKind::Sugar(crate::syntax::ast::RawExprSugar::TableLiteral {
                    indexes,
                    entries,
                }) => {
                    assert_eq!(indexes.len(), 2);
                    assert_eq!(nat_range_size(&indexes[0]), 2);
                    assert_eq!(nat_range_size(&indexes[1]), 3);
                    assert_eq!(entries.len(), 6);
                    assert_eq!(entries[0].keys[0].index.value.to_string(), "range(2)");
                    assert_eq!(entries[0].keys[0].variant.value.as_str(), "#0");
                    assert_eq!(entries[0].keys[1].index.value.to_string(), "range(3)");
                    assert_eq!(entries[0].keys[1].variant.value.as_str(), "#0");
                    assert_eq!(entries[5].keys[0].variant.value.as_str(), "#1");
                    assert_eq!(entries[5].keys[1].variant.value.as_str(), "#2");
                }
                other => panic!("expected TableLiteral, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_table_2d_nat_cols() {
        let source = r"param m: Dimensionless[Phase, 3] = table[Phase, 3] {
        Launch: 1.0, 2.0, 3.0;
        Cruise: 4.0, 5.0, 6.0;
    };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.value.as_ref().unwrap().kind {
                ExprKind::Sugar(crate::syntax::ast::RawExprSugar::TableLiteral {
                    indexes,
                    entries,
                }) => {
                    assert_eq!(indexes.len(), 2);
                    assert_eq!(named_index_name(&indexes[0]), "Phase");
                    assert_eq!(nat_range_size(&indexes[1]), 3);
                    assert_eq!(entries.len(), 6);
                    assert_eq!(entries[0].keys[0].index.value.to_string(), "Phase");
                    assert_eq!(entries[0].keys[0].variant.value.as_str(), "Launch");
                    assert_eq!(entries[0].keys[1].index.value.to_string(), "range(3)");
                    assert_eq!(entries[0].keys[1].variant.value.as_str(), "#0");
                    assert_eq!(entries[2].keys[1].variant.value.as_str(), "#2");
                }
                other => panic!("expected TableLiteral, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_table_2d_nat_rows() {
        let source = r"param m: Dimensionless[2, Maneuver] = table[2, Maneuver] {
        : Departure, Correction;
        1.0, 2.0;
        3.0, 4.0;
    };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.value.as_ref().unwrap().kind {
                ExprKind::Sugar(crate::syntax::ast::RawExprSugar::TableLiteral {
                    indexes,
                    entries,
                }) => {
                    assert_eq!(indexes.len(), 2);
                    assert_eq!(nat_range_size(&indexes[0]), 2);
                    assert_eq!(named_index_name(&indexes[1]), "Maneuver");
                    assert_eq!(entries.len(), 4);
                    assert_eq!(entries[0].keys[0].index.value.to_string(), "range(2)");
                    assert_eq!(entries[0].keys[0].variant.value.as_str(), "#0");
                    assert_eq!(entries[0].keys[1].index.value.to_string(), "Maneuver");
                    assert_eq!(entries[0].keys[1].variant.value.as_str(), "Departure");
                    assert_eq!(entries[3].keys[0].variant.value.as_str(), "#1");
                    assert_eq!(entries[3].keys[1].variant.value.as_str(), "Correction");
                }
                other => panic!("expected TableLiteral, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_table_3d() {
        let source = r"param m: Mass[Time, Phase, Maneuver] = table[Time, Phase, Maneuver] {
        [Time.T1]
        : Departure, Correction;
        Launch: 5000.0 kg, 0.0 kg;
        Cruise: 0.0 kg, 4500.0 kg;

        [Time.T2]
        : Departure, Correction;
        Launch: 4800.0 kg, 0.0 kg;
        Cruise: 0.0 kg, 4300.0 kg;
    };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.value.as_ref().unwrap().kind {
                ExprKind::Sugar(crate::syntax::ast::RawExprSugar::TableLiteral {
                    indexes,
                    entries,
                }) => {
                    assert_eq!(indexes.len(), 3);
                    assert_eq!(named_index_name(&indexes[0]), "Time");
                    assert_eq!(named_index_name(&indexes[1]), "Phase");
                    assert_eq!(named_index_name(&indexes[2]), "Maneuver");
                    assert_eq!(entries.len(), 8);
                    assert_eq!(entries[0].keys.len(), 3);
                    assert_eq!(entries[0].keys[0].index.value.to_string(), "Time");
                    assert_eq!(entries[0].keys[0].variant.value.as_str(), "T1");
                    assert_eq!(entries[0].keys[1].index.value.to_string(), "Phase");
                    assert_eq!(entries[0].keys[1].variant.value.as_str(), "Launch");
                    assert_eq!(entries[0].keys[2].index.value.to_string(), "Maneuver");
                    assert_eq!(entries[0].keys[2].variant.value.as_str(), "Departure");
                    assert_eq!(entries[4].keys[0].variant.value.as_str(), "T2");
                    assert_eq!(entries[4].keys[1].variant.value.as_str(), "Launch");
                    assert_eq!(entries[4].keys[2].variant.value.as_str(), "Departure");
                }
                other => panic!("expected TableLiteral, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_table_contextual_keyword_index_name() {
        let source = r"param m: Dimensionless[step] = table[step] {
        A: 1.0;
    };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.value.as_ref().unwrap().kind {
                ExprKind::Sugar(crate::syntax::ast::RawExprSugar::TableLiteral {
                    indexes,
                    entries,
                }) => {
                    assert_eq!(indexes.len(), 1);
                    assert_eq!(named_index_name(&indexes[0]), "step");
                    assert_eq!(entries[0].keys[0].index.value.to_string(), "step");
                }
                other => panic!("expected TableLiteral, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_table_3d_rejects_wrong_slice_axis_qualifier() {
        let source = r"param m: Mass[Time, Phase, Maneuver] = table[Time, Phase, Maneuver] {
        [Phase.T1]
        : Departure;
        Launch: 5000.0 kg;
    };";
        let err = Parser::new(source).parse_file().unwrap_err();
        assert!(matches!(
            err,
            ParseError::UnexpectedToken { expected, found, .. }
                if expected == "slice axis `Time`" && found == "Phase"
        ));
    }

    #[test]
    fn parse_table_3d_nat_slice() {
        let source = r"param m: Dimensionless[2, Phase, Maneuver] = table[2, Phase, Maneuver] {
        [#0]
        : Departure, Correction;
        Launch: 1.0, 2.0;
        Cruise: 3.0, 4.0;

        [#1]
        : Departure, Correction;
        Launch: 5.0, 6.0;
        Cruise: 7.0, 8.0;
    };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.value.as_ref().unwrap().kind {
                ExprKind::Sugar(crate::syntax::ast::RawExprSugar::TableLiteral {
                    indexes,
                    entries,
                }) => {
                    assert_eq!(indexes.len(), 3);
                    assert_eq!(nat_range_size(&indexes[0]), 2);
                    assert_eq!(named_index_name(&indexes[1]), "Phase");
                    assert_eq!(named_index_name(&indexes[2]), "Maneuver");
                    assert_eq!(entries.len(), 8);
                    assert_eq!(entries[0].keys[0].index.value.to_string(), "range(2)");
                    assert_eq!(entries[0].keys[0].variant.value.as_str(), "#0");
                    assert_eq!(entries[0].keys[1].variant.value.as_str(), "Launch");
                    assert_eq!(entries[0].keys[2].variant.value.as_str(), "Departure");
                    assert_eq!(entries[4].keys[0].variant.value.as_str(), "#1");
                }
                other => panic!("expected TableLiteral, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_table_row_length_mismatch() {
        let source = r"param m: Mass[Phase, Maneuver] = table[Phase, Maneuver] {
        : Departure, Correction, Insertion;
        Launch: 5000.0 kg, 0.0 kg;
    };";
        let err = Parser::new(source).parse_file().unwrap_err();
        assert!(matches!(
            err,
            ParseError::TableRowLengthMismatch {
                expected: 3,
                got: 2,
                ..
            }
        ));
    }
}
