//! Arrow encoding of session schemas and batches.
//!
//! A session exchanges two Arrow IPC streams: the client writes request
//! batches and the server writes one result batch per request. Each schema
//! and field carries one `graphcal` metadata key whose value is JSON
//! serialized from this crate's types.
//!
//! The request schema has one non-nullable field per varied parameter. The
//! result schema has two struct fields, so no port name can collide with a
//! protocol field:
//!
//! - `outputs`: one nullable child per read output, null exactly in failed
//!   rows;
//! - `outcome`: `status` (`UInt8`, see [`RowStatus`](crate::RowStatus)),
//!   `subject` (the failed assertion or out-of-domain parameter), and
//!   `message` (the diagnostic).
//!
//! An application can also store a session record, such as an experiment
//! log: its own fields beside a non-nullable `inputs` struct (one child per
//! varied parameter) and an `outputs` struct laid out as in a result. The
//! record's schema metadata lets [`RecordSchema::decode`] restore the session
//! schema without the model.
//!
//! Wire types map to `Float64` (quantity), `Int64` (int), `Boolean` (bool),
//! and `Dictionary<Int32, Utf8>` (key). Received keys are resolved by their
//! dictionary value, never by dictionary code.

mod metadata;
mod output;
mod record;
mod request;
mod result;
mod schema;
mod value_array;

pub use metadata::{FieldMetadata, METADATA_KEY, SchemaMetadata, StreamKind};
pub use output::OutputDecodeError;
pub use record::{
    RecordPart, RecordSchema, RecordSchemaError, record_inputs_field, record_metadata,
    record_outputs_field,
};
pub use request::RequestDecodeError;
pub use result::ResultDecodeError;
pub use schema::{
    SchemaMismatch, input_fields, output_fields, request_schema, result_schema, si_scales,
    validate_schema,
};
pub use value_array::ValueDecodeError;
