//! Spellings of the non-dimension types built into the Graphcal prelude.
//!
//! The parser recognizes these names in type position and gives each its own
//! syntax variant; the reserved-name policy forbids redeclaring them in the
//! Static namespace. Both consult this one table, so the built-in type
//! vocabulary cannot drift between the grammar and the reservation rules.

/// A non-dimension type name provided by the prelude.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinTypeName {
    /// `Dimensionless`.
    Dimensionless,
    /// `Bool`.
    Bool,
    /// `Int`.
    Int,
    /// `Datetime` or `Datetime<Scale>`.
    Datetime,
    /// `Complex<D>`.
    Complex,
    /// `Key<I>`.
    Key,
}

impl BuiltinTypeName {
    /// Every built-in type name, in prelude order.
    pub const ALL: [Self; 6] = [
        Self::Dimensionless,
        Self::Bool,
        Self::Int,
        Self::Datetime,
        Self::Complex,
        Self::Key,
    ];

    /// Source spelling of the type name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dimensionless => "Dimensionless",
            Self::Bool => "Bool",
            Self::Int => "Int",
            Self::Datetime => "Datetime",
            Self::Complex => "Complex",
            Self::Key => "Key",
        }
    }

    /// The built-in type spelled `spelling`, if any.
    #[must_use]
    pub fn parse(spelling: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|name| name.as_str() == spelling)
    }
}

impl std::fmt::Display for BuiltinTypeName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_name_round_trips_through_its_spelling() {
        for name in BuiltinTypeName::ALL {
            assert_eq!(BuiltinTypeName::parse(name.as_str()), Some(name));
            assert_eq!(name.to_string(), name.as_str());
        }
    }

    #[test]
    fn non_builtin_spellings_are_rejected() {
        for spelling in ["Length", "dimensionless", "Fin", "Key2", ""] {
            assert_eq!(BuiltinTypeName::parse(spelling), None);
        }
    }
}
