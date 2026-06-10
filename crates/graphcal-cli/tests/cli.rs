//! Allow use of unwrap in tests
#![cfg(test)]

use std::path::{Path, PathBuf};
use std::process::Command;

fn graphcal_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_graphcal"))
}

fn fixtures_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p.push("tests");
    p.push("fixtures");
    p
}

fn fixture(name: &str) -> String {
    format!(
        "{}/tests/fixtures/{}",
        env!("CARGO_MANIFEST_DIR").trim_end_matches("crates/graphcal-cli"),
        name
    )
}

fn write_temp_file(root: &Path, rel: &str, source: &str) -> PathBuf {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, source).unwrap();
    path
}

#[test]
fn eval_rocket_text_output() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/rocket.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();

    // Source order: dry_mass, fuel_mass, isp, g0, v_exhaust, mass_ratio, delta_v
    assert_eq!(lines.len(), 7);
    assert!(lines[0].contains("dry_mass"));
    assert!(lines[1].contains("fuel_mass"));
    assert!(lines[2].contains("isp"));
    assert!(lines[3].contains("g0"));
    assert!(lines[4].contains("v_exhaust"));
    assert!(lines[5].contains("mass_ratio"));
    assert!(lines[6].contains("delta_v"));

    // Check values
    assert!(lines[0].contains("1200"));
    assert!(lines[3].contains("9.80665"));
    assert!(lines[4].contains("3138.128"));
}

#[test]
fn eval_rocket_json_output() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/rocket.gcl"), "--format", "json"])
        .output()
        .expect("failed to run graphcal");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("invalid JSON");

    assert!(json["const"]["g0"]["si_value"].as_f64().is_some());
    assert!(
        (json["param"]["dry_mass"]["si_value"].as_f64().unwrap() - 1200.0).abs() < f64::EPSILON
    );
    assert!(json["node"]["v_exhaust"]["si_value"].as_f64().is_some());
}

#[test]
fn eval_nonexistent_file_fails() {
    let output = graphcal_bin()
        .args(["eval", "nonexistent.gcl"])
        .output()
        .expect("failed to run graphcal");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("file not found"),
        "expected 'file not found' in stderr: {stderr}"
    );
}
#[test]
fn eval_indexed_text_output() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/indexed.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();

    // Indexed values flatten: delta_v[Departure], delta_v[Correction], delta_v[Insertion], etc.
    // Check key lines exist
    assert!(
        lines.iter().any(|l| l.contains("delta_v[Departure]")),
        "missing delta_v[Departure]: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("total_dv")),
        "missing total_dv: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("cumulative_dv[Insertion]")),
        "missing cumulative_dv[Insertion]: {lines:?}"
    );
}

#[test]
fn eval_indexed_json_output() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/indexed.gcl"), "--format", "json"])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("invalid JSON");

    // delta_v is an indexed param
    let dv = &json["param"]["delta_v"];
    assert_eq!(dv["index"].as_str(), Some("Maneuver"));
    assert!(dv["entries"]["Departure"]["si_value"].as_f64().is_some());

    // total_dv is a scalar node
    assert!(json["node"]["total_dv"]["si_value"].as_f64().is_some());
}

#[test]
fn eval_same_leaf_imported_indexes_display_as_boundary_leaf_names() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub index Phase = { Burn, Coast };\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub index Phase = { Burn, Coast };\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node phase_a: a.Phase = a.Phase.Burn;\n\
         node phase_b: b.Phase = b.Phase.Burn;\n\
         node series_a: Dimensionless[a.Phase] = for p: a.Phase { 1.0 };\n\
         node series_b: Dimensionless[b.Phase] = for p: b.Phase { 2.0 };\n",
    )
    .unwrap();

    let output = graphcal_bin()
        .args(["eval", root.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("phase_a"), "stdout: {stdout}");
    assert!(stdout.contains("Phase.Burn"), "stdout: {stdout}");
    assert!(stdout.contains("series_a[Burn]"), "stdout: {stdout}");
    assert!(stdout.contains("series_b[Coast]"), "stdout: {stdout}");

    let output = graphcal_bin()
        .args(["eval", root.to_str().unwrap(), "--format", "json"])
        .output()
        .expect("failed to run graphcal");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("invalid JSON");
    assert_eq!(json["node"]["phase_a"]["index"].as_str(), Some("Phase"));
    assert_eq!(json["node"]["phase_b"]["index"].as_str(), Some("Phase"));
    assert_eq!(json["node"]["series_a"]["index"].as_str(), Some("Phase"));
    assert_eq!(json["node"]["series_b"]["index"].as_str(), Some("Phase"));
}

#[test]
fn check_rejects_duplicate_expected_fail_variant() {
    // Duplicate expected_fail keys are ambiguous and must be rejected at check time.
    let dir = tempfile::tempdir().unwrap();
    let root = write_temp_file(
        dir.path(),
        "main.gcl",
        r"
pub index Mode = { A, B };
param lhs: Dimensionless[Mode] = { Mode.A: 1.0, Mode.B: 1.0 };
param rhs: Dimensionless[Mode] = { Mode.A: 2.0, Mode.B: 0.0 };
#[expected_fail(Mode.A, Mode.A)]
assert order = for m: Mode { @lhs[m] > @rhs[m] };
",
    );

    let output = graphcal_bin()
        .args(["check", root.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    assert!(
        !output.status.success(),
        "duplicate expected_fail keys should be rejected"
    );
}

#[test]
fn check_rejects_foreign_expected_fail_variant() {
    // Expected-fail keys must use the assertion's semantic index, not a foreign
    // index with the same variant leaves.
    let dir = tempfile::tempdir().unwrap();
    let root = write_temp_file(
        dir.path(),
        "main.gcl",
        r"
pub index Mode = { A, B };
pub index Other = { A, B };
param lhs: Dimensionless[Mode] = { Mode.A: 1.0, Mode.B: 1.0 };
param rhs: Dimensionless[Mode] = { Mode.A: 2.0, Mode.B: 0.0 };
#[expected_fail(Other.A)]
assert order = for m: Mode { @lhs[m] > @rhs[m] };
",
    );

    let output = graphcal_bin()
        .args(["check", root.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    assert!(
        !output.status.success(),
        "expected_fail keys must belong to the assertion index"
    );
}

#[test]
fn check_rejects_duplicate_expected_fail_tuple() {
    // Duplicate multi-index expected_fail tuple keys must be rejected at check time.
    let dir = tempfile::tempdir().unwrap();
    let root = write_temp_file(
        dir.path(),
        "main.gcl",
        r"
pub index Mode = { A, B };
pub index Phase = { Hot, Cold };
param lhs: Dimensionless[Mode, Phase] = for m: Mode, p: Phase { 1.0 };
param rhs: Dimensionless[Mode, Phase] = for m: Mode, p: Phase {
    match p {
        Phase.Hot => 2.0,
        Phase.Cold => 0.0,
    }
};
#[expected_fail((Mode.A, Phase.Hot), (Mode.A, Phase.Hot))]
assert order = for m: Mode, p: Phase { @lhs[m, p] > @rhs[m, p] };
",
    );

    let output = graphcal_bin()
        .args(["check", root.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    assert!(
        !output.status.success(),
        "duplicate expected_fail tuple keys should be rejected"
    );
}

#[test]
fn check_rejects_partial_expected_fail_tuple() {
    // Multi-index assertions require full tuple keys.
    let dir = tempfile::tempdir().unwrap();
    let root = write_temp_file(
        dir.path(),
        "main.gcl",
        r"
pub index Mode = { A, B };
pub index Phase = { Hot, Cold };
param lhs: Dimensionless[Mode, Phase] = for m: Mode, p: Phase { 1.0 };
param rhs: Dimensionless[Mode, Phase] = for m: Mode, p: Phase { 2.0 };
#[expected_fail(Mode.A)]
assert order = for m: Mode, p: Phase { @lhs[m, p] > @rhs[m, p] };
",
    );

    let output = graphcal_bin()
        .args(["check", root.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    assert!(
        !output.status.success(),
        "multi-index expected_fail should require full tuple keys"
    );
}

#[test]
fn check_rejects_wrong_axis_expected_fail_tuple() {
    // Multi-index tuple keys must match the assertion's axis order exactly.
    let dir = tempfile::tempdir().unwrap();
    let root = write_temp_file(
        dir.path(),
        "main.gcl",
        r"
pub index Mode = { A, B };
pub index Phase = { Hot, Cold };
param lhs: Dimensionless[Mode, Phase] = for m: Mode, p: Phase { 1.0 };
param rhs: Dimensionless[Mode, Phase] = for m: Mode, p: Phase { 2.0 };
#[expected_fail((Phase.Hot, Mode.A))]
assert order = for m: Mode, p: Phase { @lhs[m, p] > @rhs[m, p] };
",
    );

    let output = graphcal_bin()
        .args(["check", root.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    assert!(
        !output.status.success(),
        "expected_fail tuple keys should match assertion axis order"
    );
}

#[test]
fn check_rejects_variant_arg_on_scalar_expected_fail() {
    // Scalar assertions cannot accept per-variant expected_fail metadata.
    let dir = tempfile::tempdir().unwrap();
    let root = write_temp_file(
        dir.path(),
        "main.gcl",
        r"
pub index Mode = { A, B };
param lhs: Dimensionless = 1.0;
param rhs: Dimensionless = 2.0;
#[expected_fail(Mode.A)]
assert order = @lhs > @rhs;
",
    );

    let output = graphcal_bin()
        .args(["check", root.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    assert!(
        !output.status.success(),
        "scalar expected_fail should not accept per-variant keys"
    );
}

#[test]
fn check_rejects_foreign_expected_fail_when_all_pass() {
    // Foreign expected_fail keys are invalid even if the assertion currently passes.
    let dir = tempfile::tempdir().unwrap();
    let root = write_temp_file(
        dir.path(),
        "main.gcl",
        r"
pub index Mode = { A, B };
pub index Other = { A, B };
param lhs: Dimensionless[Mode] = { Mode.A: 3.0, Mode.B: 3.0 };
param rhs: Dimensionless[Mode] = { Mode.A: 2.0, Mode.B: 2.0 };
#[expected_fail(Other.A)]
assert order = for m: Mode { @lhs[m] > @rhs[m] };
",
    );

    let output = graphcal_bin()
        .args(["check", root.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    assert!(
        !output.status.success(),
        "foreign expected_fail keys should be rejected before evaluation"
    );
}

#[test]
fn check_rejects_private_include_output() {
    // Include brace selection must not expose a private node from a public DAG
    // in another module.
    let dir = tempfile::tempdir().unwrap();
    let pkg = dir.path().join("src/pkg");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"pkg\"\n",
    )
    .unwrap();
    std::fs::write(
        pkg.join("lib.gcl"),
        "pub dag helper {\n  param x: Dimensionless;\n  node hidden: Dimensionless = @x + 1.0;\n}\n",
    )
    .unwrap();
    let root = write_temp_file(
        dir.path(),
        "src/pkg/main.gcl",
        "include pkg.lib.helper(x: 1.0).{ hidden };\nnode y: Dimensionless = @hidden;\n",
    );

    let output = graphcal_bin()
        .args([
            "check",
            "--root",
            dir.path().to_str().unwrap(),
            root.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run graphcal");
    assert!(
        !output.status.success(),
        "include should not expose private outputs across modules"
    );
}

#[test]
fn check_rejects_private_include_output_renamed() {
    // Include brace renaming must not expose a private node from a public DAG
    // in another module.
    let dir = tempfile::tempdir().unwrap();
    let pkg = dir.path().join("src/pkg");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"pkg\"\n",
    )
    .unwrap();
    std::fs::write(
        pkg.join("lib.gcl"),
        "pub dag helper {\n  param x: Dimensionless;\n  node hidden: Dimensionless = @x + 1.0;\n}\n",
    )
    .unwrap();
    let root = write_temp_file(
        dir.path(),
        "src/pkg/main.gcl",
        "include pkg.lib.helper(x: 1.0).{ hidden as leaked };\nnode y: Dimensionless = @leaked;\n",
    );

    let output = graphcal_bin()
        .args([
            "check",
            "--root",
            dir.path().to_str().unwrap(),
            root.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run graphcal");
    assert!(
        !output.status.success(),
        "include output renaming should not bypass visibility"
    );
}

#[test]
fn check_rejects_private_include_output_via_alias() {
    // Include module aliases must not expose a private node from a public DAG
    // in another module.
    let dir = tempfile::tempdir().unwrap();
    let pkg = dir.path().join("src/pkg");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"pkg\"\n",
    )
    .unwrap();
    std::fs::write(
        pkg.join("lib.gcl"),
        "pub dag helper {\n  param x: Dimensionless;\n  node hidden: Dimensionless = @x + 1.0;\n}\n",
    )
    .unwrap();
    let root = write_temp_file(
        dir.path(),
        "src/pkg/main.gcl",
        "include pkg.lib.helper(x: 1.0) as h;\nnode y: Dimensionless = @h.hidden;\n",
    );

    let output = graphcal_bin()
        .args([
            "check",
            "--root",
            dir.path().to_str().unwrap(),
            root.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run graphcal");
    assert!(
        !output.status.success(),
        "include aliases should not expose private outputs"
    );
}

#[test]
fn check_rejects_private_dag_include() {
    // A private DAG must not be instantiated from another module by full path.
    let dir = tempfile::tempdir().unwrap();
    let pkg = dir.path().join("src/pkg");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"pkg\"\n",
    )
    .unwrap();
    std::fs::write(
        pkg.join("lib.gcl"),
        "dag helper {\n  param x: Dimensionless;\n  pub node shown: Dimensionless = @x + 1.0;\n}\n",
    )
    .unwrap();
    let root = write_temp_file(
        dir.path(),
        "src/pkg/main.gcl",
        "include pkg.lib.helper(x: 1.0).{ shown };\nnode y: Dimensionless = @shown;\n",
    );

    let output = graphcal_bin()
        .args([
            "check",
            "--root",
            dir.path().to_str().unwrap(),
            root.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run graphcal");
    assert!(
        !output.status.success(),
        "private DAGs should not be includable across modules"
    );
}

#[test]
fn eval_invalid_syntax_fails() {
    // Create a temp file with invalid syntax
    let dir = std::env::temp_dir().join("graphcal_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bad.gcl");
    std::fs::write(&path, "this is not valid graphcal").unwrap();

    let output = graphcal_bin()
        .args(["eval", path.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");

    assert!(!output.status.success());
    std::fs::remove_dir_all(&dir).ok();
}

// --- --set flag tests ---

#[test]
fn eval_with_set_flag() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/rocket.gcl"), "--set", "isp=450.0 s"])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    // isp should show 450, not the default 320
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("isp") && l.contains("450")),
        "expected isp=450 in output: {stdout}"
    );
    // delta_v should be higher than default (3778)
    let dv_line = stdout.lines().find(|l| l.contains("delta_v")).unwrap();
    assert!(
        dv_line.contains("5313"),
        "expected delta_v ~5313 with isp=450: {dv_line}"
    );
}

#[test]
fn eval_with_multiple_set() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/rocket.gcl"),
            "--set",
            "isp=450.0 s",
            "--set",
            "dry_mass=1500.0 kg",
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("isp") && l.contains("450")),
        "expected isp=450: {stdout}"
    );
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("dry_mass") && l.contains("1500")),
        "expected dry_mass=1500: {stdout}"
    );
}

#[test]
fn eval_set_invalid_param() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/rocket.gcl"),
            "--set",
            "nonexistent=100",
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("nonexistent"),
        "expected error mentioning 'nonexistent': {stderr}"
    );
}

#[test]
fn eval_user_defined_dimensions() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/user_dimensions.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();

    assert_eq!(lines.len(), 3);
    assert!(lines[0].contains("storage"));
    assert!(lines[0].contains("kB"));
    assert!(lines[1].contains("rate"));
    assert!(lines[1].contains("bit/s"));
    assert!(lines[2].contains("transfer_time"));
    assert!(lines[2].contains("40000"));
    assert!(lines[2].contains(" s"));
}

#[test]
fn eval_set_node_error() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/rocket.gcl"),
            "--set",
            "delta_v=100.0 m/s",
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("node"),
        "expected error mentioning 'node': {stderr}"
    );
}

#[test]
fn eval_set_bad_value() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/rocket.gcl"), "--set", "isp=???"])
        .output()
        .expect("failed to run graphcal");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("error"), "expected parse error: {stderr}");
}

// --- Multi-file import tests ---

#[test]
fn eval_missing_import_error() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("invalid/multi/missing_module/src/missing_module/main.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(!output.status.success());
}

// --- Tagged union tests ---

#[test]
fn eval_tagged_union_text_output() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/tagged_union.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();

    // Union type value shows fields directly: maneuver.thrust, maneuver.duration
    assert!(
        lines.iter().any(|l| l.contains("maneuver.thrust")),
        "expected maneuver.thrust in output: {stdout}"
    );
    assert!(
        lines.iter().any(|l| l.contains("maneuver.duration")),
        "expected maneuver.duration in output: {stdout}"
    );

    // Single-variant (struct sugar) shows flat fields: transfer.dv1
    assert!(
        lines.iter().any(|l| l.contains("transfer.dv1")),
        "expected transfer.dv1 in output: {stdout}"
    );
    assert!(
        lines.iter().any(|l| l.contains("transfer.dv2")),
        "expected transfer.dv2 in output: {stdout}"
    );

    // Bare variant displays as label
    assert!(
        lines
            .iter()
            .any(|l| l.contains("current_status") && l.contains("Nominal")),
        "expected current_status = Nominal in output: {stdout}"
    );
}

#[test]
fn eval_tagged_union_json_output() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/tagged_union.gcl"),
            "--format",
            "json",
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("invalid JSON");

    // Union type value shows concrete type name
    let maneuver = &json["node"]["maneuver"];
    assert_eq!(maneuver["type"].as_str(), Some("LowThrust"));
    assert!(maneuver["fields"]["thrust"]["si_value"].as_f64().is_some());

    // Record type (struct sugar)
    let transfer = &json["node"]["transfer"];
    assert_eq!(transfer["type"].as_str(), Some("TransferResult"));

    // Unit type value shows concrete type name
    let status = &json["node"]["current_status"];
    assert_eq!(status["type"].as_str(), Some("Nominal"));
}

#[test]
fn eval_import_name_not_found() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("invalid/multi/bad_name_import/src/bad_name_import/main.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("nonexistent"),
        "expected error mentioning 'nonexistent': {stderr}"
    );
}

#[test]
fn check_rejects_type_only_import_for_constructor() {
    let output = graphcal_bin()
        .args([
            "check",
            &fixture("invalid/inline_dag_type_import_without_constructor.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        !output.status.success(),
        "type-only import must not bring the constructor into scope"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("TransferResult"),
        "expected error mentioning TransferResult: {stderr}"
    );
}

// --- --input JSON file tests ---

#[test]
fn eval_with_input_json() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/rocket.gcl"),
            "--input",
            &fixture("valid/input_rocket.json"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    // dry_mass should show 1500 (from JSON), not default 1200
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("dry_mass") && l.contains("1500")),
        "expected dry_mass=1500 in output: {stdout}"
    );
    // isp should show 450 (from JSON), not default 320
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("isp") && l.contains("450")),
        "expected isp=450 in output: {stdout}"
    );
}

#[test]
fn eval_input_json_set_precedence() {
    // --set should override the same param from --input
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/rocket.gcl"),
            "--input",
            &fixture("valid/input_rocket.json"),
            "--set",
            "isp=500.0 s",
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    // isp should show 500 (from --set), not 450 (from JSON)
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("isp") && l.contains("500")),
        "expected isp=500 in output: {stdout}"
    );
    // dry_mass should still come from JSON
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("dry_mass") && l.contains("1500")),
        "expected dry_mass=1500 in output: {stdout}"
    );
}

#[test]
fn eval_input_json_indexed() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/indexed.gcl"),
            "--input",
            &fixture("valid/input_indexed.json"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    // delta_v[Departure] should show 3000 (3.0 km/s in SI)
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("Departure") && l.contains('3')),
        "expected Departure delta_v ~3 km/s in output: {stdout}"
    );
}

#[test]
fn eval_input_json_tagged_union() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/tagged_union_param.gcl"),
            "--input",
            &fixture("valid/input_tagged_union.json"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    // maneuver should now be Impulsive (from JSON), not LowThrust (default)
    assert!(
        stdout.lines().any(|l| l.contains("maneuver.delta_v")),
        "expected maneuver.delta_v in output: {stdout}"
    );
    // fuel_proxy should be 0 N (Impulsive branch returns 0)
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("fuel_proxy") && l.contains('0')),
        "expected fuel_proxy=0 in output: {stdout}"
    );
}

#[test]
fn eval_input_json_unknown_param() {
    let dir = std::env::temp_dir().join("graphcal_test_input");
    std::fs::create_dir_all(&dir).unwrap();
    let json_path = dir.join("bad_param.json");
    std::fs::write(&json_path, r#"{"nonexistent": "100.0 kg"}"#).unwrap();

    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/rocket.gcl"),
            "--input",
            json_path.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run graphcal");

    // Should fail because "nonexistent" is not a param in rocket.gcl
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("nonexistent"),
        "expected error mentioning 'nonexistent': {stderr}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn eval_input_json_invalid_json() {
    let dir = std::env::temp_dir().join("graphcal_test_input_bad");
    std::fs::create_dir_all(&dir).unwrap();
    let json_path = dir.join("bad.json");
    std::fs::write(&json_path, "not valid json {{{").unwrap();

    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/rocket.gcl"),
            "--input",
            json_path.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("error"),
        "expected JSON parse error: {stderr}"
    );
    std::fs::remove_dir_all(&dir).ok();
}
// --- Assertion tests ---

#[test]
fn eval_assertions_pass() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/assertions.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("Assertions:"),
        "expected Assertions section: {stdout}"
    );
    assert!(
        stdout.contains("velocity_in_range") && stdout.contains("PASS"),
        "expected velocity_in_range PASS: {stdout}"
    );
    assert!(
        stdout.contains("mass_approx") && stdout.contains("PASS"),
        "expected mass_approx PASS: {stdout}"
    );
    assert!(
        stdout.contains("velocity_approx") && stdout.contains("PASS"),
        "expected velocity_approx PASS: {stdout}"
    );
}

#[test]
fn eval_assertions_pass_json() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/assertions.gcl"), "--format", "json"])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("invalid JSON");
    assert_eq!(
        json["assert"]["velocity_in_range"]["status"].as_str(),
        Some("pass")
    );
    assert_eq!(
        json["assert"]["mass_approx"]["status"].as_str(),
        Some("pass")
    );
    assert_eq!(
        json["assert"]["velocity_approx"]["status"].as_str(),
        Some("pass")
    );
}

#[test]
fn eval_assertions_fail_exit_code() {
    let output = graphcal_bin()
        .args(["eval", &fixture("runtime_error/assertions_fail.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected exit code 1 for assertion failure"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("x_greater") && stderr.contains("FAIL"),
        "expected x_greater FAIL in stderr: {stderr}"
    );
}

#[test]
fn eval_assertions_tolerance_fail() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("runtime_error/assertions_tolerance_fail.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected exit code 1 for tolerance failure"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("tight_check") && stderr.contains("FAIL"),
        "expected tight_check FAIL: {stderr}"
    );
    assert!(
        stderr.contains("off by"),
        "expected tolerance detail in message: {stderr}"
    );
}

#[test]
fn eval_assertions_assumes_affected_nodes() {
    let output = graphcal_bin()
        .args(["eval", &fixture("runtime_error/assertions_assumes.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected exit code 1 for assumed assertion failure"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("pressure_safe") && stderr.contains("FAIL"),
        "expected pressure_safe FAIL: {stderr}"
    );
    assert!(
        stderr.contains("affected") && stderr.contains("margin"),
        "expected affected: margin in output: {stderr}"
    );
}

#[test]
fn eval_assertions_indexed_fail() {
    let output = graphcal_bin()
        .args(["eval", &fixture("runtime_error/assertions_indexed.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected exit code 1 for indexed assertion failure"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("power_ok") && stderr.contains("FAIL"),
        "expected power_ok FAIL: {stderr}"
    );
    assert!(
        stderr.contains("Boost"),
        "expected Boost variant in failure message: {stderr}"
    );
    // Multi-index assertion: within_limits should fail with parenthesized paths
    assert!(
        stderr.contains("within_limits") && stderr.contains("FAIL"),
        "expected within_limits FAIL: {stderr}"
    );
    assert!(
        stderr.contains("(Mode.Normal, Phase.Cruise)"),
        "expected parenthesized multi-index path in failure message: {stderr}"
    );
}
#[test]
fn eval_assertions_compile_error_exit_code() {
    let output = graphcal_bin()
        .args(["eval", &fixture("invalid/assert_not_bool.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert_eq!(
        output.status.code(),
        Some(2),
        "expected exit code 2 for compile error"
    );
}
#[test]
fn eval_explicit_index_import() {
    // Bug 3: `import "./lib.gcl" { Color }` should import the Color index explicitly.
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/multi/explicit_index/src/lib/main.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("favorite") && l.contains("Red") && l.contains('1')),
        "expected favorite[Red] = 1 in output: {stdout}"
    );
}

// --- Variant comparison tests ---

#[test]
fn eval_variant_comparison() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/variant_comparison.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    // selective[Departure] = 2*2460 = 4920 m/s (doubled)
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("selective[Departure]") && l.contains("4920")),
        "expected selective[Departure] = 4920 in output: {stdout}"
    );
    // selective[Correction] = 120 m/s (unchanged)
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("selective[Correction]") && l.contains("120")),
        "expected selective[Correction] = 120 in output: {stdout}"
    );

    // selective2[Insertion] = 3*1830 = 5490 m/s (tripled, variant on LHS)
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("selective2[Insertion]") && l.contains("5490")),
        "expected selective2[Insertion] = 5490 in output: {stdout}"
    );

    // not_correction[Correction] = 0 m/s (zeroed via !=)
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("not_correction[Correction]") && l.contains("0 m/s")),
        "expected not_correction[Correction] = 0 in output: {stdout}"
    );
}

// --- Variant match tests ---

#[test]
fn eval_variant_match() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/variant_match.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    // scale_factor[Departure] = 2
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("scale_factor[Departure]") && l.contains('2')),
        "expected scale_factor[Departure] = 2 in output: {stdout}"
    );
    // scaled_dv[Departure] = 2460 * 2 = 4920
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("scaled_dv[Departure]") && l.contains("4920")),
        "expected scaled_dv[Departure] = 4920 in output: {stdout}"
    );
    // scaled_dv[Correction] = 120 * 0.5 = 60
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("scaled_dv[Correction]") && l.contains("60")),
        "expected scaled_dv[Correction] = 60 in output: {stdout}"
    );

    // Multi-binding match: adjusted_cost is a 2D table.
    // Check the table header and key values.
    assert!(
        stdout.contains("adjusted_cost"),
        "expected adjusted_cost table in output: {stdout}"
    );
    // Departure row, Burn column = 2706
    assert!(
        stdout.contains("2706"),
        "expected 2706 (adjusted_cost[Departure][Burn]) in output: {stdout}"
    );
    // Departure row, Coast column = 0
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("Departure") && l.contains('0') && l.contains("2706")),
        "expected Departure row with 0 and 2706 in output: {stdout}"
    );
}

// --- Large / realistic fixture tests ---

#[test]
fn eval_power_budget() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/power_budget.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    // Check key computed nodes exist
    assert!(
        stdout.lines().any(|l| l.contains("peak_power")),
        "expected peak_power in output: {stdout}"
    );
    assert!(
        stdout.lines().any(|l| l.contains("battery_dod")),
        "expected battery_dod in output: {stdout}"
    );
    assert!(
        stdout.lines().any(|l| l.contains("sa_margin")),
        "expected sa_margin in output: {stdout}"
    );

    // Check assertions
    assert!(
        stdout.contains("sa_positive_margin") && stdout.contains("PASS"),
        "expected sa_positive_margin PASS: {stdout}"
    );
    assert!(
        stdout.contains("battery_dod_safe") && stdout.contains("PASS"),
        "expected battery_dod_safe PASS: {stdout}"
    );
}

#[test]
fn eval_multi_decl_sliced() {
    // Multi-decl v3: multi-axis shared prefix with slice sections.
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/multi_decl_sliced.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    for phase in ["Launch", "Cruise", "Arrival"] {
        assert!(
            stdout.contains(phase),
            "expected {phase} in output: {stdout}",
        );
    }
    assert!(
        stdout.contains("total_active_power") && stdout.contains("peak_active_power"),
        "expected derived nodes in output: {stdout}",
    );
}

#[test]
fn eval_multi_decl_2d() {
    // Multi-decl v2: mixed 1-D and 2-D slots sharing one row axis.
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/multi_decl_2d.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    // The 2-D slot should render as a 2-D table with Safe / Nominal columns.
    assert!(
        stdout.contains("power_mode_active")
            && stdout.contains("Safe")
            && stdout.contains("Nominal"),
        "expected 2-D power_mode_active in output: {stdout}",
    );
    // Derived node that reads from both 1-D and 2-D slots.
    assert!(
        stdout.contains("total_safe_power"),
        "expected total_safe_power in output: {stdout}",
    );
}

#[test]
fn eval_multi_decl_1d() {
    // Multi-decl (issue #481) v1: homogeneous 1-D slots across
    // param/const-node kinds must evaluate end-to-end.
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/multi_decl_1d.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    // Every slot in the multi-decl should appear as its own declaration.
    for name in ["power_consumption", "duty_cycle", "mass_per_unit"] {
        assert!(
            stdout.contains(name),
            "expected `{name}` in eval output: {stdout}",
        );
    }
    // Derived node reading cross-slot values.
    assert!(
        stdout.contains("peak_power"),
        "expected peak_power in output: {stdout}"
    );
}

#[test]
fn eval_power_budget_json() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/power_budget.gcl"),
            "--format",
            "json",
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("invalid JSON");

    // power_draw is a 2D indexed param
    let pd = &json["param"]["power_draw"];
    assert!(
        pd["entries"].is_object(),
        "expected power_draw entries: {pd}"
    );

    // peak_power is a scalar node
    assert!(
        json["node"]["peak_power"]["si_value"].as_f64().is_some(),
        "expected peak_power scalar value"
    );

    // assertions
    assert_eq!(
        json["assert"]["sa_positive_margin"]["status"].as_str(),
        Some("pass")
    );
    assert_eq!(
        json["assert"]["battery_dod_safe"]["status"].as_str(),
        Some("pass")
    );
}

#[test]
fn eval_thermal_analysis() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/thermal_analysis.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    // Check key outputs
    assert!(
        stdout.lines().any(|l| l.contains("total_heater_power")),
        "expected total_heater_power in output: {stdout}"
    );
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("total_radiative_capacity")),
        "expected total_radiative_capacity in output: {stdout}"
    );

    // Check assertions
    assert!(
        stdout.contains("heater_budget_reasonable") && stdout.contains("PASS"),
        "expected heater_budget_reasonable PASS: {stdout}"
    );
    assert!(
        stdout.contains("has_radiative_capacity") && stdout.contains("PASS"),
        "expected has_radiative_capacity PASS: {stdout}"
    );
}

#[test]
fn eval_parenthesized_exprs() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/parenthesized_exprs.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    // Check key outputs
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("absorbed_power") && !l.contains("PASS")),
        "expected absorbed_power in output: {stdout}"
    );
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("voltage") && l.contains("50")),
        "expected voltage = 50 in output: {stdout}"
    );

    // All assertions should pass
    assert!(
        stdout.contains("absorbed_power_positive") && stdout.contains("PASS"),
        "expected absorbed_power_positive PASS: {stdout}"
    );
    assert!(
        stdout.contains("voltage_correct") && stdout.contains("PASS"),
        "expected voltage_correct PASS: {stdout}"
    );
    assert!(
        stdout.contains("charge_time_positive") && stdout.contains("PASS"),
        "expected charge_time_positive PASS: {stdout}"
    );
} // --- Expected-fail tests ---

#[test]
fn eval_expected_fail_pass() {
    // A failing assertion marked #[expected_fail] should invert to pass
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/expected_fail_pass.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "expected success for expected_fail on failing assertion, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("x_greater") && stdout.contains("PASS"),
        "expected x_greater PASS (inverted): {stdout}"
    );
}

#[test]
fn eval_expected_fail_unexpected_pass() {
    // A passing assertion marked #[expected_fail] should invert to fail
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("runtime_error/expected_fail_unexpected_pass.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected exit code 1 for unexpected pass"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("x_less") && stderr.contains("FAIL"),
        "expected x_less FAIL (unexpected pass): {stderr}"
    );
    assert!(
        stderr.contains("expected_fail"),
        "expected mention of expected_fail in message: {stderr}"
    );
}

#[test]
fn eval_expected_fail_on_node_error() {
    // #[expected_fail] on a node should produce a compile error
    let output = graphcal_bin()
        .args(["eval", &fixture("invalid/expected_fail_on_node.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert_eq!(
        output.status.code(),
        Some(2),
        "expected exit code 2 for compile error"
    );
}

#[test]
fn eval_expected_fail_all_on_indexed_error() {
    // #[expected_fail] without arguments on an indexed assertion should produce a compile error
    let output = graphcal_bin()
        .args(["eval", &fixture("invalid/expected_fail_all_on_indexed.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert_eq!(
        output.status.code(),
        Some(2),
        "expected exit code 2 for compile error"
    );
}

#[test]
fn eval_expected_fail_indexed_partial() {
    // Per-variant expected_fail should only suppress the specified variant;
    // other failing variants should still be reported.
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("runtime_error/expected_fail_indexed_partial.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected exit code 1: Eco fails but is not expected_fail"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("power_ok") && stderr.contains("FAIL") && stderr.contains("Mode.Eco"),
        "expected power_ok FAIL with Mode.Eco: {stderr}"
    );
}

#[test]
fn eval_expected_fail_indexed_unexpected_pass() {
    // Per-variant expected_fail where the expected-fail variant actually passes
    // should report "unexpected pass".
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("runtime_error/expected_fail_indexed_unexpected_pass.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected exit code 1: Boost passes but is marked expected_fail"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("power_ok")
            && stderr.contains("FAIL")
            && stderr.contains("unexpected pass"),
        "expected power_ok FAIL with unexpected pass: {stderr}"
    );
}

#[test]
fn eval_expected_fail_multi_indexed_partial() {
    // Per-tuple-key expected_fail should only suppress specified tuple keys;
    // other failing keys should still be reported.
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("runtime_error/expected_fail_multi_indexed_partial.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected exit code 1: (Eco, Cruise) fails but is not expected_fail"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("within_limits") && stderr.contains("FAIL") && stderr.contains("Eco"),
        "expected within_limits FAIL with Eco: {stderr}"
    );
}

// --- Format command tests ---

#[test]
fn format_check_already_formatted() {
    // rocket.gcl is a formatter-tested fixture and should already be formatted.
    let output = graphcal_bin()
        .args(["format", "--check", &fixture("valid/rocket.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "expected success for already-formatted file, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn format_check_unformatted_exits_nonzero() {
    // Create a temp file with valid but unformatted graphcal
    let dir = std::env::temp_dir().join("graphcal_fmt_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("unformatted.gcl");
    // Extra spaces and missing trailing newline
    std::fs::write(&path, "param   x  :  Dimensionless  =   1.0  ;").unwrap();

    let output = graphcal_bin()
        .args(["format", "--check", path.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected exit code 1 for unformatted file"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("would be reformatted"),
        "expected 'would be reformatted' message: {stderr}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn format_check_parse_error_fails() {
    // Files with parse errors are failures: CI must not pass on broken files.
    let dir = std::env::temp_dir().join("graphcal_fmt_test_err");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bad.gcl");
    std::fs::write(&path, "this is }{ not valid").unwrap();

    let output = graphcal_bin()
        .args(["format", "--check", path.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");

    assert!(
        !output.status.success(),
        "expected failure on parse errors, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    // The formatter surfaces the specific parse-error message from the
    // underlying `FormatError::Parse` variant.
    assert!(
        stderr.contains("unexpected token") || stderr.contains("stray character"),
        "expected parse-error detail: {stderr}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn format_in_place_then_check() {
    // Format a file in-place, then verify --check passes (idempotency via CLI)
    let dir = std::env::temp_dir().join("graphcal_fmt_test_inplace");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("fixme.gcl");
    std::fs::write(
        &path,
        "param   x:Dimensionless=1.0;\nparam y  : Dimensionless = 2.0 ;  \n",
    )
    .unwrap();

    // Format in-place
    let output = graphcal_bin()
        .args(["format", path.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    assert!(
        output.status.success(),
        "format in-place failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Now --check should pass
    let output = graphcal_bin()
        .args(["format", "--check", path.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    assert!(
        output.status.success(),
        "expected --check to pass after formatting, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn format_check_recursive_directory() {
    // --check on a directory should recursively find .gcl files
    let output = graphcal_bin()
        .args(["format", "--check", &fixture("valid/multi/rocket_split")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "expected all multi/rocket_split files to be formatted, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn format_directory_skips_symlinked_gcl_files() {
    let dir = tempfile::tempdir().unwrap();
    let outside = dir.path().join("outside.gcl");
    let tree = dir.path().join("tree");
    std::fs::create_dir_all(&tree).unwrap();

    let original = "param   x:Dimensionless=1.0;";
    std::fs::write(&outside, original).unwrap();
    std::fs::write(tree.join("inside.gcl"), "param x: Dimensionless = 1.0;\n").unwrap();
    std::os::unix::fs::symlink(&outside, tree.join("link.gcl")).unwrap();

    let output = graphcal_bin()
        .args(["format", tree.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    assert!(
        output.status.success(),
        "BUG: directory formatter must skip symlinked .gcl files: format failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let after = std::fs::read_to_string(&outside).unwrap();
    assert_eq!(
        after, original,
        "BUG: directory formatter must skip symlinked .gcl files: symlink target outside the formatted tree was modified",
    );
}

#[test]
fn eval_datetime_basic() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/datetime_basic.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.contains("launch"), "should contain launch");
    assert!(
        stdout.contains("2024-11-05T12:00:00 UTC"),
        "launch should be 2024-11-05T12:00:00 UTC"
    );
    assert!(
        stdout.contains("2024-11-05T13:00:00 UTC"),
        "one_hour_later should be 2024-11-05T13:00:00 UTC"
    );
    assert!(stdout.contains("3600"), "duration should be 3600 s");
    assert!(
        stdout.contains("2024-11-05T11:00:00 UTC"),
        "one_hour_before should be 2024-11-05T11:00:00 UTC"
    );
    assert!(stdout.contains("PASS"), "assertions should pass");
}

#[test]
fn eval_datetime_epoch() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/datetime_epoch.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(
        stdout.contains("2024-11-05T12:00:00 TT"),
        "t_tt should be in TT scale"
    );
    assert!(
        stdout.contains("2024-11-05T12:00:00 TAI"),
        "t_tai should be in TAI scale"
    );
    assert!(
        stdout.contains("2024-11-05T12:00:00 GPST"),
        "t_gpst should be in GPST scale"
    );
    assert!(
        stdout.contains("2024-11-05T13:00:00 TT"),
        "t_tt_later should be one hour later in TT"
    );
    assert!(stdout.contains("3600"), "tt_dur should be 3600 s");
    assert!(stdout.contains("PASS"), "assertions should pass");
}

#[test]
fn eval_datetime_scale_mismatch_error() {
    let output = graphcal_bin()
        .args(["eval", &fixture("invalid/datetime_scale_mismatch.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        !output.status.success(),
        "cross-scale operation should fail"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("dimension mismatch") || stderr.contains("time scale"),
        "error should mention dimension mismatch or time scale"
    );
}

#[test]
fn eval_datetime_conversion() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/datetime_conversion.gcl")])
        .output()
        .expect("failed to run graphcal");

    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        output.status.success(),
        "datetime conversion should succeed.\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(stdout.contains("t_utc"), "should output t_utc");
    assert!(stdout.contains("t_tai"), "should output t_tai");
    assert!(stdout.contains("t_tt_back"), "should output t_tt_back");
    assert!(stdout.contains("t_gpst"), "should output t_gpst");
    assert!(
        stdout.contains("roundtrip     PASS"),
        "roundtrip assert should pass"
    );
    assert!(
        stdout.contains("same_instant  PASS"),
        "same_instant assert should pass"
    );
}

#[test]
fn eval_datetime_conversion_non_datetime_error() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("invalid/datetime_conversion_non_datetime.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        !output.status.success(),
        "to_utc on non-Datetime should fail"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("dimension mismatch") || stderr.contains("requires a Datetime"),
        "error should mention dimension mismatch or Datetime requirement"
    );
}

#[test]
fn format_check_multiple_fixtures() {
    // --check on multiple already-formatted fixtures
    let output = graphcal_bin()
        .args([
            "format",
            "--check",
            &fixture("valid/rocket.gcl"),
            &fixture("invalid/functions.gcl"),
            &fixture("valid/generics.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "expected all fixtures to be formatted, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn format_check_multi_decl_fixtures_idempotent() {
    // Multi-decl fixtures (issue #481) round-trip through the formatter —
    // the surface form is emitted verbatim, not re-desugared into N
    // single decls.
    let output = graphcal_bin()
        .args([
            "format",
            "--check",
            &fixture("valid/multi_decl_1d.gcl"),
            &fixture("valid/multi_decl_2d.gcl"),
            &fixture("valid/multi_decl_sliced.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "expected multi-decl fixtures to be formatted idempotently, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn eval_datetime_timezone() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/datetime_timezone.gcl")])
        .output()
        .expect("failed to run graphcal");

    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        output.status.success(),
        "datetime timezone should succeed.\nstdout: {stdout}\nstderr: {stderr}"
    );

    // Timezone display produces IANA-zoned output
    assert!(
        stdout.contains("Asia/Tokyo"),
        "launch_tokyo should display in Asia/Tokyo timezone"
    );
    assert!(
        stdout.contains("America/New_York"),
        "launch_ny should display in America/New_York timezone"
    );

    // Two-arg constructor resolves to UTC
    assert!(
        stdout.contains("meeting_tokyo"),
        "should output meeting_tokyo"
    );

    // All assertions pass
    assert!(
        stdout.contains("same_instant               PASS"),
        "same_instant assert should pass"
    );
    assert!(
        stdout.contains("same_instant_ny            PASS"),
        "same_instant_ny assert should pass"
    );
    assert!(
        stdout.contains("display_preserves_instant  PASS"),
        "display_preserves_instant assert should pass"
    );
    assert!(
        stdout.contains("arith_works                PASS"),
        "arith_works assert should pass"
    );
}

#[test]
fn eval_datetime_timezone_non_datetime_error() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("invalid/datetime_timezone_non_datetime.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        !output.status.success(),
        "timezone display on non-Datetime should fail"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("dimension mismatch") || stderr.contains("requires a Datetime"),
        "error should mention dimension mismatch or Datetime requirement"
    );
}

#[test]
fn eval_datetime_extract() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/datetime_extract.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.contains("y   = 2024"), "year should be 2024");
    assert!(stdout.contains("mo  = 11"), "month should be 11");
    assert!(stdout.contains("d   = 5"), "day should be 5");
    assert!(stdout.contains("h   = 14"), "hour should be 14");
    assert!(stdout.contains("mi  = 30"), "minute should be 30");
    assert!(stdout.contains("s   = 45"), "second should be 45");
    assert!(stdout.contains("wd  = 1"), "weekday should be 1 (Tuesday)");
    assert!(stdout.contains("doy = 310"), "day_of_year should be 310");
    assert!(!stdout.contains("FAIL"), "no assertions should fail");
}

#[test]
fn eval_datetime_extract_non_datetime_error() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("invalid/datetime_extract_non_datetime.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        !output.status.success(),
        "extraction on non-Datetime should fail"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("dimension mismatch") || stderr.contains("requires a Datetime"),
        "error should mention dimension mismatch or Datetime requirement"
    );
}

#[test]
fn eval_datetime_jd_unix() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/datetime_jd_unix.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(
        stdout.contains("unix_ts     = 1730808000"),
        "unix timestamp should be 1730808000"
    );
    assert!(!stdout.contains("FAIL"), "no assertions should fail");
}

// --- Instantiated import tests ---

#[test]
fn eval_instantiated_import_selective() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/multi/instantiated_import/src/rocket/main.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    // dry_mass=800, delta_v should be ~4719 m/s
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("result") && l.contains("4719")),
        "expected result ~4719 in output: {stdout}"
    );
}

// --- Partial overrides CLI tests ---

#[test]
fn eval_partial_set_uses_defaults() {
    // Partial --set falls back to defaults for the unset params.
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/rocket.gcl"), "--set", "isp=450.0 s"])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "partial --set should fall back to defaults: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn eval_no_overrides_defaults_freely() {
    // No --set or --input at all → defaults used freely, no error
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/rocket.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "no overrides should use defaults freely: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// --- Plot tests (Vega-Lite JSON) ---

fn parse_plot_json_stdout(stdout: &str) -> serde_json::Value {
    let json: serde_json::Value = serde_json::from_str(stdout).unwrap_or_else(|err| {
        panic!("expected --plot json stdout to be only JSON: {err}: {stdout}")
    });
    assert!(
        json.is_array(),
        "expected --plot json stdout to be a top-level array: {stdout}"
    );
    json
}

#[test]
fn eval_plot_json_output() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/plot_basic.gcl"), "--plot", "json"])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let _json = parse_plot_json_stdout(&stdout);
    // Vega-Lite specs: "mark": "line" and "mark": "bar"
    assert!(
        stdout.contains("\"mark\": \"line\""),
        "expected line mark in Vega-Lite JSON: {stdout}"
    );
    assert!(
        stdout.contains("\"mark\": \"bar\""),
        "expected bar mark in Vega-Lite JSON: {stdout}"
    );
    assert!(
        stdout.contains("vega-lite"),
        "expected Vega-Lite $schema: {stdout}"
    );
}

#[test]
fn eval_plot_json_suppresses_normal_eval_output_even_with_json_format() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/plot_basic.gcl"),
            "--format",
            "json",
            "--plot",
            "json",
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json = parse_plot_json_stdout(&stdout);
    assert!(
        json.get("param").is_none() && json.get("node").is_none(),
        "--plot json should print only the plot array, not eval JSON: {stdout}"
    );
}

#[test]
fn eval_plot_scatter_json() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/plot_scatter.gcl"), "--plot", "json"])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("\"mark\": \"point\""),
        "expected point mark for scatter: {stdout}"
    );
}

#[test]
fn eval_plot_line_json() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/plot_line.gcl"), "--plot", "json"])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("\"mark\": \"line\""),
        "expected line mark: {stdout}"
    );
}

#[test]
fn eval_plot_bar_json() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/plot_bar.gcl"), "--plot", "json"])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("\"mark\": \"bar\""),
        "expected bar mark: {stdout}"
    );
}

#[test]
fn eval_plot_heatmap_json() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/plot_heatmap.gcl"), "--plot", "json"])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("\"mark\": \"rect\""),
        "expected rect mark for heatmap: {stdout}"
    );
}

#[test]
fn eval_plot_no_plots_warns() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/rocket.gcl"), "--plot", "json"])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout.trim(),
        "[]",
        "expected --plot json stdout to remain valid JSON when no plots exist: {stdout}"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("no plot declarations found"),
        "expected warning about no plots: {stderr}"
    );
}

// --- Figure tests ---

#[test]
fn eval_figure_basic_json() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/figure_basic.gcl"), "--plot", "json"])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    let json = parse_plot_json_stdout(&stdout);
    let arr = json.as_array().expect("expected JSON array");
    // 3 figures: curve_a (standalone), curve_b (standalone), comparison (figure)
    assert_eq!(
        arr.len(),
        3,
        "expected 3 figures (2 standalone + 1 combined): {stdout}"
    );
    assert_eq!(arr[0]["name"].as_str(), Some("curve_a"));
    assert_eq!(arr[1]["name"].as_str(), Some("curve_b"));
    assert_eq!(arr[2]["name"].as_str(), Some("comparison"));

    // Standalone curve_a should have a line mark
    let curve_a_spec = &arr[0]["spec"];
    assert_eq!(
        curve_a_spec["mark"].as_str(),
        Some("line"),
        "expected line mark for curve_a: {curve_a_spec}"
    );

    // Standalone curve_b should have a bar mark
    let bar_spec = &arr[1]["spec"];
    assert_eq!(
        bar_spec["mark"].as_str(),
        Some("bar"),
        "expected bar mark for curve_b: {bar_spec}"
    );

    // Comparison figure should use hconcat with 2 sub-specs
    let comparison_hconcat = arr[2]["spec"]["hconcat"]
        .as_array()
        .expect("expected hconcat array in comparison");
    assert_eq!(
        comparison_hconcat.len(),
        2,
        "expected 2 sub-specs in comparison hconcat"
    );
}

#[test]
fn eval_figure_hidden_json() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/figure_hidden.gcl"),
            "--plot",
            "json",
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    let json = parse_plot_json_stdout(&stdout);
    let arr = json.as_array().expect("expected JSON array");
    // Only 1 figure: comparison (hidden plots suppress standalone output)
    assert_eq!(
        arr.len(),
        1,
        "expected 1 figure (hidden plots suppressed): {stdout}"
    );
    assert_eq!(arr[0]["name"].as_str(), Some("comparison"));

    // The comparison figure should still contain both sub-specs via hconcat
    let comparison_hconcat = arr[0]["spec"]["hconcat"]
        .as_array()
        .expect("expected hconcat array in comparison");
    assert_eq!(
        comparison_hconcat.len(),
        2,
        "expected 2 sub-specs in comparison hconcat even though plots are hidden"
    );
}

#[test]
fn eval_plot_basic_standalone_figures() {
    // plot_basic.gcl has 2 plots, no figures — should produce 2 standalone figures
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/plot_basic.gcl"), "--plot", "json"])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    let json = parse_plot_json_stdout(&stdout);
    let arr = json.as_array().expect("expected JSON array");
    assert_eq!(
        arr.len(),
        2,
        "expected 2 standalone figures from plot_basic.gcl: {stdout}"
    );
    assert_eq!(arr[0]["name"].as_str(), Some("my_line"));
    assert_eq!(arr[1]["name"].as_str(), Some("my_bar"));
}

#[test]
fn format_check_figure_fixtures() {
    let output = graphcal_bin()
        .args([
            "format",
            "--check",
            &fixture("valid/figure_basic.gcl"),
            &fixture("valid/figure_hidden.gcl"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "expected figure fixtures to be formatted, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// --- Dynamic units ---

#[test]
fn eval_dynamic_units() {
    let output = graphcal_bin()
        .args(["eval", &fixture("valid/dynamic_units.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    // price_eur = 100 EUR (100 * 1.08 = 108 USD in SI)
    assert!(stdout.contains("price_eur"), "missing price_eur");
    assert!(stdout.contains("EUR"), "missing EUR unit");

    // price_usd = 108 USD
    assert!(stdout.contains("price_usd"), "missing price_usd");
    assert!(stdout.contains("108"), "expected 108 USD");

    // total = 158 USD (108 + 50)
    assert!(stdout.contains("total"), "missing total");
    assert!(stdout.contains("158"), "expected 158 USD");
}

#[test]
fn eval_dynamic_units_with_override() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("valid/dynamic_units.gcl"),
            "--set",
            "usd_per_eur=1.20",
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    // With usd_per_eur=1.20: price_eur = 100 * 1.20 = 120 USD
    assert!(stdout.contains("120"), "expected 120 USD for price_usd");

    // total = 120 + 50 = 170 USD
    assert!(stdout.contains("170"), "expected 170 USD for total");
}

// ---------------------------------------------------------------------------
// Invariant: any fixture that fails `check` must also fail `eval`.
// `eval` runs the static check pipeline as its first stage, so this is
// currently maintained only by call-order in the CLI. If a future refactor
// lets `eval` skip part of the check pipeline, this test surfaces the
// regression rather than letting check-level diagnostics be silently
// bypassed.
// ---------------------------------------------------------------------------

fn collect_entry_points(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut local_gcls: Vec<PathBuf> = Vec::new();
    let mut subdirs: Vec<PathBuf> = Vec::new();
    let mut has_main = false;
    for entry in std::fs::read_dir(dir).expect("read fixture dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            subdirs.push(path);
        } else if path.is_file() && path.extension().is_some_and(|e| e == "gcl") {
            if path.file_name().is_some_and(|n| n == "main.gcl") {
                has_main = true;
            }
            local_gcls.push(path);
        } else {
            // skip non-gcl files (e.g. graphcal.toml, *.json fixtures)
        }
    }
    if has_main {
        out.extend(
            local_gcls
                .into_iter()
                .filter(|p| p.file_name().is_some_and(|n| n == "main.gcl")),
        );
    } else {
        out.extend(local_gcls);
    }
    subdirs.sort();
    for d in subdirs {
        collect_entry_points(&d, out);
    }
}

fn fixture_entry_points() -> Vec<PathBuf> {
    fixture_entry_points_by_category()
        .into_iter()
        .map(|(_, path)| path)
        .collect()
}

/// Collect `(category, entry_path)` pairs for every fixture entry point under
/// `tests/fixtures/{valid,valid_library,runtime_error,invalid}`, sorted by
/// path.
fn fixture_entry_points_by_category() -> Vec<(&'static str, PathBuf)> {
    let root = fixtures_root();
    let mut entries: Vec<(&'static str, PathBuf)> = Vec::new();
    for cat in ["valid", "valid_library", "runtime_error", "invalid"] {
        let mut cat_entries = Vec::new();
        collect_entry_points(&root.join(cat), &mut cat_entries);
        for path in cat_entries {
            entries.push((cat, path));
        }
    }
    entries.sort_by(|a, b| a.1.cmp(&b.1));
    entries
}

#[test]
fn check_failure_implies_eval_failure() {
    let entries = fixture_entry_points();
    assert!(
        entries.len() >= 100,
        "found only {} entry points",
        entries.len()
    );

    let root = fixtures_root();
    let mut violations: Vec<String> = Vec::new();
    for path in &entries {
        let check = graphcal_bin()
            .args(["check", path.to_str().unwrap()])
            .output()
            .expect("graphcal check failed to spawn");
        if check.status.success() {
            continue;
        }
        let eval = graphcal_bin()
            .args(["eval", path.to_str().unwrap()])
            .output()
            .expect("graphcal eval failed to spawn");
        if eval.status.success() {
            let rel = path.strip_prefix(&root).unwrap_or(path);
            violations.push(format!(
                "{}: check exit={:?}, eval exit={:?}",
                rel.display(),
                check.status.code(),
                eval.status.code()
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "{} fixture(s) failed `check` but passed `eval` — invariant violated:\n{}",
        violations.len(),
        violations.join("\n")
    );
}

// ---------------------------------------------------------------------------
// Categorization: each fixture must live under the directory whose name
// matches its actual `check`/`eval` outcome:
//   tests/fixtures/valid/         → check passes, eval passes
//   tests/fixtures/valid_library/ → check passes (eval may pass or fail —
//                                   library files aren't designed to be
//                                   evaluated standalone, so eval result is
//                                   not load-bearing for this category)
//   tests/fixtures/runtime_error/ → check passes, eval fails
//   tests/fixtures/invalid/       → check fails
// Without this guard, fixtures can drift into the wrong bucket as language
// semantics change, silently weakening the implicit contract that snapshot
// and integration tests rely on.
// ---------------------------------------------------------------------------

/// Fixtures that are placed in the wrong category but kept there because
/// the source-of-truth intent matters more than the current `check`/`eval`
/// outcome. Each entry MUST carry a tracking issue so the allowlist shrinks
/// over time.
///
/// Format: `(relative_path, expected_category, actual_category, reason)`.
const KNOWN_MISCLASSIFIED: &[(&str, &str, &str, &str)] = &[];

/// Outcomes that a fixture's directory placement can accept. `valid_library`
/// is intentionally lenient on `eval` — see the comment block above.
fn category_accepts(expected: &str, actual: &str) -> bool {
    match expected {
        "valid_library" => actual == "valid" || actual == "runtime_error",
        _ => expected == actual,
    }
}

#[test]
fn fixtures_match_their_category() {
    let entries = fixture_entry_points_by_category();
    assert!(
        entries.len() >= 100,
        "found only {} entry points",
        entries.len()
    );

    let root = fixtures_root();
    let mut new_violations: Vec<String> = Vec::new();
    let mut stale_allowlist: Vec<String> = Vec::new();
    for (expected, path) in &entries {
        let check = graphcal_bin()
            .args(["check", path.to_str().unwrap()])
            .output()
            .expect("graphcal check failed to spawn");
        let actual = if check.status.success() {
            let eval = graphcal_bin()
                .args(["eval", path.to_str().unwrap()])
                .output()
                .expect("graphcal eval failed to spawn");
            if eval.status.success() {
                "valid"
            } else {
                "runtime_error"
            }
        } else {
            "invalid"
        };
        let rel = path.strip_prefix(&root).unwrap_or(path);
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        let allowlisted = KNOWN_MISCLASSIFIED
            .iter()
            .find(|(p, _, _, _)| *p == rel_str);
        let accepted = category_accepts(expected, actual);
        match (accepted, allowlisted) {
            (true, None) => {}
            (true, Some((_, _, _, reason))) => {
                stale_allowlist.push(format!("{rel_str}: now categorized correctly ({reason})"));
            }
            (false, Some((_, exp, act, _))) if expected == exp && actual == *act => {}
            (false, Some((_, exp, act, _))) => {
                new_violations.push(format!(
                    "{rel_str}: allowlist says expected `{exp}` actual `{act}`, \
                     but observed expected `{expected}` actual `{actual}`"
                ));
            }
            (false, None) => {
                new_violations.push(format!(
                    "{rel_str}: expected `{expected}`, actual `{actual}`"
                ));
            }
        }
    }

    assert!(
        new_violations.is_empty(),
        "{} fixture(s) misclassified — move each to the matching directory, \
         fix the underlying regression, or add to KNOWN_MISCLASSIFIED with a \
         tracking note:\n{}",
        new_violations.len(),
        new_violations.join("\n")
    );
    assert!(
        stale_allowlist.is_empty(),
        "{} fixture(s) on KNOWN_MISCLASSIFIED now match their category — \
         remove them from the allowlist:\n{}",
        stale_allowlist.len(),
        stale_allowlist.join("\n")
    );
}

// ---------------------------------------------------------------------------
// Eval-idempotence: `graphcal format` must preserve `graphcal eval` output
// for every `valid/` fixture.
//
// The existing `idempotent_*` macros only check `format(format(x)) ==
// format(x)`; they happily accept a formatter that consistently changes
// semantics (issue #575 was exactly that). This stronger property test
// guards against any future paren-elision regression.
// ---------------------------------------------------------------------------

/// Walk up from `entry` until we hit either a directory containing
/// `graphcal.toml` (the package root for multi-file fixtures) or the
/// `valid/` parent (single-file fixture). Returns `(root_path,
/// entry_relative_to_root)` so the caller can mirror the structure into
/// a temp dir and re-run `graphcal eval` against the same logical entry.
fn fixture_format_scope(entry: &Path) -> (PathBuf, PathBuf) {
    let valid_root = fixtures_root().join("valid");
    let mut dir = entry.parent().expect("entry has parent").to_path_buf();
    while dir != valid_root {
        if dir.join("graphcal.toml").exists() {
            let rel = entry.strip_prefix(&dir).unwrap().to_path_buf();
            return (dir, rel);
        }
        let Some(parent) = dir.parent() else {
            break;
        };
        if parent == valid_root {
            // Single-file or single-dir fixture sitting directly under valid/.
            let rel = entry.strip_prefix(&dir).unwrap().to_path_buf();
            return (dir, rel);
        }
        dir = parent.to_path_buf();
    }
    // Single file directly under valid/.
    (
        entry.to_path_buf(),
        PathBuf::from(entry.file_name().unwrap()),
    )
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("create dst dir");
    for entry in std::fs::read_dir(src).expect("read src dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        let target = dst.join(path.file_name().unwrap());
        if path.is_dir() {
            copy_dir_recursive(&path, &target);
        } else {
            std::fs::copy(&path, &target).expect("copy file");
        }
    }
}

#[test]
fn eval_idempotent_under_format() {
    let entries = fixture_entry_points_by_category();
    let valid_entries: Vec<&PathBuf> = entries
        .iter()
        .filter_map(|(cat, p)| (*cat == "valid").then_some(p))
        .collect();
    assert!(
        valid_entries.len() >= 40,
        "expected many valid entry points, got {}",
        valid_entries.len()
    );

    let temp_root = std::env::temp_dir().join("graphcal_eval_idempotent");
    let _ = std::fs::remove_dir_all(&temp_root);
    std::fs::create_dir_all(&temp_root).expect("create temp root");

    let mut failures: Vec<String> = Vec::new();
    for (idx, entry) in valid_entries.iter().enumerate() {
        let original = graphcal_bin()
            .args(["eval", entry.to_str().unwrap()])
            .output()
            .expect("eval original");
        if !original.status.success() {
            // `valid` fixtures should eval cleanly; if not, the
            // categorization test catches it. Skip here so this property
            // test stays focused on its own invariant.
            continue;
        }

        let (scope_root, entry_rel) = fixture_format_scope(entry);
        let target_root = temp_root.join(format!("scope_{idx}"));
        let _ = std::fs::remove_dir_all(&target_root);
        if scope_root.is_file() {
            std::fs::create_dir_all(&target_root).expect("create scope dir");
            let target_file = target_root.join(scope_root.file_name().unwrap());
            std::fs::copy(&scope_root, &target_file).expect("copy single-file scope");
        } else {
            copy_dir_recursive(&scope_root, &target_root);
        }

        let format_target = if scope_root.is_file() {
            target_root.join(scope_root.file_name().unwrap())
        } else {
            target_root.clone()
        };
        let format_out = graphcal_bin()
            .args(["format", format_target.to_str().unwrap()])
            .output()
            .expect("graphcal format");
        if !format_out.status.success() {
            failures.push(format!(
                "{}: format failed: {}",
                entry.display(),
                String::from_utf8_lossy(&format_out.stderr)
            ));
            continue;
        }

        let new_entry = if scope_root.is_file() {
            target_root.join(scope_root.file_name().unwrap())
        } else {
            target_root.join(&entry_rel)
        };
        let after = graphcal_bin()
            .args(["eval", new_entry.to_str().unwrap()])
            .output()
            .expect("eval formatted");
        if !after.status.success() {
            failures.push(format!(
                "{}: eval-after-format failed: {}",
                entry.display(),
                String::from_utf8_lossy(&after.stderr)
            ));
            continue;
        }

        if original.stdout != after.stdout {
            failures.push(format!(
                "{}: eval output diverged after format",
                entry.display()
            ));
        }
    }

    let _ = std::fs::remove_dir_all(&temp_root);
    assert!(
        failures.is_empty(),
        "{} fixture(s) had eval output that differs after `graphcal format` \
         — formatter is not eval-idempotent:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn format_check_fails_on_unparsable_file() {
    // Regression: `graphcal format --check` exited 0 when a file failed to
    // parse — CI passed silently on syntactically broken files while
    // `graphcal check` on the same file failed.
    let dir = tempfile::tempdir().unwrap();
    let path = write_temp_file(dir.path(), "broken.gcl", "node x: = ;\n");

    let output = graphcal_bin()
        .args(["format", "--check", path.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    assert!(
        !output.status.success(),
        "format --check must fail on a parse error"
    );

    // Plain `format` must also report failure.
    let output = graphcal_bin()
        .args(["format", path.to_str().unwrap()])
        .output()
        .expect("failed to run graphcal");
    assert!(
        !output.status.success(),
        "format must fail on a parse error"
    );
}
