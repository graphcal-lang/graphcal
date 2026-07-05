//! End-to-end runtime tests: real wasm modules built from WAT at test time
//! (no binary fixtures in the repository), with manifests embedded through
//! the ABI crate — the same path the authoring SDK will take.
#![cfg(test)]

use std::sync::Arc;

use graphcal_compiler::syntax::function_name::FnName;
use graphcal_plugin_abi::{
    ManifestDecodeError, ManifestFromWasmError, ManifestFunction, ManifestMonomial, ManifestParam,
    ManifestRational, ManifestValueKind, ManifestVarPower, PluginManifest, SectionError,
    embed_manifest,
};
use graphcal_plugin_host::{
    ConvertErrorKind, PluginCallError, PluginHost, PluginLimits, PluginLoadError,
};

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

const fn manifest(functions: Vec<ManifestFunction>) -> PluginManifest {
    PluginManifest {
        abi_version: graphcal_plugin_abi::ABI_VERSION,
        functions,
    }
}

fn function(
    name: &str,
    dim_vars: &[&str],
    params: &[(&str, ManifestValueKind)],
    result: ManifestValueKind,
) -> ManifestFunction {
    ManifestFunction {
        name: name.to_string(),
        dim_vars: dim_vars.iter().map(|&v| v.to_string()).collect(),
        params: params
            .iter()
            .map(|(name, kind)| ManifestParam {
                name: (*name).to_string(),
                kind: kind.clone(),
            })
            .collect(),
        result,
    }
}

/// Compile WAT and embed the manifest — a complete graphcal plugin.
fn plugin(wat_source: &str, manifest: &PluginManifest) -> Vec<u8> {
    let wasm = wat::parse_str(wat_source).expect("test WAT must compile");
    manifest.embed_into(&wasm).expect("embedding must succeed")
}

fn fn_name(name: &str) -> FnName {
    FnName::expect_valid(name)
}

const LERP_WAT: &str = r#"
(module
  (memory (export "memory") 1)
  (func (export "lerp") (param f64 f64 f64) (result f64)
    (f64.add
      (local.get 0)
      (f64.mul (f64.sub (local.get 1) (local.get 0)) (local.get 2)))))
"#;

fn lerp_manifest() -> PluginManifest {
    manifest(vec![function(
        "lerp",
        &["D"],
        &[
            ("a", scalar_var("D")),
            ("b", scalar_var("D")),
            ("t", dimensionless()),
        ],
        scalar_var("D"),
    )])
}

#[test]
fn calls_a_scalar_kernel() {
    let host = PluginHost::new();
    let module = host.load(&plugin(LERP_WAT, &lerp_manifest())).unwrap();
    let result = module.call(&fn_name("lerp"), &[0.0, 10.0, 0.25]).unwrap();
    assert!((result - 2.5).abs() < f64::EPSILON);
    // Second call reuses the pooled instance.
    let result = module.call(&fn_name("lerp"), &[1.0, 3.0, 0.5]).unwrap();
    assert!((result - 2.0).abs() < f64::EPSILON);
}

#[test]
fn exposes_typed_signatures_from_the_manifest() {
    let host = PluginHost::new();
    let module = host.load(&plugin(LERP_WAT, &lerp_manifest())).unwrap();
    let signature = module.signature(&fn_name("lerp")).unwrap();
    assert_eq!(signature.arity(), 3);
    assert_eq!(module.functions().len(), 1);
    assert!(module.signature(&fn_name("missing")).is_none());
}

#[test]
fn caches_modules_by_content_hash() {
    let host = PluginHost::new();
    let bytes = plugin(LERP_WAT, &lerp_manifest());
    let first = host.load(&bytes).unwrap();
    let second = host.load(&bytes).unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(first.sha256_hex().len(), 64);
}

#[test]
fn fail_import_reports_the_plugin_message_and_recovers() {
    let wat = r#"
    (module
      (import "graphcal" "fail" (func $fail (param i32 i32)))
      (memory (export "memory") 1)
      (data (i32.const 8) "division by zero")
      (func (export "inverse") (param f64) (result f64)
        (if (f64.eq (local.get 0) (f64.const 0))
          (then
            (call $fail (i32.const 8) (i32.const 16))
            (unreachable)))
        (f64.div (f64.const 1) (local.get 0))))
    "#;
    let manifest = manifest(vec![function(
        "inverse",
        &["D"],
        &[("x", scalar_var("D"))],
        ManifestValueKind::Scalar(ManifestMonomial {
            vars: vec![ManifestVarPower {
                var: "D".to_string(),
                pow: ManifestRational { num: -1, den: 1 },
            }],
            fixed: Vec::new(),
        }),
    )]);
    let host = PluginHost::new();
    let module = host.load(&plugin(wat, &manifest)).unwrap();

    let err = module.call(&fn_name("inverse"), &[0.0]).unwrap_err();
    assert_eq!(
        err,
        PluginCallError::Failed {
            message: "division by zero".to_string()
        }
    );

    // The damaged instance is discarded; the next call gets a fresh one.
    let ok = module.call(&fn_name("inverse"), &[4.0]).unwrap();
    assert!((ok - 0.25).abs() < f64::EPSILON);
}

#[test]
fn runaway_plugins_run_out_of_fuel() {
    let wat = r#"
    (module
      (func (export "spin") (param f64) (result f64)
        (loop $l (br $l))
        (f64.const 0)))
    "#;
    let manifest = manifest(vec![function(
        "spin",
        &[],
        &[("x", dimensionless())],
        dimensionless(),
    )]);
    let host = PluginHost::with_limits(PluginLimits {
        fuel_per_call: 10_000,
        ..PluginLimits::default()
    });
    let module = host.load(&plugin(wat, &manifest)).unwrap();
    assert_eq!(
        module.call(&fn_name("spin"), &[0.0]).unwrap_err(),
        PluginCallError::OutOfFuel { fuel: 10_000 }
    );
}

#[test]
fn runaway_start_functions_run_out_of_fuel_at_instantiation() {
    let wat = r#"
    (module
      (func $init (loop $l (br $l)))
      (start $init)
      (func (export "id") (param f64) (result f64) (local.get 0)))
    "#;
    let manifest = manifest(vec![function(
        "id",
        &[],
        &[("x", dimensionless())],
        dimensionless(),
    )]);
    let host = PluginHost::with_limits(PluginLimits {
        fuel_per_call: 10_000,
        ..PluginLimits::default()
    });
    let module = host.load(&plugin(wat, &manifest)).unwrap();
    assert_eq!(
        module.call(&fn_name("id"), &[1.0]).unwrap_err(),
        PluginCallError::OutOfFuel { fuel: 10_000 }
    );
}

#[test]
fn memory_growth_is_capped() {
    // Grows one 64 KiB page at a time until the limiter denies it, then
    // returns the number of successful grows.
    let wat = r#"
    (module
      (memory (export "memory") 1)
      (func (export "grow") (param f64) (result f64)
        (local $n i32)
        (block $done
          (loop $l
            (br_if $done (i32.eq (memory.grow (i32.const 1)) (i32.const -1)))
            (local.set $n (i32.add (local.get $n) (i32.const 1)))
            (br $l)))
        (f64.convert_i32_s (local.get $n))))
    "#;
    let manifest = manifest(vec![function(
        "grow",
        &[],
        &[("x", dimensionless())],
        dimensionless(),
    )]);
    let max_memory_bytes = 4 * 1024 * 1024; // 64 pages
    let host = PluginHost::with_limits(PluginLimits {
        max_memory_bytes,
        ..PluginLimits::default()
    });
    let module = host.load(&plugin(wat, &manifest)).unwrap();
    let grown = module.call(&fn_name("grow"), &[0.0]).unwrap();
    // Started at 1 page; the limiter must stop growth at 64 pages total.
    assert!((grown - 63.0).abs() < f64::EPSILON, "grew {grown} pages");
}

#[test]
fn traps_are_reported_per_call() {
    let wat = r#"
    (module
      (func (export "boom") (param f64) (result f64) (unreachable)))
    "#;
    let manifest = manifest(vec![function(
        "boom",
        &[],
        &[("x", dimensionless())],
        dimensionless(),
    )]);
    let host = PluginHost::new();
    let module = host.load(&plugin(wat, &manifest)).unwrap();
    assert!(matches!(
        module.call(&fn_name("boom"), &[0.0]).unwrap_err(),
        PluginCallError::Trap { .. }
    ));
}

#[test]
fn non_finite_results_are_not_a_host_error() {
    // Policing non-finite values is the evaluator's job (check_finite);
    // the host returns them verbatim.
    let wat = r#"
    (module
      (func (export "inf") (param f64) (result f64)
        (f64.div (f64.const 1) (local.get 0))))
    "#;
    let manifest = manifest(vec![function(
        "inf",
        &[],
        &[("x", dimensionless())],
        dimensionless(),
    )]);
    let host = PluginHost::new();
    let module = host.load(&plugin(wat, &manifest)).unwrap();
    assert!(module.call(&fn_name("inf"), &[0.0]).unwrap().is_infinite());
}

#[test]
fn forbidden_imports_are_rejected_at_load() {
    let wat = r#"
    (module
      (import "wasi_snapshot_preview1" "fd_write"
        (func (param i32 i32 i32 i32) (result i32)))
      (func (export "f") (param f64) (result f64) (local.get 0)))
    "#;
    let manifest = manifest(vec![function(
        "f",
        &[],
        &[("x", dimensionless())],
        dimensionless(),
    )]);
    let err = PluginHost::new().load(&plugin(wat, &manifest)).unwrap_err();
    assert_eq!(
        err,
        PluginLoadError::ForbiddenImport {
            module: "wasi_snapshot_preview1".to_string(),
            name: "fd_write".to_string(),
        }
    );
}

#[test]
fn mistyped_fail_import_is_rejected_at_load() {
    let wat = r#"
    (module
      (import "graphcal" "fail" (func (param i64)))
      (memory (export "memory") 1)
      (func (export "f") (param f64) (result f64) (local.get 0)))
    "#;
    let manifest = manifest(vec![function(
        "f",
        &[],
        &[("x", dimensionless())],
        dimensionless(),
    )]);
    assert!(matches!(
        PluginHost::new().load(&plugin(wat, &manifest)).unwrap_err(),
        PluginLoadError::FailImportTypeMismatch { .. }
    ));
}

#[test]
fn fail_import_without_memory_export_is_rejected_at_load() {
    let wat = r#"
    (module
      (import "graphcal" "fail" (func (param i32 i32)))
      (func (export "f") (param f64) (result f64) (local.get 0)))
    "#;
    let manifest = manifest(vec![function(
        "f",
        &[],
        &[("x", dimensionless())],
        dimensionless(),
    )]);
    assert_eq!(
        PluginHost::new().load(&plugin(wat, &manifest)).unwrap_err(),
        PluginLoadError::MissingMemoryExport
    );
}

#[test]
fn missing_manifest_section_is_rejected_at_load() {
    let wasm = wat::parse_str(LERP_WAT).unwrap();
    assert_eq!(
        PluginHost::new().load(&wasm).unwrap_err(),
        PluginLoadError::Manifest(ManifestFromWasmError::Section(
            SectionError::MissingManifest
        ))
    );
}

#[test]
fn future_abi_versions_are_rejected_with_a_version_error() {
    let wasm = wat::parse_str(LERP_WAT).unwrap();
    let wasm = embed_manifest(&wasm, br#"{"abi_version":2,"shape":"unknown"}"#).unwrap();
    assert_eq!(
        PluginHost::new().load(&wasm).unwrap_err(),
        PluginLoadError::Manifest(ManifestFromWasmError::Decode(
            ManifestDecodeError::UnsupportedAbiVersion {
                found: 2,
                supported: graphcal_plugin_abi::ABI_VERSION,
            }
        ))
    );
}

#[test]
fn manifest_functions_must_be_exported() {
    let manifest = manifest(vec![
        lerp_manifest().functions[0].clone(),
        function("missing", &[], &[("x", dimensionless())], dimensionless()),
    ]);
    let err = PluginHost::new()
        .load(&plugin(LERP_WAT, &manifest))
        .unwrap_err();
    assert_eq!(
        err,
        PluginLoadError::MissingFunctionExport {
            function: fn_name("missing")
        }
    );
}

#[test]
fn exported_wasm_type_must_match_the_manifest_arity() {
    // Manifest says two parameters; the wasm export takes one.
    let wat = r#"
    (module
      (func (export "add") (param f64) (result f64) (local.get 0)))
    "#;
    let manifest = manifest(vec![function(
        "add",
        &[],
        &[("a", dimensionless()), ("b", dimensionless())],
        dimensionless(),
    )]);
    let err = PluginHost::new().load(&plugin(wat, &manifest)).unwrap_err();
    assert!(matches!(
        err,
        PluginLoadError::FunctionTypeMismatch { function, .. } if function == fn_name("add")
    ));
}

#[test]
fn non_f64_exports_are_rejected() {
    let wat = r#"
    (module
      (func (export "f") (param i32) (result i32) (local.get 0)))
    "#;
    let manifest = manifest(vec![function(
        "f",
        &[],
        &[("x", dimensionless())],
        dimensionless(),
    )]);
    assert!(matches!(
        PluginHost::new().load(&plugin(wat, &manifest)).unwrap_err(),
        PluginLoadError::FunctionTypeMismatch { .. }
    ));
}

#[test]
fn manifest_signatures_using_non_base_dimensions_are_rejected() {
    let manifest = manifest(vec![function(
        "speed",
        &[],
        &[(
            "x",
            ManifestValueKind::Scalar(ManifestMonomial {
                vars: Vec::new(),
                fixed: vec![graphcal_plugin_abi::ManifestDimPower {
                    dim: "Velocity".to_string(),
                    pow: ManifestRational { num: 1, den: 1 },
                }],
            }),
        )],
        dimensionless(),
    )]);
    let err = PluginHost::new()
        .load(&plugin(LERP_WAT, &manifest))
        .unwrap_err();
    let PluginLoadError::InvalidSignature(convert) = err else {
        panic!("expected InvalidSignature, got {err:?}");
    };
    assert!(matches!(
        convert.kind,
        ConvertErrorKind::UnknownBaseDimension { dim } if dim == "Velocity"
    ));
}

#[test]
fn invalid_wasm_bytes_are_rejected() {
    // A structurally valid section layout wrapping garbage code: build a
    // valid header + manifest, then corrupt the module by appending a bogus
    // non-custom section that wasmi will reject.
    let manifest_bytes = lerp_manifest().to_json().unwrap();
    let mut wasm = graphcal_plugin_abi::section::EMPTY_MODULE.to_vec();
    wasm.extend_from_slice(&[1, 2, 0xFF, 0xFF]); // type section with garbage
    let wasm = embed_manifest(&wasm, manifest_bytes.as_bytes()).unwrap();
    assert!(matches!(
        PluginHost::new().load(&wasm).unwrap_err(),
        PluginLoadError::InvalidModule { .. }
    ));
}
