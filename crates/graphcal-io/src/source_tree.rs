//! Deterministic, symlink-safe package source-tree hashing.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::{
    ByteLimit, CancellationSignal, EntryLimit, FileSystemEntryKind, FileSystemReadError,
    FileSystemReader,
};

/// Resource limits for one source-tree hash traversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceTreeHashLimits {
    file_bytes: ByteLimit,
    total_bytes: u64,
    entries: u64,
}

impl SourceTreeHashLimits {
    /// Construct explicit per-file, aggregate-byte, and traversal-entry limits.
    #[must_use]
    pub const fn new(max_file_bytes: ByteLimit, max_total_bytes: u64, max_entries: u64) -> Self {
        Self {
            file_bytes: max_file_bytes,
            total_bytes: max_total_bytes,
            entries: max_entries,
        }
    }

    /// Limits suitable for a trusted, interactive lock-generation operation.
    /// Individual reads remain bounded by the platform's address space.
    #[must_use]
    pub const fn unbounded() -> Self {
        Self::new(ByteLimit::new(u64::MAX), u64::MAX, u64::MAX)
    }
}

/// Successful deterministic source-tree digest plus consumed resources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceTreeHash {
    sha256: String,
    files: u64,
    bytes: u64,
    entries: u64,
}

impl SourceTreeHash {
    /// Lowercase hexadecimal SHA-256 digest.
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// Number of regular files hashed.
    #[must_use]
    pub const fn files(&self) -> u64 {
        self.files
    }

    /// Aggregate file bytes hashed.
    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Filesystem entries inspected, including directories.
    #[must_use]
    pub const fn entries(&self) -> u64 {
        self.entries
    }
}

/// Failure to enumerate or hash a package source tree.
#[derive(Debug, thiserror::Error)]
pub enum SourceTreeHashError {
    /// Root or entry canonicalization failed.
    #[error("could not canonicalize `{}`: {source}", path.display())]
    Canonicalize {
        /// Path being canonicalized.
        path: PathBuf,
        /// Underlying filesystem error.
        source: io::Error,
    },
    /// Entry metadata could not be inspected.
    #[error("could not inspect `{}`: {source}", path.display())]
    Inspect {
        /// Entry being inspected.
        path: PathBuf,
        /// Underlying filesystem error.
        source: io::Error,
    },
    /// A bounded directory listing failed.
    #[error("could not read directory `{}`: {source}", path.display())]
    ReadDirectory {
        /// Directory being listed.
        path: PathBuf,
        /// Bounded filesystem error.
        source: FileSystemReadError,
    },
    /// A bounded regular-file read failed.
    #[error("could not read `{}`: {source}", path.display())]
    ReadFile {
        /// File being read.
        path: PathBuf,
        /// Bounded filesystem error.
        source: FileSystemReadError,
    },
    /// Source trees never follow symbolic links.
    #[error("symbolic links are not allowed in package source trees: `{}`", path.display())]
    Symlink {
        /// Linked entry.
        path: PathBuf,
    },
    /// Sockets, devices, FIFOs, and other special entries are unsupported.
    #[error("unsupported package source entry `{}`", path.display())]
    UnsupportedEntry {
        /// Unsupported entry.
        path: PathBuf,
    },
    /// Canonicalization resolved an entry outside the approved package root.
    #[error(
        "package source entry `{}` resolves outside approved root `{}`",
        path.display(),
        root.display()
    )]
    OutsideRoot {
        /// Entry spelling from the traversal.
        path: PathBuf,
        /// Canonical approved package root.
        root: PathBuf,
    },
    /// Relative source-tree names must be UTF-8 so lock digests are portable
    /// and collision-free across host path encodings.
    #[error("package source path is not valid UTF-8: `{}`", path.display())]
    NonUtf8Path {
        /// Relative path that could not be encoded.
        path: PathBuf,
    },
    /// A caller-provided traversal or aggregate-byte budget was exhausted.
    #[error("source-tree hash exceeds the {resource} limit of {limit}")]
    ResourceLimit {
        /// Closed resource category used for diagnostics only.
        resource: SourceTreeResource,
        /// Configured maximum.
        limit: u64,
    },
    /// Cooperative cancellation interrupted traversal.
    #[error("source-tree hash cancelled")]
    Cancelled,
}

/// Bounded resource category used by [`SourceTreeHashError::ResourceLimit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceTreeResource {
    /// Files and directories visited.
    Entries,
    /// Aggregate regular-file bytes.
    Bytes,
}

impl std::fmt::Display for SourceTreeResource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Entries => formatter.write_str("entry-count"),
            Self::Bytes => formatter.write_str("aggregate-byte"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PortableRelativePath(String);

fn portable_relative_path(path: &Path) -> Result<PortableRelativePath, SourceTreeHashError> {
    let mut out = String::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => {
                let part = part
                    .to_str()
                    .ok_or_else(|| SourceTreeHashError::NonUtf8Path {
                        path: path.to_path_buf(),
                    })?;
                if !out.is_empty() {
                    out.push('/');
                }
                out.push_str(part);
            }
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(SourceTreeHashError::OutsideRoot {
                    path: path.to_path_buf(),
                    root: PathBuf::from("."),
                });
            }
        }
    }
    Ok(PortableRelativePath(out))
}

fn contains_git_component(path: &Path) -> bool {
    path.components().any(
        |component| matches!(component, Component::Normal(name) if name == std::ffi::OsStr::new(".git")),
    )
}

fn canonical_inside(
    fs: &dyn FileSystemReader,
    path: &Path,
    canonical_root: &Path,
) -> Result<PathBuf, SourceTreeHashError> {
    let canonical = fs
        .canonicalize(path)
        .map_err(|source| SourceTreeHashError::Canonicalize {
            path: path.to_path_buf(),
            source,
        })?;
    if canonical.starts_with(canonical_root) {
        Ok(canonical)
    } else {
        Err(SourceTreeHashError::OutsideRoot {
            path: path.to_path_buf(),
            root: canonical_root.to_path_buf(),
        })
    }
}

fn map_directory_error(path: &Path, error: FileSystemReadError) -> SourceTreeHashError {
    match error {
        FileSystemReadError::Cancelled => SourceTreeHashError::Cancelled,
        FileSystemReadError::EntryLimitExceeded { limit } => SourceTreeHashError::ResourceLimit {
            resource: SourceTreeResource::Entries,
            limit: limit.get(),
        },
        source => SourceTreeHashError::ReadDirectory {
            path: path.to_path_buf(),
            source,
        },
    }
}

fn map_file_error(path: &Path, error: FileSystemReadError) -> SourceTreeHashError {
    match error {
        FileSystemReadError::Cancelled => SourceTreeHashError::Cancelled,
        source => SourceTreeHashError::ReadFile {
            path: path.to_path_buf(),
            source,
        },
    }
}

/// Hash `graphcal.toml` and `source_dir` with one deterministic algorithm.
///
/// Traversal is iterative, rejects every symlink/special entry, checks
/// canonical containment before reading, and serializes UTF-8 relative paths
/// with `/` separators. The same function is used when creating and verifying
/// lockfiles.
///
/// # Errors
///
/// Returns a typed filesystem, containment, encoding, cancellation, or budget
/// error.
pub fn hash_source_tree(
    fs: &dyn FileSystemReader,
    root: &Path,
    source_dir: &Path,
    limits: SourceTreeHashLimits,
    cancellation: &dyn CancellationSignal,
) -> Result<SourceTreeHash, SourceTreeHashError> {
    let canonical_root =
        fs.canonicalize(root)
            .map_err(|source| SourceTreeHashError::Canonicalize {
                path: root.to_path_buf(),
                source,
            })?;
    let mut pending = vec![PathBuf::from("graphcal.toml"), source_dir.to_path_buf()];
    let mut files = BTreeMap::new();
    let mut entries = 0_u64;

    while let Some(relative) = pending.pop() {
        if cancellation.is_cancelled() {
            return Err(SourceTreeHashError::Cancelled);
        }
        if contains_git_component(&relative) {
            continue;
        }
        entries = entries
            .checked_add(1)
            .filter(|count| *count <= limits.entries)
            .ok_or(SourceTreeHashError::ResourceLimit {
                resource: SourceTreeResource::Entries,
                limit: limits.entries,
            })?;
        let path = canonical_root.join(&relative);
        let kind = fs
            .entry_kind(&path)
            .map_err(|source| SourceTreeHashError::Inspect {
                path: path.clone(),
                source,
            })?;
        match kind {
            FileSystemEntryKind::File => {
                let canonical = canonical_inside(fs, &path, &canonical_root)?;
                files.insert(portable_relative_path(&relative)?, canonical);
            }
            FileSystemEntryKind::Directory => {
                let canonical = canonical_inside(fs, &path, &canonical_root)?;
                let remaining = limits.entries.saturating_sub(entries);
                let mut children = fs
                    .read_directory_bounded(&canonical, EntryLimit::new(remaining), cancellation)
                    .map_err(|error| map_directory_error(&canonical, error))?;
                children.sort();
                pending.extend(children.into_iter().rev().map(|child| relative.join(child)));
            }
            FileSystemEntryKind::Symlink => {
                return Err(SourceTreeHashError::Symlink { path });
            }
            FileSystemEntryKind::Other => {
                return Err(SourceTreeHashError::UnsupportedEntry { path });
            }
        }
    }

    let mut hasher = Sha256::new();
    let mut total_bytes = 0_u64;
    for (relative, path) in &files {
        if cancellation.is_cancelled() {
            return Err(SourceTreeHashError::Cancelled);
        }
        let remaining = limits.total_bytes.saturating_sub(total_bytes);
        let limit = ByteLimit::new(limits.file_bytes.get().min(remaining));
        let bytes = fs
            .read_bytes_bounded(path, limit, cancellation)
            .map_err(|error| map_file_error(path, error))?;
        total_bytes = total_bytes.checked_add(bytes.len() as u64).ok_or(
            SourceTreeHashError::ResourceLimit {
                resource: SourceTreeResource::Bytes,
                limit: limits.total_bytes,
            },
        )?;
        if total_bytes > limits.total_bytes {
            return Err(SourceTreeHashError::ResourceLimit {
                resource: SourceTreeResource::Bytes,
                limit: limits.total_bytes,
            });
        }
        hasher.update(relative.0.as_bytes());
        hasher.update([0]);
        hasher.update(bytes.len().to_string().as_bytes());
        hasher.update([0]);
        hasher.update(bytes);
        hasher.update([0]);
    }

    Ok(SourceTreeHash {
        sha256: hex_string(&hasher.finalize()),
        files: files.len() as u64,
        bytes: total_bytes,
        entries,
    })
}

fn hex_string(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NeverCancel, RealFileSystem};

    #[test]
    fn hashes_files_in_portable_relative_order() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join("src/pkg")).unwrap();
        std::fs::write(directory.path().join("graphcal.toml"), "manifest").unwrap();
        std::fs::write(directory.path().join("src/pkg/z.gcl"), "z").unwrap();
        std::fs::write(directory.path().join("src/pkg/a.gcl"), "a").unwrap();
        let fs = RealFileSystem::rooted(directory.path()).unwrap();

        let first = hash_source_tree(
            &fs,
            directory.path(),
            Path::new("src"),
            SourceTreeHashLimits::unbounded(),
            &NeverCancel,
        )
        .unwrap();
        let second = hash_source_tree(
            &fs,
            directory.path(),
            Path::new("src"),
            SourceTreeHashLimits::unbounded(),
            &NeverCancel,
        )
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(first.files(), 3);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_file_and_directory_symlinks() {
        use std::os::unix::fs::symlink;

        for target_is_directory in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(directory.path().join("src")).unwrap();
            std::fs::write(directory.path().join("graphcal.toml"), "manifest").unwrap();
            let target = if target_is_directory {
                outside.path().to_path_buf()
            } else {
                let target = outside.path().join("outside.gcl");
                std::fs::write(&target, "outside").unwrap();
                target
            };
            symlink(&target, directory.path().join("src/link")).unwrap();
            let fs = RealFileSystem::rooted(directory.path()).unwrap();

            let error = hash_source_tree(
                &fs,
                directory.path(),
                Path::new("src"),
                SourceTreeHashLimits::unbounded(),
                &NeverCancel,
            )
            .unwrap_err();
            assert!(matches!(error, SourceTreeHashError::Symlink { .. }));
        }
    }
}
