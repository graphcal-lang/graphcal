//! Fingerprints identifying a model interface.

use std::fmt;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::interface::ModelInterface;

const PREFIX: &str = "sha256:";

/// The SHA-256 digest of a [`ModelInterface`]'s canonical JSON encoding.
///
/// Two interfaces have the same fingerprint exactly when their parameters,
/// outputs, types, units, domains, and defaults agree. A change to a
/// formula that keeps the interface keeps the fingerprint.
///
/// Spelled `sha256:` followed by 64 lowercase hexadecimal digits.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct InterfaceFingerprint([u8; 32]);

/// Why a string is not an interface fingerprint spelling.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("`{spelling}` is not `sha256:` followed by 64 lowercase hexadecimal digits")]
pub struct InterfaceFingerprintError {
    /// The rejected spelling.
    pub spelling: String,
}

impl InterfaceFingerprint {
    /// The fingerprint of `interface`.
    #[must_use]
    #[expect(
        clippy::disallowed_methods,
        reason = "an interface holds only strings, numbers, and string-keyed maps, which always serialize"
    )]
    pub fn of(interface: &ModelInterface) -> Self {
        // Serializing typed structs with ordered maps is deterministic.
        let canonical = serde_json::to_vec(interface).unwrap_or_default();
        Self(Sha256::digest(&canonical).into())
    }

    /// Parse a `sha256:<hex>` spelling.
    ///
    /// # Errors
    ///
    /// Returns [`InterfaceFingerprintError`] for any other spelling.
    pub fn parse(spelling: &str) -> Result<Self, InterfaceFingerprintError> {
        let error = || InterfaceFingerprintError {
            spelling: spelling.to_owned(),
        };
        let hex = spelling.strip_prefix(PREFIX).ok_or_else(error)?;
        if hex.len() != 64 {
            return Err(error());
        }
        let mut bytes = [0_u8; 32];
        for (byte, pair) in bytes.iter_mut().zip(hex.as_bytes().as_chunks::<2>().0) {
            let high = hex_nibble(pair[0]).ok_or_else(error)?;
            let low = hex_nibble(pair[1]).ok_or_else(error)?;
            *byte = (high << 4) | low;
        }
        Ok(Self(bytes))
    }
}

const fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

impl fmt::Display for InterfaceFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut spelling = String::with_capacity(PREFIX.len() + 64);
        spelling.push_str(PREFIX);
        for byte in self.0 {
            write!(spelling, "{byte:02x}")?;
        }
        formatter.write_str(&spelling)
    }
}

impl fmt::Debug for InterfaceFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "InterfaceFingerprint({self})")
    }
}

impl TryFrom<String> for InterfaceFingerprint {
    type Error = InterfaceFingerprintError;

    fn try_from(spelling: String) -> Result<Self, Self::Error> {
        Self::parse(&spelling)
    }
}

impl From<InterfaceFingerprint> for String {
    fn from(fingerprint: InterfaceFingerprint) -> Self {
        fingerprint.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interface::tests::sample_interface;

    #[test]
    fn spelling_round_trips() {
        let fingerprint = InterfaceFingerprint::of(&sample_interface());
        let spelling = fingerprint.to_string();
        assert!(spelling.starts_with("sha256:"));
        assert_eq!(spelling.len(), 71);
        assert_eq!(InterfaceFingerprint::parse(&spelling), Ok(fingerprint));
    }

    #[test]
    fn rejects_other_spellings() {
        for spelling in [
            "",
            "sha256:",
            "md5:00",
            &format!("sha256:{}", "A".repeat(64)),
            &format!("sha256:{}", "0".repeat(63)),
        ] {
            assert!(InterfaceFingerprint::parse(spelling).is_err(), "{spelling}");
        }
    }

    #[test]
    fn fingerprint_tracks_interface_changes() {
        let interface = sample_interface();
        let fewer_outputs = crate::interface::ModelInterface::new(
            interface.parameters().to_vec(),
            interface.outputs()[..1].to_vec(),
        )
        .unwrap();
        assert_eq!(
            InterfaceFingerprint::of(&interface),
            InterfaceFingerprint::of(&sample_interface())
        );
        assert_ne!(
            InterfaceFingerprint::of(&interface),
            InterfaceFingerprint::of(&fewer_outputs)
        );
    }
}
