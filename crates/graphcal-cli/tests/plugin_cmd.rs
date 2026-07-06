//! Binary-level tests for `graphcal plugin new` and `graphcal plugin test`.
//!
//! The test module is built from WAT at test time with the manifest
//! embedded through the ABI crate — no binary fixtures in the repository.

#![cfg(test)]

use std::path::Path;
use std::process::Command;

use graphcal_plugin_abi::{
    ManifestFunction, ManifestMonomial, ManifestParam, ManifestRational, ManifestValueKind,
    ManifestVarPower, PluginManifest,
};

fn graphcal_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_graphcal"))
}

fn scalar_var(var: &str) -> ManifestValueKind {
    ManifestValueKind::Scalar(ManifestMonomial {
        vars: vec![ManifestVarPower {
            var: var.to_string(),
            pow: ManifestRational { num: 1, den: 1 },
        }],
        fixed: Vec::new(),
    })
}

fn dimensionless() -> ManifestValueKind {
    ManifestValueKind::Scalar(ManifestMonomial::default())
}

/// The manifest entry for the array `twice` function.
fn twice_manifest_function() -> ManifestFunction {
    let element = ManifestMonomial {
        vars: vec![ManifestVarPower {
            var: "D".to_string(),
            pow: ManifestRational { num: 1, den: 1 },
        }],
        fixed: Vec::new(),
    };
    ManifestFunction {
        name: "twice".to_string(),
        dim_vars: vec!["D".to_string()],
        index_vars: vec!["I".to_string()],
        params: vec![ManifestParam {
            name: "xs".to_string(),
            kind: ManifestValueKind::Array {
                element: element.clone(),
                index: "I".to_string(),
            },
        }],
        result: ManifestValueKind::Array {
            element,
            index: "I".to_string(),
        },
    }
}

/// A lerp plugin plus an Int/Bool `step` function and an array `twice`
/// function (buffer protocol), compiled from WAT.
fn test_module_bytes() -> Vec<u8> {
    let wat = r#"
    (module
      (memory (export "memory") 1)
      (global $bump (mut i32) (i32.const 1024))
      (func (export "graphcal_alloc") (param $size i32) (result i32)
        (local $ptr i32)
        (local.set $ptr (global.get $bump))
        (global.set $bump
          (i32.add
            (global.get $bump)
            (i32.and (i32.add (local.get $size) (i32.const 7)) (i32.const -8))))
        (local.get $ptr))
      (func (export "graphcal_free") (param i32 i32))
      (func (export "lerp") (param f64 f64 f64) (result f64)
        (f64.add
          (local.get 0)
          (f64.mul (f64.sub (local.get 1) (local.get 0)) (local.get 2))))
      (func (export "step") (param f64 f64) (result f64)
        (if (result f64) (f64.eq (local.get 1) (f64.const 1))
          (then (f64.add (local.get 0) (f64.const 1)))
          (else (f64.sub (local.get 0) (f64.const 1)))))
      (func (export "twice") (param $ptr i32) (param $len i32) (param $out i32)
        (local $i i32)
        (block $done
          (loop $loop
            (br_if $done (i32.ge_s (local.get $i) (local.get $len)))
            (f64.store
              (i32.add (local.get $out) (i32.mul (local.get $i) (i32.const 8)))
              (f64.mul
                (f64.load (i32.add (local.get $ptr) (i32.mul (local.get $i) (i32.const 8))))
                (f64.const 2)))
            (local.set $i (i32.add (local.get $i) (i32.const 1)))
            (br $loop)))))
    "#;
    let manifest = PluginManifest {
        abi_version: graphcal_plugin_abi::ABI_VERSION,
        functions: vec![
            ManifestFunction {
                name: "lerp".to_string(),
                dim_vars: vec!["D".to_string()],
                index_vars: Vec::new(),
                params: vec![
                    ManifestParam {
                        name: "a".to_string(),
                        kind: scalar_var("D"),
                    },
                    ManifestParam {
                        name: "b".to_string(),
                        kind: scalar_var("D"),
                    },
                    ManifestParam {
                        name: "t".to_string(),
                        kind: dimensionless(),
                    },
                ],
                result: scalar_var("D"),
            },
            ManifestFunction {
                name: "step".to_string(),
                dim_vars: Vec::new(),
                index_vars: Vec::new(),
                params: vec![
                    ManifestParam {
                        name: "n".to_string(),
                        kind: ManifestValueKind::Int,
                    },
                    ManifestParam {
                        name: "up".to_string(),
                        kind: ManifestValueKind::Bool,
                    },
                ],
                result: ManifestValueKind::Int,
            },
            twice_manifest_function(),
        ],
    };
    let wasm = wat::parse_str(wat).expect("test WAT compiles");
    manifest.embed_into(&wasm).expect("manifest embeds")
}

fn write_test_module(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("kernels.wasm");
    std::fs::write(&path, test_module_bytes()).unwrap();
    path
}

#[test]
fn plugin_test_reports_identity_and_import_block() {
    let dir = tempfile::tempdir().unwrap();
    let module = write_test_module(dir.path());

    let output = graphcal_bin()
        .args(["plugin", "test"])
        .arg(&module)
        .output()
        .expect("failed to run graphcal");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("sha256: "), "{stdout}");
    assert!(stdout.contains("abi: version 2, 3 function(s)"), "{stdout}");
    assert!(stdout.contains("as kernels {"), "{stdout}");
    assert!(
        stdout.contains("fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D;"),
        "{stdout}"
    );
    assert!(
        stdout.contains("fn step(n: Int, up: Bool) -> Int;"),
        "{stdout}"
    );
    assert!(
        stdout.contains("fn twice<D: Dim, I: Index>(xs: D[I]) -> D[I];"),
        "{stdout}"
    );
}

#[test]
fn plugin_test_calls_functions_with_typed_arguments() {
    let dir = tempfile::tempdir().unwrap();
    let module = write_test_module(dir.path());

    let output = graphcal_bin()
        .args(["plugin", "test"])
        .arg(&module)
        .args(["--call", "lerp", "1.0", "3.0", "0.5"])
        .output()
        .expect("failed to run graphcal");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("lerp(1.0, 3.0, 0.5) = 2"), "{stdout}");

    let output = graphcal_bin()
        .args(["plugin", "test"])
        .arg(&module)
        .args(["--call", "step", "41", "true"])
        .output()
        .expect("failed to run graphcal");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("step(41, true) = 42"), "{stdout}");

    let output = graphcal_bin()
        .args(["plugin", "test"])
        .arg(&module)
        .args(["--call", "twice", "[1.5,2.0,-3.0]"])
        .output()
        .expect("failed to run graphcal");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("twice([1.5,2.0,-3.0]) = [3, 4, -6]"),
        "{stdout}"
    );
}

#[test]
fn plugin_test_rejects_bad_calls_with_usage_errors() {
    let dir = tempfile::tempdir().unwrap();
    let module = write_test_module(dir.path());

    // Unknown function: exit 2, lists what the module provides.
    let output = graphcal_bin()
        .args(["plugin", "test"])
        .arg(&module)
        .args(["--call", "nope"])
        .output()
        .expect("failed to run graphcal");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("does not provide `nope`"), "{stderr}");
    assert!(stderr.contains("lerp, step"), "{stderr}");

    // Bool argument that isn't true/false: exit 2.
    let output = graphcal_bin()
        .args(["plugin", "test"])
        .arg(&module)
        .args(["--call", "step", "41", "yes"])
        .output()
        .expect("failed to run graphcal");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("expected `true` or `false`"), "{stderr}");

    // Arity mismatch: exit 2.
    let output = graphcal_bin()
        .args(["plugin", "test"])
        .arg(&module)
        .args(["--call", "lerp", "1.0"])
        .output()
        .expect("failed to run graphcal");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("takes 3 argument(s), got 1"), "{stderr}");
}

#[test]
fn plugin_test_rejects_invalid_modules() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("not-a-plugin.wasm");
    // A valid wasm module with no manifest section.
    std::fs::write(&path, wat::parse_str("(module)").unwrap()).unwrap();

    let output = graphcal_bin()
        .args(["plugin", "test"])
        .arg(&path)
        .output()
        .expect("failed to run graphcal");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("graphcal-manifest"), "{stderr}");

    let missing = dir.path().join("missing.wasm");
    let output = graphcal_bin()
        .args(["plugin", "test"])
        .arg(&missing)
        .output()
        .expect("failed to run graphcal");
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn plugin_new_scaffolds_a_buildable_crate_layout() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("fluid-props");

    let output = graphcal_bin()
        .args(["plugin", "new", "fluid-props", "--dir"])
        .arg(&root)
        .output()
        .expect("failed to run graphcal");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    for file in [
        "Cargo.toml",
        "rust-toolchain.toml",
        ".gitignore",
        "justfile",
        "src/lib.rs",
        "README.md",
    ] {
        assert!(root.join(file).is_file(), "missing {file}");
    }
    let cargo = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(cargo.contains("name = \"fluid-props\""), "{cargo}");
    assert!(cargo.contains("graphcal-plugin"), "{cargo}");
    let lib = std::fs::read_to_string(root.join("src/lib.rs")).unwrap();
    assert!(lib.contains("graphcal_plugin::plugin!"), "{lib}");

    // Re-running against the same directory refuses to overwrite.
    let output = graphcal_bin()
        .args(["plugin", "new", "fluid-props", "--dir"])
        .arg(&root)
        .output()
        .expect("failed to run graphcal");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("already exists"), "{stderr}");
}

#[test]
fn plugin_new_rejects_invalid_names() {
    let dir = tempfile::tempdir().unwrap();
    let output = graphcal_bin()
        .current_dir(dir.path())
        .args(["plugin", "new", "Bad_Name"])
        .output()
        .expect("failed to run graphcal");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("must start with a lowercase letter"),
        "{stderr}"
    );
}
