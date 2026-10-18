//! The document `graphcal model describe` prints.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::fingerprint::InterfaceFingerprint;
use crate::interface::ModelInterface;
use crate::version::PROTOCOL_VERSION;

/// A model interface together with the protocol version and fingerprint.
///
/// The fingerprint always matches the interface: a document is only built
/// from an interface or accepted after recomputing it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "DescribeRepr", into = "DescribeRepr")]
pub struct DescribeDocument {
    graphcal_version: String,
    interface_fingerprint: InterfaceFingerprint,
    interface: ModelInterface,
}

/// Why a describe document is not acceptable.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DescribeDocumentError {
    #[error("protocol version {found} is not supported; this client implements version {expected}")]
    UnsupportedVersion { found: u32, expected: u32 },
    #[error("interface fingerprint {stated} does not match the interface ({actual})")]
    FingerprintMismatch {
        stated: InterfaceFingerprint,
        actual: InterfaceFingerprint,
    },
}

impl DescribeDocument {
    /// Describe `interface` as served by Graphcal `graphcal_version`.
    #[must_use]
    pub fn new(graphcal_version: impl Into<String>, interface: ModelInterface) -> Self {
        Self {
            graphcal_version: graphcal_version.into(),
            interface_fingerprint: InterfaceFingerprint::of(&interface),
            interface,
        }
    }

    /// The Graphcal version that produced the document (informational).
    #[must_use]
    pub fn graphcal_version(&self) -> &str {
        &self.graphcal_version
    }

    /// The interface fingerprint.
    #[must_use]
    pub const fn interface_fingerprint(&self) -> InterfaceFingerprint {
        self.interface_fingerprint
    }

    /// The model interface.
    #[must_use]
    pub const fn interface(&self) -> &ModelInterface {
        &self.interface
    }

    /// Take the model interface.
    #[must_use]
    pub fn into_interface(self) -> ModelInterface {
        self.interface
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DescribeRepr {
    protocol_version: u32,
    graphcal_version: String,
    interface_fingerprint: InterfaceFingerprint,
    interface: ModelInterface,
}

impl TryFrom<DescribeRepr> for DescribeDocument {
    type Error = DescribeDocumentError;

    fn try_from(repr: DescribeRepr) -> Result<Self, Self::Error> {
        if repr.protocol_version != PROTOCOL_VERSION {
            return Err(DescribeDocumentError::UnsupportedVersion {
                found: repr.protocol_version,
                expected: PROTOCOL_VERSION,
            });
        }
        let document = Self::new(repr.graphcal_version, repr.interface);
        if document.interface_fingerprint != repr.interface_fingerprint {
            return Err(DescribeDocumentError::FingerprintMismatch {
                stated: repr.interface_fingerprint,
                actual: document.interface_fingerprint,
            });
        }
        Ok(document)
    }
}

impl From<DescribeDocument> for DescribeRepr {
    fn from(document: DescribeDocument) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            graphcal_version: document.graphcal_version,
            interface_fingerprint: document.interface_fingerprint,
            interface: document.interface,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interface::tests::sample_interface;

    #[test]
    fn round_trips_and_checks_version_and_fingerprint() {
        let document = DescribeDocument::new("0.0.1", sample_interface());
        let json = serde_json::to_value(&document).unwrap();
        assert_eq!(json["protocol_version"], PROTOCOL_VERSION);
        assert_eq!(
            serde_json::from_value::<DescribeDocument>(json.clone()).unwrap(),
            document
        );

        let mut wrong_version = json.clone();
        wrong_version["protocol_version"] = (PROTOCOL_VERSION + 1).into();
        assert!(serde_json::from_value::<DescribeDocument>(wrong_version).is_err());

        let mut wrong_fingerprint = json;
        wrong_fingerprint["interface_fingerprint"] = format!("sha256:{}", "0".repeat(64)).into();
        assert!(serde_json::from_value::<DescribeDocument>(wrong_fingerprint).is_err());
    }

    #[test]
    fn rejects_unknown_fields() {
        let mut json =
            serde_json::to_value(DescribeDocument::new("0.0.1", sample_interface())).unwrap();
        json["extra"] = true.into();
        assert!(serde_json::from_value::<DescribeDocument>(json).is_err());
    }
}
