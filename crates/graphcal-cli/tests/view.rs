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

/// One view the page's contract test (`web/view/src/contract.test.ts`)
/// feeds to its schemas.
struct ContractFixture {
    name: &'static str,
    /// The file, relative to the repository root.
    file: &'static str,
    /// The project root, relative to the repository root.
    project_root: Option<&'static str>,
    /// The view of the include instance the node with this label is read
    /// through, instead of the file's own view.
    instance_of: Option<&'static str>,
}

const INCLUDE_BINDINGS: &str =
    "crates/graphcal-project/tests/fixtures/view_model/include_bindings/src/review/main.gcl";
const INCLUDE_BINDINGS_ROOT: &str =
    "crates/graphcal-project/tests/fixtures/view_model/include_bindings";

const CONTRACT_FIXTURES: &[ContractFixture] = &[
    ContractFixture {
        name: "power_max",
        file: "crates/graphcal-project/tests/fixtures/view_model/power_project/src/power_project/power_max.gcl",
        project_root: Some("crates/graphcal-project/tests/fixtures/view_model/power_project"),
        instance_of: None,
    },
    ContractFixture {
        name: "range_index",
        file: "tests/fixtures/valid/range_index.gcl",
        project_root: None,
        instance_of: None,
    },
    ContractFixture {
        name: "linear_algebra",
        file: "tests/fixtures/valid/linear_algebra.gcl",
        project_root: None,
        instance_of: None,
    },
    ContractFixture {
        name: "recurrence_groups",
        file: "tests/fixtures/valid/recurrence_groups.gcl",
        project_root: None,
        instance_of: None,
    },
    ContractFixture {
        name: "inline_dag_call",
        file: "tests/fixtures/valid/inline_dag_call_basic/main.gcl",
        project_root: None,
        instance_of: None,
    },
    ContractFixture {
        name: "include_bindings",
        file: INCLUDE_BINDINGS,
        project_root: Some(INCLUDE_BINDINGS_ROOT),
        instance_of: None,
    },
    ContractFixture {
        name: "include_bindings_instance",
        file: INCLUDE_BINDINGS,
        project_root: Some(INCLUDE_BINDINGS_ROOT),
        instance_of: Some("p::out"),
    },
];

/// The `graphcal view` arguments for `fixture`'s file: the file and its
/// project root.
fn fixture_args(root: &Path, fixture: &ContractFixture) -> Vec<String> {
    let mut args = vec![
        "view".to_string(),
        root.join(fixture.file).to_str().unwrap().to_string(),
    ];
    if let Some(project_root) = fixture.project_root {
        args.push("--root".to_string());
        args.push(root.join(project_root).to_str().unwrap().to_string());
    }
    args
}

/// `graphcal view` output for the contract fixtures, as committed under
/// `web/view/src/contract/`. Set `GRAPHCAL_UPDATE_VIEW_CONTRACT=1` to write
/// the current output there after an intended model change.
#[test]
fn view_contract_fixtures_are_current() {
    let root = repo_root();
    let update = std::env::var_os("GRAPHCAL_UPDATE_VIEW_CONTRACT").is_some();
    for fixture in CONTRACT_FIXTURES {
        let name = fixture.name;
        let mut args = fixture_args(&root, fixture);
        if let Some(label) = fixture.instance_of {
            let file_args: Vec<&str> = args.iter().map(String::as_str).collect();
            let view = view_json(&file_args);
            let instance = node(&view, label)["origin"]["instance"]["id"]
                .as_str()
                .unwrap()
                .to_string();
            args.push("--instance".to_string());
            args.push(instance);
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

#[test]
fn view_prints_an_include_instance_with_the_values_its_include_gives() {
    let root = repo_root();
    let file = root.join(INCLUDE_BINDINGS);
    let project_root = root.join(INCLUDE_BINDINGS_ROOT);
    let file_args = [
        "view",
        file.to_str().unwrap(),
        "--root",
        project_root.to_str().unwrap(),
    ];
    let main = view_json(&file_args);
    let instance = node(&main, "p::out")["origin"]["instance"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut args = file_args.to_vec();
    args.extend(["--instance", &instance]);
    let p = view_json(&args);
    assert_eq!(p["target"]["kind"], "instance");
    assert_eq!(p["target"]["instance"]["name"], "p");
    assert_eq!(p["module"]["id"], "review.lib");
    assert_eq!(node(&p, "out")["value"]["display"], "2");
    assert_eq!(node(&p, "out")["id"], node(&main, "p::out")["id"]);

    args.pop();
    args.push("no-such-instance");
    let output = run(&args);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no include instance"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn view_reports_a_value_its_display_unit_cannot_show_and_fails_like_eval() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("huge.gcl");
    std::fs::write(
        &file,
        "node first: Dimensionless = 1.0;\nnode huge: Length = 1.0e305 km -> mm;\nnode last: Dimensionless = 2.0;\n",
    )
    .unwrap();
    let output = run(&["view", file.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("presentation: huge:"), "{stderr}");
    let view: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(node(&view, "last")["value"]["display"], "2");
    assert_eq!(node(&view, "huge")["value"]["display"], "1e308 m");
    assert_eq!(
        node(&view, "huge")["presentation"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
