//! Typed newtype wrappers for string identifiers.
//!
//! Each newtype represents a distinct semantic category of name in the graphcal
//! language, preventing accidental mixing at compile time.

/// Macro to define a newtype wrapper around `String` with standard trait impls.
macro_rules! define_name_type {
    (
        $(#[$meta:meta])*
        $vis:vis struct $Name:ident;
    ) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        $vis struct $Name(String);

        impl std::fmt::Debug for $Name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                // Delegate to the inner String's Debug so that Vec<$Name> formats
                // as ["foo", "bar"] rather than [TypeName("foo"), TypeName("bar")].
                std::fmt::Debug::fmt(&self.0, f)
            }
        }

        impl $Name {
            /// Create a new name from a string.
            #[must_use]
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }

            /// Get the underlying string slice.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Consume and return the inner `String`.
            #[must_use]
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl std::fmt::Display for $Name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl AsRef<str> for $Name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl std::borrow::Borrow<str> for $Name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl From<String> for $Name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<&str> for $Name {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }
    };
}

define_name_type! {
    /// Name of a const, param, or node declaration.
    pub struct DeclName;
}

define_name_type! {
    /// Name of a dimension (e.g., `"Length"`, `"Velocity"`).
    pub struct DimName;
}

define_name_type! {
    /// Name of a unit (e.g., `"m"`, `"km"`, `"hour"`).
    pub struct UnitName;
}

define_name_type! {
    /// Name of a struct type (e.g., `"TransferResult"`).
    pub struct StructTypeName;
}

define_name_type! {
    /// Name of an index type (e.g., `"Maneuver"`).
    pub struct IndexName;
}

define_name_type! {
    /// Name of a function (e.g., `"sqrt"`, `"lerp"`).
    pub struct FnName;
}

define_name_type! {
    /// Name of a struct field (e.g., `"dv1"`, `"altitude"`).
    pub struct FieldName;
}

define_name_type! {
    /// Name of an index variant (e.g., `"Departure"`, `"Correction"`).
    pub struct VariantName;
}

define_name_type! {
    /// Name of a generic type parameter (e.g., `"D"`, `"I"`).
    pub struct GenericParamName;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn newtype_display() {
        let name = DeclName::new("dry_mass");
        assert_eq!(format!("{name}"), "dry_mass");
    }

    #[test]
    fn newtype_as_str() {
        let name = DimName::new("Length");
        assert_eq!(name.as_str(), "Length");
    }

    #[test]
    fn newtype_into_inner() {
        let name = UnitName::new("km");
        assert_eq!(name.into_inner(), "km");
    }

    #[test]
    fn newtype_hash_map_borrow_lookup() {
        let mut map = HashMap::new();
        map.insert(DeclName::new("x"), 42);
        // Lookup with &str via Borrow<str>
        assert_eq!(map.get("x"), Some(&42));
    }

    #[test]
    fn newtype_from_string() {
        let name: FieldName = "dv1".to_string().into();
        assert_eq!(name.as_str(), "dv1");
    }

    #[test]
    fn newtype_from_str() {
        let name: VariantName = "Departure".into();
        assert_eq!(name.as_str(), "Departure");
    }

    #[test]
    fn newtype_equality() {
        assert_eq!(IndexName::new("Maneuver"), IndexName::new("Maneuver"));
        assert_ne!(IndexName::new("Maneuver"), IndexName::new("Phase"));
    }

    #[test]
    fn newtype_ord() {
        let a = FnName::new("alpha");
        let b = FnName::new("beta");
        assert!(a < b);
    }
}
