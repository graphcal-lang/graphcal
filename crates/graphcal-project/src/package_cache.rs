//! Shared package-cache root and typed checkout path derivation.

use std::path::{Path, PathBuf};

use graphcal_package::{GitSourceId, Sha256Digest};

const CACHE_ENV: &str = "GRAPHCAL_CACHE_DIR";

/// Absolute root of Graphcal's local package cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageCacheRoot(PathBuf);

impl PackageCacheRoot {
    /// Resolve an explicit cache root to an absolute identity.
    ///
    /// # Errors
    ///
    /// Returns [`PackageCacheRootError`] if a relative path cannot be resolved
    /// against the current directory.
    pub fn from_path(path: impl Into<PathBuf>) -> Result<Self, PackageCacheRootError> {
        let path = path.into();
        if path.is_absolute() {
            Ok(Self(path))
        } else {
            std::env::current_dir()
                .map(|current| Self(current.join(path)))
                .map_err(PackageCacheRootError::CurrentDirectory)
        }
    }

    /// Resolve the configured cache root from the process environment.
    ///
    /// Relative overrides are resolved once against the current directory so
    /// producers and consumers retain the same identity if the process later
    /// changes directories.
    ///
    /// # Errors
    ///
    /// Returns [`PackageCacheRootError`] when no platform cache directory can
    /// be derived or the current directory is unavailable.
    pub fn from_environment() -> Result<Self, PackageCacheRootError> {
        let configured = std::env::var_os(CACHE_ENV)
            .map(PathBuf::from)
            .or_else(platform_cache_directory)
            .ok_or(PackageCacheRootError::Unavailable)?;
        Self::from_path(configured)
    }

    /// Borrow the absolute cache root.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// Directory containing immutable Git checkouts.
    #[must_use]
    pub fn git_directory(&self) -> PathBuf {
        self.0.join("git")
    }

    /// Directory containing content-addressed generations of one immutable
    /// Git source.
    #[must_use]
    pub fn git_source_directory(&self, source: &GitSourceId) -> PathBuf {
        self.git_directory().join(source.cache_key().to_string())
    }

    /// Checkout path for one verified source-tree generation.
    #[must_use]
    pub fn git_checkout(&self, source: &GitSourceId, tree_hash: &Sha256Digest) -> PathBuf {
        self.git_source_directory(source)
            .join(tree_hash.to_string())
    }

    /// Advisory writer-lock path for one typed immutable Git source.
    #[must_use]
    pub fn git_writer_lock(&self, source: &GitSourceId) -> PathBuf {
        self.git_directory()
            .join("locks")
            .join(format!("{}.lock", source.cache_key()))
    }
}

/// Per-platform default cache directory, used when [`CACHE_ENV`] is unset.
///
/// Unix follows the XDG base directory specification. Windows has no XDG
/// convention and sets neither `XDG_CACHE_HOME` nor `HOME`, so probing only
/// those leaves every Windows user unable to resolve a cache root at all; it
/// uses the conventional per-user local application data directory instead.
#[cfg(not(windows))]
fn platform_cache_directory() -> Option<PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .map(|base| PathBuf::from(base).join("graphcal"))
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache").join("graphcal"))
        })
}

#[cfg(windows)]
fn platform_cache_directory() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(|base| PathBuf::from(base).join("graphcal").join("cache"))
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .map(|home| PathBuf::from(home).join(".cache").join("graphcal"))
        })
}

/// Failure to derive a package cache root.
#[derive(Debug, thiserror::Error)]
pub enum PackageCacheRootError {
    /// No supported cache-root environment variable was available.
    #[error("could not determine Graphcal cache directory; set {CACHE_ENV}")]
    Unavailable,
    /// A relative override could not be resolved against the current directory.
    #[error("could not determine current directory for relative package cache root: {0}")]
    CurrentDirectory(std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_cache_roots_are_resolved_once_to_absolute_paths() {
        let root = PackageCacheRoot::from_path("relative-graphcal-cache").unwrap();

        assert!(root.as_path().is_absolute());
        assert!(root.as_path().ends_with("relative-graphcal-cache"));
    }
}
