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

/// The node the module spells `label`.
fn node<'a>(view: &'a serde_json::Value, label: &str) -> &'a serde_json::Value {
    view["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["label"] == label)
        .unwrap_or_else(|| panic!("no node {label}"))
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
    assert_eq!(output.stdout, [] as [u8; 0]);
    assert_ne!(output.stderr, [] as [u8; 0]);
}

/// The repository root.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// The views the page's contract test (`web/view/src/contract.test.ts`)
/// feeds to its schemas: name, file and project root (relative to the
/// repository root).
const CONTRACT_FIXTURES: &[(&str, &str, Option<&str>)] = &[
    (
        "power_max",
        "crates/graphcal-project/tests/fixtures/view_model/power_project/src/power_project/power_max.gcl",
        Some("crates/graphcal-project/tests/fixtures/view_model/power_project"),
    ),
    ("range_index", "tests/fixtures/valid/range_index.gcl", None),
    (
        "linear_algebra",
        "tests/fixtures/valid/linear_algebra.gcl",
        None,
    ),
    (
        "recurrence_groups",
        "tests/fixtures/valid/recurrence_groups.gcl",
        None,
    ),
];

/// `graphcal view` output for the contract fixtures, as committed under
/// `web/view/src/contract/`. Set `GRAPHCAL_UPDATE_VIEW_CONTRACT=1` to write
/// the current output there after an intended model change.
#[test]
fn view_contract_fixtures_are_current() {
    let root = repo_root();
    let update = std::env::var_os("GRAPHCAL_UPDATE_VIEW_CONTRACT").is_some();
    for (name, file, project_root) in CONTRACT_FIXTURES {
        let mut args = vec![
            "view".to_string(),
            root.join(file).to_str().unwrap().to_string(),
        ];
        if let Some(project_root) = project_root {
            args.push("--root".to_string());
            args.push(root.join(project_root).to_str().unwrap().to_string());
        }
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let output = run(&args);
        assert!(
            output.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let current = String::from_utf8(output.stdout).unwrap();
        let committed_path = root.join(format!("web/view/src/contract/{name}.json"));
        if update {
            std::fs::create_dir_all(committed_path.parent().unwrap()).unwrap();
            std::fs::write(&committed_path, &current).unwrap();
            continue;
        }
        let committed = std::fs::read_to_string(&committed_path)
            .unwrap_or_else(|error| panic!("{}: {error}", committed_path.display()));
        assert!(
            committed == current,
            "{} is stale; run `GRAPHCAL_UPDATE_VIEW_CONTRACT=1 cargo test -p graphcal --test view` and review the diff",
            committed_path.display()
        );
    }
}
