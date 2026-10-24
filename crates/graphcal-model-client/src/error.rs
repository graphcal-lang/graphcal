//! Client errors.

use std::process::ExitStatus;

use graphcal_model_protocol::arrow::{ResultDecodeError, SchemaMismatch};
use graphcal_model_protocol::{KeyValue, PortName, RequestBatchError, SessionError};
use thiserror::Error;

/// Why a client operation failed.
#[derive(Debug, Error)]
pub enum ClientError {
    #[error("cannot run the graphcal executable: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("graphcal model describe failed ({status}): {stderr}")]
    Describe { status: ExitStatus, stderr: String },
    #[error("graphcal model describe printed an invalid document: {0}")]
    DescribeDocument(#[source] serde_json::Error),
    #[error("graphcal model unit failed ({status}): {stderr}")]
    Units { status: ExitStatus, stderr: String },
    #[error("graphcal model unit printed an invalid document: {0}")]
    UnitsDocument(#[source] serde_json::Error),
    #[error("graphcal model unit did not answer the requested units in order")]
    UnitsMismatch,
    #[error("the model has no parameter named `{name}`")]
    UnknownInput { name: String },
    #[error("the model has no public output named `{name}`")]
    UnknownOutput { name: String },
    #[error("port `{name}` cannot be used: {reason}")]
    UnsupportedPort { name: PortName, reason: String },
    #[error("port `{name}` is not a {expected} port")]
    KindMismatch {
        name: PortName,
        expected: &'static str,
    },
    #[error("the handle for port `{name}` belongs to another model")]
    ForeignHandle { name: PortName },
    #[error("index of parameter `{input}` has no variant `{variant}`")]
    UnknownVariant { input: PortName, variant: String },
    #[error("index of parameter `{input}` has no entry at {key}")]
    UnknownKey { input: PortName, key: KeyValue },
    #[error("a key of parameter `{owner}` was supplied for parameter `{input}`")]
    ForeignKey { input: PortName, owner: PortName },
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error("parameter `{name}` is not varied by this session")]
    NotVaried { name: PortName },
    #[error("output `{name}` is not read by this session")]
    NotRead { name: PortName },
    #[error("the column for parameter `{name}` was given more than once")]
    DuplicateColumn { name: PortName },
    #[error("the session varies parameter `{name}`, but no column was given for it")]
    MissingColumn { name: PortName },
    #[error("parameter `{input}` has a non-finite value at row {row}")]
    NonFinite { input: PortName, row: usize },
    #[error("parameter `{input}` has {found} entries at row {row}; its axis has {expected}")]
    EntryCount {
        input: PortName,
        row: usize,
        expected: usize,
        found: usize,
    },
    #[error(transparent)]
    Request(#[from] RequestBatchError),
    #[error("the request or result belongs to another session")]
    ForeignSession,
    #[error("the model server sent an unexpected stream schema: {0}")]
    StreamSchema(#[from] SchemaMismatch),
    #[error("the model server sent an invalid result: {0}")]
    Result(#[from] ResultDecodeError),
    #[error("the model server answered {received} rows to a request of {expected}")]
    RowCount { expected: usize, received: usize },
    #[error("the model server sent a result no request asked for")]
    UnsolicitedResult,
    #[error("the session stopped after an earlier failure")]
    SessionClosed,
    #[error("Arrow IPC failure: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
    #[error("I/O failure talking to the model server: {0}")]
    Io(#[from] std::io::Error),
    #[error("the model server stopped ({status}): {stderr}")]
    ServerStopped { status: ExitStatus, stderr: String },
}
