//! The typed report of one exhausted loader budget.

use std::path::PathBuf;

/// Closed loader resource category reported when a budget is exhausted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoaderResource {
    /// One Graphcal source file.
    SourceFileBytes,
    /// One `graphcal.toml` manifest.
    ManifestBytes,
    /// One `graphcal.lock` lockfile.
    LockfileBytes,
    /// One WASM plugin module.
    PluginBytes,
    /// One regular file in a verified dependency source tree.
    SourceTreeFileBytes,
    /// Number of loaded artifacts and verified source-tree entries.
    FileCount,
    /// Aggregate bytes read during the complete load.
    TotalBytes,
}

impl std::fmt::Display for LoaderResource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SourceFileBytes => formatter.write_str("source-file byte"),
            Self::ManifestBytes => formatter.write_str("manifest byte"),
            Self::LockfileBytes => formatter.write_str("lockfile byte"),
            Self::PluginBytes => formatter.write_str("plugin byte"),
            Self::SourceTreeFileBytes => formatter.write_str("source-tree file byte"),
            Self::FileCount => formatter.write_str("file-count"),
            Self::TotalBytes => formatter.write_str("aggregate byte"),
        }
    }
}

/// One concrete loader-budget violation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "loader {resource} limit of {limit} exceeded while reading `{}`",
    path.display()
)]
pub struct LoaderBudgetExceeded {
    /// Artifact whose read exhausted the policy.
    pub path: PathBuf,
    /// Closed resource category.
    pub resource: LoaderResource,
    /// Configured maximum.
    pub limit: u64,
}
