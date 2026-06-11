//! Functional core for `.gcl` discovery and format checking.
//!
//! These helpers are pure (or deterministic over the filesystem) and produce
//! no stdout/stderr side effects: the CLI shell renders user-facing output and
//! performs writes around them. Keeping them here lets tests exercise the same
//! discovery and decision logic the binary uses without spawning a process.

use std::path::{Path, PathBuf};

/// Directories skipped during recursive `.gcl` collection.
const SKIP_DIRS: &[&str] = &[".git", "target", "node_modules", ".build", "__pycache__"];

/// Outcome of comparing a file's contents to its canonical formatting.
///
/// Modeled as a closed enum rather than `Result<Option<String>>` so each
/// outcome the shell must branch on is named.
pub enum FormatStatus {
    /// Already canonically formatted; no change needed.
    Unchanged,
    /// Differs from canonical form; carries the formatted text.
    Changed(String),
    /// Could not be parsed/formatted (usually a syntax error).
    Error(graphcal_fmt::FormatError),
}

/// Classify `source` against its canonical formatting. Pure in `source`.
#[must_use]
pub fn format_status(source: &str) -> FormatStatus {
    match graphcal_fmt::format_source(source) {
        Ok(formatted) if formatted == source => FormatStatus::Unchanged,
        Ok(formatted) => FormatStatus::Changed(formatted),
        Err(e) => FormatStatus::Error(e),
    }
}

/// Recursively collect all `.gcl` files under `dir`, sorted for deterministic
/// output.
///
/// Uses `walkdir` for safe traversal: only regular files are collected,
/// symlinks are not followed, and common generated directories (`.git`,
/// `target`, `node_modules`, etc.) are skipped. Traversal errors are returned
/// to the caller rather than logged, so the imperative shell decides how to
/// surface them.
#[must_use]
pub fn collect_gcl_files(dir: &Path) -> (Vec<PathBuf>, Vec<walkdir::Error>) {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut warnings: Vec<walkdir::Error> = Vec::new();
    for entry in walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            // Skip well-known generated/vendored directories.
            // Non-UTF-8 directory names are not skipped — they won't match
            // any SKIP_DIRS entry anyway, so we intentionally let them
            // through rather than treating them as if they were the empty
            // string.
            if e.file_type().is_dir() {
                return !e
                    .file_name()
                    .to_str()
                    .is_some_and(|name| SKIP_DIRS.contains(&name));
            }
            true
        })
    {
        match entry {
            Ok(e)
                if e.file_type().is_file()
                    && e.path().extension().is_some_and(|ext| ext == "gcl") =>
            {
                files.push(e.into_path());
            }
            Ok(_) => {}
            Err(err) => warnings.push(err),
        }
    }
    files.sort();
    (files, warnings)
}
