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
