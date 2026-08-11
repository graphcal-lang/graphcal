//! End-to-end runtime tests: real wasm modules built from WAT at test time
//! (no binary fixtures in the repository), with manifests embedded through
//! the ABI crate — the same path the authoring SDK will take.
#![cfg(test)]

use std::sync::Arc;

use graphcal_compiler::syntax::function_name::FnName;
use graphcal_eval::host_fns::{HostArray, HostFnValue};
use graphcal_plugin_abi::{
    ManifestDecodeError, ManifestFromWasmError, ManifestFunction, ManifestMonomial, ManifestParam,
    ManifestRational, ManifestValueKind, ManifestVarPower, PluginManifest, SectionError,
    embed_manifest,
};
use graphcal_plugin_host::{
    ConvertErrorKind, PluginCallError, PluginHost, PluginLimits, PluginLoadError,
};

fn quantity_var(var: &str) -> ManifestValueKind {
    ManifestValueKind::Quantity(ManifestMonomial {
        vars: vec![ManifestVarPower {
            var: var.to_string(),
            pow: ManifestRational { num: 1, den: 1 },
        }],
        fixed: Vec::new(),
    })
}

fn dimensionless() -> ManifestValueKind {
    ManifestValueKind::Quantity(ManifestMonomial::default())
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
        index_vars: Vec::new(),
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

/// Wrap raw floats as single-slot host values for a call.
fn f64_values(values: &[f64]) -> Vec<HostFnValue> {
    values.iter().map(|v| HostFnValue::F64(*v)).collect()
}

fn vector(values: Vec<f64>) -> HostFnValue {
    HostFnValue::Array(HostArray::vector(values).unwrap())
}

/// Unwrap an f64 result (panics on a composite value — a test bug).
fn f64_value(value: &HostFnValue) -> f64 {
    match value {
        HostFnValue::F64(raw) => *raw,
        HostFnValue::Array(_) | HostFnValue::Record(_) => {
            panic!("expected an f64 result, got a composite value")
        }
    }
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
            ("a", quantity_var("D")),
            ("b", quantity_var("D")),
            ("t", dimensionless()),
        ],
        quantity_var("D"),
    )])
}

#[test]
fn calls_a_quantity_kernel() {
    let host = PluginHost::new();
    let module = host.load(&plugin(LERP_WAT, &lerp_manifest())).unwrap();
    let result = module
        .call(&fn_name("lerp"), &f64_values(&[0.0, 10.0, 0.25]))
        .unwrap();
    assert!((f64_value(&result) - 2.5).abs() < f64::EPSILON);
    // Second call reuses the pooled instance.
    let result = module
        .call(&fn_name("lerp"), &f64_values(&[1.0, 3.0, 0.5]))
        .unwrap();
    assert!((f64_value(&result) - 2.0).abs() < f64::EPSILON);
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
        &[("x", quantity_var("D"))],
        ManifestValueKind::Quantity(ManifestMonomial {
            vars: vec![ManifestVarPower {
                var: "D".to_string(),
                pow: ManifestRational { num: -1, den: 1 },
            }],
            fixed: Vec::new(),
        }),
    )]);
    let host = PluginHost::new();
    let module = host.load(&plugin(wat, &manifest)).unwrap();

    let err = module
        .call(&fn_name("inverse"), &f64_values(&[0.0]))
        .unwrap_err();
    assert_eq!(
        err,
        PluginCallError::Failed {
            message: "division by zero".to_string()
        }
    );

    // The damaged instance is discarded; the next call gets a fresh one.
    let ok = module
        .call(&fn_name("inverse"), &f64_values(&[4.0]))
        .unwrap();
    assert!((f64_value(&ok) - 0.25).abs() < f64::EPSILON);
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
        module
            .call(&fn_name("spin"), &f64_values(&[0.0]))
            .unwrap_err(),
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
        module
            .call(&fn_name("id"), &f64_values(&[1.0]))
            .unwrap_err(),
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
    let grown = module.call(&fn_name("grow"), &f64_values(&[0.0])).unwrap();
    // Started at 1 page; the limiter must stop growth at 64 pages total.
    let grown = f64_value(&grown);
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
        module
            .call(&fn_name("boom"), &f64_values(&[0.0]))
            .unwrap_err(),
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
    assert!(f64_value(&module.call(&fn_name("inf"), &f64_values(&[0.0])).unwrap()).is_infinite());
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
    let wasm = embed_manifest(&wasm, br#"{"abi_version":5,"shape":"unknown"}"#).unwrap();
    assert_eq!(
        PluginHost::new().load(&wasm).unwrap_err(),
        PluginLoadError::Manifest(ManifestFromWasmError::Decode(
            ManifestDecodeError::UnsupportedAbiVersion {
                found: 5,
                supported: graphcal_plugin_abi::ABI_VERSION,
            }
        ))
    );
}

#[test]
fn v2_manifests_are_rejected_with_a_version_error() {
    // Older modules report a version error asking for a rebuild instead of a
    // misleading shape error under the current shaped-array ABI.
    let wasm = wat::parse_str(LERP_WAT).unwrap();
    let wasm = embed_manifest(&wasm, br#"{"abi_version":2,"functions":[]}"#).unwrap();
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
            ManifestValueKind::Quantity(ManifestMonomial {
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

// ---------------------------------------------------------------------------
// Array (buffer protocol) fixtures — issue #25 Phase D
// ---------------------------------------------------------------------------

/// A plugin with the buffer protocol: a bump allocator plus
/// `scale(xs: D[I], k) -> D[I]` and `total(xs: D[I]) -> D`.
const ARRAY_WAT: &str = r#"
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
  (func (export "scale") (param $ptr i32) (param $len i32) (param $k f64) (param $out i32)
    (local $i i32)
    (block $done
      (loop $loop
        (br_if $done (i32.ge_s (local.get $i) (local.get $len)))
        (f64.store
          (i32.add (local.get $out) (i32.mul (local.get $i) (i32.const 8)))
          (f64.mul
            (f64.load (i32.add (local.get $ptr) (i32.mul (local.get $i) (i32.const 8))))
            (local.get $k)))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $loop))))
  (func (export "total") (param $ptr i32) (param $len i32) (result f64)
    (local $i i32)
    (local $sum f64)
    (block $done
      (loop $loop
        (br_if $done (i32.ge_s (local.get $i) (local.get $len)))
        (local.set $sum
          (f64.add
            (local.get $sum)
            (f64.load (i32.add (local.get $ptr) (i32.mul (local.get $i) (i32.const 8))))))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $loop)))
    (local.get $sum)))
"#;

fn array_kind(var: &str, index: &str) -> ManifestValueKind {
    ManifestValueKind::Array {
        element: ManifestMonomial {
            vars: vec![ManifestVarPower {
                var: var.to_string(),
                pow: ManifestRational { num: 1, den: 1 },
            }],
            fixed: Vec::new(),
        },
        indexes: vec![index.to_string()],
    }
}

fn array_function(
    name: &str,
    params: &[(&str, ManifestValueKind)],
    result: ManifestValueKind,
) -> ManifestFunction {
    ManifestFunction {
        name: name.to_string(),
        dim_vars: vec!["D".to_string()],
        index_vars: vec!["I".to_string()],
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

fn array_manifest() -> PluginManifest {
    manifest(vec![
        array_function(
            "scale",
            &[("xs", array_kind("D", "I")), ("k", dimensionless())],
            array_kind("D", "I"),
        ),
        array_function("total", &[("xs", array_kind("D", "I"))], quantity_var("D")),
    ])
}

#[test]
fn calls_an_array_kernel_with_an_array_result() {
    let host = PluginHost::new();
    let module = host.load(&plugin(ARRAY_WAT, &array_manifest())).unwrap();
    let result = module
        .call(
            &fn_name("scale"),
            &[vector(vec![1.0, 2.5, -4.0]), HostFnValue::F64(2.0)],
        )
        .unwrap();
    assert_eq!(result, vector(vec![2.0, 5.0, -8.0]));

    // The pooled instance is reused and the buffers were freed: a second
    // call must see fresh inputs, not stale memory.
    let result = module
        .call(
            &fn_name("scale"),
            &[vector(vec![10.0]), HostFnValue::F64(0.5)],
        )
        .unwrap();
    assert_eq!(result, vector(vec![5.0]));
}

const MATRIX_WAT: &str = r#"
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
  (func (export "transpose")
    (param $ptr i32) (param $rows i32) (param $columns i32) (param $out i32)
    (local $row i32)
    (local $column i32)
    (block $columns_done
      (loop $columns_loop
        (br_if $columns_done (i32.ge_u (local.get $column) (local.get $columns)))
        (local.set $row (i32.const 0))
        (block $rows_done
          (loop $rows_loop
            (br_if $rows_done (i32.ge_u (local.get $row) (local.get $rows)))
            (f64.store
              (i32.add
                (local.get $out)
                (i32.mul
                  (i32.add
                    (i32.mul (local.get $column) (local.get $rows))
                    (local.get $row))
                  (i32.const 8)))
              (f64.load
                (i32.add
                  (local.get $ptr)
                  (i32.mul
                    (i32.add
                      (i32.mul (local.get $row) (local.get $columns))
                      (local.get $column))
                    (i32.const 8)))))
            (local.set $row (i32.add (local.get $row) (i32.const 1)))
            (br $rows_loop)))
        (local.set $column (i32.add (local.get $column) (i32.const 1)))
        (br $columns_loop))))
)
"#;

fn matrix_manifest() -> PluginManifest {
    let element = ManifestMonomial {
        vars: vec![ManifestVarPower {
            var: "D".to_string(),
            pow: ManifestRational { num: 1, den: 1 },
        }],
        fixed: Vec::new(),
    };
    manifest(vec![ManifestFunction {
        name: "transpose".to_string(),
        dim_vars: vec!["D".to_string()],
        index_vars: vec!["I".to_string(), "J".to_string()],
        params: vec![ManifestParam {
            name: "matrix".to_string(),
            kind: ManifestValueKind::Array {
                element: element.clone(),
                indexes: vec!["I".to_string(), "J".to_string()],
            },
        }],
        result: ManifestValueKind::Array {
            element,
            indexes: vec!["J".to_string(), "I".to_string()],
        },
    }])
}

#[test]
fn calls_a_multi_axis_kernel_and_reorders_result_shape() {
    let host = PluginHost::new();
    let module = host.load(&plugin(MATRIX_WAT, &matrix_manifest())).unwrap();
    let input = HostArray::try_new(vec![2, 3], vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]).unwrap();
    let result = module
        .call(&fn_name("transpose"), &[HostFnValue::Array(input)])
        .unwrap();
    assert_eq!(
        result,
        HostFnValue::Array(
            HostArray::try_new(vec![3, 2], vec![1.0, 4.0, 2.0, 5.0, 3.0, 6.0]).unwrap()
        )
    );
}

#[test]
fn calls_an_array_kernel_with_a_quantity_result() {
    let host = PluginHost::new();
    let module = host.load(&plugin(ARRAY_WAT, &array_manifest())).unwrap();
    let result = module
        .call(&fn_name("total"), &[vector(vec![1.0, 2.0, 3.5])])
        .unwrap();
    assert!((f64_value(&result) - 6.5).abs() < f64::EPSILON);
}

#[test]
fn array_manifests_require_the_allocator_exports() {
    // Memory but no graphcal_alloc/graphcal_free.
    let wat = r#"
    (module
      (memory (export "memory") 1)
      (func (export "total") (param i32 i32) (result f64) (f64.const 0)))
    "#;
    let manifest = manifest(vec![array_function(
        "total",
        &[("xs", array_kind("D", "I"))],
        quantity_var("D"),
    )]);
    assert!(matches!(
        PluginHost::new().load(&plugin(wat, &manifest)).unwrap_err(),
        PluginLoadError::MissingBufferProtocolExport { export, .. } if export == "graphcal_alloc"
    ));
}

#[test]
fn array_manifests_require_an_exported_memory() {
    let wat = r#"
    (module
      (func (export "graphcal_alloc") (param i32) (result i32) (i32.const 0))
      (func (export "graphcal_free") (param i32 i32))
      (func (export "total") (param i32 i32) (result f64) (f64.const 0)))
    "#;
    let manifest = manifest(vec![array_function(
        "total",
        &[("xs", array_kind("D", "I"))],
        quantity_var("D"),
    )]);
    assert!(matches!(
        PluginHost::new().load(&plugin(wat, &manifest)).unwrap_err(),
        PluginLoadError::MissingBufferProtocolExport { export, .. } if export == "memory"
    ));
}

#[test]
fn denied_allocator_growth_reports_allocation_failure_before_invoking_the_kernel() {
    let wat = r#"
    (module
      (memory (export "memory") 1)
      (func (export "graphcal_alloc") (param i32) (result i32)
        (if (result i32)
          (i32.eq (memory.grow (i32.const 1)) (i32.const -1))
          (then (i32.const 0))
          (else (i32.const 8))))
      (func (export "graphcal_free") (param i32 i32))
      (func (export "total") (param i32 i32) (result f64)
        (unreachable)))
    "#;
    let manifest = manifest(vec![array_function(
        "total",
        &[("xs", array_kind("D", "I"))],
        quantity_var("D"),
    )]);
    let host = PluginHost::with_limits(PluginLimits {
        max_memory_bytes: 64 * 1024,
        ..PluginLimits::default()
    });
    let module = host.load(&plugin(wat, &manifest)).unwrap();

    assert_eq!(
        module
            .call(&fn_name("total"), &[vector(vec![1.0, 2.0, 3.0])])
            .unwrap_err(),
        PluginCallError::AllocationFailed { bytes: 24 }
    );
}

#[test]
fn misaligned_allocator_pointer_is_rejected_before_invoking_the_kernel() {
    let wat = r#"
    (module
      (memory (export "memory") 1)
      (func (export "graphcal_alloc") (param i32) (result i32) (i32.const 12))
      (func (export "graphcal_free") (param i32 i32))
      (func (export "total") (param i32 i32) (result f64)
        (unreachable)))
    "#;
    let manifest = manifest(vec![array_function(
        "total",
        &[("xs", array_kind("D", "I"))],
        quantity_var("D"),
    )]);
    let module = PluginHost::new().load(&plugin(wat, &manifest)).unwrap();

    assert_eq!(
        module
            .call(&fn_name("total"), &[vector(vec![1.0])])
            .unwrap_err(),
        PluginCallError::MisalignedAllocatorPointer {
            pointer: 12,
            required_alignment: 8,
        }
    );
}

#[test]
fn out_of_bounds_allocator_range_is_rejected_before_invoking_the_kernel() {
    let wat = r#"
    (module
      (memory (export "memory") 1)
      (func (export "graphcal_alloc") (param i32) (result i32) (i32.const 65528))
      (func (export "graphcal_free") (param i32 i32))
      (func (export "total") (param i32 i32) (result f64)
        (unreachable)))
    "#;
    let manifest = manifest(vec![array_function(
        "total",
        &[("xs", array_kind("D", "I"))],
        quantity_var("D"),
    )]);
    let module = PluginHost::new().load(&plugin(wat, &manifest)).unwrap();

    assert_eq!(
        module
            .call(&fn_name("total"), &[vector(vec![1.0, 2.0])])
            .unwrap_err(),
        PluginCallError::AllocatorBufferOutOfBounds {
            pointer: 0xFFF8,
            bytes: 16,
            memory_bytes: 0x0001_0000,
        }
    );
}

#[test]
fn array_functions_with_single_value_wasm_types_are_rejected() {
    // The manifest declares an array parameter, but the export takes f64s.
    let wat = r#"
    (module
      (memory (export "memory") 1)
      (func (export "graphcal_alloc") (param i32) (result i32) (i32.const 0))
      (func (export "graphcal_free") (param i32 i32))
      (func (export "total") (param f64) (result f64) (local.get 0)))
    "#;
    let manifest = manifest(vec![array_function(
        "total",
        &[("xs", array_kind("D", "I"))],
        quantity_var("D"),
    )]);
    assert!(matches!(
        PluginHost::new().load(&plugin(wat, &manifest)).unwrap_err(),
        PluginLoadError::FunctionTypeMismatch { expected, .. }
            if expected == "(i32, i32) -> (f64)"
    ));
}

#[test]
fn manifests_with_duplicate_index_vars_are_rejected() {
    let mut fun = array_function("total", &[("xs", array_kind("D", "I"))], quantity_var("D"));
    fun.index_vars = vec!["I".to_string(), "I".to_string()];
    let manifest = manifest(vec![fun]);
    let err = PluginHost::new()
        .load(&plugin(ARRAY_WAT, &manifest))
        .unwrap_err();
    let PluginLoadError::Manifest(ManifestFromWasmError::Decode(ManifestDecodeError::Invalid(
        invalid,
    ))) = err
    else {
        panic!("expected a manifest validation error, got {err:?}");
    };
    assert!(matches!(
        invalid,
        graphcal_plugin_abi::ManifestValidationError::DuplicateIndexVar { .. }
    ));
}

// ---------------------------------------------------------------------------
// Struct returns (issue #25 Phase D)
// ---------------------------------------------------------------------------

/// The buffer-protocol plugin extended with `span(xs: D[I]) -> {min, max}`.
const STRUCT_WAT: &str = r#"
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
  (func (export "span") (param $ptr i32) (param $len i32) (param $out i32)
    (local $i i32)
    (local $x f64)
    (local $min f64)
    (local $max f64)
    (local.set $min (f64.load (local.get $ptr)))
    (local.set $max (f64.load (local.get $ptr)))
    (block $done
      (loop $loop
        (br_if $done (i32.ge_s (local.get $i) (local.get $len)))
        (local.set $x
          (f64.load (i32.add (local.get $ptr) (i32.mul (local.get $i) (i32.const 8)))))
        (local.set $min (f64.min (local.get $min) (local.get $x)))
        (local.set $max (f64.max (local.get $max) (local.get $x)))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $loop)))
    (f64.store (local.get $out) (local.get $min))
    (f64.store (i32.add (local.get $out) (i32.const 8)) (local.get $max))))
"#;

fn struct_manifest() -> PluginManifest {
    use graphcal_plugin_abi::{ManifestField, ManifestFieldKind};

    let mut function = array_function("span", &[("xs", array_kind("D", "I"))], quantity_var("D"));
    function.result = ManifestValueKind::Struct {
        fields: vec![
            ManifestField {
                name: "min".to_string(),
                kind: ManifestFieldKind::Quantity(ManifestMonomial::default()),
            },
            ManifestField {
                name: "max".to_string(),
                kind: ManifestFieldKind::Quantity(ManifestMonomial::default()),
            },
        ],
    };
    manifest(vec![function])
}

#[test]
fn calls_a_struct_returning_kernel() {
    let host = PluginHost::new();
    let module = host.load(&plugin(STRUCT_WAT, &struct_manifest())).unwrap();
    let result = module
        .call(&fn_name("span"), &[vector(vec![3.0, -1.5, 2.0])])
        .unwrap();
    assert_eq!(result, HostFnValue::Record(vec![-1.5, 3.0]));
}

#[test]
fn struct_field_monomials_with_dim_vars_are_rejected() {
    use graphcal_plugin_abi::{ManifestField, ManifestFieldKind};

    let mut function = array_function("span", &[("xs", array_kind("D", "I"))], quantity_var("D"));
    function.result = ManifestValueKind::Struct {
        fields: vec![ManifestField {
            name: "min".to_string(),
            kind: ManifestFieldKind::Quantity(ManifestMonomial {
                vars: vec![ManifestVarPower {
                    var: "D".to_string(),
                    pow: ManifestRational { num: 1, den: 1 },
                }],
                fixed: Vec::new(),
            }),
        }],
    };
    let err = PluginHost::new()
        .load(&plugin(STRUCT_WAT, &manifest(vec![function])))
        .unwrap_err();
    let PluginLoadError::Manifest(ManifestFromWasmError::Decode(ManifestDecodeError::Invalid(
        invalid,
    ))) = err
    else {
        panic!("expected a manifest validation error, got {err:?}");
    };
    assert!(matches!(
        invalid,
        graphcal_plugin_abi::ManifestValidationError::StructFieldWithDimVars { .. }
    ));
}
