use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

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

fn git_output(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|stdout| non_empty_trimmed(&stdout))
}

fn cargo_vcs_hash() -> Option<String> {
    let path = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR")?).join(".cargo_vcs_info.json");
    let contents = fs::read_to_string(path).ok()?;
    let info: CargoVcsInfo = serde_json::from_str(&contents).ok()?;
    non_empty_trimmed(&info.git?.sha1)
}

fn main() {
    println!("cargo:rerun-if-env-changed=GRAPHCAL_GIT_HASH");
    println!("cargo:rerun-if-changed=.cargo_vcs_info.json");

    let packaged_hash = cargo_vcs_hash();
    let has_packaged_hash = packaged_hash.is_some();
    let git_hash = env_git_hash()
        .or(packaged_hash)
        .or_else(|| git_output(&["rev-parse", "--short=7", "HEAD"]))
        .map(|hash| short_commit_sha(&hash));
    let git_hash_value = git_hash.as_deref().unwrap_or("");
    println!("cargo:rustc-env=GIT_HASH={git_hash_value}");

    if !has_packaged_hash {
        // Rebuild when HEAD moves to another ref or when the current branch advances.
        if let Some(git_head_path) = git_output(&["rev-parse", "--git-path", "HEAD"]) {
            println!("cargo:rerun-if-changed={git_head_path}");
        }
        if let Some(head_ref) = git_output(&["symbolic-ref", "-q", "HEAD"])
            && let Some(head_ref_path) = git_output(&["rev-parse", "--git-path", &head_ref])
        {
            println!("cargo:rerun-if-changed={head_ref_path}");
        }
    }
}
