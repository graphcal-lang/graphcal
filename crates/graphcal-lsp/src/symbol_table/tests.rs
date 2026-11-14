use super::declarations::register_builtins;
use super::*;
use crate::symbol_identity::{ReferenceTarget, SymbolId, UnresolvedSymbol};
use graphcal_compiler::builtin::BuiltinFn;
use graphcal_compiler::syntax::span::Span;

fn definition_key(table: &SymbolTable, category: SymbolCategory, name: &str) -> SymbolId {
    table
        .definitions
        .iter()
        .find_map(|(key, definition)| {
            (definition.category == category && definition.name == name).then(|| key.clone())
        })
        .unwrap_or_else(|| panic!("missing {category:?} definition `{name}`"))
}

fn reference_key(table: &SymbolTable, offset: usize) -> Option<SymbolId> {
    table
        .find_reference_at(offset)
        .and_then(|reference| table.resolve_local_target(&reference.target))
}

#[test]
fn builtin_function_details_show_documented_arities() {
    let mut table = SymbolTable::default();
    register_builtins(&mut table);
    assert_eq!(table.definition_conflicts(), []);
    let detail = |name: &str| {
        let key = definition_key(&table, SymbolCategory::BuiltinFn, name);
        table.definitions.get(&key).unwrap().detail.clone()
    };
    for (name, expected) in [
        ("sqrt", "builtin, arity 1"),
        ("clamp", "builtin, arity 3"),
        ("abs", "builtin, arity 1"),
        ("polar", "builtin, arity 2"),
        ("count", "builtin, arity 1"),
        ("solve", "builtin, arity 2"),
        ("datetime", "builtin"),
        ("epoch", "builtin"),
        ("to_utc", "builtin"),
        ("coord", "builtin"),
    ] {
        assert_eq!(detail(name).as_deref(), Some(expected), "`{name}`");
    }
    assert_eq!(
        table
            .definitions
            .iter()
            .filter(|(key, _)| matches!(key, SymbolId::BuiltinFunction(_)))
            .count(),
        BuiltinFn::all().count()
    );
}

#[test]
fn finite_loop_variables_describe_their_normalized_cardinality() {
    let table = table_for(
        "node a: Dimensionless[Fin(2)] = for i: Fin(1 + 1) { 1.0 };\n\
             node b: Dimensionless[Fin(3)] = for j: Fin(3) { 1.0 };",
    );
    let mut details: Vec<_> = table
        .definitions
        .values()
        .filter_map(|definition| definition.detail.clone())
        .filter(|detail| detail.starts_with("loop variable over"))
        .collect();
    details.sort();
    assert_eq!(
        details,
        ["loop variable over Fin(2)", "loop variable over Fin(3)"]
    );
}

#[test]
fn duplicate_semantic_identity_is_rejected_without_replacement() {
    let source = "const node x: Dimensionless = 1.0;\n\
                      const node x: Dimensionless = 2.0;";
    let table = table_for(source);
    let definitions = table
        .definitions
        .values()
        .filter(|definition| definition.name == "x")
        .collect::<Vec<_>>();

    assert_eq!(definitions.len(), 1);
    assert_eq!(table.definition_conflicts().len(), 1);
    assert_eq!(
        table.definition_conflicts()[0].first_span,
        definitions[0].name_span
    );
}

#[test]
fn build_symbol_table_basic() {
    let source = "param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x + 1.0;";
    let raw_file = graphcal_compiler::syntax::parser::Parser::new(source)
        .parse_file()
        .unwrap();
    let desugared = graphcal_compiler::desugar::desugared_ast::File::from(raw_file);
    let file = desugared;
    let table = build_for_buffer(&file, source);

    let x_key = definition_key(&table, SymbolCategory::Param, "x");
    let y_key = definition_key(&table, SymbolCategory::Node, "y");
    assert!(table.definitions.contains_key(&x_key));
    assert!(table.definitions.contains_key(&y_key));
    assert_eq!(
        table.definitions.get(&x_key).unwrap().category,
        SymbolCategory::Param
    );
    assert_eq!(
        table.definitions.get(&y_key).unwrap().category,
        SymbolCategory::Node
    );

    // @x is a reference
    assert!(
        table
            .references
            .iter()
            .any(|reference| table.resolve_local_target(&reference.target) == Some(x_key.clone())),
        "expected @x reference"
    );
}

#[test]
fn indexes_grouped_attributes_index_bounds_and_compound_constraints() {
    let source = r"
assert first = true;
assert second = true;
#[assumes((first, second))]
node guarded: Dimensionless = 1.0;
const node START: Dimensionless = 0.0;
const node END: Dimensionless = 10.0;
const node STEP: Dimensionless = 1.0;
index Axis = range(@START, @END, step: @STEP);
const node LOWER: Dimensionless = 0.0;
param bounded: Dimensionless(min: @LOWER + 1.0) = 2.0;
";
    let table = table_for(source);

    for spelling in ["first, second", "@START", "@END", "@STEP", "@LOWER"] {
        let offset = source.find(spelling).unwrap() + usize::from(spelling != "first, second");
        assert!(
            reference_key(&table, offset).is_some(),
            "missing reference at `{spelling}`"
        );
    }
    let second = source.find("first, second").unwrap() + "first, ".len();
    assert!(reference_key(&table, second).is_some());
}

#[test]
fn field_accesses_use_the_owning_constructor_when_spellings_collide() {
    let source = r"
type Item { Item(mass: Dimensionless), }
type Other { Other(mass: Dimensionless), }
param item: Item = Item(mass: 1.0);
param other: Other = Other(mass: 2.0);
node item_mass: Dimensionless = @item.mass;
node other_mass: Dimensionless = @other.mass;
";
    let table = table_for(source);
    let item_access = source.find("@item.mass").unwrap() + "@item.".len();
    let other_access = source.find("@other.mass").unwrap() + "@other.".len();
    let item_key = reference_key(&table, item_access)
        .unwrap_or_else(|| panic!("missing item field in {:#?}", table.references));
    let other_key = reference_key(&table, other_access)
        .unwrap_or_else(|| panic!("missing other field in {:#?}", table.references));

    assert_ne!(item_key, other_key);
    assert!(matches!(item_key, SymbolId::Field(_)));
    assert!(matches!(other_key, SymbolId::Field(_)));
    assert_eq!(table.find_all_references(&item_key).len(), 2);
    assert_eq!(table.find_all_references(&other_key).len(), 2);
}

#[test]
fn recursively_indexes_dag_include_and_composition_references() {
    let source = r"
param outer: Dimensionless = 2.0;
dag d {
    param inner: Dimensionless;
    node doubled: Dimensionless = @inner * 2.0;
}
include d(inner: @outer)::{ doubled as result };
plot p = { mark: point, encode: { x: @outer } };
figure f = { plots: [p] };
";
    let table = table_for(source);
    let outer = definition_key(&table, SymbolCategory::Param, "outer");
    let inner = definition_key(&table, SymbolCategory::Param, "inner");
    let plot = definition_key(&table, SymbolCategory::Plot, "p");

    assert_eq!(table.find_all_references(&outer).len(), 2);
    assert_eq!(table.find_all_references(&inner).len(), 2);
    assert_eq!(table.find_all_references(&plot).len(), 1);
    for (spelling, relative_offset) in [
        ("@inner", 1),
        ("inner: @outer", 0),
        ("@outer", 1),
        ("plots: [p]", "plots: [".len()),
    ] {
        let offset = source.find(spelling).unwrap() + relative_offset;
        assert!(
            reference_key(&table, offset).is_some(),
            "missing reference at `{spelling}`"
        );
    }
}

#[test]
fn unfold_axis_and_three_locals_are_navigable() {
    let source = r"
index Step = range(0.0 s, 1.0 s, step: 1.0 s);
node y: Dimensionless[Step] = unfold(
    Step,
    0.0,
    |prev_y, prev_t, t| prev_y + (t - prev_t) / 1.0 s
);
";
    let raw_file = graphcal_compiler::syntax::parser::Parser::new(source)
        .parse_file()
        .unwrap();
    let file = graphcal_compiler::desugar::desugared_ast::File::from(raw_file);
    let table = build_for_buffer(&file, source);

    let axis_offset = source.find("unfold(\n    Step").unwrap() + "unfold(\n    ".len();
    assert_eq!(
        reference_key(&table, axis_offset),
        Some(definition_key(&table, SymbolCategory::Index, "Step"))
    );
    for local in ["prev_y", "prev_t", "t"] {
        let key = table
            .definitions
            .iter()
            .find_map(|(key, definition)| {
                (definition.category == SymbolCategory::LocalVar && definition.name == local)
                    .then_some(key)
            })
            .unwrap_or_else(|| panic!("missing unfold local `{local}`"));
        assert!(
            !table.find_all_references(key).is_empty(),
            "missing body reference for unfold local `{local}`"
        );
    }
}

#[test]
fn generic_parameter_references_are_scoped_to_their_type() {
    let source = r"
index Component = { A };
type Sized<N: Nat, I: Index = Component, M: Nat = N + 1> {
    Sized(values: Dimensionless[I, N]),
}
";
    let raw_file = graphcal_compiler::syntax::parser::Parser::new(source)
        .parse_file()
        .unwrap();
    let file = graphcal_compiler::desugar::desugared_ast::File::from(raw_file);
    let table = build_for_buffer(&file, source);
    let n_key = definition_key(&table, SymbolCategory::GenericParam, "N");
    let i_key = definition_key(&table, SymbolCategory::GenericParam, "I");

    assert_eq!(
        table.definitions.get(&n_key).unwrap().category,
        SymbolCategory::GenericParam
    );
    assert_eq!(table.find_all_references(&n_key).len(), 2);
    assert_eq!(table.find_all_references(&i_key).len(), 1);
    assert_eq!(
        table
            .find_all_references(&definition_key(&table, SymbolCategory::Index, "Component",))
            .len(),
        1
    );
}

#[test]
fn multi_decl_slots_each_produce_separate_symbols() {
    // Multi-decl (issue #481) desugars to N single declarations. The
    // symbol table should register each slot with its own name span
    // so goto-def / rename / hover land on the slot header.
    let source = "\
index I = { A, B };

param p: Int[I],
param q: Int[I]
  = table[I, (_, _)] {
      : _, _;
      A: 1, 3;
      B: 2, 4;
  };
";
    let raw_file = graphcal_compiler::syntax::parser::Parser::new(source)
        .parse_file()
        .unwrap();
    let desugared = graphcal_compiler::desugar::desugared_ast::File::from(raw_file);
    let file = desugared;
    let table = build_for_buffer(&file, source);

    let p_key = definition_key(&table, SymbolCategory::Param, "p");
    let q_key = definition_key(&table, SymbolCategory::Param, "q");
    assert!(
        table.definitions.contains_key(&p_key),
        "expected slot `p` in symbol table",
    );
    assert!(
        table.definitions.contains_key(&q_key),
        "expected slot `q` in symbol table",
    );
    assert_eq!(
        table.definitions.get(&p_key).unwrap().category,
        SymbolCategory::Param
    );
    assert_eq!(
        table.definitions.get(&q_key).unwrap().category,
        SymbolCategory::Param
    );

    // The slot name spans should cover the slot header's identifier in the
    // multi-decl surface, not the (synthesized) value expression.
    let p_def = table.definitions.get(&p_key).unwrap();
    let p_slice =
        &source[p_def.name_span.offset()..p_def.name_span.offset() + p_def.name_span.len()];
    assert_eq!(p_slice, "p");
    let q_def = table.definitions.get(&q_key).unwrap();
    let q_slice =
        &source[q_def.name_span.offset()..q_def.name_span.offset() + q_def.name_span.len()];
    assert_eq!(q_slice, "q");
}

#[test]
fn find_reference_at_offset() {
    let source = "param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x;";
    let raw_file = graphcal_compiler::syntax::parser::Parser::new(source)
        .parse_file()
        .unwrap();
    let desugared = graphcal_compiler::desugar::desugared_ast::File::from(raw_file);
    let file = desugared;
    let table = build_for_buffer(&file, source);

    // Find the @x reference -- it should be near the end of the source
    let at_x_offset = source.find("@x").unwrap() + 1; // offset of 'x' in '@x'
    let reference = table.find_reference_at(at_x_offset);
    assert!(reference.is_some(), "expected to find reference at @x");
    assert_eq!(
        table.resolve_local_target(&reference.unwrap().target),
        Some(definition_key(&table, SymbolCategory::Param, "x"))
    );
}
#[test]
fn same_spelling_in_distinct_namespaces_has_distinct_identity() {
    let source = "pub const node scale: Dimensionless = 2.0;\n\
                      pub const unit scale: Length = 1.0 m;\n\
                      node value: Length = @scale * 1.0 scale;";
    let table = table_for(source);
    let constant = definition_key(&table, SymbolCategory::Const, "scale");
    let unit = definition_key(&table, SymbolCategory::Unit, "scale");

    assert_ne!(constant, unit);
    assert!(matches!(constant, SymbolId::Declaration(_)));
    assert!(matches!(unit, SymbolId::Unit(_)));
    assert_eq!(table.find_all_references(&constant).len(), 1);
    assert_eq!(table.find_all_references(&unit).len(), 1);
}
// --- Inline DAG invocation LSP coverage (issue #451) ---

/// Issues #827/#828 repro: a table literal over a named index plus a
/// qualified `Index#Variant` access.
const TABLE_SOURCE: &str = "\
pub index Maneuver = { Departure, Correction };
param dv: Velocity[Maneuver] = table[Maneuver] {
    Departure: 2.0 km/s;
    Correction: 0.1 km/s;
};
node total: Velocity = @dv[Maneuver#Departure];
";

fn table_for(source: &str) -> SymbolTable {
    let raw_file = graphcal_compiler::syntax::parser::Parser::new(source)
        .parse_file()
        .unwrap();
    let desugared = graphcal_compiler::desugar::desugared_ast::File::from(raw_file);
    build_for_buffer(&desugared, source)
}

fn slice(source: &str, span: Span) -> &str {
    &source[span.offset()..span.offset() + span.len()]
}

#[test]
fn duplicate_declarations_keep_the_rest_of_the_file_navigable() {
    // The duplicate `a` is a compile error, but every declaration keeps
    // its definition and references still reach their targets.
    let source = "node a: Dimensionless = 1.0;\n\
                      node a: Dimensionless = 2.0;\n\
                      node b: Dimensionless = @a + 1.0;\n";
    let table = table_for(source);
    let a = definition_key(&table, SymbolCategory::Node, "a");
    definition_key(&table, SymbolCategory::Node, "b");
    let offset = source.find("@a").unwrap() + 1;
    assert_eq!(reference_key(&table, offset), Some(a));
}

/// #2290: a call to a removed built-in is an unresolved function
/// reference, like a call to an unknown function, and its arguments keep
/// their references.
#[test]
fn removed_builtin_call_is_an_unresolved_function_reference() {
    let source = "param t: Datetime = datetime(\"2026-01-01T00:00:00Z\");\n\
                      node jd: Dimensionless = to_jd(@t);";
    let table = table_for(source);
    let reference = table
        .find_reference_at(source.find("to_jd").unwrap())
        .expect("a reference at the removed built-in");
    assert!(
        matches!(
            &reference.target,
            ReferenceTarget::Unresolved(UnresolvedSymbol::Function(path))
                if path.leaf().as_str() == "to_jd"
        ),
        "{reference:?}"
    );
    let t = definition_key(&table, SymbolCategory::Param, "t");
    assert_eq!(
        reference_key(&table, source.find("@t").unwrap() + 1),
        Some(t)
    );
}

#[test]
fn unresolved_call_retains_resolved_and_unresolved_argument_references() {
    let source = "param known: Dimensionless = 1.0;\n\
                      node out: Dimensionless = mystery(@known + also_missing);";
    let table = table_for(source);

    let known = definition_key(&table, SymbolCategory::Param, "known");
    for spelling in ["mystery", "@known", "also_missing"] {
        let offset = source.find(spelling).unwrap() + usize::from(spelling.starts_with('@'));
        let reference = table
            .find_reference_at(offset)
            .unwrap_or_else(|| panic!("missing retained reference at `{spelling}`"));
        if spelling == "@known" {
            assert_eq!(
                table.resolve_local_target(&reference.target),
                Some(known.clone())
            );
        } else {
            assert!(matches!(reference.target, ReferenceTarget::Unresolved(_)));
        }
    }
}

/// Issue #827: row-key reference spans must cover exactly the variant
/// label, not a multi-line merge starting at the table-axis token.
#[test]
fn table_row_key_references_are_label_precise() {
    let source = TABLE_SOURCE;
    let table = table_for(source);
    let departure_key = definition_key(&table, SymbolCategory::IndexVariant, "Departure");
    let refs = table.find_all_references(&departure_key);
    assert!(!refs.is_empty(), "expected references to `Departure`");
    for r in refs {
        assert_eq!(
            slice(source, r.span),
            "Departure",
            "reference span must cover exactly the variant identifier"
        );
    }
}

/// Issue #827: the cursor on a row key must resolve to *that* row's
/// variant — overlapping merged spans used to send `Departure` to
/// `Correction` and `Correction` to nothing.
#[test]
fn table_row_key_cursor_resolves_to_own_variant() {
    let source = TABLE_SOURCE;
    let table = table_for(source);
    for variant in ["Departure", "Correction"] {
        let row_offset = source.find(&format!("    {variant}:")).unwrap() + 4;
        let reference = table
            .find_reference_at(row_offset)
            .unwrap_or_else(|| panic!("expected reference at `{variant}` row key"));
        assert_eq!(
            table.resolve_local_target(&reference.target),
            Some(definition_key(
                &table,
                SymbolCategory::IndexVariant,
                variant,
            )),
            "row key must resolve to its own variant"
        );
    }
}

/// The table-axis token inside `table[...]` and the index segment of a
/// qualified `Index#Variant` path resolve to the index declaration.
#[test]
fn index_segments_resolve_to_index_declaration() {
    let source = TABLE_SOURCE;
    let table = table_for(source);
    let axis_offset = source.find("table[Maneuver]").unwrap() + "table[".len();
    let qualifier_offset = source.find("@dv[Maneuver#Departure]").unwrap() + "@dv[".len();
    for offset in [axis_offset, qualifier_offset] {
        let reference = table
            .find_reference_at(offset)
            .expect("expected index reference at index segment");
        assert_eq!(
            table.resolve_local_target(&reference.target),
            Some(definition_key(&table, SymbolCategory::Index, "Maneuver"))
        );
    }
}

/// Issue #828: the reference recorded for `Maneuver#Departure` in an
/// index access must address only the `Departure` segment so rename
/// rewrites `@dv[Maneuver#Departure]` into `@dv[Maneuver#Begin]`.
#[test]
fn qualified_variant_reference_is_segment_precise() {
    let source = TABLE_SOURCE;
    let table = table_for(source);
    let departure_offset = source.find("Maneuver#Departure").unwrap() + "Maneuver.".len();
    let reference = table
        .find_reference_at(departure_offset)
        .expect("expected variant reference at `Departure` segment");
    assert_eq!(
        table.resolve_local_target(&reference.target),
        Some(definition_key(
            &table,
            SymbolCategory::IndexVariant,
            "Departure",
        ))
    );
    assert_eq!(slice(source, reference.span), "Departure");
}

/// Variant literals (`Season#Winter` in expression position) and match
/// patterns get the same segment-precise spans.
#[test]
fn variant_literal_and_match_pattern_references_are_segment_precise() {
    let source = "\
index Season = { Winter, Summer };
node pick: Season = Season#Winter;
node out: Dimensionless = match @pick { Season#Winter => 1.0, Season#Summer => 2.0 };
";
    let table = table_for(source);
    let winter_key = definition_key(&table, SymbolCategory::IndexVariant, "Winter");
    let refs = table.find_all_references(&winter_key);
    assert_eq!(
        refs.len(),
        2,
        "expected literal + match-pattern references to `Winter`"
    );
    for r in refs {
        assert_eq!(slice(source, r.span), "Winter");
    }
}
