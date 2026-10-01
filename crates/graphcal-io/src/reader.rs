//! The [`FileSystemReader`] capability boundary and its read results.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use graphcal_compiler::{cancellation::CancellationToken, outcome::Outcome};

use crate::limits::{ByteLimit, EntryLimit};

/// Failure of a bounded filesystem read.
#[derive(Debug, thiserror::Error)]
pub enum FileSystemReadError {
    /// The underlying filesystem operation failed.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// The file was larger than the caller's explicit bound.
    #[error("file exceeds the read limit of {limit} bytes")]
    ByteLimitExceeded {
        /// Maximum accepted byte count.
        limit: ByteLimit,
    },
    /// The directory had more children than the caller allowed.
    #[error("directory exceeds the listing limit of {limit} entries")]
    EntryLimitExceeded {
        /// Maximum accepted child count.
        limit: EntryLimit,
    },
}

impl FileSystemReadError {
    /// Underlying [`io::ErrorKind`] when this is an ordinary I/O failure.
    #[must_use]
    pub fn io_kind(&self) -> Option<io::ErrorKind> {
        match self {
            Self::Io(error) => Some(error.kind()),
            Self::ByteLimitExceeded { .. } | Self::EntryLimitExceeded { .. } => None,
        }
    }
}

impl From<FileSystemReadError> for Outcome<FileSystemReadError> {
    fn from(error: FileSystemReadError) -> Self {
        Self::Failed(error)
    }
}

/// SHA-256 and resource usage from one bounded streaming file hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundedFileHash {
    sha256: [u8; 32],
    bytes: u64,
}

impl BoundedFileHash {
    /// Construct a completed bounded file hash.
    #[must_use]
    pub const fn new(sha256: [u8; 32], bytes: u64) -> Self {
        Self { sha256, bytes }
    }

    /// Exact SHA-256 bytes.
    #[must_use]
    pub const fn sha256(self) -> [u8; 32] {
        self.sha256
    }

    /// Bytes consumed while hashing.
    #[must_use]
    pub const fn bytes(self) -> u64 {
        self.bytes
    }
}

/// Kind of an entry without following the entry itself when it is a symlink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSystemEntryKind {
    /// Regular file.
    File,
    /// Directory.
    Directory,
    /// Symbolic link.
    Symlink,
    /// Socket, device, FIFO, or another unsupported kind.
    Other,
}

/// Abstraction over filesystem read operations.
///
/// The project loader is generic over this trait so all reads, canonical path
/// decisions, symlink policy, and directory traversal pass through one
/// capability boundary.
pub trait FileSystemReader {
    /// Read at most `limit` bytes. Implementations must reject an oversized
    /// regular file before allocating its declared full length and must check
    /// `cancellation` while streaming and report it as [`Outcome::Cancelled`].
    fn read_bytes_bounded(
        &self,
        path: &Path,
        limit: ByteLimit,
        cancellation: &CancellationToken,
    ) -> Result<Vec<u8>, Outcome<FileSystemReadError>>;

    /// Hash one file without accepting more than `limit` bytes.
    ///
    /// Concrete disk implementations should override this to stream without
    /// allocating the complete artifact.
    fn hash_file_sha256_bounded(
        &self,
        path: &Path,
        limit: ByteLimit,
        cancellation: &CancellationToken,
    ) -> Result<BoundedFileHash, Outcome<FileSystemReadError>> {
        let bytes = self.read_bytes_bounded(path, limit, cancellation)?;
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        Ok(BoundedFileHash::new(digest, bytes.len() as u64))
    }

    /// Read a bounded UTF-8 file.
    fn read_to_string_bounded(
        &self,
        path: &Path,
        limit: ByteLimit,
        cancellation: &CancellationToken,
    ) -> Result<String, Outcome<FileSystemReadError>> {
        let bytes = self.read_bytes_bounded(path, limit, cancellation)?;
        String::from_utf8(bytes).map_err(|error| {
            FileSystemReadError::Io(io::Error::new(io::ErrorKind::InvalidData, error)).into()
        })
    }

    /// Return the canonical, absolute form of a path.
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, io::Error>;

    /// Inspect an entry without following the entry itself when it is a
    /// symbolic link.
    fn entry_kind(&self, path: &Path) -> Result<FileSystemEntryKind, io::Error>;

    /// List direct child names with an explicit allocation bound.
    fn read_directory_bounded(
        &self,
        path: &Path,
        limit: EntryLimit,
        cancellation: &CancellationToken,
    ) -> Result<Vec<OsString>, Outcome<FileSystemReadError>>;

    /// Return `true` if `path` points to a regular file.
    fn is_file(&self, path: &Path) -> bool;

    /// Return `true` if `path` points to an existing filesystem entry.
    fn exists(&self, path: &Path) -> bool;
}
