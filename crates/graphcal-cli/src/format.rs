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

/// One failed traversal operation, retaining the requested discovery root when
/// `walkdir` cannot identify a more specific path.
#[derive(Debug)]
pub struct TraversalFailure {
    root: PathBuf,
    source: walkdir::Error,
}

impl TraversalFailure {
    /// Path to report for this failure.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.source.path().unwrap_or(&self.root)
    }

    /// Underlying traversal error.
    #[must_use]
    pub const fn source(&self) -> &walkdir::Error {
        &self.source
    }

    /// Consume the wrapper and return the underlying traversal error.
    #[must_use]
    pub fn into_source(self) -> walkdir::Error {
        self.source
    }
}

/// A collection statically guaranteed to contain at least one traversal failure.
#[derive(Debug)]
pub struct TraversalFailures {
    first: TraversalFailure,
    rest: Vec<TraversalFailure>,
}

impl TraversalFailures {
    fn from_errors(root: &Path, errors: Vec<walkdir::Error>) -> Option<Self> {
        let mut failures = errors.into_iter().map(|source| TraversalFailure {
            root: root.to_path_buf(),
            source,
        });
        failures.next().map(|first| Self {
            first,
            rest: failures.collect(),
        })
    }

    fn append(&mut self, other: Self) {
        self.rest.push(other.first);
        self.rest.extend(other.rest);
    }

    /// Number of traversal failures.
    #[must_use]
    pub const fn len(&self) -> usize {
        1 + self.rest.len()
    }

    /// This collection is non-empty by construction.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }

    /// Visit every failure in traversal order.
    pub fn iter(&self) -> impl Iterator<Item = &TraversalFailure> {
        std::iter::once(&self.first).chain(&self.rest)
    }

    /// Consume and visit every failure in traversal order.
    pub fn into_failures(self) -> impl Iterator<Item = TraversalFailure> {
        std::iter::once(self.first).chain(self.rest)
    }
}

/// Files discovered before at least one traversal operation failed.
#[derive(Debug)]
pub struct IncompleteFileDiscovery {
    files: Vec<PathBuf>,
    failures: TraversalFailures,
}

impl IncompleteFileDiscovery {
    /// Consume the incomplete result without discarding either side.
    #[must_use]
    pub fn into_parts(self) -> (Vec<PathBuf>, TraversalFailures) {
        (self.files, self.failures)
    }
}

/// Result of recursive source discovery.
///
/// A caller must handle the incomplete case explicitly; there is no tuple shape
/// from which traversal errors can be casually discarded.
#[derive(Debug)]
pub enum FileDiscovery {
    /// Every requested directory entry was inspected.
    Complete(Vec<PathBuf>),
    /// Some files were found, but at least one traversal operation failed.
    Incomplete(IncompleteFileDiscovery),
}

impl FileDiscovery {
    /// Construct a complete result, including explicit CLI file arguments.
    #[must_use]
    pub const fn complete(files: Vec<PathBuf>) -> Self {
        Self::Complete(files)
    }

    /// Merge two independent discovery results while preserving all files and
    /// all failures.
    #[must_use]
    pub fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::Complete(mut left), Self::Complete(right)) => {
                left.extend(right);
                Self::Complete(left)
            }
            (Self::Complete(mut left), Self::Incomplete(mut right)) => {
                left.append(&mut right.files);
                right.files = left;
                Self::Incomplete(right)
            }
            (Self::Incomplete(mut left), Self::Complete(right)) => {
                left.files.extend(right);
                Self::Incomplete(left)
            }
            (Self::Incomplete(mut left), Self::Incomplete(mut right)) => {
                left.files.append(&mut right.files);
                left.failures.append(right.failures);
                Self::Incomplete(left)
            }
        }
    }

    /// Transform only the discovered file list without weakening completeness.
    #[must_use]
    pub fn map_files(self, map: impl FnOnce(Vec<PathBuf>) -> Vec<PathBuf>) -> Self {
        match self {
            Self::Complete(files) => Self::Complete(map(files)),
            Self::Incomplete(mut incomplete) => {
                incomplete.files = map(incomplete.files);
                Self::Incomplete(incomplete)
            }
        }
    }

    /// Consume the result while retaining an optional, statically non-empty
    /// failure collection.
    #[must_use]
    pub fn into_parts(self) -> (Vec<PathBuf>, Option<TraversalFailures>) {
        match self {
            Self::Complete(files) => (files, None),
            Self::Incomplete(incomplete) => {
                let (files, failures) = incomplete.into_parts();
                (files, Some(failures))
            }
        }
    }
}

/// Recursively collect all `.gcl` files under `dir`, sorted for deterministic
/// output.
///
/// Uses `walkdir` for safe traversal: only regular files are collected,
/// symlinks are not followed, and common generated directories (`.git`,
/// `target`, `node_modules`, etc.) are skipped. Incomplete traversal is a
/// distinct result variant so certification commands cannot accidentally treat
/// a partial file list as complete.
#[must_use]
pub fn collect_gcl_files(dir: &Path) -> FileDiscovery {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut errors: Vec<walkdir::Error> = Vec::new();
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
            Err(err) => errors.push(err),
        }
    }
    files.sort();
    match TraversalFailures::from_errors(dir, errors) {
        Some(failures) => FileDiscovery::Incomplete(IncompleteFileDiscovery { files, failures }),
        None => FileDiscovery::Complete(files),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_root_is_an_incomplete_discovery() {
        let root = Path::new("/definitely/missing/graphcal/discovery/root");
        let FileDiscovery::Incomplete(incomplete) = collect_gcl_files(root) else {
            panic!("missing root must not be reported as a complete empty tree");
        };
        let (files, failures) = incomplete.into_parts();
        assert!(files.is_empty());
        assert_eq!(failures.len(), 1);
        assert_eq!(failures.iter().next().unwrap().path(), root);
    }

    #[test]
    fn merging_incomplete_discoveries_never_loses_failures() {
        let first = collect_gcl_files(Path::new("/definitely/missing/graphcal/one"));
        let second = collect_gcl_files(Path::new("/definitely/missing/graphcal/two"));
        let (_, failures) = first.merge(second).into_parts();
        assert_eq!(failures.unwrap().len(), 2);
    }
}
