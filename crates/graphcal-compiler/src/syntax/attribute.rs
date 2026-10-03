//! Attribute names recognized by the language.

use thiserror::Error;

/// Attribute names the language gives semantics to.
///
/// Reserved names without semantics (see [`ReservedAttributeName`]) are not
/// part of this vocabulary, so a consumer of an `AttributeName` never has to
/// handle an attribute that cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttributeName {
    Assumes,
    ExpectedFail,
    Hidden,
}

impl AttributeName {
    /// Whether at most one occurrence may be attached to a source target.
    #[must_use]
    pub const fn is_singleton(self) -> bool {
        match self {
            Self::Assumes | Self::ExpectedFail | Self::Hidden => true,
        }
    }
}

impl std::fmt::Display for AttributeName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Assumes => "assumes",
            Self::ExpectedFail => "expected_fail",
            Self::Hidden => "hidden",
        })
    }
}

/// Attribute names reserved by the language but not yet given semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReservedAttributeName {
    Lazy,
}

impl std::fmt::Display for ReservedAttributeName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Lazy => "lazy",
        })
    }
}

/// Error returned when parsing an unknown attribute name.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown attribute `{raw}`")]
pub struct UnknownAttributeName {
    raw: String,
}

impl UnknownAttributeName {
    /// Create an unknown-attribute-name error from the original source text.
    #[must_use]
    fn new(raw: impl Into<String>) -> Self {
        Self { raw: raw.into() }
    }

    /// The unrecognized attribute name text.
    #[must_use]
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// Consume and return the unrecognized attribute name text.
    #[must_use]
    pub fn into_raw(self) -> String {
        self.raw
    }
}

/// Why source text is not a usable [`AttributeName`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AttributeNameError {
    /// The name is not part of the language vocabulary.
    #[error(transparent)]
    Unknown(#[from] UnknownAttributeName),
    /// The name is reserved but has no semantics yet.
    #[error("attribute `{0}` is reserved but not supported")]
    Reserved(ReservedAttributeName),
}

impl std::str::FromStr for AttributeName {
    type Err = AttributeNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "assumes" => Ok(Self::Assumes),
            "expected_fail" => Ok(Self::ExpectedFail),
            "hidden" => Ok(Self::Hidden),
            "lazy" => Err(AttributeNameError::Reserved(ReservedAttributeName::Lazy)),
            _ => Err(UnknownAttributeName::new(s).into()),
        }
    }
}
