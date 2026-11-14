//! Inline and cross-file DAG calls.

use super::*;

#[test]
fn eval_inline_dag_call_basic() {
    let source = "\
dag scale {
    param factor: Dimensionless;
    param v: Length;
    pub node result: Length = @v * @factor;
}

param src: Length = 10.0 m;
node doubled: Length = @scale(factor: 2.0, v: @src)::result;
";
    let result = compile_and_eval(source).unwrap();
    let doubled = find_value(&result, "doubled");
    assert!(
        (doubled - 20.0).abs() < 1e-10,
        "expected 20.0, got {doubled}"
    );
}

#[test]
fn eval_inline_dag_call_binds_reordered_args_by_name() {
    let source = "\
dag combine {
    param a: Dimensionless;
    param b: Dimensionless;
    pub node result: Dimensionless = @a * 10.0 + @b;
}

node combined: Dimensionless = @combine(b: 2.0, a: 1.0)::result;
";
    let result = compile_and_eval(source).unwrap();
    assert!((find_value(&result, "combined") - 12.0).abs() < 1e-10);
}

#[test]
fn eval_include_binds_reordered_args_by_name() {
    let source = "\
dag combine {
    param a: Dimensionless;
    param b: Dimensionless;
    pub node result: Dimensionless = @a * 10.0 + @b;
}

include combine(b: 2.0, a: 1.0) as combined;
node result: Dimensionless = @combined::result;
";
    let result = compile_and_eval(source).unwrap();
    assert!((find_value(&result, "result") - 12.0).abs() < 1e-10);
}

#[test]
fn eval_inline_dag_call_projects_effective_param_value() {
    let source = "\
dag config {
    param factor: Dimensionless = 2.0;
}

node default_factor: Dimensionless = @config()::factor;
node bound_factor: Dimensionless = @config(factor: 3.0)::factor;
";
    let result = compile_and_eval(source).unwrap();
    assert!((find_value(&result, "default_factor") - 2.0).abs() < 1e-10);
    assert!((find_value(&result, "bound_factor") - 3.0).abs() < 1e-10);
}

#[test]
fn eval_inline_dag_call_chains_through_body_nodes() {
    // An inline call where the dag body has an intermediate node; tests that
    // earlier nodes are evaluated and visible to later ones.
    let source = "\
dag two_step {
    param v: Length;
    node mid: Length = @v * 2.0;
    pub node result: Length = @mid + 1.0 m;
}

param src: Length = 3.0 m;
node out: Length = @two_step(v: @src)::result;
";
    let result = compile_and_eval(source).unwrap();
    let out = find_value(&result, "out");
    // (3 * 2) + 1 = 7
    assert!((out - 7.0).abs() < 1e-10, "expected 7.0, got {out}");
}

#[test]
fn eval_inline_dag_call_imports_parent_const_with_alias() {
    let source = "\
pub const node seed_len: Length = 3.0 m;

dag scaled {
    import test::{seed_len as imported_seed};

    param factor: Dimensionless;
    pub node result: Length = @imported_seed * @factor;
}

node out: Length = @scaled(factor: 4.0)::result;
";
    let result = compile_and_eval_named(source, "test.gcl").unwrap();
    let out = find_value(&result, "out");
    assert!((out - 12.0).abs() < 1e-10, "expected 12.0, got {out}");
}

#[test]
fn cached_inline_template_refines_parent_const_types_for_repeated_instances() {
    let source = "\
pub const node radius: Length = 10.0 m;

dag shifted {
    import test::{radius};

    param altitude: Length;
    pub node result: Length = @radius + @altitude;
}

include shifted(altitude: 1.0 m)::{ result as first };
include shifted(altitude: 2.0 m)::{ result as shifted_again };
";
    let result = compile_and_eval_named(source, "test.gcl").unwrap();
    assert!((find_value(&result, "first") - 11.0).abs() < 1e-10);
    assert!((find_value(&result, "shifted_again") - 12.0).abs() < 1e-10);
}

#[test]
fn eval_qualified_inline_dag_call_imports_parent_const_with_alias() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../tests/fixtures/valid/inline_dag_call_cross_file_parent_const/src/lib/main.gcl",
    );
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let earth_half = find_value(&result, "earth_half");
    assert!(
        (earth_half - 3_185_500.0).abs() < 1e-10,
        "expected 3185500.0, got {earth_half}"
    );
    assert!((find_value(&result, "default_factor") - 1.0).abs() < 1e-10);
    assert!((find_value(&result, "bound_factor") - 0.5).abs() < 1e-10);
}

#[test]
fn imported_file_and_inline_dag_aliases_are_both_directly_callable() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/callable_module_aliases/src/callable/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!((find_value(&result, "file_result") - 4.0).abs() < 1e-10);
    assert!((find_value(&result, "file_param_result") - 7.0).abs() < 1e-10);
    assert!((find_value(&result, "inline_result") - 9.0).abs() < 1e-10);
    assert!((find_value(&result, "selected_result") - 12.0).abs() < 1e-10);
    assert!((find_value(&result, "unaliased_file_result") - 10.0).abs() < 1e-10);
    assert!((find_value(&result, "unaliased_inline_result") - 18.0).abs() < 1e-10);
    assert!((find_value(&result, "nested_scope_result") - 40.0).abs() < 1e-10);
}

#[test]
fn absolute_inline_call_path_reports_imported_name_diagnostic() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../tests/fixtures/invalid/callable_module_alias_absolute_path/src/callable/main.gcl",
    );
    let err = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(matches!(
        err,
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Module(ModuleError::UnknownModule { name, .. }), .. }), .. }) if name.as_str() == "callable"
    ));
}

#[test]
fn eval_inline_dag_namespace_alias_at_field() {
    // Issue #518: `include foo() as bar; @bar::member` was N002.
    // Two instances confirm distinct namespaces.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/inline_dag_namespace/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let doubled = find_value(&result, "doubled_result");
    let tripled = find_value(&result, "tripled_result");
    assert!((doubled - 20.0).abs() < 1e-10, "doubled = {doubled}");
    assert!((tripled - 30.0).abs() < 1e-10, "tripled = {tripled}");
}

#[test]
fn field_access_on_value_is_not_hijacked_by_include_alias() {
    // `.` after a graph reference is always struct field access; `::` is the
    // only namespace-member boundary. A value can no longer share an include
    // alias's name (both are Term names; see
    // `module_aliases_and_same_named_nodes_are_duplicate_names`), so the value
    // and the alias expose the same member name instead.
    let source = "dag d { pub node out: Dimensionless = 8.0; }\n\
                  include d() as inst;\n\
                  type S { S(out: Dimensionless) }\n\
                  node value: S = S(out: 1.0);\n\
                  node y: Dimensionless = @value.out;\n\
                  node z: Dimensionless = @inst::out;";
    let result = compile_and_eval(source).unwrap();
    let y = find_value(&result, "y");
    let z = find_value(&result, "z");
    assert!((y - 1.0).abs() < 1e-10, "y = {y}");
    assert!((z - 8.0).abs() < 1e-10, "z = {z}");
}

#[test]
fn dot_member_access_on_include_alias_is_rejected_everywhere() {
    // `.` is always field access, so `@inst.LIM` is an unknown graph
    // reference in nodes, plot encodings, and type-annotation domain bounds
    // alike; the namespace member is spelled `@inst::LIM`.
    let prelude = "index Step = { A, B };\n\
                   dag d { pub const node LIM: Dimensionless = 5.0; }\n\
                   include d() as inst;\n";
    for body in [
        "node y: Dimensionless = @inst.LIM;",
        "plot p = { mark: point, encode: { x: for s: Step { @inst.LIM }, \
         y: for s: Step { @inst.LIM } }, title: \"t\" };",
        "param p: Dimensionless(max: @inst.LIM) = 3.0;",
    ] {
        let error = compile_and_eval(&format!("{prelude}{body}")).unwrap_err();
        assert!(
            matches!(
                &error,
                CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Name(NameError::UnknownGraphRef { name, .. }), .. }), .. })
                    if name == &scoped_name("inst")
            ),
            "`{body}`: unexpected error: {error:?}"
        );
    }
}

#[test]
fn eval_cross_file_include_namespace_alias_at_field() {
    // Issue #518: `include path(...) as alias; @alias::member` across files.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/multi/instantiated_import_module/src/rocket/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let dv = find_value(&result, "dv");
    assert!(dv > 0.0, "dv should be positive, got {dv}");
}

#[test]
fn eval_import_namespace_alias_at_field() {
    // Issue #518: `import path as alias; @alias::const_member` across files.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/multi/module_import_alias/src/constants/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let g = find_value(&result, "g");
    assert!((g - 9.806_65).abs() < 1e-10, "g = {g}");
}

#[test]
fn eval_qualified_const_refs_with_colliding_leaf_names() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub const node shared: Dimensionless = 2.0;\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub const node shared: Dimensionless = 3.0;\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import collide.a as a;\n\
         import collide.b as b;\n\
         const node combined: Dimensionless = @a::shared + @b::shared;\n\
         const node shared: Dimensionless = @combined + 1.0;\n\
         node out: Dimensionless = @shared;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let out = find_value(&result, "out");
    assert!((out - 6.0).abs() < 1e-10, "out = {out}");
}

#[test]
fn eval_included_struct_array_field_access_uses_imported_type_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/repro");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"repro\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("types.gcl"),
        "pub type Item {\n    Item(mass: Mass),\n}\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("data.gcl"),
        "import repro.types::{ type Item, Item };\n\
         pub index ItemId = { Only };\n\
         pub node items: Item[ItemId] = {\n\
             ItemId#Only: Item(mass: 1.0 kg),\n\
         };\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import repro.types::{ type Item };\n\
         import repro.data::{ index ItemId };\n\
         include repro.data()::{ items };\n\
         node first_mass: Mass = @items[ItemId#Only].mass;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let first_mass = find_value(&result, "first_mass");
    assert!(
        (first_mass - 1.0).abs() < 1e-10,
        "first_mass = {first_mass}"
    );
}

#[test]
fn eval_qualified_runtime_refs_with_colliding_leaf_names() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub node shared: Dimensionless = 2.0;\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub node shared: Dimensionless = 3.0;\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "include collide.a() as a;\n\
         include collide.b() as b;\n\
         node total: Dimensionless = @a::shared + @b::shared;\n\
         node shared: Dimensionless = @total + 1.0;\n\
         node out: Dimensionless = @shared;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let out = find_value(&result, "out");
    assert!((out - 6.0).abs() < 1e-10, "out = {out}");
}

#[test]
fn eval_qualified_params_with_colliding_leaf_names() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "param shared: Dimensionless = 2.0;\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "param shared: Dimensionless = 3.0;\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "include collide.a() as a;\n\
         include collide.b() as b;\n\
         param shared: Dimensionless = 100.0;\n\
         node total: Dimensionless = @a::shared + @b::shared + @shared;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let total = find_value(&result, "total");
    assert!((total - 105.0).abs() < 1e-10, "total = {total}");
}

#[test]
fn eval_selective_import_aliases_with_colliding_leaf_names() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub const node shared: Dimensionless = 2.0;\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub const node shared: Dimensionless = 3.0;\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import collide.a::{ shared as a_shared };\n\
         import collide.b::{ shared as b_shared };\n\
         const node shared: Dimensionless = 100.0;\n\
         node total: Dimensionless = @a_shared + @b_shared + @shared;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let total = find_value(&result, "total");
    assert!((total - 105.0).abs() < 1e-10, "total = {total}");
}

#[test]
fn eval_overrides_reject_included_implementation_params() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "param shared: Dimensionless = 2.0;\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "param shared: Dimensionless = 3.0;\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "include collide.a()::{ shared as a_shared };\n\
         include collide.b()::{ shared as b_shared };\n\
         node total: Dimensionless = @a_shared + @b_shared;\n",
    )
    .unwrap();

    let mut overrides = HashMap::new();
    overrides.insert(DeclName::expect_valid("a_shared"), parse_expr("20.0"));
    overrides.insert(DeclName::expect_valid("b_shared"), parse_expr("30.0"));
    let result = compile_and_eval_project(&root, &overrides, None, &fs());
    match result {
        Err(CompileError::Binding(BindingError::NotAParam {
            name,
            actual_kind:
                graphcal_compiler::declaration_category::DeclCategory::Value(
                    graphcal_compiler::declaration_category::ValueDeclCategory::Node,
                ),
        })) => {
            assert!(name.as_str() == "a_shared" || name.as_str() == "b_shared");
        }
        other => panic!("expected included implementation override rejection, got {other:?}"),
    }
}

#[test]
fn eval_include_dep_with_aliased_module_import() {
    // End-to-end coverage for including a template that itself imports another
    // module under an alias (`import lib as mission;` + `@mission::C`). The
    // checked semantic instance must retain the template's canonical imported
    // value target.
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("lib.gcl"),
        "pub const node C: Dimensionless = 7.0;\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("dep.gcl"),
        "import collide.lib as mission;\n\
         pub node out: Dimensionless = @mission::C * 2.0;\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "include collide.dep()::{ out as dep_out };\n\
         node total: Dimensionless = @dep_out + 1.0;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let total = find_value(&result, "total");
    assert!((total - 15.0).abs() < 1e-10, "total = {total}");
}

#[test]
fn eval_inline_dag_include_cross_file_self_import() {
    // Cross-file `include` of a DAG whose body has `import <self>::{...}`
    // (resolved against the DAG's parent file). The checked instance must keep
    // that canonical parent import available during evaluation.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../tests/fixtures/valid/inline_dag_include_cross_file_self_import/src/lib/main.gcl",
    );
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let out = find_value(&result, "out");
    assert!(
        (out - 3_185_500.0).abs() < 1e-10,
        "expected 3185500.0, got {out}"
    );
}

#[test]
fn eval_inline_dag_call_in_for_comp_with_loop_var() {
    // Motivating shape: inline call inside a `for` whose arg references the
    // loop variable via an indexed graph ref.
    let source = "\
pub index Region = { A, B };

dag id_len {
    param v: Length;
    pub node result: Length = @v;
}

param dist: Length[Region] = { Region#A: 1.0 m, Region#B: 2.0 m };
node distances: Length[Region] = for r: Region { @id_len(v: @dist[r])::result };
";
    let result = compile_and_eval(source).unwrap();
    // distances is indexed, look it up by cell.
    let distances_entry = result
        .nodes()
        .find(|(n, _)| n.to_string() == "distances")
        .expect("distances node")
        .1
        .as_ref()
        .expect("distances value");
    match distances_entry {
        graphcal_eval::eval::types::Value::Indexed { entries, .. } => {
            let mut seen: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
            for (variant, value) in entries {
                seen.insert(variant.to_string(), value.si_value().unwrap().get());
            }
            assert!((seen["A"] - 1.0).abs() < 1e-10);
            assert!((seen["B"] - 2.0).abs() < 1e-10);
        }
        other => panic!("expected Indexed, got {other:?}"),
    }
}

#[test]
fn eval_inline_dag_call_in_match_arm_fresh_instances_per_syntactic_site() {
    // Each arm has a distinct syntactic call site; the eval-time selection
    // picks one arm's value but every call site semantically is a fresh
    // instantiation. This test exercises the motivating for/match shape.
    let source = "\
pub index Source = { Primary, Secondary };
pub index Region = { A, B };

dag id_len {
    param v: Length;
    pub node result: Length = @v;
}

param dist_primary: Length[Region] = { Region#A: 1.0 m, Region#B: 2.0 m };
param dist_secondary: Length[Region] = { Region#A: 10.0 m, Region#B: 20.0 m };

node effective: Length[Source, Region] = for s: Source, r: Region {
    match s {
        Source#Primary   => @id_len(v: @dist_primary[r])::result,
        Source#Secondary => @id_len(v: @dist_secondary[r])::result,
    }
};
";
    let result = compile_and_eval(source).unwrap();
    let entry = result
        .nodes()
        .find(|(n, _)| n.to_string() == "effective")
        .expect("effective node")
        .1
        .as_ref()
        .expect("effective value");
    // Nested Indexed: outer Source, inner Region.
    let graphcal_eval::eval::types::Value::Indexed { entries: outer, .. } = entry else {
        panic!("expected Indexed, got {entry:?}");
    };
    let mut cells: std::collections::HashMap<(String, String), f64> =
        std::collections::HashMap::new();
    for (svar, sval) in outer {
        let graphcal_eval::eval::types::Value::Indexed { entries: inner, .. } = sval else {
            panic!("expected inner Indexed, got {sval:?}");
        };
        for (rvar, rval) in inner {
            cells.insert(
                (svar.to_string(), rvar.to_string()),
                rval.si_value().unwrap().get(),
            );
        }
    }
    assert!((cells[&("Primary".into(), "A".into())] - 1.0).abs() < 1e-10);
    assert!((cells[&("Primary".into(), "B".into())] - 2.0).abs() < 1e-10);
    assert!((cells[&("Secondary".into(), "A".into())] - 10.0).abs() < 1e-10);
    assert!((cells[&("Secondary".into(), "B".into())] - 20.0).abs() < 1e-10);
}

#[test]
fn eval_inline_dag_call_composition_fixture() {
    let source =
        include_str!("../../../../tests/fixtures/valid/inline_dag_call_composition/main.gcl");
    let result = compile_and_eval(source).unwrap();
    // ((3 * 2) + 1) m = 7 m
    let y = find_value(&result, "y");
    assert!((y - 7.0).abs() < 1e-10, "expected 7.0, got {y}");
}

#[test]
fn eval_inline_dag_call_in_for_fixture() {
    let source = include_str!("../../../../tests/fixtures/valid/inline_dag_call_in_for/main.gcl");
    let _result = compile_and_eval(source).unwrap();
}

#[test]
fn eval_inline_dag_call_in_match_fixture() {
    let source = include_str!("../../../../tests/fixtures/valid/inline_dag_call_in_match/main.gcl");
    let _result = compile_and_eval(source).unwrap();
}

#[test]
fn eval_inline_dag_call_forward_reference_within_body() {
    // MVP walked the dag body in source order, which made this fail at eval
    // because `b` was evaluated before `a` was bound. The compile-pipeline
    // refactor runs the body in topological order.
    let source = "\
dag forward {
    param v: Length;
    pub node b: Length = @a * 2.0;
    node a: Length = @v + 1.0 m;
}

param src: Length = 3.0 m;
node out: Length = @forward(v: @src)::b;
";
    let result = compile_and_eval(source).unwrap();
    let out = find_value(&result, "out");
    // (3 + 1) * 2 = 8
    assert!((out - 8.0).abs() < 1e-10, "expected 8.0, got {out}");
}

#[test]
fn eval_cross_file_inline_dag_nested_call_uses_canonical_target_with_same_leaf_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub dag helper {\n\
             pub node result: Dimensionless = 2.0;\n\
         }\n\
         pub dag wrapper {\n\
             pub node result: Dimensionless = @helper()::result + 10.0;\n\
         }\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub dag helper {\n\
             pub node result: Dimensionless = 100.0;\n\
         }\n\
         pub dag wrapper {\n\
             pub node result: Dimensionless = @helper()::result + 1000.0;\n\
         }\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import collide.a as a;\n\
         import collide.b as b;\n\
         dag helper {\n\
             pub node result: Dimensionless = 10000.0;\n\
         }\n\
         node out_a: Dimensionless = @a.wrapper()::result;\n\
         node out_b: Dimensionless = @b.wrapper()::result;\n\
         node out_local: Dimensionless = @helper()::result;\n\
         node total: Dimensionless = @out_a + @out_b + @out_local;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let out_a = find_value(&result, "out_a");
    let out_b = find_value(&result, "out_b");
    let out_local = find_value(&result, "out_local");
    let total = find_value(&result, "total");
    assert!((out_a - 12.0).abs() < 1e-10, "out_a = {out_a}");
    assert!((out_b - 1100.0).abs() < 1e-10, "out_b = {out_b}");
    assert!(
        (out_local - 10000.0).abs() < 1e-10,
        "out_local = {out_local}"
    );
    assert!((total - 11112.0).abs() < 1e-10, "total = {total}");
}

#[test]
fn eval_public_values_preserve_same_leaf_imported_index_owners() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub index Phase = { Burn, Coast };\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub index Phase = { Burn, Coast };\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series_a: Dimensionless[a::Phase] = for p: a::Phase { 1.0 };\n\
         node series_b: Dimensionless[b::Phase] = for p: b::Phase { 2.0 };\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();

    let indexed_owner = |name: &str| {
        let value = result
            .nodes()
            .find(|(n, _)| n.to_string() == name)
            .unwrap_or_else(|| panic!("value `{name}` not found"))
            .1
            .as_ref()
            .unwrap_or_else(|e| panic!("value `{name}` has error: {e}"));
        let Value::Indexed {
            index_name,
            entries,
            ..
        } = value
        else {
            panic!("expected indexed value for `{name}`, got {value:?}");
        };
        assert_eq!(index_name.display_name().to_string(), "Phase");
        assert_eq!(entries.len(), 2);
        index_name
            .declared_resolved()
            .cloned()
            .unwrap_or_else(|| panic!("expected declared index for `{name}`"))
    };

    assert_ne!(indexed_owner("series_a"), indexed_owner("series_b"));
}

#[test]
fn eval_inline_dag_call_indexed_output_projection() {
    // Projected output is itself indexed; the call site reads one cell.
    let source = "\
pub index Region = { A, B };

dag doubler {
    import input::{ index Region };

    param v: Length[Region];
    pub node result: Length[Region] = for r: Region { @v[r] * 2.0 };
}

param dist: Length[Region] = { Region#A: 1.0 m, Region#B: 3.0 m };
node out_a: Length = @doubler(v: @dist)::result[Region#A];
node out_b: Length = @doubler(v: @dist)::result[Region#B];
";
    let result = compile_and_eval(source).unwrap();
    let a = find_value(&result, "out_a");
    let b = find_value(&result, "out_b");
    assert!((a - 2.0).abs() < 1e-10, "expected 2.0, got {a}");
    assert!((b - 6.0).abs() < 1e-10, "expected 6.0, got {b}");
}

#[test]
fn product_and_rss_support_linear_algebra_ergonomics() {
    let source = include_str!("../../../../tests/fixtures/valid/linear_algebra_ergonomics.gcl");
    let result = compile_and_eval(source).unwrap();

    assert!((find_value(&result, "box_volume") - 24.0).abs() < 1e-12);
    assert!((find_value(&result, "budget_sigma") - 5.0).abs() < 1e-12);
    assert!((find_value(&result, "chained_product") - 720.0).abs() < 1e-12);
    assert!(result.assertions.iter().all(|assertion| matches!(
        assertion.outcome,
        graphcal_eval::eval::types::AssertResult::Pass
    )));
}
