//! Filesystem-event classification, client watcher registration, and read tracking.
//!
//! The LSP shell owns registration and event delivery. This module keeps the
//! supported artifact set and the filesystem inputs consumed by one project
//! load explicit and typed.

use std::collections::HashSet;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use graphcal_io::{
    BoundedFileHash, ByteLimit, CancellationSignal, EntryLimit, FileSystemEntryKind,
    FileSystemReadError, FileSystemReader,
};
use tower_lsp::lsp_types::{
    DidChangeWatchedFilesRegistrationOptions, FileSystemWatcher, GlobPattern, Registration,
    WatchKind,
};

const WATCH_REGISTRATION_ID: &str = "graphcal-filesystem-inputs";
const WATCHED_FILES_METHOD: &str = "workspace/didChangeWatchedFiles";

/// Artifact categories whose disk contents can affect an LSP analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchedArtifactKind {
    Source,
    Manifest,
    Lockfile,
    Plugin,
}

impl WatchedArtifactKind {
    /// Classify a path at the filesystem/protocol boundary.
    #[must_use]
    pub fn from_path(path: &Path) -> Option<Self> {
        match path.file_name().and_then(|name| name.to_str()) {
            Some("graphcal.toml") => Some(Self::Manifest),
            Some("graphcal.lock") => Some(Self::Lockfile),
            _ => match path.extension().and_then(|extension| extension.to_str()) {
                Some(extension) if extension.eq_ignore_ascii_case("gcl") => Some(Self::Source),
                Some(extension) if extension.eq_ignore_ascii_case("wasm") => Some(Self::Plugin),
                _ => None,
            },
        }
    }
}

/// Dynamic registration for all events that can change a project load.
pub fn watcher_registration() -> Result<Registration, serde_json::Error> {
    let kinds = WatchKind::Create | WatchKind::Change | WatchKind::Delete;
    let watchers = [
        "**/*.gcl",
        "**/graphcal.toml",
        "**/graphcal.lock",
        "**/*.wasm",
    ]
    .into_iter()
    .map(|pattern| FileSystemWatcher {
        glob_pattern: GlobPattern::String(pattern.to_string()),
        kind: Some(kinds),
    })
    .collect();
    Ok(Registration {
        id: WATCH_REGISTRATION_ID.to_string(),
        method: WATCHED_FILES_METHOD.to_string(),
        register_options: Some(serde_json::to_value(
            DidChangeWatchedFilesRegistrationOptions { watchers },
        )?),
    })
}

/// A filesystem decorator that records relevant files consulted by the loader.
///
/// Failed existence, metadata, canonicalization, and read operations are
/// recorded too. This is what lets creation of a previously missing import
/// invalidate the open importer that attempted to load it.
pub struct TrackingFileSystem<F> {
    inner: F,
    accessed: Mutex<HashSet<PathBuf>>,
}

impl<F> TrackingFileSystem<F> {
    #[must_use]
    pub fn new(inner: F) -> Self {
        Self {
            inner,
            accessed: Mutex::new(HashSet::new()),
        }
    }

    fn record(&self, path: &Path) {
        if WatchedArtifactKind::from_path(path).is_some() {
            self.accessed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(path.to_path_buf());
        }
    }

    #[must_use]
    pub fn accessed_paths(&self) -> HashSet<PathBuf> {
        self.accessed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl<F: FileSystemReader> FileSystemReader for TrackingFileSystem<F> {
    fn read_bytes_bounded(
        &self,
        path: &Path,
        limit: ByteLimit,
        cancellation: &dyn CancellationSignal,
    ) -> Result<Vec<u8>, FileSystemReadError> {
        self.record(path);
        self.inner.read_bytes_bounded(path, limit, cancellation)
    }

    fn hash_file_sha256_bounded(
        &self,
        path: &Path,
        limit: ByteLimit,
        cancellation: &dyn CancellationSignal,
    ) -> Result<BoundedFileHash, FileSystemReadError> {
        self.record(path);
        self.inner
            .hash_file_sha256_bounded(path, limit, cancellation)
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, io::Error> {
        self.record(path);
        self.inner.canonicalize(path)
    }

    fn entry_kind(&self, path: &Path) -> Result<FileSystemEntryKind, io::Error> {
        self.record(path);
        self.inner.entry_kind(path)
    }

    fn read_directory_bounded(
        &self,
        path: &Path,
        limit: EntryLimit,
        cancellation: &dyn CancellationSignal,
    ) -> Result<Vec<OsString>, FileSystemReadError> {
        self.inner.read_directory_bounded(path, limit, cancellation)
    }

    fn is_file(&self, path: &Path) -> bool {
        self.record(path);
        self.inner.is_file(path)
    }

    fn exists(&self, path: &Path) -> bool {
        self.record(path);
        self.inner.exists(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_classification_is_explicit_and_case_safe_for_extensions() {
        assert_eq!(
            WatchedArtifactKind::from_path(Path::new("src/main.GCL")),
            Some(WatchedArtifactKind::Source)
        );
        assert_eq!(
            WatchedArtifactKind::from_path(Path::new("graphcal.toml")),
            Some(WatchedArtifactKind::Manifest)
        );
        assert_eq!(
            WatchedArtifactKind::from_path(Path::new("graphcal.lock")),
            Some(WatchedArtifactKind::Lockfile)
        );
        assert_eq!(
            WatchedArtifactKind::from_path(Path::new("plugins/model.wasm")),
            Some(WatchedArtifactKind::Plugin)
        );
        assert_eq!(WatchedArtifactKind::from_path(Path::new("notes.md")), None);
    }

    #[test]
    fn watcher_registration_covers_create_change_and_delete() {
        let registration = watcher_registration().unwrap();
        assert_eq!(registration.method, WATCHED_FILES_METHOD);
        let options: DidChangeWatchedFilesRegistrationOptions =
            serde_json::from_value(registration.register_options.unwrap()).unwrap();
        assert_eq!(options.watchers.len(), 4);
        assert!(options.watchers.iter().all(|watcher| {
            watcher.kind.is_some_and(|kind| {
                kind.contains(WatchKind::Create)
                    && kind.contains(WatchKind::Change)
                    && kind.contains(WatchKind::Delete)
            })
        }));
    }
}
