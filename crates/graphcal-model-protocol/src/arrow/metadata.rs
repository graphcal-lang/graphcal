//! The JSON values of the `graphcal` schema and field metadata keys.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::domain::Domain;
use crate::fingerprint::InterfaceFingerprint;
use crate::label::PortName;
use crate::unit_scale::SiScale;
use crate::wire_type::WireType;

/// The metadata key used on schemas and fields.
pub const METADATA_KEY: &str = "graphcal";

/// Which kind of session stream a schema describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    /// Client-to-server request batches.
    Request,
    /// Server-to-client result batches.
    Result,
    /// A stored session record: request rows joined with their output
    /// values, written by an application rather than exchanged with a
    /// server (see [`RecordSchema`](super::RecordSchema)).
    Record,
}

/// The value of a schema's `graphcal` metadata key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaMetadata {
    /// The protocol version.
    pub protocol_version: u32,
    /// The fingerprint of the interface the session was configured over.
    pub interface_fingerprint: InterfaceFingerprint,
    /// Which stream the schema describes.
    pub stream: StreamKind,
    /// The scale of every port the session carries in a chosen unit.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub si_scales: BTreeMap<PortName, SiScale>,
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
    /// A record's struct of varied parameter values.
    Inputs,
    /// The result's or a record's struct of read outputs.
    Outputs,
    /// The result's struct of row outcomes.
    Outcome,
}
