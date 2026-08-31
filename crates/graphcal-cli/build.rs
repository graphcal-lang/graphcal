use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use gix::bstr::ByteSlice;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const UPDATE_REPORT_ENGINE_ENV: &str = "GRAPHCAL_UPDATE_REPORT_ENGINE";
const HEX: &[u8; 16] = b"0123456789abcdef";
const REPORT_ENGINE_SOURCE_PATHS: &[&str] = &[
    "Cargo.lock",
    "Cargo.toml",
    "rust-toolchain.toml",
    "crates/graphcal-compiler/Cargo.toml",
    "crates/graphcal-compiler/src",
    "crates/graphcal-eval/Cargo.toml",
    "crates/graphcal-eval/src",
    "crates/graphcal-io/Cargo.toml",
    "crates/graphcal-io/src",
    "crates/graphcal-package/Cargo.toml",
    "crates/graphcal-package/src",
    "crates/graphcal-report/Cargo.toml",
    "crates/graphcal-report/src",
    "crates/graphcal-wasm/Cargo.toml",
    "crates/graphcal-wasm/src",
];

#[derive(Deserialize)]
struct CargoVcsInfo {
    git: Option<CargoVcsGitInfo>,
}

#[derive(Deserialize)]
struct CargoVcsGitInfo {
    sha1: String,
}

fn non_empty_trimmed(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn short_commit_sha(value: &str) -> String {
    value.chars().take(7).collect()
}

fn env_git_hash() -> Option<String> {
    env::var("GRAPHCAL_GIT_HASH")
        .ok()
        .and_then(|value| non_empty_trimmed(&value))
}

fn manifest_dir() -> Option<PathBuf> {
    env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from)
}

fn cargo_vcs_hash() -> Option<String> {
    let path = manifest_dir()?.join(".cargo_vcs_info.json");
    let contents = fs::read_to_string(path).ok()?;
    let info: CargoVcsInfo = serde_json::from_str(&contents).ok()?;
    non_empty_trimmed(&info.git?.sha1)
}

fn discover_git_repo() -> Option<gix::Repository> {
    gix::discover(manifest_dir()?).ok()
}

fn git_head_hash(repo: &gix::Repository) -> Option<String> {
    Some(repo.head_id().ok()?.detach().to_hex().to_string())
}

fn rerun_if_changed(path: &Path) {
    println!("cargo:rerun-if-changed={}", path.display());
}

fn emit_git_rerun_paths(repo: &gix::Repository) {
    rerun_if_changed(&repo.git_dir().join("HEAD"));
    rerun_if_changed(&repo.common_dir().join("packed-refs"));

    if let Ok(Some(head_name)) = repo.head_name() {
        rerun_if_changed(&repo.common_dir().join(head_name.as_bstr().to_path_lossy()));
    }
}

fn collect_source_files(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    rerun_if_changed(path);
    if path.is_file() {
        files.push(path.to_path_buf());
        return Ok(());
    }
    if !path.is_dir() {
        return Err(format!(
            "report engine input {} does not exist",
            path.display()
        ));
    }

    let mut entries = fs::read_dir(path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("could not enumerate {}: {error}", path.display()))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    entries
        .into_iter()
        .try_for_each(|entry| collect_source_files(&entry.path(), files))
}

fn report_engine_source_digest(workspace_root: &Path) -> Result<String, String> {
    let mut files = Vec::new();
    REPORT_ENGINE_SOURCE_PATHS.iter().try_for_each(|relative| {
        collect_source_files(&workspace_root.join(relative), &mut files)
    })?;
    files.sort();

    let mut hasher = Sha256::new();
    files.into_iter().try_for_each(|path| {
        let relative = path.strip_prefix(workspace_root).map_err(|error| {
            format!(
                "could not make report engine input {} relative: {error}",
                path.display()
            )
        })?;
        let portable_path = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let content = fs::read(&path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        let path_len = u64::try_from(portable_path.len())
            .map_err(|error| format!("report engine input path is too long: {error}"))?;
        let content_len = u64::try_from(content.len())
            .map_err(|error| format!("report engine input is too large: {error}"))?;
        hasher.update(path_len.to_le_bytes());
        hasher.update(portable_path.as_bytes());
        hasher.update(content_len.to_le_bytes());
        hasher.update(content);
        Ok::<(), String>(())
    })?;

    let bytes = hasher.finalize();
    Ok(bytes.iter().fold(
        String::with_capacity(bytes.len() * 2),
        |mut output, byte| {
            output.push(char::from(HEX[usize::from(byte >> 4)]));
            output.push(char::from(HEX[usize::from(byte & 0x0f)]));
            output
        },
    ))
}

fn verify_report_engine_fingerprint() -> Result<(), String> {
    let manifest = manifest_dir().ok_or_else(|| "CARGO_MANIFEST_DIR is not set".to_string())?;
    let Some(workspace_root) = manifest.parent().and_then(Path::parent) else {
        return Err(format!(
            "could not locate the workspace above {}",
            manifest.display()
        ));
    };

    // Published crate sources do not contain sibling workspace crates. Their
    // engine was verified before packaging, so only source checkouts can and
    // need to recompute the fingerprint.
    if !workspace_root.join("crates/graphcal-wasm/src").is_dir() {
        return Ok(());
    }

    let fingerprint_path = manifest.join("assets/report-engine/source-tree.digest");
    rerun_if_changed(&fingerprint_path);
    let actual = report_engine_source_digest(workspace_root)?;
    match env::var_os(UPDATE_REPORT_ENGINE_ENV) {
        Some(value) if value == "1" => fs::write(&fingerprint_path, format!("{actual}\n"))
            .map_err(|error| format!("could not update {}: {error}", fingerprint_path.display())),
        Some(value) => Err(format!(
            "{UPDATE_REPORT_ENGINE_ENV} must be `1`, got `{}`",
            value.to_string_lossy()
        )),
        None => {
            let expected = fs::read_to_string(&fingerprint_path).map_err(|error| {
                format!(
                    "could not read report engine fingerprint {}: {error}\n\
                     run `just wasm-report-update` to regenerate the embedded engine",
                    fingerprint_path.display()
                )
            })?;
            if expected.trim() == actual {
                Ok(())
            } else {
                Err(
                    "the embedded browser engine is stale for the current Graphcal sources\n\
                     run `just wasm-report-update` to rebuild it"
                        .to_string(),
                )
            }
        }
    }
}

fn main() -> Result<(), String> {
    println!("cargo:rerun-if-env-changed=GRAPHCAL_GIT_HASH");
    println!("cargo:rerun-if-env-changed={UPDATE_REPORT_ENGINE_ENV}");
    println!("cargo:rerun-if-changed=.cargo_vcs_info.json");

    verify_report_engine_fingerprint()?;

    let packaged_hash = cargo_vcs_hash();
    let has_packaged_hash = packaged_hash.is_some();
    let git_repo = (!has_packaged_hash).then(discover_git_repo).flatten();
    let git_hash = env_git_hash()
        .or(packaged_hash)
        .or_else(|| git_repo.as_ref().and_then(git_head_hash))
        .map(|hash| short_commit_sha(&hash));
    let git_hash_value = git_hash.as_deref().unwrap_or("");
    println!("cargo:rustc-env=GIT_HASH={git_hash_value}");

    if let Some(repo) = git_repo {
        emit_git_rerun_paths(&repo);
    }

    Ok(())
}
