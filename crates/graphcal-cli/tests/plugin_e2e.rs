//! End-to-end authoring-workflow test (Phase C of issue #25):
//! `graphcal plugin new` → `cargo build --target wasm32-unknown-unknown`
//! → vendor → `graphcal deps lock` → `graphcal eval`, plus a direct
//! host-level check that Rust panics inside a plugin surface as readable
//! failure messages.
//!
//! Requires the `wasm32-unknown-unknown` target (and `rustup` to detect
//! it); otherwise the test skips with a message on stderr. CI installs the
//! target so the full path always runs there. Build artifacts are cached
//! under the workspace `target/` so repeated local runs stay fast.

#![cfg(test)]

use graphcal_eval::host_fns::HostFnValue;
use std::path::{Path, PathBuf};
use std::process::Command;

fn graphcal_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_graphcal"))
}

fn cargo_bin() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

fn repo_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

/// Shared cargo target dir for the plugin builds, kept inside the
/// workspace `target/` so artifacts survive across test runs.
fn plugin_target_dir() -> PathBuf {
    repo_root().join("target").join("plugin-e2e-target")
}

fn wasm_target_installed() -> bool {
    Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .is_ok_and(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .any(|line| line.trim() == "wasm32-unknown-unknown")
        })
}

/// Point the scaffold at the workspace SDK crate (the scaffold pins the
/// published version, which need not exist while developing) and drop the
/// toolchain file so the test never triggers a rustup component install.
fn localize_scaffold(scaffold: &Path) {
    let manifest_path = scaffold.join("Cargo.toml");
    let manifest = std::fs::read_to_string(&manifest_path).unwrap();
    let sdk_path = repo_root().join("crates").join("graphcal-plugin");
    let needle = format!("graphcal-plugin = \"={}\"", env!("CARGO_PKG_VERSION"));
    assert!(
        manifest.contains(&needle),
        "scaffold Cargo.toml no longer pins the SDK version:\n{manifest}"
    );
    let manifest = manifest.replace(
        &needle,
        &format!("graphcal-plugin = {{ path = {sdk_path:?} }}"),
    );
    std::fs::write(&manifest_path, manifest).unwrap();
    std::fs::remove_file(scaffold.join("rust-toolchain.toml")).unwrap();
}

/// Build a plugin crate for wasm and return the artifact path.
///
/// Ambient flags are scrubbed: under `cargo llvm-cov` the inherited
/// `RUSTFLAGS` would inject `-C instrument-coverage`, which cannot link
/// for `wasm32-unknown-unknown` — and a plugin build should not inherit
/// this workspace's flags in any case.
fn build_wasm(scaffold: &Path, artifact: &str) -> PathBuf {
    let output = cargo_bin()
        .current_dir(scaffold)
        .env("CARGO_TARGET_DIR", plugin_target_dir())
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env_remove("LLVM_PROFILE_FILE")
        .args(["build", "--release", "--target", "wasm32-unknown-unknown"])
        .output()
        .expect("failed to spawn cargo");
    assert!(
        output.status.success(),
        "wasm build failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let path = plugin_target_dir()
        .join("wasm32-unknown-unknown")
        .join("release")
        .join(artifact);
    assert!(path.is_file(), "missing artifact {}", path.display());
    path
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "sequential end-to-end workflow steps: scaffold, build, test, vendor, lock, eval"
)]
fn scaffolded_plugin_builds_locks_and_evaluates() {
    if !wasm_target_installed() {
        eprintln!(
            "SKIPPED: scaffolded_plugin_builds_locks_and_evaluates needs the \
             wasm32-unknown-unknown target (rustup target add wasm32-unknown-unknown)"
        );
        return;
    }

    let dir = tempfile::tempdir().unwrap();

    // --- graphcal plugin new (template kept verbatim except localization).
    let scaffold = dir.path().join("e2e-kernels");
    let output = graphcal_bin()
        .args(["plugin", "new", "e2e-kernels", "--dir"])
        .arg(&scaffold)
        .output()
        .expect("failed to run graphcal");
    assert!(
        output.status.success(),
        "plugin new failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    localize_scaffold(&scaffold);

    // --- cargo build --target wasm32-unknown-unknown
    let artifact = build_wasm(&scaffold, "e2e_kernels.wasm");

    // --- graphcal plugin test: validation + a real sandboxed call.
    let output = graphcal_bin()
        .args(["plugin", "test"])
        .arg(&artifact)
        .args(["--call", "lerp", "1.0", "3.0", "0.5"])
        .output()
        .expect("failed to run graphcal");
    assert!(
        output.status.success(),
        "plugin test failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D;"),
        "{stdout}"
    );
    assert!(
        stdout.contains("fn share<D: Dim, I: Index>(xs: D[I]) -> Dimensionless[I];"),
        "{stdout}"
    );
    assert!(stdout.contains("= 2"), "{stdout}");

    // --- a sandboxed array call through the buffer protocol.
    let output = graphcal_bin()
        .args(["plugin", "test"])
        .arg(&artifact)
        .args(["--call", "share", "[1.0,3.0]"])
        .output()
        .expect("failed to run graphcal");
    assert!(
        output.status.success(),
        "plugin test --call share failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("[0.25, 0.75]"), "{stdout}");

    // --- vendor into a package-mode project and pin via deps lock.
    let project = dir.path().join("proj");
    std::fs::create_dir_all(project.join("src/e2e")).unwrap();
    std::fs::create_dir_all(project.join("plugins")).unwrap();
    std::fs::write(project.join("graphcal.toml"), "[package]\nname = \"e2e\"\n").unwrap();
    std::fs::copy(&artifact, project.join("plugins/e2e_kernels.wasm")).unwrap();
    std::fs::write(
        project.join("src/e2e/main.gcl"),
        r#"import plugin "plugins/e2e_kernels.wasm" as kernels {
    fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D;
    fn checked_sqrt(x: Dimensionless) -> Dimensionless;
    fn share<D: Dim, I: Index>(xs: D[I]) -> Dimensionless[I];
}

index Leg = { Ascent, Coast, Descent };

param a: Length = 1.0 m;
node mid: Length = kernels.lerp(@a, 3.0 m, 0.5);
node bad: Dimensionless = kernels.checked_sqrt(-1.0);
node fine: Dimensionless = kernels.checked_sqrt(9.0);

node dv: Velocity[Leg] = {
    Leg.Ascent: 3.0 km/s,
    Leg.Coast: 0.5 km/s,
    Leg.Descent: 0.5 km/s,
};
node dv_share: Dimensionless[Leg] = kernels.share(@dv);
node ascent_share: Dimensionless = @dv_share[Leg.Ascent];
"#,
    )
    .unwrap();

    let output = graphcal_bin()
        .args(["deps", "lock", "--root"])
        .arg(&project)
        .output()
        .expect("failed to run graphcal");
    assert!(
        output.status.success(),
        "deps lock failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lock = std::fs::read_to_string(project.join("graphcal.lock")).unwrap();
    assert!(lock.contains("plugins/e2e_kernels.wasm"), "{lock}");
    assert!(lock.contains("sha256"), "{lock}");

    // --- evaluate: the failing node is contained, the rest evaluates.
    let output = graphcal_bin()
        .arg("eval")
        .arg(project.join("src/e2e/main.gcl"))
        .args(["--format", "json"])
        .output()
        .expect("failed to run graphcal");
    // `bad` fails, so eval exits 1 — but with per-node containment.
    assert_eq!(
        output.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("checked_sqrt: negative input -1"),
        "{stdout}"
    );
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let nodes = &json["node"];
    let mid = nodes
        .as_object()
        .and_then(|nodes| {
            nodes
                .iter()
                .find(|(name, _)| name.ends_with("mid"))
                .map(|(_, value)| value)
        })
        .unwrap_or_else(|| panic!("no `mid` node in {stdout}"));
    let si = mid["si_value"].as_f64().expect("mid must evaluate");
    assert!((si - 2.0).abs() < 1e-12, "mid = {si}");
    let fine = nodes
        .as_object()
        .and_then(|nodes| {
            nodes
                .iter()
                .find(|(name, _)| name.ends_with("fine"))
                .map(|(_, value)| value)
        })
        .unwrap_or_else(|| panic!("no `fine` node in {stdout}"));
    let si = fine["si_value"].as_f64().expect("fine must evaluate");
    assert!((si - 3.0).abs() < 1e-12, "fine = {si}");
    let ascent_share = nodes
        .as_object()
        .and_then(|nodes| {
            nodes
                .iter()
                .find(|(name, _)| name.ends_with("ascent_share"))
                .map(|(_, value)| value)
        })
        .unwrap_or_else(|| panic!("no `ascent_share` node in {stdout}"));
    let si = ascent_share["si_value"]
        .as_f64()
        .expect("ascent_share must evaluate");
    assert!((si - 0.75).abs() < 1e-12, "ascent_share = {si}");
}

#[test]
fn plugin_panics_surface_as_failure_messages() {
    if !wasm_target_installed() {
        eprintln!(
            "SKIPPED: plugin_panics_surface_as_failure_messages needs the \
             wasm32-unknown-unknown target (rustup target add wasm32-unknown-unknown)"
        );
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let crate_dir = dir.path().join("panic-probe");
    std::fs::create_dir_all(crate_dir.join("src")).unwrap();
    let sdk_path = repo_root().join("crates").join("graphcal-plugin");
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        format!(
            r#"[package]
name = "panic-probe"
version = "0.1.0"
edition = "2024"
publish = false

[lib]
crate-type = ["cdylib"]

[dependencies]
graphcal-plugin = {{ path = {sdk_path:?} }}

[profile.release]
panic = "abort"
"#
        ),
    )
    .unwrap();
    std::fs::write(
        crate_dir.join("src/lib.rs"),
        r#"graphcal_plugin::plugin! {
    /// Panics on negative input: the SDK's hook must forward the message.
    fn probe(x: Dimensionless) -> Dimensionless {
        assert!(x >= 0.0, "probe: negative input {x}");
        x.sqrt()
    }
}
"#,
    )
    .unwrap();

    let artifact = build_wasm(&crate_dir, "panic_probe.wasm");
    let bytes = std::fs::read(&artifact).unwrap();
    let host = graphcal_plugin_host::PluginHost::new();
    let module = host.load(&bytes).expect("panic-probe must validate");

    let name = graphcal_compiler::syntax::function_name::FnName::expect_valid("probe");
    let ok = module
        .call(&name, &[HostFnValue::Scalar(9.0)])
        .expect("probe(9) succeeds");
    let graphcal_eval::host_fns::HostFnValue::Scalar(ok) = ok else {
        panic!("expected a scalar result, got {ok:?}");
    };
    assert!((ok - 3.0).abs() < 1e-12);

    let err = module
        .call(&name, &[HostFnValue::Scalar(-1.0)])
        .expect_err("probe(-1) fails");
    let graphcal_plugin_host::PluginCallError::Failed { message } = &err else {
        panic!("expected a Failed error with the panic message, got {err:?}");
    };
    assert!(message.contains("probe: negative input -1"), "{message}");
    assert!(message.contains("panicked"), "{message}");
}
