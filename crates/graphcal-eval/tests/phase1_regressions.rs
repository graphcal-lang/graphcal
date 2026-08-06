//! Phase 1 regression tests from `.local/2026-08-05_code-review-chunk-3.md`.
#![cfg(test)]

use std::collections::HashMap;

use graphcal_compiler::registry::error::GraphcalError;
use graphcal_eval::eval::{CompileError, compile_and_eval, compile_and_eval_project};
use graphcal_io::RealFileSystem;

fn compile_graphcal_error(source: &str) -> GraphcalError {
    match compile_and_eval(source).unwrap_err() {
        CompileError::Eval(error) => error,
        other => panic!("expected semantic error, got {other:?}"),
    }
}

#[test]
fn sink_graph_references_are_lowered_strictly() {
    let sources = [
        r"
plot p = {
    mark: line,
    encode: { x: @missing },
};
",
        r"
plot p = {
    mark: line { stroke_width: @missing },
    encode: { x: 1.0 },
};
",
        r"
plot p = {
    mark: line,
    encode: { x: 1.0 },
    width: @missing,
};
",
        r"
plot p = { mark: line, encode: { x: 1.0 } };
figure f = { plots: [p], title: @missing };
",
        r"
plot p = { mark: line, encode: { x: 1.0 } };
layer l = { plots: [p], title: @missing };
",
    ];

    for source in sources {
        assert!(
            matches!(
                compile_graphcal_error(source),
                GraphcalError::UnknownGraphRef { .. }
            ),
            "sink accepted an unresolved graph reference:\n{source}"
        );
    }
}

#[test]
fn sink_constructor_function_and_unit_names_are_lowered_strictly() {
    for source in [
        "plot p = { mark: line, encode: { x: Missing(value: 1.0) } };",
        "plot p = { mark: line, encode: { x: missing(1.0) } };",
        "plot p = { mark: line, encode: { x: 1.0 missing_unit } };",
    ] {
        assert!(
            compile_and_eval(source).is_err(),
            "sink accepted an unresolved semantic name: {source}"
        );
    }
}

#[test]
fn same_span_dag_calls_from_distinct_files_keep_their_hir_targets() {
    let directory = tempfile::tempdir().unwrap();
    let package = directory.path().join("src/calls");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        directory.path().join("graphcal.toml"),
        "[package]\nname = \"calls\"\n",
    )
    .unwrap();

    let library = r"
dag helper {
    param x: Dimensionless;
    pub node out: Dimensionless = @x + 1.0;
}
param input: Dimensionless;
pub node result: Dimensionless = @helper(x: @input).out;
";
    std::fs::write(package.join("a.gcl"), library).unwrap();
    std::fs::write(package.join("b.gcl"), library).unwrap();
    let root = package.join("main.gcl");
    std::fs::write(
        &root,
        r"
include calls.a(input: 1.0) as a;
include calls.b(input: 2.0) as b;
node total: Dimensionless = @a.result + @b.result;
",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &RealFileSystem::default())
        .unwrap_or_else(|error| panic!("same-layout DAG calls must compile: {error:?}"));
    let total = result
        .nodes
        .iter()
        .find(|(name, _)| name.to_string() == "total")
        .unwrap()
        .1
        .as_ref()
        .unwrap()
        .si_value()
        .unwrap();
    assert!((total - 5.0).abs() < f64::EPSILON);
}

#[test]
fn dag_call_in_plot_property_uses_hir_directly() {
    let result = compile_and_eval(
        r"
dag chart_config {
    pub node width: Dimensionless = 320.0;
}
plot p = {
    mark: line,
    encode: { x: 1.0 },
    width: @chart_config().width,
};
",
    );
    assert!(result.is_ok(), "plot DAG call failed: {result:?}");
}

fn write_owner_dimension_project() -> (tempfile::TempDir, std::path::PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let package = directory.path().join("src/owner_dims");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        directory.path().join("graphcal.toml"),
        "[package]\nname = \"owner_dims\"\n",
    )
    .unwrap();
    let library = r"
pub base dim Foo;
pub base unit foo: Foo;
pub index Axis = { A, B };
pub type Box<D: Dim> { Box(x: D) }
";
    std::fs::write(package.join("a.gcl"), library).unwrap();
    std::fs::write(package.join("b.gcl"), library).unwrap();
    (directory, package.join("main.gcl"))
}

#[test]
fn generic_dimension_arguments_keep_canonical_module_owners() {
    let (_directory, root) = write_owner_dimension_project();
    for imports in [
        "import owner_dims.a as a;\nimport owner_dims.b as b;",
        "import owner_dims.b as b;\nimport owner_dims.a as a;",
    ] {
        std::fs::write(
            &root,
            format!(
                r"
{imports}
base dim Foo;
base unit foo: Foo;
node local: Foo = 1.0 foo;
node from_a: a.Box<a.Foo> = a.Box<a.Foo>(x: 1.0 a.foo);
node from_b: b.Box<b.Foo> = b.Box<b.Foo>(x: 1.0 b.foo);
node compound: a.Box<a.Foo * b.Foo> =
    a.Box<a.Foo * b.Foo>(x: (1.0 a.foo) * (1.0 b.foo));
"
            ),
        )
        .unwrap();

        let result =
            compile_and_eval_project(&root, &HashMap::new(), None, &RealFileSystem::default());
        assert!(
            result.is_ok(),
            "canonical dimension lookup depended on import order: {result:?}"
        );
    }
}

#[test]
fn same_leaf_plot_axes_from_distinct_owners_are_incompatible() {
    let (_directory, root) = write_owner_dimension_project();
    std::fs::write(
        &root,
        r"
import owner_dims.a as a;
import owner_dims.b as b;
plot p = {
    mark: point,
    encode: {
        x: for item: a.Axis { item },
        y: for item: b.Axis { item },
    },
};
",
    )
    .unwrap();

    let error = compile_and_eval_project(&root, &HashMap::new(), None, &RealFileSystem::default())
        .unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(GraphcalError::PlotEncodingAxisMismatch { ref channels, .. })
            if channels.contains("owner_dims.a.Axis")
                && channels.contains("owner_dims.b.Axis")
    ));
}

#[test]
fn same_leaf_dimension_mismatch_diagnostic_qualifies_owners() {
    let (_directory, root) = write_owner_dimension_project();
    std::fs::write(
        &root,
        r"
import owner_dims.a as a;
import owner_dims.b as b;
node bad: a.Box<a.Foo> = a.Box<a.Foo>(x: 1.0 b.foo);
",
    )
    .unwrap();

    let error = compile_and_eval_project(&root, &HashMap::new(), None, &RealFileSystem::default())
        .unwrap_err();
    match error {
        CompileError::Eval(GraphcalError::FieldDimensionMismatch {
            expected, found, ..
        }) => {
            assert_ne!(expected, found);
            assert!(expected.contains("owner_dims.a.Foo"), "{expected}");
            assert!(found.contains("owner_dims.b.Foo"), "{found}");
        }
        other => panic!("expected owner-qualified field mismatch, got {other:?}"),
    }
}

#[test]
fn source_nested_dag_calls_are_runtime_includes_in_compile_time_contexts() {
    for source in [
        r"
dag helper {
    pub const node fixed: Dimensionless = 1.0;
    pub node out: Dimensionless = @fixed;
}
const node value: Dimensionless = @helper().out;
",
        r"
dag helper {
    param value: Dimensionless = 1.0;
}
const node value: Dimensionless = @helper().value;
",
        r"
dag helper {
    pub node min_value: Dimensionless = 0.0;
}
param value: Dimensionless(min: @helper().min_value) = 1.0;
",
        r"
dag helper {
    pub node min_value: Dimensionless = 0.0;
}
type Box { Box(value: Dimensionless(min: @helper().min_value)) }
",
    ] {
        assert!(
            matches!(
                compile_graphcal_error(source),
                GraphcalError::DagCallInCompileTime { .. }
            ),
            "compile-time DAG call was accepted:\n{source}"
        );
    }
}

#[test]
fn file_root_dag_call_is_also_runtime_only() {
    let directory = tempfile::tempdir().unwrap();
    let package = directory.path().join("src/call_phase");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        directory.path().join("graphcal.toml"),
        "[package]\nname = \"call_phase\"\n",
    )
    .unwrap();
    std::fs::write(
        package.join("library.gcl"),
        "pub node out: Dimensionless = 1.0;\n",
    )
    .unwrap();
    let root = package.join("main.gcl");
    std::fs::write(
        &root,
        "import call_phase.library as library;\nconst node value: Dimensionless = @library().out;\n",
    )
    .unwrap();

    let error = compile_and_eval_project(&root, &HashMap::new(), None, &RealFileSystem::default())
        .unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(GraphcalError::DagCallInCompileTime { .. })
    ));
}

#[expect(
    clippy::result_large_err,
    reason = "test helper preserves the production compile error for exact diagnostic assertions"
)]
fn compile_reconciliation_project_with_files(
    library: &str,
    main: &str,
    additional_files: &[(&str, &str)],
) -> Result<graphcal_eval::eval::EvalResult, CompileError> {
    let directory = tempfile::tempdir().unwrap();
    let package = directory.path().join("src/reconcile");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        directory.path().join("graphcal.toml"),
        "[package]\nname = \"reconcile\"\n",
    )
    .unwrap();
    std::fs::write(package.join("library.gcl"), library).unwrap();
    for (name, source) in additional_files {
        std::fs::write(package.join(name), source).unwrap();
    }
    let root = package.join("main.gcl");
    std::fs::write(&root, main).unwrap();
    compile_and_eval_project(&root, &HashMap::new(), None, &RealFileSystem::default())
}

#[expect(
    clippy::result_large_err,
    reason = "test helper preserves the production compile error for exact diagnostic assertions"
)]
fn compile_reconciliation_project(
    library: &str,
    main: &str,
) -> Result<graphcal_eval::eval::EvalResult, CompileError> {
    compile_reconciliation_project_with_files(library, main, &[])
}

fn assert_reconciliation_error(
    error: CompileError,
    expected_override: &str,
    expected_kind: &str,
    expected_orphan: &str,
) {
    match error {
        CompileError::Eval(GraphcalError::IncludeMustReconcileOverride {
            overridden,
            overridden_kind,
            orphan_decl,
            src,
            span,
            ..
        }) => {
            assert_eq!(overridden, expected_override);
            assert_eq!(overridden_kind, expected_kind);
            assert_eq!(orphan_decl, expected_orphan);
            assert!(src.name().ends_with("main.gcl"));
            assert!(span.offset() + span.len() <= src.inner().len());
            assert!(src.inner()[span.offset()..span.offset() + span.len()].contains("include"));
        }
        other => panic!("expected typed V005 reconciliation error, got {other:?}"),
    }
}

fn assert_override_requires_reconciliation(
    library: &str,
    main: &str,
    expected_override: &str,
    expected_kind: &str,
    expected_orphan: &str,
) {
    let error = compile_reconciliation_project(library, main).unwrap_err();
    assert_reconciliation_error(error, expected_override, expected_kind, expected_orphan);
}

fn assert_type_override_requires_reconciliation(library: &str, main: &str, orphan: &str) {
    assert_override_requires_reconciliation(library, main, "Record", "type", orphan);
}

#[test]
fn type_override_reconciliation_uses_field_owner_not_field_spelling() {
    let library = r"
pub(bind) type Record { Record(x: Dimensionless) }
param record: Record = Record(x: 1.0);
param extracted: Dimensionless = @record.x;
pub node out: Dimensionless = @extracted;
";

    for main in [
        r"
type Other { Other(x: Dimensionless) }
include reconcile.library(Record: Other, record: Other(x: 2.0)) as instance;
",
        r"
type Other { Other(y: Dimensionless) }
include reconcile.library(Record: Other, record: Other(y: 2.0)) as instance;
",
    ] {
        assert_type_override_requires_reconciliation(library, main, "extracted");
    }
}

#[test]
fn type_override_reconciliation_uses_canonical_import_aliases() {
    let error = compile_reconciliation_project_with_files(
        r"
pub(bind) type Record { Record(x: Dimensionless) }
param record: Record = Record(x: 1.0);
param extracted: Dimensionless = @record.x;
pub node out: Dimensionless = @extracted;
",
        r"
import reconcile.replacement.{ type Other as Replacement, Other as MakeReplacement };
include reconcile.library(
    Record: Replacement,
    record: MakeReplacement(x: 2.0),
) as instance;
",
        &[(
            "replacement.gcl",
            "pub type Other { Other(x: Dimensionless) }\n",
        )],
    )
    .unwrap_err();

    assert_reconciliation_error(error, "Record", "type", "extracted");
}

#[test]
fn canonical_alias_equal_to_replacement_is_not_a_false_dependency() {
    let result = compile_reconciliation_project_with_files(
        r"
import reconcile.replacement.{ type Other as Existing, Other as MakeExisting };
pub(bind) type Record { Record(x: Dimensionless) }
param record: Record = Record(x: 1.0);
param existing: Existing = MakeExisting(x: 3.0);
param extracted: Dimensionless = @existing.x;
pub node out: Dimensionless = @record.x + @extracted;
",
        r"
import reconcile.replacement.{ type Other as Existing, Other as MakeExisting };
include reconcile.library(
    Record: Existing,
    record: MakeExisting(x: 2.0),
) as instance;
",
        &[(
            "replacement.gcl",
            "pub type Other { Other(x: Dimensionless) }\n",
        )],
    );

    assert!(
        result.is_ok(),
        "an independent canonical replacement dependency was rejected: {result:?}"
    );
}

#[test]
fn type_override_reconciliation_uses_match_constructor_owner() {
    let library = r"
pub(bind) type Record { Left(value: Dimensionless), Right(value: Dimensionless) }
param record: Record = Left(value: 1.0);
param extracted: Dimensionless = match @record {
    Left(value: v) => v,
    Right(value: v) => v,
};
pub node out: Dimensionless = @extracted;
";
    for main in [
        r"
type Other { Left(value: Dimensionless), Right(value: Dimensionless) }
include reconcile.library(Record: Other, record: Left(value: 2.0)) as instance;
",
        r"
type Other { Alpha(value: Dimensionless), Beta(value: Dimensionless) }
include reconcile.library(Record: Other, record: Alpha(value: 2.0)) as instance;
",
    ] {
        assert_type_override_requires_reconciliation(library, main, "extracted");
    }
}

#[test]
fn type_override_reconciliation_follows_nested_field_types() {
    let library = r"
pub(bind) type Record { Record(x: Dimensionless) }
pub type Wrapper { Wrapper(value: Record) }
param seed: Wrapper;
param wrapper: Wrapper = @seed;
param extracted: Dimensionless = @wrapper.value.x;
pub node out: Dimensionless = @extracted;
";
    let main = r"
type Other { Other(x: Dimensionless) }
import reconcile.library.{ type Wrapper };
include reconcile.library(Record: Other) as instance;
";

    assert_type_override_requires_reconciliation(library, main, "extracted");
}

#[test]
fn type_override_reconciliation_checks_generic_type_arguments() {
    let library = r"
pub(bind) type Record { Record(x: Dimensionless) }
pub type Wrapper<T: Type> { Wrapper(value: T) }
param record: Record = Record(x: 1.0);
param wrapped: Wrapper<Record> = Wrapper<Record>(value: @record);
pub node out: Dimensionless = @record.x;
";
    let main = r"
type Other { Other(x: Dimensionless) }
import reconcile.library.{ type Wrapper, Wrapper };
include reconcile.library(Record: Other, record: Other(x: 2.0)) as instance;
";

    assert_type_override_requires_reconciliation(library, main, "wrapped");
}

#[test]
fn override_reconciliation_checks_generic_index_arguments() {
    let library = r"
pub(bind) index Axis;
pub type Vector<I: Index> { Vector(values: Dimensionless[I]) }
param values: Dimensionless[Axis] = for i: Axis { 1.0 };
param wrapped: Vector<Axis> = Vector<Axis>(values: @values);
pub node out: Dimensionless = sum(@values);
";

    for main in [
        r"
index Other = { X, Y };
import reconcile.library.{ type Vector, Vector };
include reconcile.library(
    Axis: Other,
    values: { Other.X: 3.0, Other.Y: 4.0 },
) as instance;
",
        r"
import reconcile.library.{ type Vector, Vector };
include reconcile.library(
    Axis: Fin(2),
    values: for i: Fin(2) { 3.0 },
) as instance;
",
    ] {
        assert_override_requires_reconciliation(library, main, "Axis", "index", "wrapped");
    }
}

#[test]
fn cross_file_inline_dag_modules_use_typed_override_dependencies() {
    let library = r"
pub dag reusable {
    pub(bind) type Record { Record(x: Dimensionless) }
    param record: Record = Record(x: 1.0);
    param extracted: Dimensionless = @record.x;
    pub node out: Dimensionless = @extracted;
}
";
    let main = r"
type Other { Other(x: Dimensionless) }
include reconcile.library.reusable(
    Record: Other,
    record: Other(x: 2.0),
) as instance;
";

    assert_type_override_requires_reconciliation(library, main, "extracted");
}

#[test]
fn inline_dag_modules_use_the_same_override_reconciliation_rule() {
    let error = compile_graphcal_error(
        r"
dag reusable {
    pub(bind) type Record { Record(x: Dimensionless) }
    param record: Record = Record(x: 1.0);
    param extracted: Dimensionless = @record.x;
    pub node out: Dimensionless = @extracted;
}
type Other { Other(x: Dimensionless) }
include reusable(Record: Other, record: Other(x: 2.0)) as instance;
",
    );

    assert!(matches!(
        error,
        GraphcalError::IncludeMustReconcileOverride {
            overridden,
            overridden_kind,
            orphan_decl,
            ..
        } if overridden == "Record"
            && overridden_kind == "type"
            && orphan_decl == "extracted"
    ));
}

#[test]
fn explicitly_rebinding_the_dependent_param_reconciles_type_override() {
    let result = compile_reconciliation_project(
        r"
pub(bind) type Record { Record(x: Dimensionless) }
param record: Record = Record(x: 1.0);
param extracted: Dimensionless = @record.x;
pub node out: Dimensionless = @extracted;
",
        r"
type Other { Other(x: Dimensionless) }
include reconcile.library(
    Record: Other,
    record: Other(x: 2.0),
    extracted: 2.0,
) as instance;
",
    );
    assert!(result.is_ok(), "explicit reconciliation failed: {result:?}");
}
