//! Instantiated file includes, requested plots, and duplicate-name rules.

use super::*;

/// Helper: extract indexed entries as `Vec<(variant, si_value)>`.
fn indexed_si_values(value: &Value) -> Vec<(&str, f64)> {
    match value {
        Value::Indexed { entries, .. } => entries
            .iter()
            .map(|(k, v)| {
                (
                    k.as_named()
                        .expect("helper is only used with named indexes")
                        .as_str(),
                    v.si_value().unwrap().get(),
                )
            })
            .collect(),
        _ => panic!("expected indexed value, got {value:?}"),
    }
}

#[test]
fn project_instantiated_import_selective() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/multi/instantiated_import/src/rocket/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    // dry_mass overridden to 800 kg, fuel_mass default 2800 kg, isp default 320 s
    // delta_v = 320 * 9.80665 * ln((800 + 2800) / 800) = 3138.128 * ln(4.5)
    let expected_delta_v = 320.0 * 9.80665 * (3600.0_f64 / 800.0).ln();
    let result_val = find_value(&result, "result");
    assert!(
        (result_val - expected_delta_v).abs() < 0.01,
        "result = {result_val}, expected = {expected_delta_v}"
    );
}
#[test]
fn project_instantiated_import_graph_ref() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/multi/instantiated_import_graph_ref/src/rocket/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    // my_mass = 800 kg, passed as dry_mass binding via @my_mass
    // delta_v = 320 * 9.80665 * ln(3600/800)
    let expected_delta_v = 320.0 * 9.80665 * (3600.0_f64 / 800.0).ln();
    let result_val = find_value(&result, "result");
    assert!(
        (result_val - expected_delta_v).abs() < 0.01,
        "result = {result_val}, expected = {expected_delta_v}"
    );
}

fn write_nested_file_include_project(main_source: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let package_dir = dir.path().join("src/demo");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"demo\"\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("leaf.gcl"),
        "param x: Dimensionless;\n\
         node private_value: Dimensionless = @x + 1.0;\n\
         pub node doubled: Dimensionless = @x * 2.0;\n\
         pub assert positive = @doubled > 0.0;\n\
         pub plot chart = {\n\
             mark: line,\n\
             encode: { x: @x, y: @doubled },\n\
         };\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("middle.gcl"),
        "param x: Dimensionless;\n\
         include demo.leaf(x: @x) as leaf;\n\
         pub node out: Dimensionless = @leaf::doubled;\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("upper.gcl"),
        "param x: Dimensionless;\n\
         include demo.middle(x: @x) as middle;\n\
         pub node out: Dimensionless = @middle::out;\n",
    )
    .unwrap();
    let root = package_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

#[test]
fn nested_instantiated_file_include_evaluates_immediate_scope_binding() {
    let (_dir, root) = write_nested_file_include_project(
        "include demo.middle(x: 3.0) as middle;\n\
         pub node result: Dimensionless = @middle::out;\n",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!((find_value(&result, "result") - 6.0).abs() < f64::EPSILON);
}

fn write_package_root_child_include_project(
    main_source: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let package_dir = dir.path().join("src/app");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"app\"\nsource_dir = \"src\"\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("defaults.gcl"),
        "pub node output: Dimensionless = 2.0;\n",
    )
    .unwrap();
    let root = dir.path().join("src/app.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

#[test]
fn include_instance_can_share_its_display_path_with_a_source_module() {
    let (_dir, root) = write_package_root_child_include_project(
        "include app.defaults() as defaults;\n\
         pub node result: Dimensionless = @defaults::output;\n",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!((find_value(&result, "result") - 2.0).abs() < f64::EPSILON);
}

#[test]
fn aliased_include_does_not_bind_the_source_module_name() {
    let (_dir, root) = write_package_root_child_include_project(
        "include app.defaults() as configured;\n\
         pub node leaked: Dimensionless = @defaults()::output;\n",
    );

    let error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(
        matches!(
            &error,
            CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Module(kind @ ModuleError::ModuleResolution { .. }), .. }), .. })
                if kind.to_string() == "unknown module `app.defaults`"
        ),
        "unexpected alias-leak diagnostic: {error:?}"
    );
}

#[test]
fn nested_instantiated_file_include_keeps_multiple_instances_isolated() {
    let (_dir, root) = write_nested_file_include_project(
        "include demo.middle(x: 3.0) as first;\n\
         include demo.middle(x: 5.0) as second;\n\
         pub node result: Dimensionless = @first::out + @second::out;\n",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!((find_value(&result, "result") - 16.0).abs() < f64::EPSILON);
}

#[test]
fn nested_instantiated_file_include_resolves_aliased_sibling_outputs_in_bindings() {
    let dir = tempfile::tempdir().unwrap();
    let package_dir = dir.path().join("src/example");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"example\"\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("axes.gcl"),
        "pub index Item = { One, Two };\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("a.gcl"),
        "import example.axes::{ index Item };\n\
         param input: Dimensionless[Item];\n\
         pub node a_values: Dimensionless[Item] = @input;\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("b.gcl"),
        "import example.axes::{ index Item };\n\
         param input: Dimensionless[Item];\n\
         pub node b_values: Dimensionless[Item] =\n\
             for item: Item { @input[item] * 2.0 };\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("c.gcl"),
        "import example.axes::{ index Item };\n\
         param left: Dimensionless[Item];\n\
         param right: Dimensionless[Item];\n\
         pub node combined: Dimensionless[Item] =\n\
             for item: Item { @left[item] + @right[item] };\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("middle.gcl"),
        "import example.axes::{ index Item };\n\
         param input: Dimensionless[Item];\n\
         include example.a(input: @input) as a;\n\
         include example.b(input: @a::a_values) as b;\n\
         include example.c(left: @a::a_values, right: @b::b_values) as c;\n\
         pub node result: Dimensionless[Item] = @c::combined;\n",
    )
    .unwrap();
    let root = package_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import example.axes::{ index Item };\n\
         node input: Dimensionless[Item] = {\n\
             Item#One: 1.0,\n\
             Item#Two: 2.0,\n\
         };\n\
         include example.middle(input: @input)::{ pub result };\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert_eq!(
        indexed_si_values(&find_entry(&result, "result")),
        vec![("One", 3.0), ("Two", 6.0)]
    );
}

#[test]
fn nested_instantiated_file_include_reexports_requested_plot() {
    let (dir, root) = write_nested_file_include_project(
        "include demo.middle(x: 3.0)::{ out, chart };\n\
         pub node result: Dimensionless = @out;\n",
    );
    std::fs::write(
        dir.path().join("src/demo/middle.gcl"),
        "param x: Dimensionless;\n\
         include demo.leaf(x: @x)::{ doubled, pub chart };\n\
         pub node out: Dimensionless = @doubled;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!((find_value(&result, "result") - 6.0).abs() < f64::EPSILON);
    assert_eq!(result.plots.len(), 1);
    assert_eq!(result.plots[0].name.to_string(), "chart");
}

#[test]
fn requested_plot_specializes_its_required_index_axis() {
    let result = compile_and_eval(
        "index Axis = { One, Two };\n\
         node input: Dimensionless[Axis] = { Axis#One: 1.0, Axis#Two: 2.0 };\n\
         dag chart {\n\
             pub(bind) index Item;\n\
             param input: Dimensionless[Item];\n\
             pub plot output = {\n\
                 mark: line,\n\
                 encode: { x: @input, y: @input },\n\
             };\n\
         }\n\
         include chart(index Item: Axis, input: @input)::{ output };\n",
    )
    .unwrap();

    let plot = result.plots.first().expect("requested plot must render");
    assert_eq!(plot.name.to_string(), "output");
    let (_, values) = plot.encodings.first().expect("plot must contain x data");
    let graphcal_eval::eval::types::PlotFieldValue::Numbers(values) = values else {
        panic!("expected numeric plot data, got {values:?}");
    };
    assert_eq!(values.as_slice(), [1.0, 2.0]);
}

#[test]
fn requested_plot_keeps_instance_owned_dynamic_unit_presentation() {
    let directory = tempfile::tempdir().unwrap();
    let package = directory.path().join("src/demo");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        directory.path().join("graphcal.toml"),
        "[package]\nname = \"demo\"\n",
    )
    .unwrap();
    std::fs::write(
        package.join("fx.gcl"),
        "pub base dim Money;\n\
         pub base unit USD: Money;\n\
         param rate: Dimensionless = 2.0;\n\
         pub unit EUR: Money = (@rate) USD;\n\
         param amount: Money = 3.0 EUR;\n\
         pub plot chart = { mark: bar, encode: { x: 1.0, y: @amount } };\n",
    )
    .unwrap();
    let root = package.join("main.gcl");
    std::fs::write(&root, "include demo.fx()::{ chart };\n").unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let plot = result.plots.first().expect("requested plot must render");
    let (_, values) = plot
        .encodings
        .iter()
        .find(|(channel, _)| *channel == graphcal_compiler::syntax::ast::EncodingChannel::Y)
        .expect("plot must contain y data");
    let graphcal_eval::eval::types::PlotFieldValue::Numbers(values) = values else {
        panic!("expected numeric plot data, got {values:?}");
    };
    assert_eq!(values.as_slice(), [3.0]);
}

#[test]
fn requested_instance_plot_reports_its_failed_instance_dependency() {
    // The plot body is the template's; the failed dependency is keyed by the
    // instance's own declaration, so the report must resolve the body's
    // reference through the instance frame (#842 inside an include).
    let directory = tempfile::tempdir().unwrap();
    let package = directory.path().join("src/demo");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        directory.path().join("graphcal.toml"),
        "[package]\nname = \"demo\"\n",
    )
    .unwrap();
    std::fs::write(
        package.join("div.gcl"),
        "pub index Step = { A, B };\n\
         param values: Dimensionless[Step] = { Step#A: 1.0, Step#B: 0.0 };\n\
         node inv: Dimensionless[Step] = for s: Step { 1.0 / @values[s] };\n\
         pub plot chart = {\n\
             mark: line,\n\
             encode: { x: for s: Step { @values[s] }, y: for s: Step { @inv[s] } },\n\
         };\n",
    )
    .unwrap();
    let root = package.join("main.gcl");
    std::fs::write(&root, "include demo.div()::{ chart };\n").unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!(result.plots.is_empty());
    let [error] = result.plot_errors.as_slice() else {
        panic!("expected one plot error, got {:?}", result.plot_errors);
    };
    assert_eq!(error.name.to_string(), "chart");
    let graphcal_eval::eval::PlotUnavailable::Evaluation(NodeUnavailable::EvalFailed { message }) =
        &error.reason
    else {
        panic!("expected an evaluation failure, got {:?}", error.reason);
    };
    assert!(
        // The dependency is named as the output names it: the private include
        // scope takes its readable name.
        message.starts_with("dependency failed: div::inv (")
            && message.contains("division by zero"),
        "expected the failed instance dependency with its root cause: {message}"
    );
}

#[test]
fn composition_of_an_unavailable_requested_instance_plot_reports_the_plot() {
    // A figure names a requested instance plot by its local alias. When that
    // plot is unavailable, the figure is blocked by it and reports it by that
    // alias, not by the instance's plot declaration identity; the alias binds
    // no declaration identity, so it must not be looked up as one (this used
    // to abort evaluation with X001).
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "leaf.gcl",
                "param input: Dimensionless;\n\
                 node reciprocal: Dimensionless = 1.0 / @input;\n\
                 pub plot chart = { mark: point, encode: { x: @input, y: @reciprocal } };\n",
            ),
            (
                "main.gcl",
                "include pipeline.leaf(input: 0.0)::{ chart as ch };\n\
                 figure summary = { plots: [ch] };\n",
            ),
        ],
        "main.gcl",
    );
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!(result.plots.is_empty());
    assert!(result.figures.is_empty());
    let names = result
        .plot_errors
        .iter()
        .map(|error| error.name.to_string())
        .collect::<Vec<_>>();
    assert_eq!(names, ["ch", "summary"]);
    let graphcal_eval::eval::PlotUnavailable::ComposedPlots(
        graphcal_eval::eval::ComposedPlotsUnavailable::Failed { failed_plots },
    ) = &result.plot_errors[1].reason
    else {
        panic!(
            "expected the figure to be blocked by its plot, got {:?}",
            result.plot_errors[1].reason
        );
    };
    let [plot] = failed_plots.as_slice() else {
        panic!("expected one failed plot, got {failed_plots:?}");
    };
    assert_eq!(plot.as_str(), "ch");
    assert_eq!(
        result.plot_errors[1].reason.to_string(),
        "dependency failed: ch"
    );
}

#[test]
fn compositions_name_unavailable_plots_as_the_root_does() {
    // Figures and layers report the plots blocking them by their names in the
    // root, for root plots and requested inline-DAG instance plots alike,
    // never by declaration identities such as `main.<include@N>.inner`.
    let result = compile_and_eval_named(
        "dag lib { pub plot inner = { mark: line { stroke_width: 1.0 / 0.0 }, encode: { x: 1.0, y: 2.0 } }; }\n\
         include lib()::{ inner as shown };\n\
         plot broken = { mark: line { stroke_width: 1.0 / 0.0 }, encode: { x: 1.0, y: 2.0 } };\n\
         node missing: Dimensionless = todo<Dimensionless> {};\n\
         plot pending = { mark: point, encode: { y: @missing } };\n\
         figure comparison = { plots: [broken], title: \"C\" };\n\
         layer overlay = { plots: [shown, broken] };\n\
         figure mixed = { plots: [pending, shown] };\n",
        "main.gcl",
    )
    .unwrap();
    let reasons = result
        .plot_errors
        .iter()
        .map(|error| (error.name.to_string(), error.reason.to_string()))
        .collect::<HashMap<_, _>>();
    assert_eq!(reasons["comparison"], "dependency failed: broken");
    assert_eq!(reasons["overlay"], "dependency failed: broken, shown");
    assert!(
        reasons["mixed"].ends_with("; dependency failed: shown"),
        "{}",
        reasons["mixed"]
    );
    let mixed = &result
        .plot_errors
        .iter()
        .find(|error| error.name.to_string() == "mixed")
        .unwrap()
        .reason;
    assert!(mixed.is_incomplete());
    assert!(mixed.has_failure());
}

#[test]
fn three_level_instantiated_file_include_preserves_assertion_instance_path() {
    let (_dir, root) = write_nested_file_include_project(
        "include demo.upper(x: -1.0) as upper;\n\
         pub node result: Dimensionless = @upper::out;\n",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!((find_value(&result, "result") - (-2.0)).abs() < f64::EPSILON);
    assert!(result.assertions.iter().any(|assertion| {
        assertion.name.to_string() == "upper.middle.leaf::positive"
            && matches!(
                assertion.outcome,
                graphcal_eval::eval::types::AssertResult::Fail { .. }
            )
    }));
    assert!(
        result
            .output_surface()
            .contains(&member_name(&["upper"], "out"))
    );
    assert!(
        !result
            .output_surface()
            .contains(&member_name(&["upper", "middle"], "out"))
    );
    assert!(
        !result
            .output_surface()
            .contains(&member_name(&["upper", "middle", "leaf"], "private_value"))
    );
}

fn write_same_leaf_include_project(main_source: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let package_dir = dir.path().join("src/app");
    std::fs::create_dir_all(package_dir.join("analysis")).unwrap();
    std::fs::create_dir_all(package_dir.join("presentation")).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"app\"\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("analysis/shared.gcl"),
        "param input: Dimensionless;\npub node output: Dimensionless = @input + 1.0;\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("presentation/shared.gcl"),
        "param input: Dimensionless;\npub node output: Dimensionless = @input * 2.0;\n",
    )
    .unwrap();
    let root = package_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

#[test]
fn project_selective_includes_allow_distinct_modules_with_same_leaf_name() {
    let (_dir, root) = write_same_leaf_include_project(
        "include app.analysis.shared(input: 2.0)::{ output as analyzed };\n\
         include app.presentation.shared(input: 5.0)::{ output as rendered };\n\
         node combined: Dimensionless = @analyzed + @rendered;\n",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!((find_value(&result, "combined") - 13.0).abs() < f64::EPSILON);

    let internal_input_owners = result
        .entries
        .iter()
        .filter(|EvalEntry { name, .. }| name.leaf().as_str() == "input" && name.is_qualified())
        .map(|EvalEntry { name, .. }| name.qualifier().to_vec())
        .collect::<HashSet<_>>();
    assert_eq!(internal_input_owners.len(), 2);
}

#[test]
fn module_aliases_and_same_named_nodes_are_duplicate_names() {
    // B4 regression: an include alias is a Term name exactly like an import
    // alias, including when it instantiates a local inline DAG.
    let velocity = "dag velocity { param r: Dimensionless; pub node v: Dimensionless = @r; }\n";
    for alias_decl in [
        "include velocity(r: 1.0) as parking;",
        "import velocity as parking;",
    ] {
        let source = format!("{velocity}{alias_decl}\nnode parking: Dimensionless = 2.0;\n");
        match compile_and_eval(&source) {
            Err(CompileError::Eval(RenderedSemanticError {
                error:
                    SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                        kind: SemanticErrorKind::Name(NameError::DuplicateName { name, .. }),
                        ..
                    }),
                ..
            })) => {
                assert_eq!(name.to_string(), "parking", "{alias_decl}");
            }
            other => panic!("expected N001 for `{alias_decl}`, got {other:?}"),
        }
    }
}

#[test]
fn project_selective_includes_still_reject_duplicate_local_names() {
    let (_dir, root) = write_same_leaf_include_project(
        "include app.analysis.shared(input: 2.0)::{ output as duplicate };\n\
         include app.presentation.shared(input: 5.0)::{ output as duplicate };\n",
    );
    let project = crate::loader::load_project(&root, None, &fs()).unwrap();

    match graphcal_compiler::outcome::without_cancellation(|cancellation| {
        project.build_module_resolver(cancellation)
    }) {
        Err(graphcal_compiler::resolve::error::ModuleResolveError::DuplicateImportName {
            namespace,
            name,
            ..
        }) => {
            assert_eq!(
                namespace,
                graphcal_compiler::resolve::namespace::Namespace::Term
            );
            assert_eq!(name.as_str(), "duplicate");
        }
        other => panic!("expected a duplicate local declaration diagnostic, got {other:?}"),
    }
}

#[test]
fn project_module_includes_still_reject_duplicate_default_aliases() {
    // Two module aliases of one name are a duplicate name (N001) reported at
    // the second alias, for includes and imports alike (RES-13).
    for declarations in [
        "include app.analysis.shared(input: 2.0);\n\
         include app.presentation.shared(input: 5.0);\n",
        "import app.analysis.shared;\n\
         import app.presentation.shared;\n",
    ] {
        let (_dir, root) = write_same_leaf_include_project(declarations);
        match compile_and_eval_project(&root, &HashMap::new(), None, &fs()) {
            Err(CompileError::Eval(RenderedSemanticError {
                error:
                    SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                        primary,
                        kind: SemanticErrorKind::Name(NameError::DuplicateName { name, first }),
                        ..
                    }),
                ..
            })) => {
                assert_eq!(name.to_string(), "shared", "{declarations}");
                assert!(first.offset() < primary.offset(), "{declarations}");
            }
            other => panic!("expected N001 for {declarations:?}, got {other:?}"),
        }
    }
}

#[test]
fn project_selective_includes_allow_multiple_instances_of_same_module() {
    let dir = tempfile::tempdir().unwrap();
    let package_dir = dir.path().join("src/app");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"app\"\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("shared.gcl"),
        "param input: Dimensionless;\npub node output: Dimensionless = @input * 2.0;\n",
    )
    .unwrap();
    let root = package_dir.join("main.gcl");
    std::fs::write(
        &root,
        "include app.shared(input: 2.0)::{ output as first };\n\
         include app.shared(input: 3.0)::{ output as other };\n\
         node combined: Dimensionless = @first + @other;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!((find_value(&result, "combined") - 10.0).abs() < f64::EPSILON);
}

#[test]
fn project_import_preserves_structural_finite_index_identity() {
    let dir = tempfile::tempdir().unwrap();
    let source_dir = dir.path().join("src/app");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"app\"\n",
    )
    .unwrap();
    std::fs::write(
        source_dir.join("lib.gcl"),
        "pub const node values: Dimensionless[Fin(2)] = table[Fin(2)] { 1.0; 2.0; };\n",
    )
    .unwrap();
    let root = source_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import app.lib::{ values };\nconst node copied: Dimensionless[Fin(2)] = @values;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let copied = result
        .consts()
        .find(|(name, _)| name.to_string() == "copied")
        .and_then(|(_, value)| value.as_ref().ok())
        .expect("copied imported finite-indexed value");
    assert!(matches!(copied, Value::Indexed { entries, .. } if entries.len() == 2));
}

#[test]
fn project_selective_import_item_rejects_unknown_attribute() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/attr");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"attr\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("lib.gcl"),
        "pub node x: Dimensionless = 1.0;\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import attr.lib::{ #[bogus] x };\nnode y: Dimensionless = @x;\n",
    )
    .unwrap();

    match compile_and_eval_project(&root, &HashMap::new(), None, &fs()) {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Attribute(AttributeError::UnknownAttribute { name, .. }),
                    ..
                }),
            ..
        })) => {
            assert_eq!(name.as_str(), "bogus");
        }
        other => panic!("expected UnknownAttribute, got {other:?}"),
    }
}

#[test]
fn project_qualified_index_type_annotation_and_variant_arg() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/mission");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"mission\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("lib.gcl"),
        "pub index Phase = { Burn, Coast };\n\
         pub dim GravityAccel = Length / Time^2;\n\
         pub node thrust: Dimensionless[Phase] = { Phase#Burn: 3.0, Phase#Coast: 5.0 };\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import mission.lib as lib;\n\
         node thrust: Dimensionless[lib::Phase] = { lib::Phase#Burn: 3.0, lib::Phase#Coast: 5.0 };\n\
         node burn: Dimensionless = @thrust[lib::Phase#Burn];\n\
         node accel: lib::GravityAccel = 9.80665 m/s^2;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();

    assert!((find_value(&result, "burn") - 3.0).abs() < f64::EPSILON);
    assert!((find_value(&result, "accel") - 9.80665).abs() < f64::EPSILON);
}
