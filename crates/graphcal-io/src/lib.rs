//! Filesystem capabilities and implementations for Graphcal.
//!
//! This crate owns the [`FileSystemReader`] boundary and provides concrete
//! implementations:
//! - atomic same-directory replacement for mutating shells
//! - [`RealFileSystem`] — delegates to `std::fs`
//! - [`InMemoryFileSystem`] — for tests and WASM
//! - [`OverlayFileSystem`] — layers in-memory editor buffers over a base reader
//!
//! Reads always carry an explicit byte limit and the caller's
//! [`CancellationToken`](graphcal_compiler::cancellation::CancellationToken);
//! a cancelled read fails with
//! [`Outcome::Cancelled`](graphcal_compiler::outcome::Outcome::Cancelled),
//! never with a read error. This keeps callers from accidentally allocating an
//! unbounded file before they can enforce a policy.

mod atomic_write;
mod in_memory_fs;
mod ingestion;
mod limits;
mod overlay_fs;
mod reader;
mod real_fs;
mod source_tree;
mod virtual_path;

pub use atomic_write::{
    AtomicWriteError, create_file_atomically, replace_file_atomically_if_unchanged,
};
pub use in_memory_fs::{InMemoryFileSystem, InMemoryFileSystemError};
pub use ingestion::ProjectIngestionPolicy;
pub use limits::{ByteLimit, EntryLimit};
pub use overlay_fs::{OverlayFileSystem, OverlayFileSystemError};
pub use reader::{BoundedFileHash, FileSystemEntryKind, FileSystemReadError, FileSystemReader};
pub use real_fs::RealFileSystem;
pub use source_tree::{
    SourceTreeHash, SourceTreeHashError, SourceTreeHashLimits, SourceTreeSnapshot,
    capture_source_tree, hash_source_tree,
};
pub use virtual_path::{VirtualAbsolutePath, VirtualPathError};
