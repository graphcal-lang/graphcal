//! Shared package-cache root and typed checkout path derivation.

use std::ffi::OsString;
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
        let configured = configured_cache_directory(|name| std::env::var_os(name))
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

/// Cache directory selected by the environment, read through `var`.
///
/// An empty variable counts as unset: an empty `GRAPHCAL_CACHE_DIR` or
/// `XDG_CACHE_HOME` would otherwise resolve to the current directory and put
/// full clones inside the project, where they are easily committed. A
/// non-empty relative [`CACHE_ENV`] is an explicit user choice and is resolved
/// against the current directory by [`PackageCacheRoot::from_path`].
fn configured_cache_directory(var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let set = |name: &str| var(name).filter(|value| !value.is_empty());
    set(CACHE_ENV)
        .map(PathBuf::from)
        .or_else(|| platform_cache_directory(set))
}

/// Per-platform default cache directory, used when [`CACHE_ENV`] is unset.
///
/// Unix follows the XDG base directory specification, which says a relative
/// `XDG_CACHE_HOME` is invalid and must be ignored; a relative `HOME` is
/// ignored for the same reason. Windows has no XDG convention and sets neither
/// `XDG_CACHE_HOME` nor `HOME`, so probing only those leaves every Windows user
/// unable to resolve a cache root at all; it uses the conventional per-user
/// local application data directory instead.
#[cfg(not(windows))]
fn platform_cache_directory(set: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let absolute = |name: &str| {
        set(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    absolute("XDG_CACHE_HOME")
        .map(|base| base.join("graphcal"))
        .or_else(|| absolute("HOME").map(|home| home.join(".cache").join("graphcal")))
}

#[cfg(windows)]
fn platform_cache_directory(set: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let absolute = |name: &str| {
        set(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    absolute("LOCALAPPDATA")
        .map(|base| base.join("graphcal").join("cache"))
        .or_else(|| absolute("USERPROFILE").map(|home| home.join(".cache").join("graphcal")))
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

    fn resolve(vars: &[(&str, &str)]) -> Option<PathBuf> {
        configured_cache_directory(|name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        })
    }

    fn absolute(name: &str) -> String {
        std::path::absolute(std::env::temp_dir())
            .unwrap()
            .join(name)
            .to_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn explicit_cache_override_wins_and_may_be_relative() {
        assert_eq!(
            resolve(&[(CACHE_ENV, "relative-cache")]),
            Some(PathBuf::from("relative-cache"))
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn empty_or_relative_cache_variables_never_select_the_current_directory() {
        let home = absolute("home");
        let expected = Some(PathBuf::from(&home).join(".cache").join("graphcal"));
        assert_eq!(
            resolve(&[(CACHE_ENV, ""), ("XDG_CACHE_HOME", ""), ("HOME", &home)]),
            expected
        );
        assert_eq!(
            resolve(&[("XDG_CACHE_HOME", "relative"), ("HOME", &home)]),
            expected
        );
        let xdg = absolute("xdg");
        assert_eq!(
            resolve(&[("XDG_CACHE_HOME", &xdg), ("HOME", &home)]),
            Some(PathBuf::from(&xdg).join("graphcal"))
        );
        assert_eq!(resolve(&[(CACHE_ENV, ""), ("HOME", "")]), None);
        assert_eq!(resolve(&[("HOME", "relative-home")]), None);
    }

    #[cfg(windows)]
    #[test]
    fn empty_or_relative_cache_variables_never_select_the_current_directory() {
        let local = absolute("local");
        assert_eq!(
            resolve(&[(CACHE_ENV, ""), ("LOCALAPPDATA", &local)]),
            Some(PathBuf::from(&local).join("graphcal").join("cache"))
        );
        assert_eq!(
            resolve(&[("LOCALAPPDATA", ""), ("USERPROFILE", "relative")]),
            None
        );
    }
}
