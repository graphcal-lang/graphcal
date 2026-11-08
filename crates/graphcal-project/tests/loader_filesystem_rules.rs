//! Regression tests for the loader's filesystem rules (lane L14, #2255):
//! unreadable sources, sandbox escapes, and module file names.
#![cfg(test)]

use std::path::{Path, PathBuf};

use graphcal_io::RealFileSystem;
use graphcal_project::compile_error::CompileError;
use graphcal_project::load_error::LoadError;
use graphcal_project::loader::load_project;

/// Write `files` (relative path, contents) below a fresh temporary directory.
fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    for (path, contents) in files {
        let path = directory.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
    directory
}

fn code(error: &CompileError) -> Option<String> {
    miette::Diagnostic::code(error).map(|code| code.to_string())
}

/// The readers the CLI and LSP use (rooted at the project) and the ambient one.
fn readers(project_root: &Path) -> [RealFileSystem; 2] {
    [
        RealFileSystem::rooted(project_root).unwrap(),
        RealFileSystem::default(),
    ]
}

/// An existing entry that cannot be read is a source-read failure, not a
/// manifest error (M015 "failed to parse graphcal.toml").
#[test]
fn unreadable_entry_source_is_a_source_read_failure() {
    let directory = tree(&[("graphcal.toml", "[package]\nname = \"pkg\"\n")]);
    let entry: PathBuf = directory.path().join("src/pkg/main.gcl");
    // A directory spelled like a source file exists but is not a regular file.
    std::fs::create_dir_all(&entry).unwrap();

    for reader in readers(directory.path()) {
        let error = load_project(&entry, None, &reader).expect_err("entry is unreadable");
        assert!(
            matches!(
                &error,
                CompileError::Load(LoadError::SourceReadFailed { path, .. })
                    if path.ends_with("src/pkg/main.gcl")
            ),
            "{error:?}"
        );
        assert_eq!(code(&error).as_deref(), Some("graphcal::M039"));
    }
}
