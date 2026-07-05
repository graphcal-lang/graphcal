//! Filesystem abstractions and implementations for the Graphcal compiler.
//!
//! This crate owns the [`FileSystemReader`] trait and provides concrete
//! implementations:
//! - [`RealFileSystem`] — delegates to `std::fs`
//! - [`InMemoryFileSystem`] — for tests and WASM
//! - [`OverlayFileSystem`] — layers an in-memory file over a base reader

mod in_memory_fs;
mod overlay_fs;
mod real_fs;

pub use in_memory_fs::InMemoryFileSystem;
pub use overlay_fs::OverlayFileSystem;
pub use real_fs::RealFileSystem;

use std::io;
use std::path::{Path, PathBuf};

/// Abstraction over filesystem read operations.
///
/// The project loader is generic over this trait so that the compiler core
/// never calls `std::fs` directly.
pub trait FileSystemReader {
    /// Read the entire contents of a file as a UTF-8 string.
    fn read_to_string(&self, path: &Path) -> Result<String, io::Error>;

    /// Read the entire contents of a file as raw bytes (e.g. a vendored
    /// `.wasm` plugin module).
    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, io::Error>;

    /// Return the canonical, absolute form of a path.
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, io::Error>;

    /// Return `true` if `path` points to a regular file.
    fn is_file(&self, path: &Path) -> bool;

    /// Return `true` if `path` points to an existing filesystem entry.
    fn exists(&self, path: &Path) -> bool;
}
