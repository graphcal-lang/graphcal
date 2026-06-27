use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use gix::bstr::ByteSlice;
use serde::Deserialize;

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

fn main() {
    println!("cargo:rerun-if-env-changed=GRAPHCAL_GIT_HASH");
    println!("cargo:rerun-if-changed=.cargo_vcs_info.json");

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
}
