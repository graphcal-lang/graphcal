//! Explicit authority for locked dependency reads, independent of the loader.

use std::collections::BTreeMap;
use std::path::PathBuf;

use graphcal_io::InMemoryFileSystem;
use graphcal_package::PackageInstanceId;

/// One isolated package filesystem. Its paths never grant access to another
/// package or the native filesystem; the loader still verifies its lock hash.
#[derive(Debug, Clone)]
pub struct EmbeddedPackage {
    pub root: PathBuf,
    pub filesystem: InMemoryFileSystem,
}

/// Dependency authority selected by the application shell. Embedded loading
/// never falls back to the native cache, including when invoked on native hosts.
#[derive(Clone, Copy)]
pub enum DependencySources<'a> {
    NativeCache,
    Embedded(&'a BTreeMap<PackageInstanceId, EmbeddedPackage>),
}
