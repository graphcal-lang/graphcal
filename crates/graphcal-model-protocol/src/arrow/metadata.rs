//! The JSON values of the `graphcal` schema and field metadata keys.

use serde::{Deserialize, Serialize};

use crate::domain::Domain;
use crate::fingerprint::InterfaceFingerprint;
use crate::wire_type::WireType;

/// The metadata key used on schemas and fields.
pub const METADATA_KEY: &str = "graphcal";

/// Which of a session's two streams a schema describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    /// Client-to-server request batches.
    Request,
    /// Server-to-client result batches.
    Result,
}

/// The value of a schema's `graphcal` metadata key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaMetadata {
    /// The protocol version.
    pub protocol_version: u32,
    /// The fingerprint of the interface the session was configured over.
    pub interface_fingerprint: InterfaceFingerprint,
    /// Which stream the schema describes.
    pub stream: StreamKind,
}

/// The value of a field's `graphcal` metadata key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case", deny_unknown_fields)]
pub enum FieldMetadata {
    /// A varied parameter of a request.
    Input {
        /// The wire type of its values.
        wire: WireType,
        /// The declared domain.
        domain: Option<Domain>,
    },
    /// A read output, a child of the result's `outputs` struct.
    Output {
        /// The wire type of its values.
        wire: WireType,
    },
    /// The result's struct of read outputs.
    Outputs,
    /// The result's struct of row outcomes.
    Outcome,
}
