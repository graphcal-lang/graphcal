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
//! Wire types map to `Float64` (quantity), `Int64` (int), `Boolean` (bool),
//! and `Dictionary<Int32, Utf8>` (key). Received keys are resolved by their
//! dictionary value, never by dictionary code.

mod metadata;
mod request;
mod result;
mod schema;
mod value_array;

pub use metadata::{FieldMetadata, METADATA_KEY, SchemaMetadata, StreamKind};
pub use request::RequestDecodeError;
pub use result::ResultDecodeError;
pub use schema::{SchemaMismatch, request_schema, result_schema, si_scales, validate_schema};
pub use value_array::ValueDecodeError;
