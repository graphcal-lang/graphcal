//! Opaque, validated labels carried on the wire.
//!
//! Each label kind is its own type so a variant name cannot stand in for a
//! port name. Labels carry no hidden structure; they are only required to be
//! non-empty and free of control characters.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Why a string is not a valid label.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LabelError {
    #[error("a {kind} must not be empty")]
    Empty { kind: &'static str },
    #[error("{kind} `{label}` contains a control character")]
    ControlCharacter { kind: &'static str, label: String },
}

fn validate(kind: &'static str, label: String) -> Result<String, LabelError> {
    if label.is_empty() {
        Err(LabelError::Empty { kind })
    } else if label.chars().any(char::is_control) {
        Err(LabelError::ControlCharacter { kind, label })
    } else {
        Ok(label)
    }
}

macro_rules! label_type {
    ($(#[$meta:meta])* $name:ident, $kind:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            /// Validate a label.
            ///
            /// # Errors
            ///
            /// Returns [`LabelError`] for an empty label or one containing a
            /// control character.
            pub fn new(label: impl Into<String>) -> Result<Self, LabelError> {
                validate($kind, label.into()).map(Self)
            }

            /// The label text.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = LabelError;

            fn try_from(label: String) -> Result<Self, Self::Error> {
                Self::new(label)
            }
        }

        impl From<$name> for String {
            fn from(label: $name) -> Self {
                label.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

label_type!(
    /// The name of a model parameter or output port.
    PortName,
    "port name"
);
label_type!(
    /// The name of a categorical index.
    IndexName,
    "index name"
);
label_type!(
    /// The name of one variant of a categorical index.
    VariantName,
    "variant name"
);
label_type!(
    /// The name of a base dimension, such as `Length`.
    BaseDimensionName,
    "base dimension name"
);
label_type!(
    /// The human-readable name of a concrete algebraic type, such as
    /// `Orbit` or `Pair<Length, Mass>`.
    TypeName,
    "type name"
);
label_type!(
    /// The name of one constructor of an algebraic type.
    ConstructorName,
    "constructor name"
);
label_type!(
    /// The name of one field of a constructor.
    FieldName,
    "field name"
);
label_type!(
    /// The source spelling of a unit expression, such as `kg*m/s^2`.
    ///
    /// The spelling is compared exactly; it is never normalized.
    UnitSpelling,
    "unit spelling"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_reject_empty_and_control_characters() {
        assert_eq!(
            PortName::new(""),
            Err(LabelError::Empty { kind: "port name" })
        );
        assert!(matches!(
            VariantName::new("a\nb"),
            Err(LabelError::ControlCharacter { .. })
        ));
        assert_eq!(PortName::new("load").unwrap().as_str(), "load");
    }

    #[test]
    fn labels_validate_when_deserialized() {
        assert!(serde_json::from_str::<PortName>("\"\"").is_err());
        let name: PortName = serde_json::from_str("\"load\"").unwrap();
        assert_eq!(serde_json::to_string(&name).unwrap(), "\"load\"");
    }
}
