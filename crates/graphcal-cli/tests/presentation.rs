//! Binary output must explain nonzero presentation status without removing SI.
use std::process::Command;

#[test]
fn json_and_text_keep_values_and_report_display_only_failures() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("display.gcl");
    std::fs::write(&path, "param rate: Dimensionless = 0.0; unit bad: Length = (@rate) m; node output: Length = 6.0 m -> bad;").unwrap();
    for format in ["text", "json"] {
        let output = Command::new(env!("CARGO_BIN_EXE_graphcal"))
            .args(["eval", path.to_str().unwrap(), "--format", format])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("presentation:"));
        assert!(stderr.contains("SI value retained"));
        if format == "json" {
            let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(json["node"]["output"]["si_value"], 6.0);
            assert_eq!(
                json["presentation_diagnostics"][0]["kind"],
                "presentation_error"
            );
        } else {
            assert!(String::from_utf8(output.stdout).unwrap().contains('6'));
        }
    }
}
