#![cfg(test)]

use std::process::{Command, Output};

fn run(source: &str, args: &[&str]) -> Output {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("model.gcl");
    std::fs::write(&path, source).unwrap();
    Command::new(env!("CARGO_BIN_EXE_graphcal"))
        .args(args)
        .arg(path)
        .output()
        .unwrap()
}

const MODEL: &str = "node unfinished: Length = todo {}; node blocked: Length = @unfinished; node known: Length = 1.0 m;";

#[test]
fn check_accepts_incomplete_source_but_deny_todo_rejects_it_without_evaluation() {
    let source = format!("{MODEL} node bad: Length = 1.0 m / 0.0;");
    let output = run(&source, &["check"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 unfinished node"));
    assert!(!run(&source, &["check", "--deny-todo"]).status.success());
    assert!(
        !run(
            "dag unused { node x: Length = todo {}; }",
            &["check", "--deny-todo"]
        )
        .status
        .success()
    );
}

#[test]
fn eval_prints_partial_results_and_requires_explicit_permission_for_success() {
    let output = run(MODEL, &["eval"]);
    assert!(!output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("TODO"));
    assert!(text.contains("BLOCKED"));
    assert!(text.contains("known"));
    assert!(!text.contains("ERROR"));
    assert!(run(MODEL, &["eval", "--allow-incomplete"]).status.success());
}

#[test]
fn json_distinguishes_incompleteness_without_fabricating_values() {
    let output = run(MODEL, &["eval", "--allow-incomplete", "--format", "json"]);
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["incomplete"], true);
    assert_eq!(json["node"]["unfinished"]["status"], "todo");
    assert_eq!(json["node"]["blocked"]["status"], "blocked");
    assert!(json["node"]["unfinished"].get("si").is_none());
    assert!(json["node"]["unfinished"].get("error").is_none());
    assert!(json["node"]["known"].get("status").is_none());
}

#[test]
fn allowing_incomplete_does_not_hide_private_call_failures() {
    let source = "dag model { node missing: Length = todo {}; node bad: Length = 1.0 m / 0.0; pub node known: Length = 1.0 m; } node known: Length = @model()::known;";
    let output = run(source, &["eval", "--allow-incomplete"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("ERROR"));
}

#[test]
fn allowing_incomplete_never_suppresses_real_failures() {
    for extra in ["node bad: Length = 1.0 m / 0.0;", "assert bad = false;"] {
        let output = run(&format!("{MODEL} {extra}"), &["eval", "--allow-incomplete"]);
        assert!(!output.status.success());
    }
}
