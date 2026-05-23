//! Attribute names recognized by the language.

use thiserror::Error;

/// Known attribute names in the language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributeName {
    Assumes,
    ExpectedFail,
    Lazy,
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
    pub fn new(raw: impl Into<String>) -> Self {
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

impl std::str::FromStr for AttributeName {
    type Err = UnknownAttributeName;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "assumes" => Ok(Self::Assumes),
            "expected_fail" => Ok(Self::ExpectedFail),
            "lazy" => Ok(Self::Lazy),
            _ => Err(UnknownAttributeName::new(s)),
        }
    }
}
