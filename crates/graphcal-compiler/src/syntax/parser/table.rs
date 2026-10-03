use crate::syntax::ast::{Expr, ExprKind, MapEntry, MapEntryKey, TableIndexSpec};
use crate::syntax::fin_position::FinPosition;
use crate::syntax::index_name::IndexVariantName;
use crate::syntax::names::NamePath;
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::span::Span;
use crate::syntax::span::Spanned;
use crate::syntax::token::{ContextualKeyword, Token};

use super::{Expected, InvalidNumberReason, ParseError, ParseErrorKind, Parser};

enum TableColumnKeys {
    Named {
        axis: Spanned<NamePath>,
        labels: Vec<Spanned<IndexVariantName>>,
    },
    Finite {
        cardinality: u64,
        span: Span,
    },
}

impl TableColumnKeys {
    fn key_at(&self, position: usize) -> Option<MapEntryKey> {
        match self {
            Self::Named { axis, labels } => labels
                .get(position)
                .map(|label| MapEntryKey::named(axis.clone(), label.clone())),
            Self::Finite { cardinality, span } => u64::try_from(position)
                .ok()
                .and_then(|position| FinPosition::try_new(*cardinality, position).ok())
                .map(|position| finite_key(*span, Spanned::new(position, *span))),
        }
    }
}

/// A key at `position` on the `Fin(N)` axis written at `axis_span`.
const fn finite_key(axis_span: Span, position: Spanned<FinPosition>) -> MapEntryKey {
    MapEntryKey::Finite {
        axis_span,
        position,
    }
}

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
    /// Each axis is either a named index path or an explicit `Fin(N)` with a
    /// concrete Nat cardinality.
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

        // Empty named-axis tables are not valid maps. In 2D+, header and
        // slice labels also live only on entries, so accepting no data rows
        // could not round-trip through the table-literal AST.
        if entries.is_empty()
            && indexes
                .iter()
                .any(|index| matches!(index, TableIndexSpec::Named(_)))
        {
            return Err(self.missing_table_data_row());
        }

        let (_, end_span) = self.expect(Token::RBrace)?;
        let span = start_span.merge(end_span);

        Ok(Expr::new(
            ExprKind::Sugar(crate::syntax::ast::RawExprSugar::TableLiteral { indexes, entries }),
            span,
        ))
    }

    /// Parse one table axis: a named index path or concrete `Fin(N)`.
    pub(super) fn parse_table_index_spec(&mut self) -> Result<TableIndexSpec, ParseError> {
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

    /// The `#value` slice label on a `Fin(cardinality)` axis.
    pub(super) fn slice_position(
        cardinality: u64,
        value: u64,
        span: Span,
    ) -> Result<FinPosition, ParseError> {
        FinPosition::try_new(cardinality, value).map_err(|error| {
            Self::invalid_number(
                InvalidNumberReason::SliceIndexOutOfRange {
                    value: error.position,
                    cardinality: error.cardinality,
                },
                span,
            )
        })
    }

    pub(super) fn table_count_from_len(count: usize, span: Span) -> Result<u64, ParseError> {
        u64::try_from(count)
            .map_err(|_| Self::invalid_number(InvalidNumberReason::TableValueCount, span))
    }

    fn table_column_count(columns: &TableColumnKeys, span: Span) -> Result<u64, ParseError> {
        match columns {
            TableColumnKeys::Named { labels, .. } => Self::table_count_from_len(labels.len(), span),
            TableColumnKeys::Finite { cardinality, .. } => Ok(*cardinality),
        }
    }

    fn missing_table_data_row(&mut self) -> ParseError {
        match self.lexer.peek_with_span() {
            Some((token, span)) => Self::unexpected(Expected::TableDataRow, *token, span),
            None => self.unexpected_eof(Expected::TableDataRow),
        }
    }

    /// Parse a 1D table body.
    ///
    /// Named index: `Label: expr; ...`
    /// `Finite` index: `expr; ...` (no labels, exactly N rows)
    fn parse_table_1d(&mut self, indexes: &[TableIndexSpec]) -> Result<Vec<MapEntry>, ParseError> {
        match &indexes[0] {
            TableIndexSpec::Named(name) => self.parse_table_1d_named(name),
            TableIndexSpec::Finite { cardinality, span } => {
                self.parse_table_1d_finite(*cardinality, *span)
            }
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
                keys: NonEmpty::singleton(MapEntryKey::named(index.clone(), label.classify())),
                value,
            });
        }
        Ok(entries)
    }

    fn parse_table_1d_finite(&mut self, n: u64, span: Span) -> Result<Vec<MapEntry>, ParseError> {
        let mut entries = Vec::new();
        let mut row_count: usize = 0;
        let start_offset = span.offset();
        while self.lexer.peek() != Some(&Token::RBrace) {
            let value = self.parse_expr()?;
            self.expect(Token::Semicolon)?;
            let i = Self::table_count_from_len(row_count, value.span)?;
            row_count += 1;
            // A row past `Fin(n)` has no key; the row-count check below
            // reports it once every row has been parsed.
            if let Ok(position) = FinPosition::try_new(n, i) {
                entries.push(MapEntry {
                    keys: NonEmpty::singleton(finite_key(span, Spanned::new(position, value.span))),
                    value,
                });
            }
        }
        let got = Self::table_count_from_len(row_count, span)?;
        if got != n {
            let end_span = self.lexer.peek_with_span().map_or(span, |(_, s)| s);
            let body_span = Span::new(
                start_offset,
                end_span.offset() + end_span.len() - start_offset,
            );
            return Err(ParseError::new(
                ParseErrorKind::TableRowLengthMismatch { expected: n, got },
                body_span,
            ));
        }
        Ok(entries)
    }

    /// Parse a single 2D table (optional header row + data rows).
    /// `prefix_keys` are prepended to every entry (from slice labels in 3D+).
    fn parse_table_single(
        &mut self,
        indexes: &[TableIndexSpec],
        prefix_keys: &[MapEntryKey],
    ) -> Result<Vec<MapEntry>, ParseError> {
        let n = indexes.len();
        let row_spec = &indexes[n - 2];
        let col_spec = &indexes[n - 1];

        // Parse the column header row.
        // - Named column axis: requires `: ColLabel1, ColLabel2, ...;`
        // - Finite column axis: no header; auto-generate `#0..#(n-1)` labels.
        let col_labels = match col_spec {
            TableIndexSpec::Named(axis) => {
                self.expect(Token::Colon)?;
                let mut labels = Vec::new();
                loop {
                    let label = self.parse_any_ident()?;
                    labels.push(label.classify());
                    if self.lexer.peek() == Some(&Token::Comma) {
                        self.lexer.next_token();
                    } else {
                        break;
                    }
                }
                self.expect(Token::Semicolon)?;
                TableColumnKeys::Named {
                    axis: axis.clone(),
                    labels,
                }
            }
            TableIndexSpec::Finite { cardinality, span } => TableColumnKeys::Finite {
                cardinality: *cardinality,
                span: *span,
            },
        };

        // Parse data rows.
        let mut entries = Vec::new();
        let mut row_index_counter: u64 = 0;
        while self.lexer.peek() != Some(&Token::RBrace)
            && self.lexer.peek() != Some(&Token::LBracket)
        {
            // Determine the row label for this row.
            let (row_key, row_label_span) = match row_spec {
                TableIndexSpec::Named(axis) => {
                    let row_label_ident = self.parse_any_ident()?;
                    let span = row_label_ident.span;
                    let key = MapEntryKey::named(axis.clone(), row_label_ident.classify());
                    self.expect(Token::Colon)?;
                    (Some(key), span)
                }
                TableIndexSpec::Finite { cardinality, span } => {
                    // A row beyond the finite cardinality has no key; it is
                    // still parsed and reported by the row-count check below.
                    let key = FinPosition::try_new(*cardinality, row_index_counter)
                        .ok()
                        .map(|position| finite_key(*span, Spanned::new(position, *span)));
                    let label_span = self.lexer.peek_with_span().map_or(*span, |(_, s)| s);
                    (key, label_span)
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

            let expected = Self::table_column_count(&col_labels, row_span)?;
            let got = Self::table_count_from_len(row_values.len(), row_span)?;
            if got != expected {
                return Err(ParseError::new(
                    ParseErrorKind::TableRowLengthMismatch { expected, got },
                    row_span,
                ));
            }

            if let Some(row_key) = row_key {
                for (col_idx, value) in row_values.into_iter().enumerate() {
                    let column_key = col_labels.key_at(col_idx).ok_or_else(|| {
                        Self::invalid_number(InvalidNumberReason::TableColumnPosition, value.span)
                    })?;
                    entries.push(MapEntry {
                        keys: table_entry_keys(prefix_keys.to_vec(), row_key.clone(), column_key),
                        value,
                    });
                }
            }
            row_index_counter += 1;
        }

        // For Finite row axis, validate row count.
        if let TableIndexSpec::Finite { cardinality, span } = row_spec
            && row_index_counter != *cardinality
        {
            return Err(ParseError::new(
                ParseErrorKind::TableRowLengthMismatch {
                    expected: *cardinality,
                    got: row_index_counter,
                },
                *span,
            ));
        }

        Ok(entries)
    }

    /// Parse a 3D+ table with slice sections: `[SliceLabel1, ...] header; rows; ...`
    ///
    /// Slice labels are `Index#Variant` for named axes, or `#N` for `Finite` axes.
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
                        let (index, variant, _) = self.parse_index_variant_path()?;
                        if index.value != axis.value {
                            return Err(ParseError::new(
                                ParseErrorKind::SliceAxisMismatch {
                                    expected: axis.value.clone(),
                                    found: index.value,
                                },
                                index.span,
                            ));
                        }
                        prefix_keys.push(MapEntryKey::Named {
                            index,
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
                        prefix_keys.push(finite_key(
                            *span,
                            Spanned::new(position, hash_span.merge(num_span)),
                        ));
                    }
                }
            }

            self.expect(Token::RBracket)?;

            // Parse the 2D table for this slice. Slice labels and named
            // column headers are represented only by the resulting entries;
            // accepting an empty section would silently discard them.
            let slice_entries = self.parse_table_single(indexes, &prefix_keys)?;
            if slice_entries.is_empty() {
                return Err(self.missing_table_data_row());
            }
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
            keys: NonEmpty::singleton(MapEntryKey::named(first_index, first_variant)),
            value,
        }];
        // Parse remaining entries
        while self.lexer.peek() == Some(&Token::Comma) {
            self.lexer.next_token(); // consume ','
            if self.lexer.peek() == Some(&Token::RBrace) {
                break; // trailing comma
            }
            // The first entry fixed this literal's key grammar as scalar. A
            // later `(` would introduce a tuple key, giving the literal mixed
            // key ranks that no formatted rendering could round-trip.
            if let Some((&Token::LParen, span)) = self.lexer.peek_with_span() {
                return Err(Self::unexpected(
                    Expected::SingleKeyMapEntry,
                    Token::LParen,
                    span,
                ));
            }
            let (index, variant, _) = self.parse_index_variant_path()?;
            self.expect(Token::Colon)?;
            let value = self.parse_expr()?;
            entries.push(MapEntry {
                keys: NonEmpty::singleton(MapEntryKey::named(index, variant)),
                value,
            });
        }
        let (_, end_span) = self.expect(Token::RBrace)?;
        let span = brace_span.merge(end_span);
        Ok(Expr::new(ExprKind::MapLiteral { entries }, span))
    }

    /// Parse a tuple-key map literal after `{` has been consumed.
    ///
    /// `{ (Index1#Variant1, Index2#Variant2): expr, ... }`
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
            let first_key = MapEntryKey::named(index, variant);
            // Tuple syntax denotes a key of rank two or more. A single
            // parenthesized key would build a rank-1 entry indistinguishable
            // from scalar syntax, which the formatter would then re-render in
            // scalar form and change the literal's grammar.
            self.expect(Token::Comma)?;
            let (index, variant, _) = self.parse_index_variant_path()?;
            let mut rest_keys = vec![MapEntryKey::named(index, variant)];
            while self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
                let (index, variant, _) = self.parse_index_variant_path()?;
                rest_keys.push(MapEntryKey::named(index, variant));
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
                // Mirror of the scalar path: this literal's grammar is tuple,
                // so a bare identifier here would mix key ranks.
                let scalar_key = self
                    .lexer
                    .peek_with_span()
                    .filter(|(token, _)| token.is_identifier());
                if let Some((found, span)) = scalar_key {
                    return Err(Self::unexpected(Expected::TupleKeyMapEntry, *found, span));
                }
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
    use crate::syntax::parser::Found;

    #[test]
    fn parse_map_literal() {
        let source = "param dv: Velocity[Maneuver] = { Maneuver#Departure: 2.0 km/s, Maneuver#Correction: 0.05 km/s };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.value.as_ref().unwrap().kind {
                ExprKind::MapLiteral { entries } => {
                    assert_eq!(entries.len(), 2);
                    assert_eq!(entries[0].keys[0].axis_text(), "Maneuver");
                    assert_eq!(entries[0].keys[0].entry_key().to_string(), "Departure");
                    assert_eq!(entries[1].keys[0].axis_text(), "Maneuver");
                    assert_eq!(entries[1].keys[0].entry_key().to_string(), "Correction");
                }
                other => panic!("expected MapLiteral, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    fn named_index_name(spec: &TableIndexSpec) -> &str {
        match spec {
            TableIndexSpec::Named(s) => s.value.leaf().as_str(),
            TableIndexSpec::Finite { .. } => panic!("expected Named spec"),
        }
    }

    fn finite_index_size(spec: &TableIndexSpec) -> u64 {
        match spec {
            TableIndexSpec::Finite { cardinality, .. } => *cardinality,
            TableIndexSpec::Named(_) => panic!("expected Finite spec"),
        }
    }

    /// Tuple key syntax denotes rank two or more. Accepting a single
    /// parenthesized key would build an entry indistinguishable from scalar
    /// syntax, which the formatter re-renders unparenthesized.
    #[test]
    fn tuple_key_map_requires_at_least_two_keys() {
        let source = "param x: Dimensionless[A] = { (A#One): 1.0 };";
        let error = Parser::new(source).parse_file().unwrap_err();

        assert!(
            matches!(
                error.kind,
                ParseErrorKind::UnexpectedToken {
                    expected: Expected::Token(Token::Comma),
                    found: Found::Token(Token::RParen),
                }
            ),
            "expected a missing second key, got {error:?}"
        );
    }

    /// A map literal's first entry fixes its key grammar. Mixing ranks would
    /// produce an AST whose formatted rendering no longer reparses, since the
    /// parser selects the map grammar from the first entry alone.
    #[test]
    fn map_literal_rejects_mixed_key_ranks() {
        let scalar_first = "param x: Dimensionless[A, B] = { A#One: 1.0, (A#One, B#Two): 2.0 };";
        let tuple_first = "param x: Dimensionless[A, B] = { (A#One, B#Two): 2.0, A#One: 1.0 };";

        for (source, expected_entry, expected_found) in [
            (scalar_first, Expected::SingleKeyMapEntry, Token::LParen),
            (tuple_first, Expected::TupleKeyMapEntry, Token::Ident),
        ] {
            let error = Parser::new(source).parse_file().unwrap_err();

            let ParseErrorKind::UnexpectedToken { expected, found } = error.kind else {
                panic!("expected unexpected-token error for {source}, got {error:?}");
            };
            assert_eq!(expected, expected_entry, "source: {source}");
            assert_eq!(found, Found::Token(expected_found), "source: {source}");
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
                    assert_eq!(entries[0].keys[0].axis_text(), "Maneuver");
                    assert_eq!(entries[0].keys[0].entry_key().to_string(), "Departure");
                    assert_eq!(entries[1].keys[0].entry_key().to_string(), "Correction");
                    assert_eq!(entries[2].keys[0].entry_key().to_string(), "Insertion");
                }
                other => panic!("expected TableLiteral, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_table_1d_finite() {
        let source = r"param v: Dimensionless[Fin(3)] = table[Fin(3)] {
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
                    assert_eq!(finite_index_size(&indexes[0]), 3);
                    assert_eq!(entries.len(), 3);
                    assert_eq!(entries[0].keys[0].axis_text(), "Fin(3)");
                    assert_eq!(entries[0].keys[0].entry_key().to_string(), "#0");
                    assert_eq!(entries[1].keys[0].entry_key().to_string(), "#1");
                    assert_eq!(entries[2].keys[0].entry_key().to_string(), "#2");
                }
                other => panic!("expected TableLiteral, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn table_rejects_bare_nat_axis_with_fin_suggestion() {
        let source = "param v: Dimensionless[Fin(3)] = table[3] { 1.0; 2.0; 3.0; };";
        let error = Parser::new(source).parse_file().unwrap_err();
        assert!(
            matches!(error.kind, ParseErrorKind::ExpectedIndexFoundNat { ref expression, .. } if expression == "3"
            ),
            "unexpected error: {error:?}"
        );
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
                    assert_eq!(entries[0].keys[0].axis_text(), "Phase");
                    assert_eq!(entries[0].keys[0].entry_key().to_string(), "Launch");
                    assert_eq!(entries[0].keys[1].axis_text(), "Maneuver");
                    assert_eq!(entries[0].keys[1].entry_key().to_string(), "Departure");
                    assert_eq!(entries[1].keys[1].entry_key().to_string(), "Correction");
                    assert_eq!(entries[8].keys[0].entry_key().to_string(), "Arrival");
                    assert_eq!(entries[8].keys[1].entry_key().to_string(), "Insertion");
                }
                other => panic!("expected TableLiteral, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn named_tables_require_at_least_one_data_row() {
        let sources = ["param v: M = table[M] {};", "param m:M = table[P,M]{ :o;};"];

        for source in sources {
            let error = Parser::new(source).parse_file().unwrap_err();
            assert!(
                matches!(
                    error.kind,
                    ParseErrorKind::UnexpectedToken {
                        expected: Expected::TableDataRow,
                        found: Found::Token(Token::RBrace),
                    }
                ),
                "unexpected error for {source}: {error:?}"
            );
        }
    }

    #[test]
    fn sliced_tables_require_data_rows_in_every_section() {
        let source = r"param m: M = table[T, P, M] {
            [T#Empty]
            : o;

            [T#Full]
            : o;
            r: 1;
        };";

        let error = Parser::new(source).parse_file().unwrap_err();
        assert!(matches!(
            error.kind,
            ParseErrorKind::UnexpectedToken {
                expected: Expected::TableDataRow,
                found: Found::Token(Token::LBracket),
            }
        ));
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
    fn parse_table_2d_all_finite() {
        let source = r"param m: Dimensionless[Fin(2), Fin(3)] = table[Fin(2), Fin(3)] {
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
                    assert_eq!(finite_index_size(&indexes[0]), 2);
                    assert_eq!(finite_index_size(&indexes[1]), 3);
                    assert_eq!(entries.len(), 6);
                    assert_eq!(entries[0].keys[0].axis_text(), "Fin(2)");
                    assert_eq!(entries[0].keys[0].entry_key().to_string(), "#0");
                    assert_eq!(entries[0].keys[1].axis_text(), "Fin(3)");
                    assert_eq!(entries[0].keys[1].entry_key().to_string(), "#0");
                    assert_eq!(entries[5].keys[0].entry_key().to_string(), "#1");
                    assert_eq!(entries[5].keys[1].entry_key().to_string(), "#2");
                }
                other => panic!("expected TableLiteral, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_table_2d_finite_cols() {
        let source = r"param m: Dimensionless[Phase, Fin(3)] = table[Phase, Fin(3)] {
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
                    assert_eq!(finite_index_size(&indexes[1]), 3);
                    assert_eq!(entries.len(), 6);
                    assert_eq!(entries[0].keys[0].axis_text(), "Phase");
                    assert_eq!(entries[0].keys[0].entry_key().to_string(), "Launch");
                    assert_eq!(entries[0].keys[1].axis_text(), "Fin(3)");
                    assert_eq!(entries[0].keys[1].entry_key().to_string(), "#0");
                    assert_eq!(entries[2].keys[1].entry_key().to_string(), "#2");
                }
                other => panic!("expected TableLiteral, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_table_2d_finite_rows() {
        let source = r"param m: Dimensionless[Fin(2), Maneuver] = table[Fin(2), Maneuver] {
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
                    assert_eq!(finite_index_size(&indexes[0]), 2);
                    assert_eq!(named_index_name(&indexes[1]), "Maneuver");
                    assert_eq!(entries.len(), 4);
                    assert_eq!(entries[0].keys[0].axis_text(), "Fin(2)");
                    assert_eq!(entries[0].keys[0].entry_key().to_string(), "#0");
                    assert_eq!(entries[0].keys[1].axis_text(), "Maneuver");
                    assert_eq!(entries[0].keys[1].entry_key().to_string(), "Departure");
                    assert_eq!(entries[3].keys[0].entry_key().to_string(), "#1");
                    assert_eq!(entries[3].keys[1].entry_key().to_string(), "Correction");
                }
                other => panic!("expected TableLiteral, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_table_3d() {
        let source = r"param m: Mass[Time, Phase, Maneuver] = table[Time, Phase, Maneuver] {
        [Time#T1]
        : Departure, Correction;
        Launch: 5000.0 kg, 0.0 kg;
        Cruise: 0.0 kg, 4500.0 kg;

        [Time#T2]
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
                    assert_eq!(entries[0].keys[0].axis_text(), "Time");
                    assert_eq!(entries[0].keys[0].entry_key().to_string(), "T1");
                    assert_eq!(entries[0].keys[1].axis_text(), "Phase");
                    assert_eq!(entries[0].keys[1].entry_key().to_string(), "Launch");
                    assert_eq!(entries[0].keys[2].axis_text(), "Maneuver");
                    assert_eq!(entries[0].keys[2].entry_key().to_string(), "Departure");
                    assert_eq!(entries[4].keys[0].entry_key().to_string(), "T2");
                    assert_eq!(entries[4].keys[1].entry_key().to_string(), "Launch");
                    assert_eq!(entries[4].keys[2].entry_key().to_string(), "Departure");
                }
                other => panic!("expected TableLiteral, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_table_qualified_axis_path() {
        let source = r"param v: Dimensionless[mission::Maneuver] = table[mission::Maneuver] {
        Departure: 2.46;
    };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.value.as_ref().unwrap().kind {
                ExprKind::Sugar(crate::syntax::ast::RawExprSugar::TableLiteral {
                    indexes,
                    entries,
                }) => {
                    let TableIndexSpec::Named(axis) = &indexes[0] else {
                        panic!("expected named axis")
                    };
                    assert_eq!(axis.value.display_path(), "mission::Maneuver");
                    assert_eq!(entries[0].keys[0].axis_text(), "mission::Maneuver");
                }
                other => panic!("expected TableLiteral, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_table_qualified_slice_path() {
        let source = r"param m: Dimensionless[mission::Time, Phase, Maneuver] = table[mission::Time, Phase, Maneuver] {
        [mission::Time#T1]
        : Departure;
        Launch: 1.0;
    };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.value.as_ref().unwrap().kind {
                ExprKind::Sugar(crate::syntax::ast::RawExprSugar::TableLiteral {
                    indexes,
                    entries,
                }) => {
                    let TableIndexSpec::Named(axis) = &indexes[0] else {
                        panic!("expected named axis")
                    };
                    assert_eq!(axis.value.display_path(), "mission::Time");
                    assert_eq!(entries[0].keys[0].axis_text(), "mission::Time");
                    assert_eq!(entries[0].keys[0].entry_key().to_string(), "T1");
                    assert!(matches!(
                        &entries[0].keys[0],
                        MapEntryKey::Named { additional_index_spans, .. }
                            if *additional_index_spans == vec![axis.span]
                    ));
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
                    assert_eq!(entries[0].keys[0].axis_text(), "step");
                }
                other => panic!("expected TableLiteral, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_table_rejects_qualified_ordinary_labels() {
        let qualified_column = r"param m: Dimensionless[Phase, Maneuver] = table[Phase, Maneuver] {
        : Maneuver#Departure;
        Launch: 1.0;
    };";
        assert!(Parser::new(qualified_column).parse_file().is_err());

        let qualified_row = r"param m: Dimensionless[Phase] = table[Phase] {
        Phase#Launch: 1.0;
    };";
        assert!(Parser::new(qualified_row).parse_file().is_err());
    }

    #[test]
    fn parse_table_3d_rejects_wrong_slice_axis_qualifier() {
        let source = r"param m: Mass[Time, Phase, Maneuver] = table[Time, Phase, Maneuver] {
        [Phase#T1]
        : Departure;
        Launch: 5000.0 kg;
    };";
        let err = Parser::new(source).parse_file().unwrap_err();
        assert!(matches!(
            err.kind,
            ParseErrorKind::SliceAxisMismatch { expected, found }
                if expected.to_string() == "Time" && found.to_string() == "Phase"
        ));
    }

    #[test]
    fn parse_table_3d_rejects_wrong_qualified_slice_path_with_same_leaf() {
        let source = r"param m: Mass[a::Time, Phase, Maneuver] = table[a::Time, Phase, Maneuver] {
        [b::Time#T1]
        : Departure;
        Launch: 5000.0 kg;
    };";
        let err = Parser::new(source).parse_file().unwrap_err();
        assert!(matches!(
            err.kind,
            ParseErrorKind::SliceAxisMismatch { expected, found }
                if expected.to_string() == "a::Time" && found.to_string() == "b::Time"
        ));
    }

    #[test]
    fn parse_table_3d_finite_slice() {
        let source = r"param m: Dimensionless[Fin(2), Phase, Maneuver] = table[Fin(2), Phase, Maneuver] {
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
                    assert_eq!(finite_index_size(&indexes[0]), 2);
                    assert_eq!(named_index_name(&indexes[1]), "Phase");
                    assert_eq!(named_index_name(&indexes[2]), "Maneuver");
                    assert_eq!(entries.len(), 8);
                    assert_eq!(entries[0].keys[0].axis_text(), "Fin(2)");
                    assert_eq!(entries[0].keys[0].entry_key().to_string(), "#0");
                    assert_eq!(entries[0].keys[1].entry_key().to_string(), "Launch");
                    assert_eq!(entries[0].keys[2].entry_key().to_string(), "Departure");
                    assert_eq!(entries[4].keys[0].entry_key().to_string(), "#1");
                }
                other => panic!("expected TableLiteral, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn huge_finite_column_cardinality_is_rejected_without_materializing_keys() {
        let source = "param m: Dimensionless[Fin(1), Fin(18446744073709551615)] = table[Fin(1), Fin(18446744073709551615)] { 1.0; };";
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
    fn parse_table_row_length_mismatch() {
        let source = r"param m: Mass[Phase, Maneuver] = table[Phase, Maneuver] {
        : Departure, Correction, Insertion;
        Launch: 5000.0 kg, 0.0 kg;
    };";
        let err = Parser::new(source).parse_file().unwrap_err();
        assert!(matches!(
            err.kind,
            ParseErrorKind::TableRowLengthMismatch {
                expected: 3,
                got: 2,
                ..
            }
        ));
    }
}
