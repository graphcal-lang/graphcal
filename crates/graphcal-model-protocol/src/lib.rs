//! Wire vocabulary of the Graphcal batch model-evaluation protocol.
//!
//! A model exposes a [`ModelInterface`]: its parameters and public outputs
//! with their wire types, units, and domains. `graphcal model describe`
//! publishes it as a [`DescribeDocument`]. A client selects the parameters it
//! varies and the outputs it reads with a [`SessionSpec`]; configuring the
//! interface yields the [`SessionSchema`] that both sides of one
//! `graphcal model serve` process share. Request and result batches
//! ([`RequestBatch`], [`ResultBatch`]) are validated against that schema.
//!
//! This crate is pure: it performs no I/O and does not depend on the Graphcal
//! compiler, so non-Graphcal model implementations can describe themselves
//! with the same vocabulary. The optional `arrow` feature adds the Arrow
//! schema and record-batch encoding used on the wire.

#[cfg(feature = "arrow")]
pub mod arrow;
mod batch;
mod describe;
mod dimension;
mod domain;
mod fingerprint;
mod finite;
mod interface;
mod label;
mod outcome;
mod session;
mod value;
mod version;
mod wire_type;

pub use batch::{
    InputColumn, OutputColumn, RequestBatch, RequestBatchError, ResultBatch, ResultBatchBuilder,
    ResultBatchError, RowValues,
};
pub use describe::{DescribeDocument, DescribeDocumentError};
pub use dimension::{Dimension, Exponent, ExponentError};
pub use domain::{Bounds, BoundsError, Domain};
pub use fingerprint::{InterfaceFingerprint, InterfaceFingerprintError};
pub use finite::{FiniteF64, NonFiniteError};
pub use interface::{
    InterfaceError, ModelInterface, OutputDescription, ParameterDescription, ParameterError,
};
pub use label::{BaseDimensionName, IndexName, LabelError, PortName, UnitSpelling, VariantName};
pub use outcome::{RowFailure, RowOutcome, RowStatus, UnknownRowStatus};
pub use session::{
    InputSlot, OutputSlot, SessionError, SessionInput, SessionOutput, SessionSchema, SessionSpec,
};
pub use value::{Value, ValueRef};
pub use version::PROTOCOL_VERSION;
pub use wire_type::{
    KeyType, KeyTypeError, PortWire, QuantityType, QuantityTypeError, VariantPosition, WireType,
};
