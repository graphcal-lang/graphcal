//! Filesystem implementations for the Graphcal compiler.
//!
//! This crate provides concrete [`graphcal_eval::io::FileSystemReader`] implementations that bridge
//! the compiler's abstract filesystem trait (defined in [`graphcal_eval::io`]) with
//! real operating-system I/O. Shell crates (`graphcal-cli`, `graphcal-lsp`) depend
//! on this crate instead of calling `std::fs` directly.

mod overlay_fs;
mod real_fs;

pub use overlay_fs::OverlayFileSystem;
pub use real_fs::RealFileSystem;
