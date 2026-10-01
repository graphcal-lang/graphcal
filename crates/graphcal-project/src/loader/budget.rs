//! Byte and file budgets for one project load, and the loader's error
//! constructors shared by source acquisition.

use std::path::Path;

use graphcal_compiler::graphcal_error::GraphcalError;
use graphcal_io::{
    ByteLimit, FileSystemReadError, FileSystemReader, ProjectIngestionPolicy, SourceTreeHashLimits,
};
use graphcal_package::{LockfileParseLimits, PackageInstanceId};

use crate::compile_error::CompileError;

use super::budget_violation::{LoaderBudgetExceeded, LoaderResource};

#[cfg(test)]
pub(super) const MEBIBYTE: u64 = 1024 * 1024;

/// Per-artifact byte limits for one project load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoaderArtifactByteLimits {
    source_file: u64,
    manifest: u64,
    lockfile: u64,
    plugin: u64,
    source_tree_file: u64,
}

impl LoaderArtifactByteLimits {
    /// Construct explicit byte limits for every artifact category. Zero is a
    /// valid deny-all policy for a category.
    #[must_use]
    pub const fn new(
        source_file: u64,
        manifest: u64,
        lockfile: u64,
        plugin: u64,
        source_tree_file: u64,
    ) -> Self {
        Self {
            source_file,
            manifest,
            lockfile,
            plugin,
            source_tree_file,
        }
    }

    /// Maximum bytes accepted for one Graphcal source document.
    #[must_use]
    pub const fn source_file_bytes(self) -> u64 {
        self.source_file
    }
}

impl Default for LoaderArtifactByteLimits {
    fn default() -> Self {
        let policy = ProjectIngestionPolicy::default();
        Self::new(
            policy.source_file().get(),
            policy.manifest().get(),
            policy.lockfile().get(),
            policy.plugin().get(),
            policy.source_tree_file().get(),
        )
    }
}

/// Resource policy for one complete project load.
///
/// The budget covers root/dependency source files, manifests, lockfiles,
/// plugin modules, and every regular file read while verifying a locked source
/// tree. Aggregate counters are private and are created afresh for each load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoaderBudget {
    artifacts: LoaderArtifactByteLimits,
    max_files: u64,
    max_total_bytes: u64,
}

impl LoaderBudget {
    /// Construct an explicit loader policy. Zero values are valid and reject
    /// the corresponding resource immediately.
    #[must_use]
    pub const fn new(
        artifacts: LoaderArtifactByteLimits,
        max_files: u64,
        max_total_bytes: u64,
    ) -> Self {
        Self {
            artifacts,
            max_files,
            max_total_bytes,
        }
    }
}

impl Default for LoaderBudget {
    fn default() -> Self {
        let policy = ProjectIngestionPolicy::default();
        Self::new(
            LoaderArtifactByteLimits::default(),
            policy.max_entries(),
            policy.max_total_bytes(),
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum LoaderArtifact {
    SourceFile,
    Manifest,
    Lockfile,
    Plugin,
}

impl LoaderArtifact {
    pub(super) const fn resource(self) -> LoaderResource {
        match self {
            Self::SourceFile => LoaderResource::SourceFileBytes,
            Self::Manifest => LoaderResource::ManifestBytes,
            Self::Lockfile => LoaderResource::LockfileBytes,
            Self::Plugin => LoaderResource::PluginBytes,
        }
    }

    pub(super) const fn byte_limit(self, limits: LoaderArtifactByteLimits) -> u64 {
        match self {
            Self::SourceFile => limits.source_file,
            Self::Manifest => limits.manifest,
            Self::Lockfile => limits.lockfile,
            Self::Plugin => limits.plugin,
        }
    }
}

#[derive(Debug)]
pub(super) enum LoaderReadError {
    Budget(LoaderBudgetExceeded),
    Filesystem(FileSystemReadError),
}

impl std::fmt::Display for LoaderReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Budget(error) => error.fmt(formatter),
            Self::Filesystem(error) => error.fmt(formatter),
        }
    }
}

pub(super) struct LoaderBudgetState {
    policy: LoaderBudget,
    files_read: u64,
    total_bytes: u64,
}

impl LoaderBudgetState {
    pub(super) const fn new(policy: LoaderBudget) -> Self {
        Self {
            policy,
            files_read: 0,
            total_bytes: 0,
        }
    }

    pub(super) fn lockfile_parse_limits(&self) -> LockfileParseLimits {
        LockfileParseLimits::new(usize::try_from(self.policy.max_files).unwrap_or(usize::MAX))
    }

    pub(super) fn exceeded(path: &Path, resource: LoaderResource, limit: u64) -> LoaderReadError {
        LoaderReadError::Budget(LoaderBudgetExceeded {
            path: path.to_path_buf(),
            resource,
            limit,
        })
    }

    pub(super) fn read_bytes(
        &mut self,
        fs: &dyn FileSystemReader,
        path: &Path,
        artifact: LoaderArtifact,
        cancellation: &graphcal_compiler::cancellation::CancellationToken,
    ) -> Result<Vec<u8>, LoaderReadError> {
        if self.files_read >= self.policy.max_files {
            return Err(Self::exceeded(
                path,
                LoaderResource::FileCount,
                self.policy.max_files,
            ));
        }
        let artifact_limit = artifact.byte_limit(self.policy.artifacts);
        let total_remaining = self.policy.max_total_bytes.saturating_sub(self.total_bytes);
        let (read_limit, exhausted_resource) = if artifact_limit <= total_remaining {
            (artifact_limit, artifact.resource())
        } else {
            (total_remaining, LoaderResource::TotalBytes)
        };
        let cancellation_signal = || cancellation.is_cancelled();
        let bytes = fs
            .read_bytes_bounded(path, ByteLimit::new(read_limit), &cancellation_signal)
            .map_err(|error| match error {
                FileSystemReadError::ByteLimitExceeded { .. } => Self::exceeded(
                    path,
                    exhausted_resource,
                    match exhausted_resource {
                        LoaderResource::TotalBytes => self.policy.max_total_bytes,
                        _ => artifact_limit,
                    },
                ),
                other => LoaderReadError::Filesystem(other),
            })?;
        self.files_read = self.files_read.saturating_add(1);
        self.total_bytes = self.total_bytes.saturating_add(bytes.len() as u64);
        Ok(bytes)
    }

    pub(super) fn read_text(
        &mut self,
        fs: &dyn FileSystemReader,
        path: &Path,
        artifact: LoaderArtifact,
        cancellation: &graphcal_compiler::cancellation::CancellationToken,
    ) -> Result<String, LoaderReadError> {
        let bytes = self.read_bytes(fs, path, artifact, cancellation)?;
        String::from_utf8(bytes).map_err(|error| {
            LoaderReadError::Filesystem(FileSystemReadError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                error,
            )))
        })
    }

    pub(super) const fn remaining_files(&self) -> u64 {
        self.policy.max_files.saturating_sub(self.files_read)
    }

    pub(super) const fn remaining_bytes(&self) -> u64 {
        self.policy.max_total_bytes.saturating_sub(self.total_bytes)
    }

    pub(super) const fn source_tree_limits(&self) -> SourceTreeHashLimits {
        SourceTreeHashLimits::new(
            ByteLimit::new(self.policy.artifacts.source_tree_file),
            self.remaining_bytes(),
            self.remaining_files(),
        )
    }

    pub(super) fn account_source_tree(
        &mut self,
        root: &Path,
        entries: u64,
        bytes: u64,
    ) -> Result<(), LoaderBudgetExceeded> {
        let next_files = self.files_read.saturating_add(entries);
        if next_files > self.policy.max_files {
            return Err(LoaderBudgetExceeded {
                path: root.to_path_buf(),
                resource: LoaderResource::FileCount,
                limit: self.policy.max_files,
            });
        }
        let next_bytes = self.total_bytes.saturating_add(bytes);
        if next_bytes > self.policy.max_total_bytes {
            return Err(LoaderBudgetExceeded {
                path: root.to_path_buf(),
                resource: LoaderResource::TotalBytes,
                limit: self.policy.max_total_bytes,
            });
        }
        self.files_read = next_files;
        self.total_bytes = next_bytes;
        Ok(())
    }
}

pub(super) fn loader_manifest_error(error: impl std::fmt::Display) -> CompileError {
    CompileError::Eval(GraphcalError::ManifestError {
        message: error.to_string(),
    })
}

/// A locked package instance without a captured source authority.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(super) enum PackageAuthorityError {
    #[error("lockfile package `{0}` has no source root")]
    NoSourceRoot(PackageInstanceId),
    #[error("lockfile package `{0}` has no filesystem capability")]
    NoFilesystem(PackageInstanceId),
}

/// Helper to create a `FileNotFound` error (used for the root file itself).
pub(super) fn io_not_found(path: &Path) -> CompileError {
    CompileError::Eval(GraphcalError::FileNotFound {
        path: path.display().to_string(),
    })
}
