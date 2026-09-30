//! Explicit allocation bounds carried by every filesystem read.

use std::fmt;

/// Maximum number of bytes one filesystem read may return.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ByteLimit(u64);

impl ByteLimit {
    /// Construct a limit. Zero is a valid deny-all policy.
    #[must_use]
    pub const fn new(bytes: u64) -> Self {
        Self(bytes)
    }

    /// Maximum accepted byte count.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ByteLimit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Maximum number of entries one directory listing may return.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct EntryLimit(u64);

impl EntryLimit {
    /// Construct a limit. Zero is a valid deny-all policy.
    #[must_use]
    pub const fn new(entries: u64) -> Self {
        Self(entries)
    }

    /// Maximum accepted entry count.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for EntryLimit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}
