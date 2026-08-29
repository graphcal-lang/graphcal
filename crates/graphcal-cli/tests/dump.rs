//! Binary-level tests for the experimental pipeline dump command.
#![cfg(test)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn graphcal_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_graphcal"))
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

fn write_source(root: &Path, source: &str) -> PathBuf {
    let path = root.join("model.gcl");
    std::fs::write(&path, source).unwrap();
    path
}

fn run(args: &[&str]) -> Output {
    graphcal_bin()
        .args(args)
        .output()
        .expect("failed to run graphcal")
}

#[test]
fn source_prints_raw_debug_output() {
    let path = fixture("valid/rocket.gcl");
    let output = run(&["dump", "source", path.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("SourceUnit"));
    assert!(stdout.contains("rocket.gcl"));
    // Compare path *components*, not the canonicalized string: Windows
    // canonicalization returns a `\\?\C:\...` verbatim prefix that the CLI does
    // not print, so a whole-string match is Unix-only.
    for component in ["fixtures", "valid", "rocket.gcl"] {
        assert!(
            stdout.contains(component),
            "expected `{component}` in dump output:\n{stdout}"
        );
    }
    assert!(stdout.contains("param isp"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("experimental"));
}

#[test]
fn tokens_retain_lexical_errors_without_requiring_an_ast() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_source(dir.path(), "param x: Dimensionless = §1.0;\n");
    let output = run(&["dump", "tokens", path.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("TokenizedSource"));
    assert!(stdout.contains("Param"));
    assert!(stdout.contains("first_error_span: Some"));
}

#[test]
fn syntax_stages_do_not_follow_missing_imports_but_modules_does() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_source(
        dir.path(),
        "import missing;\nnode x: Dimensionless = 1.0;\n",
    );
    for stage in ["ast", "desugared"] {
        let output = run(&["dump", stage, path.to_str().unwrap()]);
        assert!(
            output.status.success(),
            "stage {stage}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let modules = run(&["dump", "modules", path.to_str().unwrap()]);
    assert!(!modules.status.success());
    assert!(modules.stdout.is_empty());
}

#[test]
fn hir_exists_before_static_checking_and_tir_requires_validation() {
    let valid = fixture("valid/rocket.gcl");
    let output = run(&["dump", "tir", valid.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("TIR {"));

    let dir = tempfile::tempdir().unwrap();
    let invalid = write_source(dir.path(), "node bad: Length(min: 0.0 m) = 1.0 s;\n");
    let hir = run(&["dump", "hir", invalid.to_str().unwrap()]);
    assert!(
        hir.status.success(),
        "{}",
        String::from_utf8_lossy(&hir.stderr)
    );
    let hir_stdout = String::from_utf8_lossy(&hir.stdout);
    assert!(hir_stdout.starts_with("HirProject {"));
    assert!(hir_stdout.contains("bad"));
    assert!(hir_stdout.contains("type_ann: TypeAnnotation"));
    assert!(hir_stdout.contains("domain_bounds: ["));
    assert!(hir_stdout.contains("value: Expr"));
    assert!(!hir_stdout.contains("type_resolution_owner"));

    let tir = run(&["dump", "tir", invalid.to_str().unwrap()]);
    assert!(!tir.status.success());
    assert!(tir.stdout.is_empty());
}

#[test]
fn plan_accepts_required_runtime_params_but_runtime_requires_a_binding() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_source(
        dir.path(),
        "param required: Dimensionless;\nnode out: Dimensionless = @required;\n",
    );
    let plan = run(&["dump", "plan", path.to_str().unwrap()]);
    assert!(
        plan.status.success(),
        "{}",
        String::from_utf8_lossy(&plan.stderr)
    );
    assert!(String::from_utf8_lossy(&plan.stdout).starts_with("ExecPlan {"));

    let runtime = run(&["dump", "runtime", path.to_str().unwrap()]);
    assert_eq!(runtime.status.code(), Some(2));
    assert!(runtime.stdout.is_empty());
    assert!(String::from_utf8_lossy(&runtime.stderr).contains("required"));

    let bound_runtime = run(&[
        "dump",
        "runtime",
        path.to_str().unwrap(),
        "--param",
        "required=2.0",
    ]);
    assert!(
        bound_runtime.status.success(),
        "{}",
        String::from_utf8_lossy(&bound_runtime.stderr)
    );
    assert!(String::from_utf8_lossy(&bound_runtime.stdout).contains("RuntimeEvaluation"));

    let json_result = run(&[
        "dump",
        "result",
        path.to_str().unwrap(),
        "--params-json",
        r#"{"required":2.0}"#,
    ]);
    assert!(
        json_result.status.success(),
        "{}",
        String::from_utf8_lossy(&json_result.stderr)
    );
    assert!(String::from_utf8_lossy(&json_result.stdout).contains("EvalResult"));
}

#[test]
fn plan_does_not_invoke_runtime_host_functions() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_source(
        dir.path(),
        "import plugin \"graphcal:demo\" as demo {\n\
             fn inverse<D: Dim>(x: D) -> D^-1;\n\
         }\n\
         node trap: Dimensionless = demo::inverse(0.0);\n",
    );
    let plan = run(&["dump", "plan", path.to_str().unwrap()]);
    assert!(
        plan.status.success(),
        "{}",
        String::from_utf8_lossy(&plan.stderr)
    );

    let runtime = run(&["dump", "runtime", path.to_str().unwrap()]);
    assert_eq!(runtime.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&runtime.stdout);
    assert!(stdout.starts_with("RuntimeEvaluation {"));
    assert!(stdout.contains("errors:"));
    assert!(stdout.contains("trap"));
}

#[test]
fn command_surface_has_only_individual_debug_stages() {
    let output = run(&["dump", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for stage in [
        "source",
        "tokens",
        "ast",
        "desugared",
        "modules",
        "hir",
        "tir",
        "plan",
        "runtime",
        "result",
    ] {
        assert!(stdout.contains(stage), "missing stage {stage}:\n{stdout}");
    }
    assert!(!stdout.contains("  all"));
    assert!(!stdout.contains("json"));
}
