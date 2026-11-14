use tower_lsp::lsp_types::{InlayHintLabel, Position};

use crate::analysis_pipeline::{
    AnalysisDegradation, dependency_artifact_identities, run_analysis, run_analysis_run,
    test_plugin_host,
};
use crate::symbol_identity::SymbolId;
use crate::symbol_identity::VisibleBinding;
use crate::{completion, goto_definition, inlay_hints};

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tower_lsp::lsp_types::{InlayHint, Url};

use crate::analysis::AnalysisResult;
use crate::analysis_pipeline::{
    AnalysisDemand, EvaluationDemand, OpenBuffer, ViewModelDemand, run_analysis_with_cancellation,
};
use crate::file_identity::{document_identity, file_identity};
use crate::workspace_revision::{
    AnalysisInputSnapshot, CurrentRevisions, DocumentIdentity, RevisionClock,
};
use graphcal_compiler::cancellation::CancellationToken;

use crate::analysis_pipeline::{untitled_uri, write_project};

fn imported_target_for_spelling<'a>(
    analysis: &'a AnalysisResult,
    spelling: &str,
) -> Option<&'a SymbolId> {
    analysis
        .imported_bindings
        .iter()
        .find(|binding| binding.spelling().to_string() == spelling)
        .map(VisibleBinding::target)
}

#[test]
fn parse_fallback_records_empty_symbol_table_degradation() {
    let uri = Url::parse("file:///degraded-symbols.gcl").unwrap();
    let run = run_analysis_run(
        &uri,
        "node broken: Dimensionless = @",
        &[],
        test_plugin_host(),
    );

    assert!(!run.analysis.buffer_parsed);
    let [degradation] = run.degradations.as_slice() else {
        panic!("expected one degradation, got {:?}", run.degradations);
    };
    assert!(matches!(
        degradation,
        AnalysisDegradation::EmptySymbolTable { .. }
    ));
    assert!(degradation.message(&uri).contains("fallback symbol table"));
}

#[test]
fn failed_resolver_rebuild_records_file_local_resolver_degradation() {
    let uri = Url::parse("file:///degraded-resolver.gcl").unwrap();
    let run = run_analysis_run(
        &uri,
        "node duplicate: Dimensionless = 1.0;\nnode duplicate: Dimensionless = 2.0;",
        &[],
        test_plugin_host(),
    );

    assert!(run.analysis.buffer_parsed);
    let [degradation] = run.degradations.as_slice() else {
        panic!("expected one degradation, got {:?}", run.degradations);
    };
    assert!(matches!(
        degradation,
        AnalysisDegradation::FileLocalModuleResolver { .. }
    ));
    assert!(
        degradation
            .message(&uri)
            .contains("file-local module resolver")
    );
}

#[test]
fn diagnostic_ranges_use_lf_cr_and_crlf_lines_end_to_end() {
    let uri = untitled_uri();
    for line_ending in ["\n", "\r", "\r\n"] {
        let source = format!("node x: Dimensionless = 1.0;{line_ending}?");
        let analysis = run_analysis(&uri, &source, &[], test_plugin_host());
        let diagnostics = analysis
            .diagnostics
            .get(&uri)
            .expect("active document diagnostics must be present");
        let diagnostic = diagnostics
            .first()
            .unwrap_or_else(|| panic!("expected diagnostic for {line_ending:?}"));
        assert_eq!(
            diagnostic.range.start,
            Position::new(1, 0),
            "diagnostic position for {line_ending:?}"
        );
    }
}

#[test]
fn unfinished_markers_are_informational_in_executables_and_libraries() {
    let uri = untitled_uri();
    for prefix in ["", "param required: Length;"] {
        let source = format!(
            "{prefix} node missing: Length = todo<Length> {{}}; node blocked: Length = @missing;"
        );
        let analysis = run_analysis(&uri, &source, &[], test_plugin_host());
        let diagnostics = &analysis.diagnostics[&uri];
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(
            diagnostics[0].severity,
            Some(tower_lsp::lsp_types::DiagnosticSeverity::INFORMATION)
        );
        assert!(diagnostics[0].message.contains("TODO"));
        if prefix.is_empty() {
            assert!(
                analysis
                    .eval_values
                    .values()
                    .any(|value| value.contains("BLOCKED"))
            );
        }
    }
    let analysis = run_analysis(
        &uri,
        "dag unused { node missing: Length = todo<Length> {}; }",
        &[],
        test_plugin_host(),
    );
    assert_eq!(analysis.diagnostics[&uri].len(), 1);
}

#[test]
fn unfinished_constants_are_marked_and_their_readers_shown_blocked() {
    let uri = untitled_uri();
    let analysis = run_analysis(
        &uri,
        "const node missing: Length = todo<Length> {}; const node doubled: Length = 2.0 * @missing; node blocked: Length = @doubled;",
        &[],
        test_plugin_host(),
    );
    let diagnostics = &analysis.diagnostics[&uri];
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(
        diagnostics[0].severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::INFORMATION)
    );
    assert!(diagnostics[0].message.starts_with("missing: TODO"));
    let values = analysis.eval_values.values().collect::<Vec<_>>();
    assert!(
        values.iter().any(|value| value.contains("TODO")),
        "{values:?}"
    );
    assert_eq!(
        values
            .iter()
            .filter(|value| value.contains("BLOCKED"))
            .count(),
        2,
        "{values:?}"
    );
}

#[test]
fn library_file_with_required_index_has_no_diagnostics() {
    let text = "\
pub(bind) dim Speed = Length / Time;
pub(bind) dim CustomAcceleration = Length / Time^2;

pub(bind) index Phase;
pub(bind) index Step: Time;
pub(bind) index Accel: Length / Time^2;
";
    let analysis = run_analysis(&untitled_uri(), text, &[], test_plugin_host());
    assert!(
        analysis.has_no_diagnostics(),
        "library file should have no diagnostics, got: {:?}",
        analysis.diagnostics
    );
}

#[test]
fn library_file_with_required_param_has_no_diagnostics() {
    let text = "\
param mass: Mass;
";
    let analysis = run_analysis(&untitled_uri(), text, &[], test_plugin_host());
    assert!(
        analysis.has_no_diagnostics(),
        "file with required param should have no diagnostics, got: {:?}",
        analysis.diagnostics
    );
}

#[test]
fn library_file_with_required_type_port_has_no_diagnostics() {
    // TIR-2 (#2265): a root declaration mentioning a required type port
    // used to report an internal error (X001).
    let text = "\
pub(bind) type T;
pub type Box { Box(v: T) }
param b: T;
param boxed: Box;
";
    let analysis = run_analysis(&untitled_uri(), text, &[], test_plugin_host());
    assert!(
        analysis.has_no_diagnostics(),
        "library file should have no diagnostics, got: {:?}",
        analysis.diagnostics
    );
}

#[test]
fn executable_file_with_dim_mismatch_still_reports_diagnostic() {
    // Not a library (no required params/indexes) — real dim errors must still
    // surface, i.e., the library bypass must not swallow genuine diagnostics.
    let text = "\
param mass: Mass = 1.0 kg;
param length: Length = 1.0 m;
node bad: Mass = mass + length;
";
    let analysis = run_analysis(&untitled_uri(), text, &[], test_plugin_host());
    assert!(
        !analysis.has_no_diagnostics(),
        "dim mismatch in executable file must still produce a diagnostic",
    );
}

#[test]
fn non_integer_const_to_int_reports_diagnostic() {
    let uri = untitled_uri();
    let text = "const node invalid: Int = to_int(3.7);\n";
    let analysis = run_analysis(&uri, text, &[], test_plugin_host());
    let diagnostics = analysis
        .diagnostics
        .get(&uri)
        .expect("the active URI should always have a diagnostic entry");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("is not integer-valued")),
        "expected exact-conversion diagnostic, got: {diagnostics:?}",
    );
}

#[test]
fn imported_definitions_collected_for_selective_includes() {
    let dir = write_project(&[
        ("graphcal.toml", "[package]\nname = \"lib\"\n"),
        (
            "src/lib/lib.gcl",
            "param limit: Dimensionless = 100.0;\npub node doubled: Dimensionless = @limit * 2.0;\n",
        ),
        (
            "src/lib/main.gcl",
            "include lib.lib()::{ doubled };\nnode result: Dimensionless = @doubled + 1.0;\n",
        ),
    ]);
    let main_path = dir.path().join("src/lib/main.gcl");
    let uri = Url::from_file_path(&main_path).unwrap();
    let text = std::fs::read_to_string(&main_path).unwrap();
    let analysis = run_analysis(&uri, &text, &[], test_plugin_host());
    assert!(
        analysis.has_no_diagnostics(),
        "expected clean analysis, got diagnostics: {:?}",
        analysis.diagnostics,
    );

    let doubled_key = imported_target_for_spelling(&analysis, "doubled")
        .expect("expected a visible binding for selective include `doubled`");
    assert!(
        analysis.imported_definitions.contains_key(doubled_key),
        "expected imported definition for selective include `doubled`, got: {:?}",
        analysis.imported_definitions.keys().collect::<Vec<_>>(),
    );
}

#[test]
fn selective_includes_from_same_leaf_modules_have_no_diagnostics() {
    let dir = write_project(&[
        ("graphcal.toml", "[package]\nname = \"app\"\n"),
        (
            "src/app/analysis/shared.gcl",
            "param input: Dimensionless;\npub node output: Dimensionless = @input + 1.0;\n",
        ),
        (
            "src/app/presentation/shared.gcl",
            "param input: Dimensionless;\npub node output: Dimensionless = @input * 2.0;\n",
        ),
        (
            "src/app/main.gcl",
            "include app.analysis.shared(input: 2.0)::{ output as analyzed };\n\
                 include app.presentation.shared(input: 5.0)::{ output as rendered };\n\
                 node combined: Dimensionless = @analyzed + @rendered;\n",
        ),
    ]);
    let main_path = dir.path().join("src/app/main.gcl");
    let uri = Url::from_file_path(&main_path).unwrap();
    let text = std::fs::read_to_string(&main_path).unwrap();
    let analysis = run_analysis(&uri, &text, &[], test_plugin_host());

    assert!(
        analysis.has_no_diagnostics(),
        "expected clean analysis, got diagnostics: {:?}",
        analysis.diagnostics,
    );
}

#[test]
fn same_leaf_imported_types_resolve_to_their_canonical_defining_files() {
    use crate::resolve::{SymbolLocation, resolve_symbol_at};

    let dir = write_project(&[
        ("graphcal.toml", "[package]\nname = \"app\"\n"),
        (
            "src/app/left.gcl",
            "pub base dim Measure;\npub base unit u: Measure;\n",
        ),
        (
            "src/app/right.gcl",
            "pub base dim Measure;\npub base unit u: Measure;\n",
        ),
        (
            "src/app/main.gcl",
            "import app.left as left;\n\
                 import app.right as right;\n\
                 node left_value: left::Measure = 1.0 left::u;\n\
                 node right_value: right::Measure = 2.0 right::u;\n",
        ),
    ]);
    let main_path = dir.path().join("src/app/main.gcl");
    let left_uri =
        Url::from_file_path(dir.path().join("src/app/left.gcl").canonicalize().unwrap()).unwrap();
    let right_uri =
        Url::from_file_path(dir.path().join("src/app/right.gcl").canonicalize().unwrap()).unwrap();
    let main_uri = Url::from_file_path(&main_path).unwrap();
    let text = std::fs::read_to_string(&main_path).unwrap();
    let analysis = run_analysis(&main_uri, &text, &[], test_plugin_host());
    assert!(
        analysis.has_no_diagnostics(),
        "expected clean canonical-owner analysis, got diagnostics: {:?}",
        analysis.diagnostics,
    );

    for (spelling, expected_uri) in [("left::Measure", left_uri), ("right::Measure", right_uri)] {
        let offset = text.find(spelling).unwrap() + spelling.find("::").unwrap() + 3;
        let resolved =
            resolve_symbol_at(&analysis, offset).unwrap_or_else(|| panic!("resolve `{spelling}`"));
        let SymbolLocation::Imported(imported) = resolved.location else {
            panic!("expected `{spelling}` to resolve to an imported definition");
        };
        assert_eq!(imported.uri, expected_uri);
    }
}

#[test]
fn parse_error_in_imported_file_routes_to_imported_uri() {
    // lib.gcl has a syntax error; main.gcl is fine. The diagnostic must
    // surface on lib.gcl's URI, not main.gcl's URI.
    let dir = write_project(&[
        ("graphcal.toml", "[package]\nname = \"helper\"\n"),
        ("src/helper/lib.gcl", "this is not valid graphcal"),
        (
            "src/helper/main.gcl",
            "import helper.lib::{y};\nnode z: Dimensionless = 1.0;",
        ),
    ]);
    let main_path = dir.path().join("src/helper/main.gcl");
    let helper_path = dir.path().join("src/helper/lib.gcl");
    // The analyzed document keeps the URI the editor opened it under (on
    // macOS a temporary directory is a `/var/folders/...` symlink). The
    // imported file is not open, so its diagnostic is published under its
    // canonical path, as the loader identifies it.
    let main_uri = Url::from_file_path(&main_path).unwrap();
    let helper_uri = Url::from_file_path(helper_path.canonicalize().unwrap()).unwrap();
    let text = std::fs::read_to_string(&main_path).unwrap();
    let analysis = run_analysis(&main_uri, &text, &[], test_plugin_host());

    let helper_diags = analysis
        .diagnostics
        .get(&helper_uri)
        .cloned()
        .unwrap_or_default();
    let main_diags = analysis
        .diagnostics
        .get(&main_uri)
        .cloned()
        .unwrap_or_default();
    assert!(
        !helper_diags.is_empty(),
        "expected parse error in helper.gcl to surface on its own URI; full map: {:?}",
        analysis.diagnostics,
    );
    assert!(
        main_diags.is_empty(),
        "main.gcl should not carry the parse error from the imported file; got: {main_diags:?}",
    );
    // The diagnostic's range must come from helper.gcl's offsets indexed
    // against helper.gcl's text — not main.gcl's. helper.gcl's content is
    // "this is not valid graphcal" (one line); the parse error fires on
    // the first token, so the range must start at line 0 with a small
    // column. If the bug regressed (offsets indexed against main.gcl),
    // either the column would diverge or the start position would be off
    // entirely.
    let primary = &helper_diags[0];
    assert_eq!(primary.range.start.line, 0);
    assert!(
        primary.range.end.character <= u32::try_from("this".len()).unwrap(),
        "expected range bounded by the first token of helper.gcl, got: {:?}",
        primary.range,
    );
}

#[test]
fn parse_error_in_non_sibling_imported_file_routes_to_imported_uri() {
    // Regression guard for the structural-source/offset bug: when the
    // failing import is *not* a sibling of the active buffer (here, an
    // extra `util/` directory between main.gcl and lib.gcl), the
    // diagnostic must still land on lib.gcl's URI with offsets indexed
    // against lib.gcl's text. The pre-fix code's sibling resolver would
    // fail to find lib.gcl from main.gcl's directory, leak the error
    // onto main.gcl's URI, and clamp the offset against main.gcl's line
    // index — producing a misleading squiggle on innocent code.
    let dir = write_project(&[
        ("graphcal.toml", "[package]\nname = \"helper\"\n"),
        ("src/helper/util/lib.gcl", "this is not valid graphcal"),
        (
            "src/helper/main.gcl",
            "import helper.util.lib::{y};\nnode z: Dimensionless = 1.0;",
        ),
    ]);
    let main_path = dir.path().join("src/helper/main.gcl");
    let lib_path = dir.path().join("src/helper/util/lib.gcl");
    let main_uri = Url::from_file_path(main_path.canonicalize().unwrap()).unwrap();
    let lib_uri = Url::from_file_path(lib_path.canonicalize().unwrap()).unwrap();
    let text = std::fs::read_to_string(&main_path).unwrap();
    let analysis = run_analysis(&main_uri, &text, &[], test_plugin_host());

    let lib_diags = analysis
        .diagnostics
        .get(&lib_uri)
        .cloned()
        .unwrap_or_default();
    let main_diags = analysis
        .diagnostics
        .get(&main_uri)
        .cloned()
        .unwrap_or_default();
    assert!(
        !lib_diags.is_empty(),
        "expected parse error in util/lib.gcl to surface on its own URI even when not a sibling of main.gcl; full map: {:?}",
        analysis.diagnostics,
    );
    assert!(
        main_diags.is_empty(),
        "main.gcl must not carry the parse error from a non-sibling import; got: {main_diags:?}",
    );
}

#[test]
fn imported_definitions_keyed_by_local_alias_for_selective_imports() {
    let dir = write_project(&[
        ("graphcal.toml", "[package]\nname = \"helper\"\n"),
        (
            "src/helper/lib.gcl",
            "pub const node y: Dimensionless = 2.0;",
        ),
        (
            "src/helper/main.gcl",
            "import helper.lib::{y as renamed};\nnode z: Dimensionless = @renamed + 1.0;",
        ),
    ]);
    let main_path = dir.path().join("src/helper/main.gcl");
    let uri = Url::from_file_path(&main_path).unwrap();
    let text = std::fs::read_to_string(&main_path).unwrap();
    let analysis = run_analysis(&uri, &text, &[], test_plugin_host());
    assert!(
        analysis.has_no_diagnostics(),
        "expected clean analysis, got diagnostics: {:?}",
        analysis.diagnostics,
    );

    let renamed_key = imported_target_for_spelling(&analysis, "renamed")
        .expect("expected imported binding under local alias `renamed`");
    assert!(
        analysis.imported_definitions.contains_key(renamed_key),
        "expected canonical imported definition for local alias `renamed`, got: {:?}",
        analysis.imported_definitions.keys().collect::<Vec<_>>(),
    );

    let cursor = text.find("@renamed").unwrap() + 1;
    let resolved = crate::resolve::resolve_symbol_at(&analysis, cursor)
        .expect("selective alias reference should resolve");
    assert_eq!(&resolved.key, renamed_key);
    assert!(matches!(
        resolved.location,
        crate::resolve::SymbolLocation::Imported(_)
    ));
}

#[test]
fn multiple_aliases_preserve_each_visible_binding_for_one_identity() {
    let dir = write_project(&[
        ("graphcal.toml", "[package]\nname = \"helper\"\n"),
        (
            "src/helper/lib.gcl",
            "pub const node y: Dimensionless = 2.0;",
        ),
        (
            "src/helper/main.gcl",
            "import helper.lib::{y as first};\n\
                 import helper.lib::{y as other};\n\
                 node z: Dimensionless = @first + @other;",
        ),
    ]);
    let main_path = dir.path().join("src/helper/main.gcl");
    let uri = Url::from_file_path(&main_path).unwrap();
    let text = std::fs::read_to_string(&main_path).unwrap();
    let analysis = run_analysis(&uri, &text, &[], test_plugin_host());
    assert!(analysis.has_no_diagnostics(), "{:?}", analysis.diagnostics);

    let first = imported_target_for_spelling(&analysis, "first").unwrap();
    let other = imported_target_for_spelling(&analysis, "other").unwrap();
    assert_eq!(first, other);
    for spelling in ["@first", "@other"] {
        let offset = text.find(spelling).unwrap() + 1;
        assert_eq!(
            crate::resolve::resolve_symbol_at(&analysis, offset).map(|resolved| resolved.key),
            Some(first.clone())
        );
    }
}

/// A selective-imported alias should also bring across the variants of the
/// underlying index, keyed under the local alias.
#[test]
fn selectively_imported_index_brings_variants_under_local_alias() {
    let dir = write_project(&[
        ("graphcal.toml", "[package]\nname = \"lib\"\n"),
        (
            "src/lib/lib.gcl",
            "pub index Color = { Red, Green, Blue };\n\
                 param dummy: Dimensionless = 0.0;\n",
        ),
        (
            "src/lib/main.gcl",
            "import lib.lib::{ index Color as Palette };\n\
                 param favorite: Dimensionless[Palette] = {\n    \
                     Palette#Red: 1.0,\n    \
                     Palette#Green: 2.0,\n    \
                     Palette#Blue: 3.0,\n\
                 };\n",
        ),
    ]);
    let main_path = dir.path().join("src/lib/main.gcl");
    let uri = Url::from_file_path(&main_path).unwrap();
    let text = std::fs::read_to_string(&main_path).unwrap();
    let analysis = run_analysis(&uri, &text, &[], test_plugin_host());

    let palette_red_key = imported_target_for_spelling(&analysis, "Palette#Red")
        .expect("expected imported binding under local alias `Palette#Red`");
    assert!(
        analysis.imported_definitions.contains_key(palette_red_key),
        "expected imported variant under local alias `Palette#Red`, got: {:?}",
        analysis.imported_definitions.keys().collect::<Vec<_>>(),
    );
}

/// Issue #830: goto-definition and hover must resolve symbols brought in
/// by module imports — bare (`import gotom.lib;` → `@lib::g0`) and aliased
/// (`import gotom.lib as alias_lib;` → `@alias_lib::g0`).
#[test]
fn module_imported_symbols_resolve_for_goto_and_hover() {
    use crate::resolve::{SymbolLocation, resolve_symbol_at};

    let dir = write_project(&[
        ("graphcal.toml", "[package]\nname = \"gotom\"\n"),
        (
            "src/gotom/lib.gcl",
            "pub const node g0: Acceleration = 9.80665 m/s^2;\n",
        ),
        (
            "src/gotom/main.gcl",
            "import gotom.lib;\n\
                 import gotom.lib as alias_lib;\n\
                 node g: Acceleration = @lib::g0;\n\
                 node g2: Acceleration = @alias_lib::g0;\n",
        ),
    ]);
    let main_path = dir.path().join("src/gotom/main.gcl");
    let lib_path = dir.path().join("src/gotom/lib.gcl");
    let main_uri = Url::from_file_path(&main_path).unwrap();
    let lib_uri = Url::from_file_path(lib_path.canonicalize().unwrap()).unwrap();
    let text = std::fs::read_to_string(&main_path).unwrap();
    let analysis = run_analysis(&main_uri, &text, &[], test_plugin_host());
    assert!(
        analysis.has_no_diagnostics(),
        "expected clean analysis, got diagnostics: {:?}",
        analysis.diagnostics,
    );

    for (needle, prefix) in [("@lib::g0", "@lib::"), ("@alias_lib::g0", "@alias_lib::")] {
        let offset = text.find(needle).expect("reference in main.gcl") + prefix.len();
        let resolved = resolve_symbol_at(&analysis, offset)
            .unwrap_or_else(|| panic!("expected `{needle}` to resolve"));
        let SymbolLocation::Imported(imported) = resolved.location else {
            panic!("expected `{needle}` to resolve to an imported definition");
        };
        assert_eq!(imported.uri, lib_uri, "`{needle}` should jump to lib.gcl");
    }
}

/// Issue #833: analysis must overlay the latest text of *all* open
/// documents, not just the analyzed one — otherwise an importer's
/// analysis sees stale disk content of an open-but-unsaved imported file
/// and publishes phantom diagnostics on the imported file's URI.
#[test]
fn analysis_overlays_other_open_documents() {
    let dir = write_project(&[
        ("graphcal.toml", "[package]\nname = \"staleo\"\n"),
        (
            // Broken on disk; the open editor buffer has the fix.
            "src/staleo/lib.gcl",
            "pub const node g0: Acceleration = @broken_ref;\n",
        ),
        (
            "src/staleo/main.gcl",
            "import staleo.lib;\nnode x: Acceleration = @lib::g0;\n",
        ),
    ]);
    let main_path = dir.path().join("src/staleo/main.gcl");
    let lib_path = dir.path().join("src/staleo/lib.gcl");
    let main_uri = Url::from_file_path(&main_path).unwrap();
    let text = std::fs::read_to_string(&main_path).unwrap();

    // Control: without the overlay, the stale disk content produces
    // diagnostics (on lib.gcl's URI).
    let stale = run_analysis(&main_uri, &text, &[], test_plugin_host());
    assert!(
        !stale.has_no_diagnostics(),
        "expected diagnostics from the broken on-disk lib.gcl"
    );

    let open_buffers = vec![
        OpenBuffer::new(
            Url::from_file_path(&lib_path).unwrap(),
            Arc::new("pub const node g0: Acceleration = 9.80665 m/s^2;\n".to_string()),
        )
        .unwrap(),
    ];
    let analysis = run_analysis(&main_uri, &text, &open_buffers, test_plugin_host());
    assert!(
        analysis.has_no_diagnostics(),
        "the fixed editor buffer must shadow the broken disk content, got: {:?}",
        analysis.diagnostics,
    );
}

#[test]
fn analysis_ignores_open_buffers_outside_the_active_root_capability() {
    let project = write_project(&[
        ("graphcal.toml", "[package]\nname = \"active\"\n"),
        ("src/active/main.gcl", "node x: Dimensionless = 1.0;\n"),
    ]);
    let unrelated = write_project(&[
        ("graphcal.toml", "[package]\nname = \"other\"\n"),
        ("src/other/main.gcl", "node outside: Dimensionless = 2.0;\n"),
    ]);
    let main_path = project.path().join("src/active/main.gcl");
    let main_uri = Url::from_file_path(&main_path).unwrap();
    let text = std::fs::read_to_string(&main_path).unwrap();
    let open_buffers = vec![
        OpenBuffer::new(
            Url::from_file_path(unrelated.path().join("src/other/main.gcl")).unwrap(),
            Arc::new("this unrelated buffer is mid-edit }{".to_string()),
        )
        .unwrap(),
    ];

    let analysis = run_analysis(&main_uri, &text, &open_buffers, test_plugin_host());

    assert!(
        analysis.has_no_diagnostics(),
        "an unrelated open buffer must not widen or invalidate the active project: {:?}",
        analysis.diagnostics
    );
}

#[test]
fn failed_load_tracks_missing_import_and_manifest_inputs() {
    let dir = write_project(&[
        ("graphcal.toml", "[package]\nname = \"missingdep\"\n"),
        (
            "src/missingdep/main.gcl",
            "import missingdep.lib::{value};\nnode result: Dimensionless = @value;\n",
        ),
    ]);
    let main_path = dir.path().join("src/missingdep/main.gcl");
    let main_uri = Url::from_file_path(&main_path).unwrap();
    let source = std::fs::read_to_string(&main_path).unwrap();
    let analysis = run_analysis(&main_uri, &source, &[], test_plugin_host());
    let dependencies: HashSet<_> = analysis.inputs.dependencies().cloned().collect();

    assert!(!analysis.has_no_diagnostics());
    assert!(!analysis.inputs.has_complete_dependencies());
    assert!(dependencies.contains(&file_identity(&dir.path().join("src/missingdep/lib.gcl"))));
    assert!(dependencies.contains(&file_identity(&dir.path().join("graphcal.toml"))));
}

#[test]
fn dependency_watch_inputs_include_authenticated_out_of_source_plugins() {
    use graphcal_io::{InMemoryFileSystem, SourceTreeHashLimits, VirtualAbsolutePath};
    let root = std::path::Path::new("/dependency");
    let mut fs = InMemoryFileSystem::new();
    for (path, text) in [
        ("graphcal.toml", "manifest"),
        ("src/library.gcl", "source"),
        ("plugins/kernel.wasm", "binary"),
    ] {
        fs.add_file(
            VirtualAbsolutePath::new(root.join(path)).unwrap(),
            text.to_string(),
        )
        .unwrap();
    }
    let mut snapshot = graphcal_io::capture_source_tree(
        &fs,
        root,
        std::path::Path::new("src"),
        SourceTreeHashLimits::unbounded(),
        &graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
    .unwrap();
    snapshot
        .capture_artifact(
            &fs,
            root,
            std::path::Path::new("plugins/kernel.wasm"),
            SourceTreeHashLimits::unbounded(),
            &graphcal_compiler::cancellation::CancellationToken::unbounded(),
        )
        .unwrap();
    let closure = graphcal_project::loader::LoadedPackageClosure {
        lockfile: String::new(),
        dependencies: std::collections::BTreeMap::from([(
            graphcal_package::PackageInstanceId::new("dependency").unwrap(),
            graphcal_project::loader::LoadedDependency {
                root: root.to_path_buf(),
                snapshot,
            },
        )]),
    };
    let inputs: HashSet<_> = dependency_artifact_identities(&closure).collect();
    for path in ["graphcal.toml", "src/library.gcl", "plugins/kernel.wasm"] {
        assert!(inputs.contains(&DocumentIdentity::file(root.join(path))));
    }
}

#[test]
fn analysis_tracks_manifest_lockfile_and_plugin_artifacts() {
    let dir = write_project(&[
        ("graphcal.toml", "[package]\nname = \"plugininputs\"\n"),
        (
            "src/plugininputs/main.gcl",
            "import plugin \"plugin.wasm\" as plugin {\nfn value() -> Dimensionless;\n}\n",
        ),
        ("plugin.wasm", "not a wasm module"),
    ]);
    let main_path = dir.path().join("src/plugininputs/main.gcl");
    let main_uri = Url::from_file_path(&main_path).unwrap();
    let source = std::fs::read_to_string(&main_path).unwrap();
    let analysis = run_analysis(&main_uri, &source, &[], test_plugin_host());
    let dependencies: HashSet<_> = analysis.inputs.dependencies().cloned().collect();

    for input in ["graphcal.toml", "graphcal.lock", "plugin.wasm"] {
        assert!(
            dependencies.contains(&file_identity(&dir.path().join(input))),
            "missing tracked filesystem input {input}: {dependencies:?}"
        );
    }
}

#[test]
fn dependency_revision_change_invalidates_importer_analysis() {
    let dir = write_project(&[
        ("graphcal.toml", "[package]\nname = \"fresh\"\n"),
        (
            "src/fresh/lib.gcl",
            "pub const node y: Dimensionless = 2.0;\n",
        ),
        (
            "src/fresh/main.gcl",
            "import fresh.lib::{y};\nnode z: Dimensionless = @y + 1.0;\n",
        ),
    ]);
    let main_path = dir.path().join("src/fresh/main.gcl");
    let lib_path = dir.path().join("src/fresh/lib.gcl");
    let main_uri = Url::from_file_path(&main_path).unwrap();
    let main_text = std::fs::read_to_string(&main_path).unwrap();
    let root = document_identity(&main_uri);
    let dependency = DocumentIdentity::file(lib_path.canonicalize().unwrap());
    let clock = RevisionClock::default();
    let root_revision = clock.next().unwrap();
    let old_dependency_revision = clock.next().unwrap();
    let input = AnalysisInputSnapshot::new(
        root.clone(),
        root_revision,
        HashMap::from([(dependency.clone(), old_dependency_revision)]),
    );
    let analyze = |overlay: &[OpenBuffer], input: &AnalysisInputSnapshot| {
        let unbounded = CancellationToken::unbounded();
        let demand = AnalysisDemand {
            view: ViewModelDemand::Skip,
            evaluation: EvaluationDemand::Evaluate,
        };
        let host = test_plugin_host();
        run_analysis_with_cancellation(
            &main_uri, &main_text, overlay, host, input, demand, &unbounded,
        )
        .unwrap()
        .analysis
    };
    let overlay = vec![
        OpenBuffer::new(
            Url::from_file_path(&lib_path).unwrap(),
            Arc::new("pub const node y: Dimensionless = 2.0;\n".to_string()),
        )
        .unwrap(),
    ];
    let analysis = analyze(&overlay, &input);
    assert!(analysis.has_no_diagnostics());
    assert!(
        analysis
            .inputs
            .dependencies()
            .any(|identity| identity == &dependency)
    );
    let disk = HashMap::from([(dependency.clone(), old_dependency_revision)]);
    let current = CurrentRevisions::new(&disk, [(&root, root_revision)]);
    assert!(analysis.inputs.is_current(&current));
    let new_dependency_revision = clock.next().unwrap();
    let disk = HashMap::from([(dependency.clone(), new_dependency_revision)]);
    let current = CurrentRevisions::new(&disk, [(&root, root_revision)]);
    assert!(!analysis.inputs.is_current(&current));
    let refreshed_input = AnalysisInputSnapshot::new(
        root,
        root_revision,
        HashMap::from([(dependency, new_dependency_revision)]),
    );
    let refreshed_overlay = vec![
        OpenBuffer::new(
            Url::from_file_path(dir.path().join("src/fresh/lib.gcl")).unwrap(),
            Arc::new("pub const node q: Dimensionless = 4.0;\n".to_string()),
        )
        .unwrap(),
    ];
    let refreshed = analyze(&refreshed_overlay, &refreshed_input);
    assert!(!refreshed.has_no_diagnostics());
    let use_offset = main_text.rfind('y').unwrap();
    assert!(goto_definition::goto_definition(&analysis, &main_uri, use_offset).is_some());
    assert!(goto_definition::goto_definition(&refreshed, &main_uri, use_offset).is_none());
    let old_labels: HashSet<_> = completion::completion(&analysis, &main_text, use_offset)
        .unwrap()
        .into_iter()
        .map(|item| item.label)
        .collect();
    let new_labels: HashSet<_> = completion::completion(&refreshed, &main_text, use_offset)
        .unwrap()
        .into_iter()
        .map(|item| item.label)
        .collect();
    assert!(old_labels.contains("y"));
    assert!(!new_labels.contains("y"));

    let range = tower_lsp::lsp_types::Range {
        start: Position::new(0, 0),
        end: Position::new(u32::MAX, u32::MAX),
    };
    let old_hints = inlay_hints::inlay_hints(&analysis, range).unwrap();
    let new_hints = inlay_hints::inlay_hints(&refreshed, range).unwrap_or_default();
    let is_old_value =
        |hint: &InlayHint| matches!(&hint.label, InlayHintLabel::String(label) if label == " = 3");
    assert!(old_hints.iter().any(is_old_value));
    assert!(!new_hints.iter().any(is_old_value));
}

/// Issue #831: imported symbols hover with the same type fidelity as
/// local ones — the project TIR is already computed; the type must be
/// read off the dependency's own `DagTIR`, not the root's.
#[test]
fn imported_definitions_carry_resolved_types() {
    let dir = write_project(&[
        ("graphcal.toml", "[package]\nname = \"gotom\"\n"),
        (
            "src/gotom/lib.gcl",
            "pub const node g0: Acceleration = 9.80665 m/s^2;\n",
        ),
        (
            "src/gotom/main.gcl",
            "import gotom.lib::{g0};\n\
                 import gotom.lib as m;\n\
                 node g: Acceleration = @g0;\n\
                 node g2: Acceleration = @m::g0;\n",
        ),
    ]);
    let main_path = dir.path().join("src/gotom/main.gcl");
    let main_uri = Url::from_file_path(&main_path).unwrap();
    let text = std::fs::read_to_string(&main_path).unwrap();
    let analysis = run_analysis(&main_uri, &text, &[], test_plugin_host());
    assert!(
        analysis.has_no_diagnostics(),
        "expected clean analysis, got diagnostics: {:?}",
        analysis.diagnostics,
    );

    for spelling in ["g0", "m::g0"] {
        let key = imported_target_for_spelling(&analysis, spelling)
            .unwrap_or_else(|| panic!("expected imported binding for {spelling}"));
        let imported = analysis
            .imported_definitions
            .get(key)
            .unwrap_or_else(|| panic!("expected imported definition for {key:?}"));
        let type_description = imported
            .definition
            .type_description
            .as_deref()
            .unwrap_or_else(|| panic!("expected a type description for {key:?}"));
        assert!(
            type_description.contains("Acceleration"),
            "expected resolved type for {key:?}, got `{type_description}`"
        );
    }
}
