//! Real filesystem implementation backed by `std::fs`.

use std::io;
use std::path::{Path, PathBuf};

use crate::FileSystemReader;

/// Filesystem reader that delegates to the operating system via `std::fs`.
///
/// # Sandboxing
///
/// When constructed with [`RealFileSystem::rooted`], every read / canonicalize
/// / existence check first canonicalizes the incoming path and rejects it
/// (as `NotFound`) if the canonical form is not inside the canonical root.
/// Because canonicalization resolves symlinks, this also rejects symlink
/// escapes — an in-root symlink pointing at `/etc/passwd` will canonicalize
/// to `/etc/passwd`, fail the `starts_with(root)` check, and be rejected.
///
/// The default (unit-like) constructor keeps the unrestricted `std::fs`
/// behavior so one-shot CLI evals of loose files outside any project keep
/// working.
#[derive(Debug, Clone, Default)]
pub struct RealFileSystem {
    /// The canonical project root, if any. When `None`, all paths are allowed
    /// (unrestricted mode, backward compatible with single-file CLI usage).
    root: Option<PathBuf>,
}

impl RealFileSystem {
    /// Construct a sandboxed filesystem reader pinned to `project_root`.
    ///
    /// `project_root` is stored verbatim; callers are expected to pass a
    /// canonicalized path (e.g. from `loader::resolve_project_root`). Reads
    /// for paths outside the root — including symlink escapes — return
    /// `io::ErrorKind::NotFound`.
    #[must_use]
    pub const fn rooted(project_root: PathBuf) -> Self {
        Self {
            root: Some(project_root),
        }
    }

    /// Return `Ok(canonical_path)` if `path` is allowed under the current
    /// sandbox, or a `NotFound` error otherwise.
    ///
    /// In unrestricted mode (no root), this simply delegates to
    /// `std::fs::canonicalize`.
    fn check_access(&self, path: &Path) -> Result<PathBuf, io::Error> {
        let canonical = path.canonicalize()?;
        match &self.root {
            None => Ok(canonical),
            Some(root) => {
                if canonical.starts_with(root) {
                    Ok(canonical)
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!(
                            "path {} is outside the project root {}",
                            canonical.display(),
                            root.display()
                        ),
                    ))
                }
            }
        }
    }
}

impl FileSystemReader for RealFileSystem {
    fn read_to_string(&self, path: &Path) -> Result<String, io::Error> {
        if self.root.is_some() {
            let canonical = self.check_access(path)?;
            std::fs::read_to_string(canonical)
        } else {
            std::fs::read_to_string(path)
        }
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, io::Error> {
        if self.root.is_some() {
            self.check_access(path)
        } else {
            path.canonicalize()
        }
    }

    fn is_file(&self, path: &Path) -> bool {
        if self.root.is_some() {
            self.check_access(path).is_ok_and(|p| p.is_file())
        } else {
            path.is_file()
        }
    }

    fn exists(&self, path: &Path) -> bool {
        if self.root.is_some() {
            self.check_access(path).is_ok()
        } else {
            path.exists()
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "test code"
    )]

    use super::*;
    use std::fs;

    #[test]
    fn unrooted_reads_any_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.gcl");
        fs::write(&file, "param x: Dimensionless = 1.0;").unwrap();

        let fs_reader = RealFileSystem::default();
        let content = fs_reader.read_to_string(&file).unwrap();
        assert!(content.contains("param x"));
    }

    #[test]
    fn rooted_allows_paths_inside_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let file = root.join("a.gcl");
        fs::write(&file, "hello").unwrap();

        let fs_reader = RealFileSystem::rooted(root);
        assert_eq!(fs_reader.read_to_string(&file).unwrap(), "hello");
        assert!(fs_reader.is_file(&file));
        assert!(fs_reader.exists(&file));
    }

    #[test]
    fn rooted_rejects_paths_outside_root() {
        let parent = tempfile::tempdir().unwrap();
        let project = parent.path().join("project");
        let external = parent.path().join("external");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&external).unwrap();

        let secret = external.join("secret.gcl");
        fs::write(&secret, "secret content").unwrap();

        let fs_reader = RealFileSystem::rooted(project.canonicalize().unwrap());
        // Outside the root — must be NotFound, not a successful read.
        let err = fs_reader.read_to_string(&secret).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(!fs_reader.is_file(&secret));
        assert!(!fs_reader.exists(&secret));
    }

    #[cfg(unix)]
    #[test]
    fn rooted_rejects_symlink_escapes() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let project = parent.path().join("project");
        let external = parent.path().join("external");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&external).unwrap();

        let target = external.join("escaped.gcl");
        fs::write(&target, "escaped").unwrap();

        let link = project.join("link.gcl");
        symlink(&target, &link).unwrap();

        let fs_reader = RealFileSystem::rooted(project.canonicalize().unwrap());
        // The symlink points outside the root; reading it must fail.
        let err = fs_reader.read_to_string(&link).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(!fs_reader.exists(&link));
    }

    #[test]
    fn unrooted_path_outside_any_root_still_works() {
        // Default (non-rooted) behavior: anything that exists is readable.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("loose.gcl");
        fs::write(&file, "loose").unwrap();

        let fs_reader = RealFileSystem::default();
        assert!(fs_reader.exists(&file));
        assert!(fs_reader.is_file(&file));
        assert_eq!(fs_reader.read_to_string(&file).unwrap(), "loose");
    }
}
