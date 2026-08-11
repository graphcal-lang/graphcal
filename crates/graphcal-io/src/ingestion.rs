//! Shared resource policy for project and package artifact ingestion.

use crate::{ByteLimit, SourceTreeHashLimits};

const MEBIBYTE: u64 = 1024 * 1024;

/// Explicit limits shared by project loading, lock generation, and plugin
/// inspection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectIngestionPolicy {
    source_file: ByteLimit,
    manifest: ByteLimit,
    lockfile: ByteLimit,
    plugin: ByteLimit,
    source_tree_file: ByteLimit,
    max_entries: u64,
    max_total_bytes: u64,
}

impl ProjectIngestionPolicy {
    /// Construct a complete artifact-ingestion policy. Zero is a valid
    /// deny-all value for every category.
    #[must_use]
    pub const fn new(
        source_file: ByteLimit,
        manifest: ByteLimit,
        lockfile: ByteLimit,
        plugin: ByteLimit,
        source_tree_file: ByteLimit,
        max_entries: u64,
        max_total_bytes: u64,
    ) -> Self {
        Self {
            source_file,
            manifest,
            lockfile,
            plugin,
            source_tree_file,
            max_entries,
            max_total_bytes,
        }
    }

    /// Maximum bytes accepted for one Graphcal source file.
    #[must_use]
    pub const fn source_file(self) -> ByteLimit {
        self.source_file
    }

    /// Maximum bytes accepted for one package manifest.
    #[must_use]
    pub const fn manifest(self) -> ByteLimit {
        self.manifest
    }

    /// Maximum bytes accepted for one lockfile.
    #[must_use]
    pub const fn lockfile(self) -> ByteLimit {
        self.lockfile
    }

    /// Maximum bytes accepted for one WASM plugin artifact.
    #[must_use]
    pub const fn plugin(self) -> ByteLimit {
        self.plugin
    }

    /// Maximum bytes accepted for one source-tree file.
    #[must_use]
    pub const fn source_tree_file(self) -> ByteLimit {
        self.source_tree_file
    }

    /// Maximum aggregate artifact and source-tree entry count.
    #[must_use]
    pub const fn max_entries(self) -> u64 {
        self.max_entries
    }

    /// Maximum aggregate bytes read.
    #[must_use]
    pub const fn max_total_bytes(self) -> u64 {
        self.max_total_bytes
    }

    /// Source-tree limits using this policy's full aggregate ceilings.
    #[must_use]
    pub const fn source_tree_limits(self) -> SourceTreeHashLimits {
        SourceTreeHashLimits::new(
            self.source_tree_file,
            self.max_total_bytes,
            self.max_entries,
        )
    }
}

impl Default for ProjectIngestionPolicy {
    fn default() -> Self {
        Self::new(
            ByteLimit::new(16 * MEBIBYTE),
            ByteLimit::new(MEBIBYTE),
            ByteLimit::new(4 * MEBIBYTE),
            ByteLimit::new(16 * MEBIBYTE),
            ByteLimit::new(16 * MEBIBYTE),
            10_000,
            256 * MEBIBYTE,
        )
    }
}
