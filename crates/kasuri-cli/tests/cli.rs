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

    assert!(json["const"]["G0"].as_f64().is_some());
    assert!((json["param"]["dry_mass"].as_f64().unwrap() - 1200.0).abs() < f64::EPSILON);
    assert!(json["node"]["v_exhaust"].as_f64().is_some());
}

#[test]
fn eval_nonexistent_file_fails() {
    let output = kasuri_bin()
        .args(["eval", "nonexistent.ksr"])
        .output()
        .expect("failed to run kasuri");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("error"));
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
