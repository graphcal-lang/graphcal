use crate::analysis_pipeline::{run_analysis, test_plugin_host};

use tower_lsp::lsp_types::Url;

use crate::analysis_pipeline::write_project;

/// Issue #631 case 1+2: `@module.dag(args)::out` — both the DAG name and
/// the output projection must resolve to the imported library file.
#[test]
fn goto_definition_resolves_cross_file_inline_dag_call() {
    use crate::resolve::{SymbolLocation, resolve_symbol_at};

    let dir = write_project(&[
        ("graphcal.toml", "[package]\nname = \"lib\"\n"),
        (
            "src/lib/lib.gcl",
            "pub dag scale {\n    \
                     param factor: Dimensionless;\n    \
                     param v: Velocity;\n    \
                     pub node result: Velocity = @v * @factor;\n\
                 }\n",
        ),
        (
            "src/lib/main.gcl",
            "import lib.lib as lib;\n\
                 param speed: Velocity = 10.0 m/s;\n\
                 node doubled: Velocity = @lib.scale(factor: 2.0, v: @speed)::result;\n",
        ),
    ]);
    let main_path = dir.path().join("src/lib/main.gcl");
    let lib_path = dir.path().join("src/lib/lib.gcl");
    let main_uri = Url::from_file_path(&main_path).unwrap();
    let lib_uri = Url::from_file_path(lib_path.canonicalize().unwrap()).unwrap();
    let text = std::fs::read_to_string(&main_path).unwrap();
    let analysis = run_analysis(&main_uri, &text, &[], test_plugin_host());
    assert!(
        analysis.has_no_diagnostics(),
        "expected clean analysis, got diagnostics: {:?}",
        analysis.diagnostics,
    );

    // Cursor on `scale` inside `@lib.scale(...)`.
    let scale_offset = text.find("scale").expect("scale token in main.gcl");
    let scale_resolved = resolve_symbol_at(&analysis, scale_offset + 1).expect("resolve `scale`");
    let SymbolLocation::Imported(scale_imported) = scale_resolved.location else {
        panic!("expected `scale` to resolve to an imported definition");
    };
    assert_eq!(scale_imported.uri, lib_uri, "scale should jump to lib.gcl");

    // Cursor on `result` (the output projection).
    let result_offset = text.find(")::result").expect("`::result` projection") + 3;
    let result_resolved = resolve_symbol_at(&analysis, result_offset).expect("resolve `result`");
    let SymbolLocation::Imported(result_imported) = result_resolved.location else {
        panic!("expected `result` to resolve to an imported definition");
    };
    assert_eq!(
        result_imported.uri, lib_uri,
        "result should jump to lib.gcl"
    );
}

#[test]
fn goto_definition_resolves_direct_file_and_inline_dag_alias_calls() {
    use crate::resolve::{SymbolLocation, resolve_symbol_at};

    let dir = write_project(&[
        ("graphcal.toml", "[package]\nname = \"lib\"\n"),
        (
            "src/lib/library.gcl",
            "param x: Dimensionless;\n\
                 pub node doubled: Dimensionless = @x * 2.0;\n\
                 pub dag helper {\n    \
                     param x: Dimensionless;\n    \
                     pub node tripled: Dimensionless = @x * 3.0;\n\
                 }\n",
        ),
        (
            "src/lib/main.gcl",
            "import lib.library as file_dag;\n\
                 import lib.library.helper as inline_dag;\n\
                 import lib.library::{helper as selected_dag};\n\
                 node a: Dimensionless = @file_dag(x: 2.0)::doubled;\n\
                 node b: Dimensionless = @inline_dag(x: 3.0)::tripled;\n\
                 node c: Dimensionless = @selected_dag(x: 4.0)::tripled;\n",
        ),
    ]);
    let main_path = dir.path().join("src/lib/main.gcl");
    let library_path = dir.path().join("src/lib/library.gcl");
    let main_uri = Url::from_file_path(&main_path).unwrap();
    let library_uri = Url::from_file_path(library_path.canonicalize().unwrap()).unwrap();
    let text = std::fs::read_to_string(&main_path).unwrap();
    let analysis = run_analysis(&main_uri, &text, &[], test_plugin_host());
    assert!(
        analysis.has_no_diagnostics(),
        "expected clean analysis, got diagnostics: {:?}",
        analysis.diagnostics,
    );

    for token in ["doubled", "inline_dag", "selected_dag", "tripled"] {
        let offset = text.rfind(token).expect("call token in main.gcl");
        let resolved =
            resolve_symbol_at(&analysis, offset + 1).unwrap_or_else(|| panic!("resolve `{token}`"));
        let SymbolLocation::Imported(imported) = resolved.location else {
            panic!("expected `{token}` to resolve to an imported definition");
        };
        assert_eq!(
            imported.uri, library_uri,
            "{token} should jump to library.gcl"
        );
    }
}

/// Issue #631 case 3: variants of a selectively-imported index must
/// resolve back to the library file that declared them.
#[test]
fn goto_definition_resolves_variant_of_selectively_imported_index() {
    use crate::resolve::{SymbolLocation, resolve_symbol_at};

    let dir = write_project(&[
        ("graphcal.toml", "[package]\nname = \"lib\"\n"),
        (
            "src/lib/lib.gcl",
            "pub index Color = { Red, Green, Blue };\n\
                 param dummy: Dimensionless = 0.0;\n",
        ),
        (
            "src/lib/main.gcl",
            "import lib.lib::{ index Color };\n\
                 param favorite: Dimensionless[Color] = {\n    \
                     Color#Red: 1.0,\n    \
                     Color#Green: 2.0,\n    \
                     Color#Blue: 3.0,\n\
                 };\n",
        ),
    ]);
    let main_path = dir.path().join("src/lib/main.gcl");
    let lib_path = dir.path().join("src/lib/lib.gcl");
    let main_uri = Url::from_file_path(&main_path).unwrap();
    let lib_uri = Url::from_file_path(lib_path.canonicalize().unwrap()).unwrap();
    let text = std::fs::read_to_string(&main_path).unwrap();
    let analysis = run_analysis(&main_uri, &text, &[], test_plugin_host());
    assert!(
        analysis.has_no_diagnostics(),
        "expected clean analysis, got diagnostics: {:?}",
        analysis.diagnostics,
    );

    // Cursor on `Red` inside `Color#Red:`.
    let red_offset = text.find("Color#Red").expect("Color#Red token") + "Color#".len();
    let red_resolved = resolve_symbol_at(&analysis, red_offset + 1).expect("resolve `Red`");
    let SymbolLocation::Imported(red_imported) = red_resolved.location else {
        panic!("expected `Red` to resolve to an imported definition");
    };
    assert_eq!(red_imported.uri, lib_uri, "Red should jump to lib.gcl");
}

#[test]
fn goto_definition_resolves_qualified_table_axes_and_slice_labels() {
    use crate::resolve::{SymbolLocation, resolve_symbol_at};

    let dir = write_project(&[
        ("graphcal.toml", "[package]\nname = \"lib\"\n"),
        (
            "src/lib/axes.gcl",
            "pub index Scenario = { Nominal };\n\
                 pub index Row = { A };\n\
                 pub index Column = { X };\n",
        ),
        (
            "src/lib/main.gcl",
            "import lib.axes as axes;\n\
                 param cube: Dimensionless[axes::Scenario, axes::Row, axes::Column] =\n    \
                     table[axes::Scenario, axes::Row, axes::Column] {\n        \
                         [axes::Scenario#Nominal]\n        \
                         : X;\n        \
                         A: 1.0;\n    \
                     };\n",
        ),
    ]);
    let main_path = dir.path().join("src/lib/main.gcl");
    let axes_path = dir.path().join("src/lib/axes.gcl");
    let main_uri = Url::from_file_path(&main_path).unwrap();
    let axes_uri = Url::from_file_path(axes_path.canonicalize().unwrap()).unwrap();
    let text = std::fs::read_to_string(&main_path).unwrap();
    let analysis = run_analysis(&main_uri, &text, &[], test_plugin_host());
    assert!(
        analysis.has_no_diagnostics(),
        "expected clean analysis, got diagnostics: {:?}",
        analysis.diagnostics,
    );

    let table_axis = text.find("table[axes::Scenario").unwrap() + "table[axes::".len();
    let slice_axis = text.find("[axes::Scenario#Nominal]").unwrap() + "[axes::".len();
    let slice_variant = text.find("axes::Scenario#Nominal").unwrap() + "axes::Scenario#".len();
    let column_variant = text.find(": X;").unwrap() + 2;
    let row_variant = text.find("A: 1.0;").unwrap();

    for (offset, spelling) in [
        (table_axis, "Scenario"),
        (slice_axis, "Scenario"),
        (slice_variant, "Nominal"),
        (column_variant, "X"),
        (row_variant, "A"),
    ] {
        let resolved = resolve_symbol_at(&analysis, offset)
            .unwrap_or_else(|| panic!("resolve table spelling `{spelling}`"));
        let SymbolLocation::Imported(imported) = resolved.location else {
            panic!("expected `{spelling}` to resolve to an imported definition");
        };
        assert_eq!(imported.uri, axes_uri, "wrong owner for `{spelling}`");
    }
}

#[test]
fn goto_definition_uses_qualified_same_leaf_import_identity() {
    use crate::resolve::{SymbolLocation, resolve_symbol_at};

    let dir = write_project(&[
        ("graphcal.toml", "[package]\nname = \"proj\"\n"),
        (
            "src/proj/a.gcl",
            "pub index Phase = { Burn, Coast };\n\
                 pub type Item { Pick(distance: Dimensionless), Idle }\n\
                 pub const node bias: Dimensionless = 1.0;\n",
        ),
        (
            "src/proj/b.gcl",
            "pub index Phase = { Burn, Coast };\n\
                 pub type Item { Pick(distance: Dimensionless), Idle }\n\
                 pub const node bias: Dimensionless = 2.0;\n",
        ),
        (
            "src/proj/main.gcl",
            "import proj.a as a;\n\
                 import proj.b as b;\n\n\
                 const node from_a: Dimensionless = a::bias;\n\
                 node phase_score: Dimensionless[a::Phase] = for current_phase: a::Phase {\n\
                     match current_phase {\n\
                         a::Phase#Burn => a::bias,\n\
                         a::Phase#Coast => b::bias,\n\
                     }\n\
                 };\n\
                 node item: a::Item = a::Pick(distance: a::bias);\n",
        ),
    ]);
    let main_path = dir.path().join("src/proj/main.gcl");
    let a_path = dir.path().join("src/proj/a.gcl");
    let b_path = dir.path().join("src/proj/b.gcl");
    let main_uri = Url::from_file_path(&main_path).unwrap();
    let a_uri = Url::from_file_path(a_path.canonicalize().unwrap()).unwrap();
    let b_uri = Url::from_file_path(b_path.canonicalize().unwrap()).unwrap();
    let text = std::fs::read_to_string(&main_path).unwrap();
    let analysis = run_analysis(&main_uri, &text, &[], test_plugin_host());
    assert!(
        analysis.has_no_diagnostics(),
        "expected clean analysis, got diagnostics: {:?}",
        analysis.diagnostics,
    );

    for (needle, token_prefix) in [
        ("a::bias", "a::"),
        ("a::Phase]", "a::"),
        ("a::Phase#Burn", "a::Phase#"),
        ("a::Item", "a::"),
        ("a::Pick", "a::"),
    ] {
        let offset = text.find(needle).expect("token in main.gcl") + token_prefix.len();
        let resolved = resolve_symbol_at(&analysis, offset + 1)
            .unwrap_or_else(|| panic!("resolve `{needle}`"));
        let SymbolLocation::Imported(imported) = resolved.location else {
            panic!("expected `{needle}` to resolve to an imported definition");
        };
        assert_eq!(imported.uri, a_uri, "`{needle}` should jump to a.gcl");
        assert_ne!(imported.uri, b_uri, "`{needle}` must not jump to b.gcl");
    }
}
