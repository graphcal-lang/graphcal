//! Build-time I/O shell. All generated state is private to Cargo's `OUT_DIR`.
use super::bundle::{self, GLUE, Inputs, MANIFEST, Manifest, Toolchain, WASM};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const TARGET: &str = "wasm32-unknown-unknown";
const PROFILE: &str = "wasm-release";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Toml(#[from] toml::de::Error),
    #[error(transparent)]
    Integrity(#[from] bundle::IntegrityError),
    #[error("{0}")]
    Invalid(String),
    #[error(
        "could not run {command}: {source}\nSee docs/installation.md for source-build prerequisites"
    )]
    Spawn {
        command: String,
        source: std::io::Error,
    },
    #[error("{command} failed:\n{stderr}")]
    Tool { command: String, stderr: String },
}

fn run(command: &mut Command) -> Result<Output, Error> {
    let description = format!("{command:?}");
    let output = command.output().map_err(|source| Error::Spawn {
        command: description.clone(),
        source,
    })?;
    if !output.status.success() {
        return Err(Error::Tool {
            command: description,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(output)
}

fn text(command: &mut Command) -> Result<String, Error> {
    String::from_utf8(run(command)?.stdout).map_err(|error| Error::Invalid(error.to_string()))
}

#[derive(Deserialize)]
struct RustToolchain {
    toolchain: Channel,
}
#[derive(Deserialize)]
struct Channel {
    channel: String,
}
#[derive(Deserialize)]
struct Lockfile {
    package: Vec<LockedPackage>,
}
#[derive(Deserialize)]
struct LockedPackage {
    name: String,
    version: String,
}
#[derive(Deserialize)]
struct CargoManifest {
    package: PackageVersion,
}
#[derive(Deserialize)]
struct PackageVersion {
    version: VersionSource,
}
#[derive(Deserialize)]
#[serde(untagged)]
enum VersionSource {
    Packaged(String),
    Workspace { workspace: bool },
}

fn watch_tool(name: &str) -> Result<(), Error> {
    let file = format!("{name}{}", env::consts::EXE_SUFFIX);
    let executable = env::var_os("PATH")
        .and_then(|paths| {
            env::split_paths(&paths)
                .map(|path| path.join(&file))
                .find(|path| path.is_file())
        })
        .ok_or_else(|| {
            Error::Invalid(format!(
                "missing `{name}` on PATH; see docs/installation.md for source-build prerequisites"
            ))
        })?;
    watch(&executable);
    Ok(())
}

/// Do not pass host coverage/sanitizer flags, wrappers, target directories, or
/// jobserver tokens to a second target build. One worker uses the slot already
/// owned by this build script. The engine always uses the repository toolchain,
/// including when the native CLI is being checked with its older MSRV.
fn rust_command(root: &Path, channel: &str, tool: &str) -> Command {
    let mut command = Command::new("rustup");
    env::vars_os()
        .filter(|(key, _)| isolated_variable(&key.to_string_lossy()))
        .for_each(|(key, _)| {
            command.env_remove(key);
        });
    command
        .args(["run", channel, tool])
        .current_dir(root)
        .env("CARGO_ENCODED_RUSTFLAGS", "")
        .env("RUSTC_WRAPPER", "")
        .env("RUSTC_WORKSPACE_WRAPPER", "");
    command
}

fn cargo_command(root: &Path, channel: &str) -> Result<Command, Error> {
    let compiler = text(Command::new("rustup").args(["which", "rustc", "--toolchain", channel]))?;
    let cargo = text(Command::new("rustup").args(["which", "cargo", "--toolchain", channel]))?;
    watch(Path::new(compiler.trim()));
    watch(Path::new(cargo.trim()));
    let mut command = rust_command(root, channel, "cargo");
    command
        .env("RUSTC", compiler.trim())
        .env("CARGO", cargo.trim());
    Ok(command)
}

pub fn isolated_variable(key: &str) -> bool {
    key.starts_with("CARGO_CFG_")
        || key.starts_with("CARGO_FEATURE_")
        || key.starts_with("CARGO_PKG_")
        || key.starts_with("DEP_")
        || key.starts_with("CARGO_BUILD_")
        || key.starts_with("CARGO_TARGET_")
        || key.starts_with("CARGO_PROFILE_")
        || key.starts_with("RUST") && key != "RUSTUP_HOME"
        || matches!(
            key,
            "CARGO"
                | "CARGO_MANIFEST_DIR"
                | "CARGO_MANIFEST_PATH"
                | "OUT_DIR"
                | "HOST"
                | "TARGET"
                | "NUM_JOBS"
                | "DEBUG"
                | "OPT_LEVEL"
                | "PROFILE"
                | "CARGO_INCREMENTAL"
                | "CARGO_ENCODED_RUSTFLAGS"
                | "CARGO_MAKEFLAGS"
                | "MAKEFLAGS"
                | "MFLAGS"
                | "LLVM_PROFILE_FILE"
        )
}

fn watch(path: &Path) {
    println!("cargo:rerun-if-changed={}", path.display());
}

fn collect(path: &Path, files: &mut BTreeSet<PathBuf>) -> Result<(), Error> {
    // Watching a nonexistent optional file makes Cargo rebuild on every command.
    // The workspace directories below cover additions to the source tree.
    match fs::metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(metadata) if metadata.is_dir() => {
            watch(path);
            fs::read_dir(path)?.try_for_each(|entry| collect(&entry?.path(), files))
        }
        Ok(_) => {
            watch(path);
            files.insert(path.to_path_buf());
            Ok(())
        }
    }
}

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
}
#[derive(Deserialize)]
struct Package {
    name: String,
    manifest_path: PathBuf,
    dependencies: Vec<Dependency>,
}
#[derive(Deserialize)]
struct Dependency {
    path: Option<PathBuf>,
    kind: Option<DependencyKind>,
}
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum DependencyKind {
    Dev,
    Build,
    Normal,
}

fn dependency_inputs(
    package: &Package,
    packages: &[Package],
    roots: &mut BTreeSet<PathBuf>,
) -> Result<(), Error> {
    let root = package
        .manifest_path
        .parent()
        .ok_or_else(|| Error::Invalid("manifest has no parent".into()))?;
    if !roots.insert(root.to_path_buf()) {
        return Ok(());
    }
    package
        .dependencies
        .iter()
        .filter(|dependency| !matches!(dependency.kind, Some(DependencyKind::Dev)))
        .filter_map(|dependency| dependency.path.as_ref())
        .try_for_each(|path| {
            let dependency = packages
                .iter()
                .find(|candidate| candidate.manifest_path == path.join("Cargo.toml"))
                .ok_or_else(|| {
                    Error::Invalid(format!(
                        "local engine dependency {} is not a workspace member",
                        path.display()
                    ))
                })?;
            if dependency.name == "graphcal" {
                return Err(Error::Invalid(
                    "report engine must not depend on the CLI".into(),
                ));
            }
            dependency_inputs(dependency, packages, roots)
        })
}

pub fn source_digest(root: &Path, channel: &str) -> Result<[u8; 32], Error> {
    // Watch additions/removals of workspace members as well as existing inputs.
    // CLI-only edits can rerun this cheap check but do not invalidate the bundle.
    watch(&root.join("crates"));
    if root.join(".cargo").is_dir() {
        watch(&root.join(".cargo"));
    }
    let metadata: Metadata = serde_json::from_slice(
        &run(cargo_command(root, channel)?.args([
            "metadata",
            "--locked",
            "--no-deps",
            "--format-version",
            "1",
        ]))?
        .stdout,
    )?;
    let wasm = metadata
        .packages
        .iter()
        .find(|package| package.name == "graphcal-wasm")
        .ok_or_else(|| Error::Invalid("workspace has no graphcal-wasm package".into()))?;
    let mut roots = BTreeSet::new();
    dependency_inputs(wasm, &metadata.packages, &mut roots)?;
    let mut files = BTreeSet::new();
    [
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
        "crates/graphcal-cli/build.rs",
        "crates/graphcal-cli/build_support",
    ]
    .iter()
    .try_for_each(|path| collect(&root.join(path), &mut files))?;
    roots.iter().try_for_each(|path| {
        ["Cargo.toml", "build.rs", "src", "assets"]
            .iter()
            .try_for_each(|part| collect(&path.join(part), &mut files))
    })?;
    // Cargo reads configuration from the working directory's ancestors and home.
    root.ancestors().try_for_each(|path| {
        [".cargo/config", ".cargo/config.toml"]
            .iter()
            .try_for_each(|part| collect(&path.join(part), &mut files))
    })?;
    let cargo_home = env::var_os("CARGO_HOME").map(PathBuf::from).or_else(|| {
        let home = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        env::var_os(home).map(|home| PathBuf::from(home).join(".cargo"))
    });
    if let Some(home) = cargo_home {
        ["config", "config.toml"]
            .iter()
            .try_for_each(|part| collect(&home.join(part), &mut files))?;
    }
    let mut hash = Sha256::new();
    files.iter().try_for_each(|path| {
        let name = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let content = fs::read(path)?;
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name.as_bytes());
        hash.update((content.len() as u64).to_le_bytes());
        hash.update(content);
        Ok::<_, Error>(())
    })?;
    Ok(hash.finalize().into())
}

fn load(dir: &Path, version: &str) -> Result<Manifest, Error> {
    let manifest: Manifest = serde_json::from_slice(&fs::read(dir.join(MANIFEST))?)?;
    manifest.verify(
        version,
        &fs::read(dir.join(GLUE))?,
        &fs::read(dir.join(WASM))?,
    )?;
    Ok(manifest)
}

pub fn ensure(manifest_dir: &Path, out: &Path, version: &str) -> Result<(), Error> {
    let root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| Error::Invalid("could not locate workspace root".into()))?;
    let bundle = out.join("report-engine");
    let cargo_manifest: CargoManifest =
        toml::from_str(&fs::read_to_string(manifest_dir.join("Cargo.toml"))?)?;
    let packaged_version = match cargo_manifest.package.version {
        VersionSource::Packaged(version) => Some(version),
        VersionSource::Workspace { workspace: true } => None,
        VersionSource::Workspace { workspace: false } => {
            return Err(Error::Invalid(
                "package version must inherit the workspace version".into(),
            ));
        }
    };
    if let Some(packaged_version) = packaged_version {
        if packaged_version != version {
            return Err(bundle::IntegrityError::Version.into());
        }
        let packaged = manifest_dir.join("assets/report-engine");
        for name in [GLUE, WASM, MANIFEST] {
            watch(&packaged.join(name));
        }
        load(&packaged, version).map_err(|error| Error::Invalid(format!(
            "missing or invalid packaged report engine: {error}; release maintainers must run `just wasm-report-package` before packaging")))?;
        fs::create_dir_all(&bundle)?;
        [GLUE, WASM, MANIFEST]
            .iter()
            .try_for_each(|name| fs::copy(packaged.join(name), bundle.join(name)).map(|_| ()))?;
        return Ok(());
    }
    ensure_source(root, out, version)
}

fn ensure_source(root: &Path, out: &Path, version: &str) -> Result<(), Error> {
    for name in ["PATH", "CARGO_HOME", "RUSTUP_HOME", "CARGO_NET_OFFLINE"] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    ["rustup", "wasm-bindgen", "wasm-opt"]
        .iter()
        .try_for_each(|name| watch_tool(name))?;
    let toolchain: RustToolchain =
        toml::from_str(&fs::read_to_string(root.join("rust-toolchain.toml"))?)?;
    let channel = &toolchain.toolchain.channel;
    let lock: Lockfile = toml::from_str(&fs::read_to_string(root.join("Cargo.lock"))?)?;
    let bindgen_version = lock
        .package
        .iter()
        .find(|package| package.name == "wasm-bindgen")
        .ok_or_else(|| Error::Invalid("Cargo.lock has no wasm-bindgen".into()))?;
    let bindgen = text(Command::new("wasm-bindgen").arg("--version"))?;
    if bindgen.trim() != format!("wasm-bindgen {}", bindgen_version.version) {
        return Err(Error::Invalid(format!(
            "wasm-bindgen CLI must match Cargo.lock; run `cargo install wasm-bindgen-cli --version {} --locked`",
            bindgen_version.version
        )));
    }
    let inputs = Inputs {
        source: source_digest(root, channel)?,
        tools: Toolchain {
            rustc: text(rust_command(root, channel, "rustc").args(["--version", "--verbose"]))?,
            cargo: text(rust_command(root, channel, "cargo").arg("--version"))?,
            bindgen,
            optimizer: text(Command::new("wasm-opt").arg("--version"))?,
        },
        version: version.into(),
    };
    cached_bundle(out, inputs.clone(), |staging| {
        // No rustup --install or wasm-pack auto-downloads in a build script.
        let libdir = text(rust_command(root, channel, "rustc").args([
            "--print",
            "target-libdir",
            "--target",
            TARGET,
        ]))?;
        if !Path::new(libdir.trim()).is_dir() {
            return Err(Error::Invalid(format!(
                "missing Wasm standard library; run `rustup target add {TARGET} --toolchain {channel}`"
            )));
        }
        let target_dir = out.join("wasm-target");
        run(cargo_command(root, channel)?
            .args([
                "build",
                "--locked",
                "--package",
                "graphcal-wasm",
                "--lib",
                "--target",
                TARGET,
                "--profile",
                PROFILE,
                "--jobs",
                "1",
                "--target-dir",
            ])
            .arg(&target_dir)
            .env("CARGO_BUILD_BUILD_DIR", out.join("wasm-build")))?;
        run(Command::new("wasm-bindgen")
            .arg(
                target_dir
                    .join(TARGET)
                    .join(PROFILE)
                    .join("graphcal_wasm.wasm"),
            )
            .args(["--target", "no-modules", "--no-typescript", "--out-dir"])
            .arg(staging))?;
        let optimized = staging.join("optimized.wasm");
        run(Command::new("wasm-opt")
            .env("BINARYEN_CORES", "1")
            .arg(staging.join(WASM))
            .args(["-Oz", "-o"])
            .arg(&optimized))?;
        fs::rename(optimized, staging.join(WASM))?;
        // Do not certify a bundle if the source changed during the nested build.
        if source_digest(root, channel)? != inputs.source {
            return Err(Error::Invalid(
                "report-engine inputs changed during generation; rerun cargo build".into(),
            ));
        }
        Ok(())
    })
}

pub fn cached_bundle(
    out: &Path,
    inputs: Inputs,
    generate: impl FnOnce(&Path) -> Result<(), Error>,
) -> Result<(), Error> {
    let bundle = out.join("report-engine");
    for name in [GLUE, WASM, MANIFEST] {
        watch(&bundle.join(name));
    }
    if load(&bundle, &inputs.version)
        .and_then(|manifest| manifest.verify_inputs(&inputs).map_err(Into::into))
        .is_ok()
    {
        return Ok(());
    }
    let staging = tempfile::tempdir_in(out)?;
    generate(staging.path())?;
    let manifest = Manifest::new(
        inputs,
        &fs::read(staging.path().join(GLUE))?,
        &fs::read(staging.path().join(WASM))?,
    );
    fs::write(
        staging.path().join(MANIFEST),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    // Cargo serializes access to this OUT_DIR. Each concurrent target/profile
    // has its own OUT_DIR. An interrupted publication is a cache miss next time.
    if bundle.exists() {
        fs::remove_dir_all(&bundle)?;
    }
    fs::rename(staging.path(), &bundle)?;
    Ok(())
}
