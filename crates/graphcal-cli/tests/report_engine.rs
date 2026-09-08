//! Exercise the production build-support code without compiling Wasm per test.
#![allow(clippy::unwrap_used, reason = "test setup and assertions")]
#[path = "../build_support/bundle.rs"]
mod bundle;
#[expect(
    clippy::print_stdout,
    reason = "the imported build script emits Cargo protocol directives"
)]
#[path = "../build_support/engine.rs"]
mod engine;

use bundle::{GLUE, Inputs, MANIFEST, Manifest, Toolchain, WASM};
use std::{fs, path::Path};

fn inputs() -> Inputs {
    Inputs {
        source: bundle::digest(b"source"),
        tools: Toolchain {
            rustc: "rustc".into(),
            cargo: "cargo".into(),
            bindgen: "bindgen".into(),
            optimizer: "optimizer".into(),
        },
        version: "1.0.0".into(),
    }
}

fn generate(dir: &Path) -> Result<(), engine::Error> {
    fs::write(dir.join(GLUE), b"glue")?;
    fs::write(dir.join(WASM), b"wasm")?;
    Ok(())
}

#[test]
fn identity_includes_source_version_and_every_tool() {
    let original = inputs();
    let manifest = Manifest::new(original.clone(), b"glue", b"wasm");
    manifest.verify_inputs(&original).unwrap();
    for changed in [
        Inputs {
            source: bundle::digest(b"changed"),
            ..original.clone()
        },
        Inputs {
            version: "2.0.0".into(),
            ..original.clone()
        },
        Inputs {
            tools: Toolchain {
                rustc: "different".into(),
                ..original.tools.clone()
            },
            ..original.clone()
        },
        Inputs {
            tools: Toolchain {
                cargo: "different".into(),
                ..original.tools.clone()
            },
            ..original.clone()
        },
        Inputs {
            tools: Toolchain {
                bindgen: "different".into(),
                ..original.tools.clone()
            },
            ..original.clone()
        },
        Inputs {
            tools: Toolchain {
                optimizer: "different".into(),
                ..original.tools
            },
            ..original
        },
    ] {
        assert!(manifest.verify_inputs(&changed).is_err());
    }
    assert!(manifest.verify("1.0.0", b"bad glue", b"wasm").is_err());
    assert!(manifest.verify("1.0.0", b"glue", b"bad wasm").is_err());
    assert!(manifest.verify("2.0.0", b"glue", b"wasm").is_err());
    let serialized = serde_json::to_vec(&manifest).unwrap();
    let restored: Manifest = serde_json::from_slice(&serialized).unwrap();
    restored.verify("1.0.0", b"glue", b"wasm").unwrap();
}

#[test]
fn cache_reuses_valid_outputs_and_repairs_corruption() {
    let out = tempfile::tempdir().unwrap();
    engine::cached_bundle(out.path(), inputs(), generate).unwrap();
    engine::cached_bundle(out.path(), inputs(), |_| {
        Err(engine::Error::Invalid("must not regenerate".into()))
    })
    .unwrap();
    for name in [GLUE, WASM, MANIFEST] {
        fs::write(out.path().join("report-engine").join(name), b"corrupt").unwrap();
        let mut regenerated = false;
        engine::cached_bundle(out.path(), inputs(), |dir| {
            regenerated = true;
            generate(dir)
        })
        .unwrap();
        assert!(regenerated);
    }
    fs::remove_file(out.path().join("report-engine").join(WASM)).unwrap();
    engine::cached_bundle(out.path(), inputs(), generate).unwrap();
}

#[test]
fn failed_or_incomplete_generation_cannot_certify_new_inputs() {
    let out = tempfile::tempdir().unwrap();
    engine::cached_bundle(out.path(), inputs(), generate).unwrap();
    let path = out.path().join("report-engine").join(MANIFEST);
    let before = fs::read(&path).unwrap();
    let changed = Inputs {
        source: bundle::digest(b"new source"),
        ..inputs()
    };
    assert!(
        engine::cached_bundle(out.path(), changed.clone(), |dir| {
            fs::write(dir.join(GLUE), b"partial")?;
            Err(engine::Error::Invalid("tool failed".into()))
        })
        .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(engine::cached_bundle(out.path(), changed.clone(), |_| Ok(())).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    engine::cached_bundle(out.path(), changed.clone(), generate).unwrap();
    let manifest: Manifest = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    manifest.verify_inputs(&changed).unwrap();
}

#[test]
fn packaged_build_verifies_without_sources_or_tools() {
    let temp = tempfile::tempdir().unwrap();
    let manifest_dir = temp.path().join("registry/graphcal");
    let assets = manifest_dir.join("assets/report-engine");
    let out = temp.path().join("out");
    fs::create_dir_all(&assets).unwrap();
    fs::create_dir_all(&out).unwrap();
    fs::write(
        manifest_dir.join("Cargo.toml"),
        "[package]\nversion = '1.0.0'\n",
    )
    .unwrap();
    // Missing packaged assets are an error, not an instruction to download.
    assert!(engine::ensure(&manifest_dir, &out, "1.0.0").is_err());
    generate(&assets).unwrap();
    fs::write(
        assets.join(MANIFEST),
        serde_json::to_vec(&Manifest::new(inputs(), b"glue", b"wasm")).unwrap(),
    )
    .unwrap();
    engine::ensure(&manifest_dir, &out, "1.0.0").unwrap();
    assert_eq!(
        fs::read(out.join("report-engine").join(WASM)).unwrap(),
        b"wasm"
    );
    assert!(engine::ensure(&manifest_dir, &out, "2.0.0").is_err());
    fs::write(assets.join(GLUE), b"tampered").unwrap();
    assert!(engine::ensure(&manifest_dir, &out, "1.0.0").is_err());

    generate(&assets).unwrap();
    fs::write(
        manifest_dir.join("Cargo.toml"),
        "[package]\nversion.workspace = true\n",
    )
    .unwrap();
    // A broken source checkout must not silently trust staged release assets.
    assert!(engine::ensure(&manifest_dir, &out, "1.0.0").is_err());
}

#[test]
fn host_instrumentation_and_build_directories_are_isolated() {
    for key in [
        "RUSTC",
        "RUSTUP_TOOLCHAIN",
        "RUSTFLAGS",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_BUILD_DIR",
        "CARGO_PROFILE_RELEASE_LTO",
        "CARGO_MAKEFLAGS",
        "MAKEFLAGS",
        "LLVM_PROFILE_FILE",
        "CARGO_CFG_TARGET_ARCH",
        "CARGO_FEATURE_NATIVE_ONLY",
        "CARGO_PKG_VERSION",
        "CARGO_MANIFEST_DIR",
        "OUT_DIR",
        "DEP_NATIVE_LIB_PATH",
    ] {
        assert!(engine::isolated_variable(key), "{key}");
    }
    for key in [
        "PATH",
        "CARGO_HOME",
        "RUSTUP_HOME",
        "CARGO_NET_OFFLINE",
        "CARGO_REGISTRIES_CRATES_IO_PROTOCOL",
    ] {
        assert!(!engine::isolated_variable(key), "{key}");
    }
}

#[test]
fn source_fingerprint_tracks_dependency_assets_and_not_cli_only_edits() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = ['crates/*']\nresolver = '3'\n",
    )
    .unwrap();
    fs::write(root.join("Cargo.lock"), "version = 4\n").unwrap();
    fs::write(
        root.join("rust-toolchain.toml"),
        include_str!("../../../rust-toolchain.toml"),
    )
    .unwrap();
    for name in ["graphcal-wasm", "dependency", "graphcal-cli"] {
        let dir = root.join("crates").join(name);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/lib.rs"), "").unwrap();
        let deps = if name == "graphcal-wasm" {
            "[dependencies]\ndependency = { path = '../dependency' }\n[dev-dependencies]\ngraphcal-cli = { path = '../graphcal-cli' }\n"
        } else {
            ""
        };
        fs::write(
            dir.join("Cargo.toml"),
            format!("[package]\nname = '{name}'\nversion = '1.0.0'\nedition = '2024'\n{deps}"),
        )
        .unwrap();
    }
    let toolchain: toml::Value =
        toml::from_str(include_str!("../../../rust-toolchain.toml")).unwrap();
    let channel = toolchain["toolchain"]["channel"].as_str().unwrap();
    let original = engine::source_digest(root, channel).unwrap();
    fs::write(
        root.join("crates/graphcal-cli/src/lib.rs"),
        "// native only",
    )
    .unwrap();
    assert_eq!(engine::source_digest(root, channel).unwrap(), original);
    let assets = root.join("crates/dependency/assets");
    fs::create_dir_all(&assets).unwrap();
    fs::write(assets.join("runtime.js"), "// dependency asset").unwrap();
    let with_asset = engine::source_digest(root, channel).unwrap();
    assert_ne!(with_asset, original);
    fs::remove_file(assets.join("runtime.js")).unwrap();
    assert_eq!(engine::source_digest(root, channel).unwrap(), original);
    fs::write(
        root.join("crates/dependency/src/lib.rs"),
        "// changed dependency",
    )
    .unwrap();
    assert_ne!(engine::source_digest(root, channel).unwrap(), original);
}
