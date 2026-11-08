//! Identities derived from a value's content.

use serde::Serialize;
use sha2::{Digest, Sha256};

/// A digest of a value's canonical JSON encoding.
///
/// Positions and slots carry the identity of the key type, algebraic type,
/// or session schema they index, so one from another type or schema selects
/// nothing instead of the wrong entry. Equal contents have equal identities,
/// so a position stays valid for an equal copy of its type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentId(u64);

impl ContentId {
    /// The identity of `value`.
    #[expect(
        clippy::disallowed_methods,
        reason = "protocol types hold only strings, numbers, and string-keyed maps, which always serialize"
    )]
    pub(crate) fn of<T: Serialize>(value: &T) -> Self {
        // Serializing typed structs with ordered maps is deterministic.
        let digest = Sha256::digest(serde_json::to_vec(value).unwrap_or_default());
        let mut prefix = [0_u8; 8];
        prefix.copy_from_slice(&digest[..8]);
        Self(u64::from_le_bytes(prefix))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_contents_have_equal_identities() {
        assert_eq!(ContentId::of(&("a", 1)), ContentId::of(&("a", 1)));
        assert_ne!(ContentId::of(&("a", 1)), ContentId::of(&("a", 2)));
    }
}
