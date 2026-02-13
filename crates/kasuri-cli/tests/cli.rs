//! Allow use of unwrap in tests
#![allow(clippy::unwrap_used)]

use std::process::Command;

fn kasuri_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kasuri"))
}

fn fixture(name: &str) -> String {
    format!(
        "{}/tests/fixtures/{}",
        env!("CARGO_MANIFEST_DIR").trim_end_matches("crates/kasuri-cli"),
        name
    )
}

#[test]
fn eval_rocket_text_output() {
    let output = kasuri_bin()
        .args(["eval", &fixture("rocket.ksr")])
        .output()
        .expect("failed to run kasuri");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();

    // Source order: dry_mass, fuel_mass, isp, G0, v_exhaust, mass_ratio, delta_v
    assert_eq!(lines.len(), 7);
    assert!(lines[0].contains("dry_mass"));
    assert!(lines[1].contains("fuel_mass"));
    assert!(lines[2].contains("isp"));
    assert!(lines[3].contains("G0"));
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
    let output = kasuri_bin()
        .args(["eval", &fixture("rocket.ksr"), "--format", "json"])
        .output()
        .expect("failed to run kasuri");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("invalid JSON");

    assert!(json["const"]["G0"]["si_value"].as_f64().is_some());
    assert!(
        (json["param"]["dry_mass"]["si_value"].as_f64().unwrap() - 1200.0).abs() < f64::EPSILON
    );
    assert!(json["node"]["v_exhaust"]["si_value"].as_f64().is_some());
}

#[test]
fn eval_nonexistent_file_fails() {
    let output = kasuri_bin()
        .args(["eval", "nonexistent.ksr"])
        .output()
        .expect("failed to run kasuri");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("file not found"),
        "expected 'file not found' in stderr: {stderr}"
    );
}

#[test]
fn eval_functions_text_output() {
    let output = kasuri_bin()
        .args(["eval", &fixture("functions.ksr")])
        .output()
        .expect("failed to run kasuri");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();

    // Output: consts (R_EARTH, GM_EARTH), params (parking_alt, target_alt),
    // nodes (v_parking, transfer.{dv1,dv2,total_dv}, midpoint_alt, v_check)
    // Functions produce no output rows.
    assert_eq!(lines.len(), 10, "lines: {lines:?}");
    assert!(lines[0].contains("R_EARTH"));
    assert!(lines[1].contains("GM_EARTH"));
    assert!(lines[2].contains("parking_alt"));
    assert!(lines[3].contains("target_alt"));
    assert!(lines[4].contains("v_parking"));
    // transfer struct expands to 3 field lines
    assert!(lines[5].contains("transfer.dv1"));
    assert!(lines[6].contains("transfer.dv2"));
    assert!(lines[7].contains("transfer.total_dv"));
    assert!(lines[8].contains("midpoint_alt"));
    assert!(lines[9].contains("v_check"));
}

#[test]
fn eval_indexed_text_output() {
    let output = kasuri_bin()
        .args(["eval", &fixture("indexed.ksr")])
        .output()
        .expect("failed to run kasuri");

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
    let output = kasuri_bin()
        .args(["eval", &fixture("indexed.ksr"), "--format", "json"])
        .output()
        .expect("failed to run kasuri");

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
fn eval_invalid_syntax_fails() {
    // Create a temp file with invalid syntax
    let dir = std::env::temp_dir().join("kasuri_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bad.ksr");
    std::fs::write(&path, "this is not valid kasuri").unwrap();

    let output = kasuri_bin()
        .args(["eval", path.to_str().unwrap()])
        .output()
        .expect("failed to run kasuri");

    assert!(!output.status.success());
    std::fs::remove_dir_all(&dir).ok();
}

// --- --set flag tests ---

#[test]
fn eval_with_set_flag() {
    let output = kasuri_bin()
        .args(["eval", &fixture("rocket.ksr"), "--set", "isp=450 s"])
        .output()
        .expect("failed to run kasuri");

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
    let output = kasuri_bin()
        .args([
            "eval",
            &fixture("rocket.ksr"),
            "--set",
            "isp=450 s",
            "--set",
            "dry_mass=1500 kg",
        ])
        .output()
        .expect("failed to run kasuri");

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
    let output = kasuri_bin()
        .args(["eval", &fixture("rocket.ksr"), "--set", "nonexistent=100"])
        .output()
        .expect("failed to run kasuri");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("nonexistent"),
        "expected error mentioning 'nonexistent': {stderr}"
    );
}

#[test]
fn eval_set_node_error() {
    let output = kasuri_bin()
        .args(["eval", &fixture("rocket.ksr"), "--set", "delta_v=100 m/s"])
        .output()
        .expect("failed to run kasuri");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("node"),
        "expected error mentioning 'node': {stderr}"
    );
}

#[test]
fn eval_set_bad_value() {
    let output = kasuri_bin()
        .args(["eval", &fixture("rocket.ksr"), "--set", "isp=???"])
        .output()
        .expect("failed to run kasuri");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("error"), "expected parse error: {stderr}");
}

// --- Multi-file import tests ---

#[test]
fn eval_multi_file() {
    let output = kasuri_bin()
        .args(["eval", &fixture("multi/rocket_split/main.ksr")])
        .output()
        .expect("failed to run kasuri");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    // Should produce same values as single-file rocket.ksr
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("delta_v") && l.contains("3778")),
        "expected delta_v ~3778 in output: {stdout}"
    );
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("v_exhaust") && l.contains("3138")),
        "expected v_exhaust ~3138 in output: {stdout}"
    );
}

#[test]
fn eval_multi_file_with_set() {
    let output = kasuri_bin()
        .args([
            "eval",
            &fixture("multi/rocket_split/main.ksr"),
            "--set",
            "isp=450 s",
        ])
        .output()
        .expect("failed to run kasuri");

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
        "expected isp=450 in output: {stdout}"
    );
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("delta_v") && l.contains("5313")),
        "expected delta_v ~5313 with isp=450: {stdout}"
    );
}

#[test]
fn eval_circular_import_error() {
    let output = kasuri_bin()
        .args(["eval", &fixture("multi/circular_a.ksr")])
        .output()
        .expect("failed to run kasuri");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("circular") || stderr.contains("Circular"),
        "expected circular import error: {stderr}"
    );
}

#[test]
fn eval_missing_import_error() {
    let output = kasuri_bin()
        .args(["eval", &fixture("multi/missing_import.ksr")])
        .output()
        .expect("failed to run kasuri");

    assert!(!output.status.success());
}

#[test]
fn eval_import_name_not_found() {
    let output = kasuri_bin()
        .args(["eval", &fixture("multi/bad_name_import.ksr")])
        .output()
        .expect("failed to run kasuri");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("nonexistent"),
        "expected error mentioning 'nonexistent': {stderr}"
    );
}
