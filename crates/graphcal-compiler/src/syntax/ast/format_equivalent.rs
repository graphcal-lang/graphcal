//! Hand-written [`FormatEquivalent`] impls for the AST nodes whose
//! equivalence is not structural.
//!
//! Every other AST node derives the relation (see
//! [`crate::syntax::format_equivalent`]). The two here carry real logic:
//!
//! - [`Expr`] routes each tree level through the stack-growth guard, like its
//!   manual `Clone` and `Drop`.
//! - [`RawExprSugar::TableLiteral`] compares its entries as a multiset: table
//!   entries have no semantic order, so a formatter that reorders them keeps
//!   the program. This is also the extension point for future formatter
//!   transformations that reorder other nodes.

use std::collections::HashMap;

use crate::syntax::ast::{Expr, MapEntry, RawExprSugar};
use crate::syntax::format_equivalent::FormatEquivalent;
use crate::syntax::index_name::IndexEntryKey;

impl FormatEquivalent for RawExprSugar {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self::TableLiteral { indexes, entries } = self;
        let Self::TableLiteral {
            indexes: other_indexes,
            entries: other_entries,
        } = other;
        indexes.format_equivalent(other_indexes)
            && table_entries_format_equivalent(entries, other_entries)
    }
}

#[derive(PartialEq, Eq, Hash)]
struct SpanFreeMapEntryKey<'a> {
    index: &'a crate::syntax::ast::MapEntryIndex,
    variant: &'a IndexEntryKey,
}

fn span_free_table_entry_key(entry: &MapEntry) -> Vec<SpanFreeMapEntryKey<'_>> {
    entry
        .keys
        .iter()
        .map(|key| SpanFreeMapEntryKey {
            index: &key.index.value,
            variant: &key.variant.value,
        })
        .collect()
}

fn table_entries_format_equivalent(lhs: &[MapEntry], rhs: &[MapEntry]) -> bool {
    table_entries_format_equivalent_by(lhs, rhs, MapEntry::format_equivalent)
}

fn table_entries_format_equivalent_by(
    lhs: &[MapEntry],
    rhs: &[MapEntry],
    mut entries_equivalent: impl FnMut(&MapEntry, &MapEntry) -> bool,
) -> bool {
    if lhs.len() != rhs.len() {
        return false;
    }

    // Formatting normally preserves table order. Keep that common case linear
    // and allocation-free.
    if lhs
        .iter()
        .zip(rhs)
        .all(|(entry, candidate)| entries_equivalent(entry, candidate))
    {
        return true;
    }

    // Reordered entries still have multiset semantics. Index by the typed,
    // span-free key so each entry searches only the values for its exact key,
    // rather than rescanning the whole right-hand table.
    let mut rhs_by_key: HashMap<Vec<SpanFreeMapEntryKey<'_>>, Vec<&MapEntry>> = rhs.iter().fold(
        HashMap::with_capacity(rhs.len()),
        |mut entries_by_key, entry| {
            entries_by_key
                .entry(span_free_table_entry_key(entry))
                .or_default()
                .push(entry);
            entries_by_key
        },
    );

    lhs.iter().all(|entry| {
        rhs_by_key
            .get_mut(&span_free_table_entry_key(entry))
            .and_then(|candidates| {
                candidates
                    .iter()
                    .position(|candidate| entries_equivalent(entry, candidate))
                    .map(|position| candidates.swap_remove(position))
            })
            .is_some()
    })
}

impl FormatEquivalent for Expr {
    fn format_equivalent(&self, other: &Self) -> bool {
        // Mirrors the manual `Clone`: route each tree level through the
        // stack-growth guard so deep left-nested operator chains do not
        // overflow. The span is ignored — that is the whole point.
        crate::stack::with_stack_growth(|| self.kind.format_equivalent(&other.kind))
    }
}

#[cfg(test)]
mod tests {
    use super::{FormatEquivalent, table_entries_format_equivalent_by};
    use crate::syntax::ast::{DeclKind, ExprKind, File, MapEntry, RawExprSugar};
    use crate::syntax::parser::Parser;

    fn parse(source: &str) -> File {
        Parser::new(source)
            .parse_file()
            .unwrap_or_else(|err| panic!("test source should parse: {err:?}\n---\n{source}"))
    }

    fn table_entries(file: &File) -> &[MapEntry] {
        let DeclKind::Param(param) = &file.declarations[0].kind else {
            panic!("expected a param declaration");
        };
        let value = param.value.as_ref().expect("expected a param value");
        let ExprKind::Sugar(RawExprSugar::TableLiteral { entries, .. }) = &value.kind else {
            panic!("expected a table literal");
        };
        entries
    }

    fn large_finite_table(entry_count: usize) -> File {
        let rows = (0..entry_count)
            .map(|value| format!("{value}.0;"))
            .collect::<Vec<_>>()
            .join("\n");
        parse(&format!(
            "param values: Dimensionless[Fin({entry_count})] = \
             table[Fin({entry_count})] {{\n{rows}\n}};"
        ))
    }

    /// Two parses of the same text are equivalent — reflexivity over real spans.
    #[test]
    fn identical_sources_are_equivalent() {
        let source = "node x: Dimensionless = 1.0 + 2.0 * 3.0;\n";
        assert!(parse(source).format_equivalent(&parse(source)));
    }

    /// Layout-only differences (whitespace, newlines) move every span but must
    /// not change equivalence — this is the property the formatter relies on.
    #[test]
    fn whitespace_differences_are_equivalent() {
        let dense = "node x:Dimensionless=1.0+2.0*3.0;node y:Dimensionless=@x;";
        let spaced = "
            node x: Dimensionless = 1.0 + 2.0 * 3.0;

            node y: Dimensionless = @x;
        ";
        assert!(parse(dense).format_equivalent(&parse(spaced)));
    }

    /// Comments live in source metadata, not the AST, so they never affect
    /// equivalence.
    #[test]
    fn comment_differences_are_equivalent() {
        let bare = "node x: Dimensionless = 1.0;\n";
        let commented = "// leading\nnode x: Dimensionless = 1.0; // trailing\n";
        assert!(parse(bare).format_equivalent(&parse(commented)));
    }

    #[test]
    fn number_literal_difference_is_not_equivalent() {
        let a = "node x: Dimensionless = 1.0;\n";
        let b = "node x: Dimensionless = 2.0;\n";
        assert!(!parse(a).format_equivalent(&parse(b)));
    }

    #[test]
    fn reordered_duplicate_table_keys_retain_multiset_semantics() {
        let ordered = concat!(
            "param x: Dimensionless[Axis] = table[Axis] {\n",
            "A: 1.0;\n",
            "A: 2.0;\n",
            "};\n",
        );
        let reordered = concat!(
            "param x: Dimensionless[Axis] = table[Axis] {\n",
            "A: 2.0;\n",
            "A: 1.0;\n",
            "};\n",
        );
        assert!(parse(ordered).format_equivalent(&parse(reordered)));
    }

    #[test]
    fn changed_value_in_duplicate_table_key_bucket_is_not_equivalent() {
        let original = concat!(
            "param x: Dimensionless[Axis] = table[Axis] {\n",
            "A: 1.0;\n",
            "A: 2.0;\n",
            "};\n",
        );
        let changed = concat!(
            "param x: Dimensionless[Axis] = table[Axis] {\n",
            "A: 2.0;\n",
            "A: 3.0;\n",
            "};\n",
        );
        assert!(!parse(original).format_equivalent(&parse(changed)));
    }

    #[test]
    fn ordered_large_table_uses_linear_entry_comparisons() {
        const ENTRY_COUNT: usize = 1_024;
        let table = large_finite_table(ENTRY_COUNT);
        let entries = table_entries(&table);
        let mut comparison_count = 0;

        assert!(table_entries_format_equivalent_by(
            entries,
            entries,
            |entry, candidate| {
                comparison_count += 1;
                entry.format_equivalent(candidate)
            },
        ));
        assert_eq!(comparison_count, ENTRY_COUNT);
    }

    #[test]
    fn reordered_large_table_uses_linear_entry_comparisons() {
        const ENTRY_COUNT: usize = 1_024;
        let table = large_finite_table(ENTRY_COUNT);
        let entries = table_entries(&table);
        let mut reordered = entries.to_vec();
        reordered.reverse();
        let mut comparison_count = 0;

        assert!(table_entries_format_equivalent_by(
            entries,
            &reordered,
            |entry, candidate| {
                comparison_count += 1;
                entry.format_equivalent(candidate)
            },
        ));
        assert!(
            comparison_count <= ENTRY_COUNT + 1,
            "expected linear comparison work, got {comparison_count} comparisons for \
             {ENTRY_COUNT} entries"
        );
    }

    #[test]
    fn operator_difference_is_not_equivalent() {
        let a = "node x: Dimensionless = 1.0 + 2.0;\n";
        let b = "node x: Dimensionless = 1.0 - 2.0;\n";
        assert!(!parse(a).format_equivalent(&parse(b)));
    }

    #[test]
    fn name_difference_is_not_equivalent() {
        let a = "node x: Dimensionless = 1.0;\n";
        let b = "node y: Dimensionless = 1.0;\n";
        assert!(!parse(a).format_equivalent(&parse(b)));
    }

    /// Operand order is structural: `a - b` differs from `b - a` even though
    /// the spans cover the same ranges.
    #[test]
    fn operand_order_is_not_equivalent() {
        let a = "node x: Dimensionless = 1.0 - 2.0;\n";
        let b = "node x: Dimensionless = 2.0 - 1.0;\n";
        assert!(!parse(a).format_equivalent(&parse(b)));
    }

    /// A dropped declaration must be caught — the canonical "formatting changed
    /// the program" failure.
    #[test]
    fn missing_declaration_is_not_equivalent() {
        let a = "node x: Dimensionless = 1.0;\nnode y: Dimensionless = 2.0;\n";
        let b = "node x: Dimensionless = 1.0;\n";
        assert!(!parse(a).format_equivalent(&parse(b)));
    }

    /// The parser-attached `///` doc block is skipped: the formatter keeps
    /// comments through source metadata, not through the AST.
    #[test]
    fn doc_comment_differences_are_equivalent() {
        let bare = "node x: Dimensionless = 1.0;\n";
        let documented = "/// The x.\nnode x: Dimensionless = 1.0;\n";
        assert!(parse(bare).format_equivalent(&parse(documented)));
    }

    /// Derived impls compare every non-span field; each pair differs in exactly
    /// one of them.
    #[test]
    fn non_span_field_differences_are_not_equivalent() {
        let pairs = [
            (
                "node x: Dimensionless = 1.0;\n",
                "pub node x: Dimensionless = 1.0;\n",
            ),
            ("node x: Dimensionless = 1.0;\n", "node x: Bool = 1.0;\n"),
            (
                "#[doc_only]\nnode x: Dimensionless = 1.0;\n",
                "#[other]\nnode x: Dimensionless = 1.0;\n",
            ),
            (
                "node x: Length = todo { @a, @b };\n",
                "node x: Length = todo { @b, @a };\n",
            ),
            (
                "node x: Length = todo { @a };\n",
                "node x: Length = 1.0 m;\n",
            ),
            (
                "param n: Dimensionless[Fin(2 + 1)] = 1.0;\n",
                "param n: Dimensionless[Fin(2 * 1)] = 1.0;\n",
            ),
            (
                "node y: Dimensionless = @m::x;\n",
                "node y: Dimensionless = @n::x;\n",
            ),
        ];
        for (lhs, rhs) in pairs {
            assert!(
                !parse(lhs).format_equivalent(&parse(rhs)),
                "expected a difference:\n{lhs}\n{rhs}"
            );
            assert!(parse(lhs).format_equivalent(&parse(lhs)), "{lhs}");
        }
    }

    /// Spans skipped by the derive, including those nested in multi-decls,
    /// never make layout-only edits non-equivalent.
    #[test]
    fn multi_decl_layout_differences_are_equivalent() {
        let dense = "param a: Int[I],param b: Bool[I, J]=table[I, (_, J)]{: _, X;X: 1, true;};";
        let spaced = concat!(
            "param a: Int[I],\n",
            "param b: Bool[I, J]\n",
            "  = table[I, (_, J)] {\n",
            "      : _, X;\n",
            "      X: 1, true;\n",
            "  };\n",
        );
        assert!(parse(dense).format_equivalent(&parse(spaced)));
        let changed = spaced.replace("1, true", "2, true");
        assert!(!parse(spaced).format_equivalent(&parse(&changed)));
    }
}
