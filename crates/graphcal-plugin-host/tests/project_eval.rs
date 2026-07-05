//! End-to-end project tests: a `.gcl` project on disk importing a vendored
//! `.wasm` plugin, loaded through the real project loader and evaluated
//! through the registry built by [`register_project_plugins`].
#![cfg(test)]
#![expect(
    clippy::result_large_err,
    reason = "GraphcalError is inherently large and only constructed on the error path"
)]

use std::collections::HashMap;
use std::path::Path;

use graphcal_compiler::registry::error::GraphcalError;
use graphcal_eval::eval::{CompileError, EvalResult, Value};
use graphcal_eval::host_fns::HostFunctionRegistry;
use graphcal_eval::loader::{LoadedProject, load_project};
use graphcal_io::RealFileSystem;
use graphcal_plugin_abi::{
    ManifestFunction, ManifestMonomial, ManifestParam, ManifestRational, ManifestValueKind,
    ManifestVarPower, PluginManifest,
};
use graphcal_plugin_host::{PluginHost, register_project_plugins};

fn scalar_var(var: &str, num: i32, den: i32) -> ManifestValueKind {
    ManifestValueKind::Scalar(ManifestMonomial {
        vars: vec![ManifestVarPower {
            var: var.to_string(),
            pow: ManifestRational { num, den },
        }],
        fixed: Vec::new(),
    })
}

fn dimensionless() -> ManifestValueKind {
    ManifestValueKind::Scalar(ManifestMonomial::default())
}

fn manifest_fn(
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

fn plugin_bytes(wat_source: &str, functions: Vec<ManifestFunction>) -> Vec<u8> {
    let manifest = PluginManifest {
        abi_version: graphcal_plugin_abi::ABI_VERSION,
        functions,
    };
    let wasm = wat::parse_str(wat_source).unwrap();
    manifest.embed_into(&wasm).unwrap()
}

fn lerp_plugin() -> Vec<u8> {
    plugin_bytes(
        r#"
        (module
          (func (export "lerp") (param f64 f64 f64) (result f64)
            (f64.add
              (local.get 0)
              (f64.mul (f64.sub (local.get 1) (local.get 0)) (local.get 2)))))
        "#,
        vec![manifest_fn(
            "lerp",
            &["D"],
            &[
                ("a", scalar_var("D", 1, 1)),
                ("b", scalar_var("D", 1, 1)),
                ("t", dimensionless()),
            ],
            scalar_var("D", 1, 1),
        )],
    )
}

/// Write a single-file project with a vendored plugin, load it, build the
/// registry, and evaluate.
fn eval_project_with_plugin(
    dir: &Path,
    source: &str,
    plugin: Option<(&str, Vec<u8>)>,
) -> Result<EvalResult, CompileError> {
    if let Some((relative, bytes)) = plugin {
        let path = dir.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    let root = dir.join("main.gcl");
    std::fs::write(&root, source).unwrap();

    let fs = RealFileSystem::default();
    let project = load_project(&root, None, &fs)?;
    let mut registry = HostFunctionRegistry::new();
    register_project_plugins(&PluginHost::new(), &project, &mut registry);
    graphcal_eval::eval::compile_and_eval_from_project_with_host_fns(
        &project,
        &HashMap::new(),
        &registry,
    )
}

fn value_for<'a>(result: &'a EvalResult, name: &str) -> &'a Value {
    result
        .all
        .iter()
        .find(|(decl_name, _, _)| decl_name.to_string() == name)
        .unwrap_or_else(|| panic!("declaration `{name}` not found"))
        .1
        .as_ref()
        .unwrap_or_else(|err| panic!("declaration `{name}` has error: {err}"))
}

const LERP_IMPORT: &str = r#"
import plugin "plugins/demo.wasm" as demo {
    fn lerp<D>(a: D, b: D, t: Dimensionless) -> D;
}
"#;

#[test]
fn wasm_plugin_evaluates_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let source = format!(
        "{LERP_IMPORT}\nparam a: Length = 1.0 m;\nnode mid: Length = demo.lerp(@a, 3.0 m, 0.5);\n"
    );
    let result = eval_project_with_plugin(
        dir.path(),
        &source,
        Some(("plugins/demo.wasm", lerp_plugin())),
    )
    .unwrap();
    let value = value_for(&result, "mid");
    assert!((value.si_value().unwrap() - 2.0).abs() < 1e-12, "{value:?}");
}

#[test]
fn declared_signature_must_match_the_manifest() {
    let dir = tempfile::tempdir().unwrap();
    // Result declared as D^2: dimensionally different from the manifest's D.
    let source = r#"
import plugin "plugins/demo.wasm" as demo {
    fn lerp<D>(a: D, b: D, t: Dimensionless) -> D^2;
}
node x: Dimensionless = demo.lerp(1.0, 3.0, 0.5);
"#;
    let err = eval_project_with_plugin(
        dir.path(),
        source,
        Some(("plugins/demo.wasm", lerp_plugin())),
    )
    .unwrap_err();
    let CompileError::Eval(GraphcalError::ExternSignatureMismatch {
        name,
        declared,
        provided,
        ..
    }) = err
    else {
        panic!("expected ExternSignatureMismatch, got {err:?}");
    };
    assert_eq!(name.as_str(), "lerp");
    assert!(declared.contains("D^2"), "declared: {declared}");
    assert!(!provided.contains("D^2"), "provided: {provided}");
}

#[test]
fn param_and_dim_var_renaming_is_not_a_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    // Same structure as the manifest, different variable and param names.
    let source = r#"
import plugin "plugins/demo.wasm" as demo {
    fn lerp<T>(lo: T, hi: T, frac: Dimensionless) -> T;
}
node mid: Length = demo.lerp(1.0 m, 3.0 m, 0.5);
"#;
    let result = eval_project_with_plugin(
        dir.path(),
        source,
        Some(("plugins/demo.wasm", lerp_plugin())),
    )
    .unwrap();
    let value = value_for(&result, "mid");
    assert!((value.si_value().unwrap() - 2.0).abs() < 1e-12, "{value:?}");
}

#[test]
fn forbidden_imports_surface_as_a_dedicated_diagnostic() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = plugin_bytes(
        r#"
        (module
          (import "wasi_snapshot_preview1" "fd_write"
            (func (param i32 i32 i32 i32) (result i32)))
          (func (export "lerp") (param f64 f64 f64) (result f64) (local.get 0)))
        "#,
        vec![manifest_fn(
            "lerp",
            &["D"],
            &[
                ("a", scalar_var("D", 1, 1)),
                ("b", scalar_var("D", 1, 1)),
                ("t", dimensionless()),
            ],
            scalar_var("D", 1, 1),
        )],
    );
    let source = format!("{LERP_IMPORT}\nnode x: Dimensionless = demo.lerp(0.0, 1.0, 0.5);\n");
    let err = eval_project_with_plugin(dir.path(), &source, Some(("plugins/demo.wasm", bytes)))
        .unwrap_err();
    let CompileError::Eval(GraphcalError::PluginForbiddenImport {
        import_module,
        import_name,
        ..
    }) = err
    else {
        panic!("expected PluginForbiddenImport, got {err:?}");
    };
    assert_eq!(import_module, "wasi_snapshot_preview1");
    assert_eq!(import_name, "fd_write");
}

#[test]
fn missing_plugin_file_is_reported_at_the_import() {
    let dir = tempfile::tempdir().unwrap();
    let source = format!("{LERP_IMPORT}\nnode x: Dimensionless = demo.lerp(0.0, 1.0, 0.5);\n");
    let err = eval_project_with_plugin(dir.path(), &source, None).unwrap_err();
    let CompileError::Eval(GraphcalError::PluginLoadFailed { reason, .. }) = err else {
        panic!("expected PluginLoadFailed, got {err:?}");
    };
    assert!(reason.contains("cannot read"), "reason: {reason}");
}

#[test]
fn plugin_paths_may_not_leave_the_project_root() {
    let dir = tempfile::tempdir().unwrap();
    let source = r#"
import plugin "../outside.wasm" as demo {
    fn lerp<D>(a: D, b: D, t: Dimensionless) -> D;
}
node x: Dimensionless = demo.lerp(0.0, 1.0, 0.5);
"#;
    let err = eval_project_with_plugin(dir.path(), source, None).unwrap_err();
    let CompileError::Eval(GraphcalError::PluginLoadFailed { reason, .. }) = err else {
        panic!("expected PluginLoadFailed, got {err:?}");
    };
    assert!(reason.contains("must be relative"), "reason: {reason}");
}

#[test]
fn from_source_projects_report_missing_filesystem() {
    let source = format!("{LERP_IMPORT}\nnode x: Dimensionless = demo.lerp(0.0, 1.0, 0.5);\n");
    let project = LoadedProject::from_source(&source, "buffer.gcl").unwrap();
    let registry = HostFunctionRegistry::new();
    let err = graphcal_eval::eval::compile_and_eval_from_project_with_host_fns(
        &project,
        &HashMap::new(),
        &registry,
    )
    .unwrap_err();
    let CompileError::Eval(GraphcalError::PluginLoadFailed { reason, .. }) = err else {
        panic!("expected PluginLoadFailed, got {err:?}");
    };
    assert!(reason.contains("without a project on disk"), "{reason}");
}

#[test]
fn plugin_failures_are_contained_per_node() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = plugin_bytes(
        r#"
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
        "#,
        vec![manifest_fn(
            "inverse",
            &["D"],
            &[("x", scalar_var("D", 1, 1))],
            scalar_var("D", -1, 1),
        )],
    );
    let source = r#"
import plugin "plugins/inv.wasm" as inv {
    fn inverse<D>(x: D) -> D^-1;
}
param zero: Dimensionless = 0.0;
node bad: Dimensionless = inv.inverse(@zero);
node good: Dimensionless = inv.inverse(4.0);
node dependent: Dimensionless = @bad + 1.0;
"#;
    let result =
        eval_project_with_plugin(dir.path(), source, Some(("plugins/inv.wasm", bytes))).unwrap();

    let bad = result
        .all
        .iter()
        .find(|(name, _, _)| name.to_string() == "bad")
        .unwrap();
    let err = bad.1.as_ref().unwrap_err();
    assert!(
        err.to_string().contains("division by zero"),
        "unexpected error: {err}"
    );

    let good = value_for(&result, "good");
    assert!((good.si_value().unwrap() - 0.25).abs() < 1e-12);

    let dependent = result
        .all
        .iter()
        .find(|(name, _, _)| name.to_string() == "dependent")
        .unwrap();
    assert!(dependent.1.is_err(), "dependent must fail transitively");
}

/// Evaluate a manifest-ful (package-mode) project: `graphcal.toml`, the
/// root file under the package namespace, a vendored plugin, and optionally
/// a `graphcal.lock` pinning the plugin to `pinned_sha`.
fn eval_package_project(
    dir: &Path,
    plugin_bytes: &[u8],
    pinned_sha: Option<&str>,
) -> Result<EvalResult, CompileError> {
    std::fs::write(dir.join("graphcal.toml"), "[package]\nname = \"proj\"\n").unwrap();
    std::fs::create_dir_all(dir.join("src/proj")).unwrap();
    std::fs::create_dir_all(dir.join("plugins")).unwrap();
    std::fs::write(dir.join("plugins/demo.wasm"), plugin_bytes).unwrap();
    let root = dir.join("src/proj/main.gcl");
    std::fs::write(
        &root,
        format!("{LERP_IMPORT}\nnode mid: Length = demo.lerp(1.0 m, 3.0 m, 0.5);\n"),
    )
    .unwrap();

    if let Some(sha) = pinned_sha {
        let lockfile = graphcal_package::Lockfile {
            lock_version: graphcal_package::LOCK_VERSION,
            created_by: "test".to_string(),
            graphcal_version: env!("CARGO_PKG_VERSION").to_string(),
            stdlib_version: graphcal_package::STDLIB_VERSION.to_string(),
            root: graphcal_package::PackageInstanceId::new("pkg-proj").unwrap(),
            packages: vec![graphcal_package::LockedPackage {
                id: graphcal_package::PackageInstanceId::new("pkg-proj").unwrap(),
                name: graphcal_package::PackageName::new("proj").unwrap(),
                source_dir: "src".into(),
                source: graphcal_package::PackageSource::Path {
                    path: ".".to_string(),
                },
                dependencies: std::collections::BTreeMap::new(),
            }],
            plugins: vec![graphcal_package::LockedPlugin::new("plugins/demo.wasm", sha).unwrap()],
        };
        std::fs::write(
            dir.join("graphcal.lock"),
            lockfile.to_deterministic_toml().unwrap(),
        )
        .unwrap();
    }

    let fs = RealFileSystem::default();
    let project = load_project(&root, None, &fs)?;
    let mut registry = HostFunctionRegistry::new();
    register_project_plugins(&PluginHost::new(), &project, &mut registry);
    graphcal_eval::eval::compile_and_eval_from_project_with_host_fns(
        &project,
        &HashMap::new(),
        &registry,
    )
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut out, byte| {
            use std::fmt::Write as _;
            let _ = write!(out, "{byte:02x}");
            out
        })
}

#[test]
fn unpinned_plugins_are_rejected_in_package_projects() {
    let dir = tempfile::tempdir().unwrap();
    let err = eval_package_project(dir.path(), &lerp_plugin(), None).unwrap_err();
    assert!(
        matches!(
            err,
            CompileError::Eval(GraphcalError::PluginNotPinned { ref plugin, .. })
                if plugin.as_str() == "plugins/demo.wasm"
        ),
        "expected PluginNotPinned, got {err:?}"
    );
}

#[test]
fn hash_mismatches_against_the_pin_are_hard_errors() {
    let dir = tempfile::tempdir().unwrap();
    let wrong_sha = "0".repeat(64);
    let err = eval_package_project(dir.path(), &lerp_plugin(), Some(&wrong_sha)).unwrap_err();
    assert!(
        matches!(
            err,
            CompileError::Eval(GraphcalError::PluginHashMismatch { ref expected, .. })
                if *expected == wrong_sha
        ),
        "expected PluginHashMismatch, got {err:?}"
    );
}

#[test]
fn correctly_pinned_plugins_evaluate() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = lerp_plugin();
    let sha = sha256_hex(&bytes);
    let result = eval_package_project(dir.path(), &bytes, Some(&sha)).unwrap();
    let value = value_for(&result, "mid");
    assert!((value.si_value().unwrap() - 2.0).abs() < 1e-12, "{value:?}");
}

#[test]
fn virtual_projects_without_a_manifest_load_unpinned() {
    // No graphcal.toml → no lockfile regime; the sandbox and limits still
    // apply. Covered end-to-end by `wasm_plugin_evaluates_end_to_end`; this
    // test documents the boundary by contrast: same layout plus a manifest
    // demands a pin (see `unpinned_plugins_are_rejected_in_package_projects`).
    let dir = tempfile::tempdir().unwrap();
    let source = format!("{LERP_IMPORT}\nnode mid: Length = demo.lerp(1.0 m, 3.0 m, 0.5);\n");
    let result = eval_project_with_plugin(
        dir.path(),
        &source,
        Some(("plugins/demo.wasm", lerp_plugin())),
    )
    .unwrap();
    assert!((value_for(&result, "mid").si_value().unwrap() - 2.0).abs() < 1e-12);
}

#[test]
fn host_registry_plugins_coexist_with_wasm_plugins() {
    let dir = tempfile::tempdir().unwrap();
    let source = r#"
import plugin "graphcal:demo" as native {
    fn inverse<D>(x: D) -> D^-1;
}
import plugin "plugins/demo.wasm" as wasm {
    fn lerp<D>(a: D, b: D, t: Dimensionless) -> D;
}
node a: Dimensionless = native.inverse(4.0);
node b: Length = wasm.lerp(1.0 m, 3.0 m, 0.5);
"#;
    if let Some((relative, bytes)) = Some(("plugins/demo.wasm", lerp_plugin())) {
        let path = dir.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    let root = dir.path().join("main.gcl");
    std::fs::write(&root, source).unwrap();

    let fs = RealFileSystem::default();
    let project = load_project(&root, None, &fs).unwrap();
    let mut registry = graphcal_eval::host_fns::demo_registry();
    register_project_plugins(&PluginHost::new(), &project, &mut registry);
    let result = graphcal_eval::eval::compile_and_eval_from_project_with_host_fns(
        &project,
        &HashMap::new(),
        &registry,
    )
    .unwrap();

    assert!((value_for(&result, "a").si_value().unwrap() - 0.25).abs() < 1e-12);
    assert!((value_for(&result, "b").si_value().unwrap() - 2.0).abs() < 1e-12);
}
