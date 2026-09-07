use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn run(root: &Path, command: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_graphcal-pipeline-layers"))
        .arg(command)
        .arg(root)
        .output()
        .expect("run dependency guard")
}

#[test]
fn pruning_only_deletes_stale_debt_and_preserves_reviewed_reasons() {
    let directory = tempfile::tempdir().expect("temporary project");
    let root = directory.path();
    let compiler = root.join("crates/graphcal-compiler/src");
    let eval = root.join("crates/graphcal-eval/src");
    let config = root.join("internals/pipeline-layers");
    for path in [&compiler, &eval, &config] {
        fs::create_dir_all(path).expect("fixture directory");
    }
    fs::write(compiler.join("lib.rs"), "").expect("compiler root");
    let source = format!(
        "{}\nmod loading {{ pub fn load() {{}} }}\n",
        include_str!("../fixtures/negative.rs")
    );
    fs::write(eval.join("lib.rs"), &source).expect("eval root");
    let mut roles =
        "[[module]]\npackage = \"compiler\"\npath = []\nrole = \"contracts\"\n".to_string();
    for (path, role) in [
        ("", "facade"),
        ("contracts", "contracts"),
        ("checking", "checking"),
        ("interpreter", "interpreter"),
        ("facade", "facade"),
        ("loading", "loading"),
    ] {
        let segments = if path.is_empty() {
            String::new()
        } else {
            format!("{path:?}")
        };
        roles.push_str(&format!(
            "\n[[module]]\npackage = \"eval\"\npath = [{segments}]\nrole = {role:?}\n"
        ));
    }
    fs::write(config.join("role-map.toml"), roles).expect("explicit roles");
    assert!(run(root, "init-baseline").status.success());
    assert!(run(root, "check").status.success());
    let baseline_path = config.join("baseline.toml");
    let baseline = fs::read_to_string(&baseline_path).expect("initial debt");
    let repeated_bootstrap = run(root, "init-baseline");
    assert!(!repeated_bootstrap.status.success());
    assert!(
        String::from_utf8_lossy(&repeated_bootstrap.stderr).contains("baseline already exists")
    );
    assert_eq!(fs::read_to_string(&baseline_path).unwrap(), baseline);
    let reviewed = baseline
        .lines()
        .enumerate()
        .map(|(line, text)| {
            if text.starts_with("reason =") {
                format!("reason = \"human-reviewed exception {line}\"")
            } else {
                text.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(&baseline_path, &reviewed).expect("reviewed reasons");

    fs::write(
        eval.join("lib.rs"),
        source.replace(
            "crate::facade::checked();",
            "crate::facade::checked(); crate::loading::load();",
        ),
    )
    .expect("new forbidden edge");
    let refused = run(root, "prune");
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stdout).contains("NEW forbidden edge"));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("prune refuses"));
    assert_eq!(fs::read_to_string(&baseline_path).unwrap(), reviewed);

    fs::write(
        eval.join("lib.rs"),
        source.replace("crate::facade::checked();", "crate::checking::checked();"),
    )
    .expect("remove facade dependency");
    let stale = run(root, "check");
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stdout).contains("STALE exception"));
    assert!(run(root, "prune").status.success());
    assert!(run(root, "check").status.success());
    let retained = fs::read_to_string(&baseline_path).unwrap();
    let reasons = retained
        .lines()
        .filter(|line| line.starts_with("reason ="))
        .collect::<Vec<_>>();
    assert_eq!(reasons.len(), 1);
    assert!(reviewed.lines().any(|line| line == reasons[0]));
    assert!(reasons[0].contains("human-reviewed"));
}
