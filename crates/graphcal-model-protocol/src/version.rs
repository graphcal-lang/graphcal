//! The protocol version.

/// The protocol version this crate implements.
///
/// Every change to the describe document, session schema, or Arrow encoding,
/// including an added field, increments it. Peers never negotiate: a version
/// mismatch is an error.
pub const PROTOCOL_VERSION: u32 = 2;
