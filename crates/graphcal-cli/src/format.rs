//! Functional core for `.gcl` discovery and format checking.
//!
//! These helpers are pure (or deterministic over the filesystem) and produce
//! no stdout/stderr side effects: the CLI shell renders user-facing output and
//! performs writes around them. Keeping them here lets tests exercise the same
//! discovery and decision logic the binary uses without spawning a process.

use std::io::{self, Read};
use std::path::{Path, PathBuf};

use graphcal_io::ByteLimit;

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

/// What `graphcal format` operates on, decided from its path arguments.
#[derive(Debug, PartialEq, Eq)]
pub enum FormatInput {
    /// `-`: read one source from stdin and write the result to stdout.
    Stdin,
    /// Files or directories formatted in place (empty means the current
    /// directory).
    Paths(Vec<PathBuf>),
}

/// The stdin marker `-` was given together with other paths.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("`-` (stdin) cannot be combined with other paths")]
pub struct StdinCombinedWithPaths;

impl FormatInput {
    /// Classify the CLI path arguments. `-` must be the only argument.
    ///
    /// # Errors
    ///
    /// Returns [`StdinCombinedWithPaths`] when `-` occurs alongside any other
    /// argument (including a second `-`).
    pub fn from_args(paths: Vec<PathBuf>) -> Result<Self, StdinCombinedWithPaths> {
        let is_stdin = |path: &PathBuf| path.as_path() == Path::new("-");
        match paths.as_slice() {
            [only] if is_stdin(only) => Ok(Self::Stdin),
            _ if paths.iter().any(is_stdin) => Err(StdinCombinedWithPaths),
            _ => Ok(Self::Paths(paths)),
        }
    }
}

/// Failure to read a source document from a stream.
#[derive(Debug, thiserror::Error)]
pub enum SourceReadError {
    /// The stream failed or was not valid UTF-8.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// The stream was longer than the source-size limit.
    #[error("input exceeds the read limit of {limit} bytes")]
    TooLarge {
        /// Maximum accepted byte count.
        limit: ByteLimit,
    },
}

/// Read one UTF-8 source document from `reader`, accepting at most `limit`
/// bytes so an unbounded stream cannot exhaust memory.
///
/// # Errors
///
/// Returns [`SourceReadError`] when reading fails, the input is not UTF-8, or
/// it exceeds `limit`.
pub fn read_source_bounded(reader: impl Read, limit: ByteLimit) -> Result<String, SourceReadError> {
    let mut bytes = Vec::new();
    reader
        .take(limit.get().saturating_add(1))
        .read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit.get() {
        return Err(SourceReadError::TooLarge { limit });
    }
    String::from_utf8(bytes)
        .map_err(|error| SourceReadError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))
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

    /// Number of traversal failures. Always at least one.
    #[must_use]
    pub const fn count(&self) -> usize {
        1 + self.rest.len()
    }

    /// Visit every failure in traversal order.
    pub fn iter(&self) -> impl Iterator<Item = &TraversalFailure> {
        std::iter::once(&self.first).chain(&self.rest)
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
    fn into_parts(self) -> (Vec<PathBuf>, TraversalFailures) {
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
/// `target`, `node_modules`, etc.) are skipped below `dir`. `dir` itself is
/// never skipped: a directory the user names explicitly is always walked, so a
/// CI gate such as `graphcal check target` cannot pass vacuously. Incomplete
/// traversal is a
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
            // Skip well-known generated/vendored directories below the walk
            // root (depth 0 is the directory the user named).
            // Non-UTF-8 directory names are not skipped — they won't match
            // any SKIP_DIRS entry anyway, so we intentionally let them
            // through rather than treating them as if they were the empty
            // string.
            if e.depth() > 0 && e.file_type().is_dir() {
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

    fn paths(args: &[&str]) -> Vec<PathBuf> {
        args.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn lone_dash_selects_stdin() {
        assert_eq!(
            FormatInput::from_args(paths(&["-"])),
            Ok(FormatInput::Stdin)
        );
    }

    #[test]
    fn plain_paths_are_kept_in_order() {
        assert_eq!(
            FormatInput::from_args(paths(&["b.gcl", "a"])),
            Ok(FormatInput::Paths(paths(&["b.gcl", "a"])))
        );
        assert_eq!(
            FormatInput::from_args(Vec::new()),
            Ok(FormatInput::Paths(Vec::new()))
        );
    }

    #[test]
    fn dash_with_other_paths_is_rejected() {
        for args in [&["-", "a.gcl"][..], &["a.gcl", "-"], &["-", "-"]] {
            assert_eq!(
                FormatInput::from_args(paths(args)),
                Err(StdinCombinedWithPaths)
            );
        }
    }

    #[test]
    fn bounded_read_accepts_input_at_the_limit() {
        let read = read_source_bounded(&b"abc"[..], ByteLimit::new(3)).unwrap();
        assert_eq!(read, "abc");
    }

    #[test]
    fn bounded_read_rejects_input_over_the_limit() {
        let error = read_source_bounded(&b"abcd"[..], ByteLimit::new(3)).unwrap_err();
        assert!(matches!(error, SourceReadError::TooLarge { limit } if limit.get() == 3));
    }

    #[test]
    fn bounded_read_rejects_invalid_utf8() {
        let error = read_source_bounded(&[0xff_u8][..], ByteLimit::new(3)).unwrap_err();
        assert!(matches!(error, SourceReadError::Io(e) if e.kind() == io::ErrorKind::InvalidData));
    }

    #[test]
    fn missing_root_is_an_incomplete_discovery() {
        let root = Path::new("/definitely/missing/graphcal/discovery/root");
        let FileDiscovery::Incomplete(incomplete) = collect_gcl_files(root) else {
            panic!("missing root must not be reported as a complete empty tree");
        };
        let (files, failures) = incomplete.into_parts();
        assert_eq!(files, [] as [PathBuf; 0]);
        assert_eq!(failures.count(), 1);
        assert_eq!(failures.iter().next().unwrap().path(), root);
    }

    #[test]
    fn skip_dirs_apply_only_below_the_named_root() {
        let directory = tempfile::tempdir().unwrap();
        let named = directory.path().join("target");
        std::fs::create_dir_all(named.join("node_modules")).unwrap();
        std::fs::create_dir_all(named.join("nested/target")).unwrap();
        std::fs::write(named.join("a.gcl"), "").unwrap();
        std::fs::write(named.join("node_modules/skipped.gcl"), "").unwrap();
        std::fs::write(named.join("nested/b.gcl"), "").unwrap();
        std::fs::write(named.join("nested/target/skipped.gcl"), "").unwrap();

        let FileDiscovery::Complete(files) = collect_gcl_files(&named) else {
            panic!("an existing tree must be discovered completely");
        };
        assert_eq!(files, [named.join("a.gcl"), named.join("nested/b.gcl")]);
    }

    #[test]
    fn merging_incomplete_discoveries_never_loses_failures() {
        let first = collect_gcl_files(Path::new("/definitely/missing/graphcal/one"));
        let second = collect_gcl_files(Path::new("/definitely/missing/graphcal/two"));
        let (_, failures) = first.merge(second).into_parts();
        assert_eq!(failures.unwrap().count(), 2);
    }
}
