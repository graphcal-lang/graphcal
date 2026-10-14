//! Binary-level tests for the experimental `graphcal view` command.
#![cfg(test)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn graphcal_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_graphcal"))
}

/// The sample project of the view model's own tests: a power budget with an
/// included EPS sheet and an imported index.
fn power_project() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../graphcal-project/tests/fixtures/view_model/power_project")
        .canonicalize()
        .unwrap()
}

fn run(args: &[&str]) -> Output {
    graphcal_bin()
        .args(args)
        .output()
        .expect("failed to run graphcal")
}

fn view_json(args: &[&str]) -> serde_json::Value {
    let output = run(args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("view output is JSON")
}

fn node<'a>(view: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
    view["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == id)
        .unwrap_or_else(|| panic!("no node {id}"))
}

#[test]
fn view_prints_the_evaluated_model_with_paths_relative_to_the_discovered_root() {
    let root = power_project();
    let file = root.join("src/power_project/power_max.gcl");
    let view = view_json(&["view", file.to_str().unwrap()]);

    assert_eq!(view["module"]["id"], "power_project.power_max");
    assert_eq!(view["module"]["package"], "power_project");
    // Paths are relative to the project root found above the file, with `/`.
    assert_eq!(view["module"]["file"], "src/power_project/power_max.gcl");
    assert_eq!(
        node(&view, "margin")["source"]["file"],
        "src/power_project/power_max.gcl"
    );
    assert_eq!(
        node(&view, "p_budget")["source"]["file"],
        "src/power_project/eps.gcl"
    );
    // Values are attached: the project is not a library.
    assert_eq!(node(&view, "margin")["value"]["kind"], "quantity");
    assert_eq!(node(&view, "margin")["value"]["display"], "0.2");
}

#[test]
fn view_takes_an_explicit_root() {
    let root = power_project();
    let file = root.join("src/power_project/power_max.gcl");
    let view = view_json(&[
        "view",
        file.to_str().unwrap(),
        "--root",
        root.to_str().unwrap(),
    ]);
    assert_eq!(view["module"]["file"], "src/power_project/power_max.gcl");
    assert_eq!(
        node(&view, "p_budget")["source"]["file"],
        "src/power_project/eps.gcl"
    );
}

#[test]
fn view_reports_a_file_that_does_not_compile() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("broken.gcl");
    std::fs::write(&file, "node x: Power = @missing;\n").unwrap();
    let output = run(&["view", file.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}
