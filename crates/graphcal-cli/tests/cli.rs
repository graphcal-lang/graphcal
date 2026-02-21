//! Allow use of unwrap in tests
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    reason = "test code"
)]

use std::process::Command;

fn graphcal_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_graphcal"))
}

fn fixture(name: &str) -> String {
    format!(
        "{}/tests/fixtures/{}",
        env!("CARGO_MANIFEST_DIR").trim_end_matches("crates/graphcal-cli"),
        name
    )
}

#[test]
fn eval_rocket_text_output() {
    let output = graphcal_bin()
        .args(["eval", &fixture("rocket.gcl")])
        .output()
        .expect("failed to run graphcal");

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
    let output = graphcal_bin()
        .args(["eval", &fixture("rocket.gcl"), "--format", "json"])
        .output()
        .expect("failed to run graphcal");

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
fn eval_functions_text_output() {
    let output = graphcal_bin()
        .args(["eval", &fixture("functions.gcl")])
        .output()
        .expect("failed to run graphcal");

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
    let output = graphcal_bin()
        .args(["eval", &fixture("indexed.gcl")])
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
        .args(["eval", &fixture("indexed.gcl"), "--format", "json"])
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
        .args(["eval", &fixture("rocket.gcl"), "--set", "isp=450.0 s"])
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
            &fixture("rocket.gcl"),
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
        .args(["eval", &fixture("rocket.gcl"), "--set", "nonexistent=100"])
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
        .args(["eval", &fixture("user_dimensions.gcl")])
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
        .args(["eval", &fixture("rocket.gcl"), "--set", "delta_v=100.0 m/s"])
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
        .args(["eval", &fixture("rocket.gcl"), "--set", "isp=???"])
        .output()
        .expect("failed to run graphcal");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("error"), "expected parse error: {stderr}");
}

// --- Multi-file import tests ---

#[test]
fn eval_multi_file() {
    let output = graphcal_bin()
        .args(["eval", &fixture("multi/rocket_split/main.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    // Should produce same values as single-file rocket.gcl
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
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("multi/rocket_split/main.gcl"),
            "--set",
            "isp=450.0 s",
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
    let output = graphcal_bin()
        .args(["eval", &fixture("multi/circular_a.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("circular") || stderr.contains("Circular"),
        "expected circular import error: {stderr}"
    );
}

#[test]
fn eval_missing_import_error() {
    let output = graphcal_bin()
        .args(["eval", &fixture("multi/missing_import.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(!output.status.success());
}

// --- Tagged union tests ---

#[test]
fn eval_tagged_union_text_output() {
    let output = graphcal_bin()
        .args(["eval", &fixture("tagged_union.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();

    // Multi-variant struct shows variant name: maneuver::LowThrust.thrust
    assert!(
        lines
            .iter()
            .any(|l| l.contains("maneuver::LowThrust.thrust")),
        "expected maneuver::LowThrust.thrust in output: {stdout}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("maneuver::LowThrust.duration")),
        "expected maneuver::LowThrust.duration in output: {stdout}"
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
        .args(["eval", &fixture("tagged_union.gcl"), "--format", "json"])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("invalid JSON");

    // Multi-variant type includes "variant" field
    let maneuver = &json["node"]["maneuver"];
    assert_eq!(maneuver["type"].as_str(), Some("ManeuverKind"));
    assert_eq!(maneuver["variant"].as_str(), Some("LowThrust"));
    assert!(maneuver["fields"]["thrust"]["si_value"].as_f64().is_some());

    // Single-variant (struct sugar) has no "variant" field
    let transfer = &json["node"]["transfer"];
    assert_eq!(transfer["type"].as_str(), Some("TransferResult"));
    assert!(
        transfer["variant"].is_null(),
        "single-variant should not have variant field"
    );

    // Bare variant
    let status = &json["node"]["current_status"];
    assert_eq!(status["type"].as_str(), Some("Status"));
    assert_eq!(status["variant"].as_str(), Some("Nominal"));
}

#[test]
fn eval_import_name_not_found() {
    let output = graphcal_bin()
        .args(["eval", &fixture("multi/bad_name_import.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("nonexistent"),
        "expected error mentioning 'nonexistent': {stderr}"
    );
}

// --- --input JSON file tests ---

#[test]
fn eval_with_input_json() {
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("rocket.gcl"),
            "--input",
            &fixture("input_rocket.json"),
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
            &fixture("rocket.gcl"),
            "--input",
            &fixture("input_rocket.json"),
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
            &fixture("indexed.gcl"),
            "--input",
            &fixture("input_indexed.json"),
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
            &fixture("tagged_union_param.gcl"),
            "--input",
            &fixture("input_tagged_union.json"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    // maneuver should now be Impulsive variant (from JSON), not LowThrust (default)
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("maneuver::Impulsive.delta_v")),
        "expected maneuver::Impulsive.delta_v in output: {stdout}"
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
            &fixture("rocket.gcl"),
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
            &fixture("rocket.gcl"),
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

#[test]
fn eval_input_json_with_hohmann() {
    // Test scalar overrides on a file with structs and derived nodes
    let output = graphcal_bin()
        .args([
            "eval",
            &fixture("hohmann.gcl"),
            "--input",
            &fixture("input_struct.json"),
        ])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    // parking_alt should show 300 km (from JSON), not default 200
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("parking_alt") && l.contains("300")),
        "expected parking_alt=300 in output: {stdout}"
    );
}

// --- Assertion tests ---

#[test]
fn eval_assertions_pass() {
    let output = graphcal_bin()
        .args(["eval", &fixture("assertions.gcl")])
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
        .args(["eval", &fixture("assertions.gcl"), "--format", "json"])
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
        .args(["eval", &fixture("assertions_fail.gcl")])
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
        .args(["eval", &fixture("assertions_tolerance_fail.gcl")])
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
        .args(["eval", &fixture("assertions_assumes.gcl")])
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
fn eval_assertions_no_assert_flag() {
    let output = graphcal_bin()
        .args(["eval", &fixture("assertions_fail.gcl"), "--no-assert"])
        .output()
        .expect("failed to run graphcal");

    // With --no-assert, even a failing assertion should not cause exit code 1
    assert!(
        output.status.success(),
        "expected success with --no-assert, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        !stdout.contains("Assertions:"),
        "expected no Assertions section with --no-assert: {stdout}"
    );
}

#[test]
fn eval_assertions_indexed_fail() {
    let output = graphcal_bin()
        .args(["eval", &fixture("assertions_indexed.gcl")])
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
}

#[test]
fn eval_assertions_cross_file() {
    let output = graphcal_bin()
        .args(["eval", &fixture("multi/assertions/main.gcl")])
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
        stdout.contains("limit_positive") && stdout.contains("PASS"),
        "expected limit_positive PASS: {stdout}"
    );
}

#[test]
fn eval_assertions_compile_error_exit_code() {
    let output = graphcal_bin()
        .args(["eval", &fixture("errors/assert_not_bool.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert_eq!(
        output.status.code(),
        Some(2),
        "expected exit code 2 for compile error"
    );
}

#[test]
fn eval_imported_node_with_deps() {
    // Bug 2: imported nodes whose expressions contain @-references must
    // have their dependencies tracked so the evaluation DAG is correct.
    let output = graphcal_bin()
        .args(["eval", &fixture("multi/imported_deps/main.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.lines().any(|l| l.contains('x') && l.contains("10")),
        "expected x = 10 in output: {stdout}"
    );
    assert!(
        stdout.lines().any(|l| l.contains('y') && l.contains("20")),
        "expected y = 20 in output: {stdout}"
    );
    assert!(
        stdout.lines().any(|l| l.contains('z') && l.contains("21")),
        "expected z = 21 in output: {stdout}"
    );
}

#[test]
fn eval_explicit_index_import() {
    // Bug 3: `use "./lib.gcl" { Color }` should import the Color index explicitly.
    let output = graphcal_bin()
        .args(["eval", &fixture("multi/explicit_index/main.gcl")])
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
        .args(["eval", &fixture("variant_comparison.gcl")])
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
        .args(["eval", &fixture("variant_match.gcl")])
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

    // Multi-binding match: adjusted_cost[Departure][Burn] = 2460 * 1.1 = 2706
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("adjusted_cost[Departure][Burn]") && l.contains("2706")),
        "expected adjusted_cost[Departure][Burn] = 2706 in output: {stdout}"
    );
    // adjusted_cost[Departure][Coast] = 0
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("adjusted_cost[Departure][Coast]") && l.contains("0 m/s")),
        "expected adjusted_cost[Departure][Coast] = 0 in output: {stdout}"
    );
}

// --- Large / realistic fixture tests ---

#[test]
fn eval_power_budget() {
    let output = graphcal_bin()
        .args(["eval", &fixture("power_budget.gcl")])
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
fn eval_power_budget_json() {
    let output = graphcal_bin()
        .args(["eval", &fixture("power_budget.gcl"), "--format", "json"])
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
        .args(["eval", &fixture("thermal_analysis.gcl")])
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
fn eval_mission_plan_multi_file() {
    let output = graphcal_bin()
        .args(["eval", &fixture("multi/mission_plan/main.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    // Imported propulsion nodes
    assert!(
        stdout.lines().any(|l| l.contains("total_delta_v")),
        "expected total_delta_v in output: {stdout}"
    );
    // Locally computed nodes
    assert!(
        stdout.lines().any(|l| l.contains("required_dv")),
        "expected required_dv in output: {stdout}"
    );
    assert!(
        stdout.lines().any(|l| l.contains("dv_margin_pct")),
        "expected dv_margin_pct in output: {stdout}"
    );

    // Assertions should pass
    assert!(
        stdout.contains("positive_dv_margin") && stdout.contains("PASS"),
        "expected positive_dv_margin PASS: {stdout}"
    );
    assert!(
        stdout.contains("margin_at_least_5pct") && stdout.contains("PASS"),
        "expected margin_at_least_5pct PASS: {stdout}"
    );
}

#[test]
fn eval_parent_directory_import() {
    let output = graphcal_bin()
        .args(["eval", &fixture("multi/parent_import/child/main.gcl")])
        .output()
        .expect("failed to run graphcal");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    // result = base_value * SCALE = 42 * 2 = 84
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("result") && l.contains("84")),
        "expected result = 84 in output: {stdout}"
    );
}
