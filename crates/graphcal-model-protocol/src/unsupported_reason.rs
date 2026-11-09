//! Why the protocol cannot carry a port.

use serde::{Deserialize, Serialize};

/// The reason a port is listed as unsupported, as a stable code.
///
/// The interface fingerprint covers the code, not the accompanying
/// human-readable message, so rewording a message keeps fingerprints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnsupportedReason {
    /// A dimension exponent does not fit the protocol's rational exponents.
    UnrepresentableExponent,
    /// A dimension has no canonical unit, because a base dimension declares
    /// no base unit.
    NoCanonicalUnit,
    /// A coordinate index has a non-finite coordinate.
    NonFiniteCoordinate,
    /// An index axis is empty or too large for an Arrow list.
    InvalidAxis,
    /// An algebraic type has no constructors or too many.
    InvalidAlgebraicType,
    /// A dimension or an index is declared where no module path names it.
    UnnamedDeclaration,
    /// Two distinct base dimensions of one dimension have the same wire
    /// identity.
    AmbiguousBaseDimension,
}
