//! Single values carried by the protocol.

use crate::finite::FiniteF64;
use crate::label::VariantName;
use crate::wire_type::WireType;

/// One owned value of a supported wire type.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A finite real quantity in the port's wire unit.
    Quantity(FiniteF64),
    /// A signed integer.
    Int(i64),
    /// A Boolean.
    Bool(bool),
    /// A categorical variant, identified by name.
    Key(VariantName),
}

impl Value {
    /// Whether this value has the shape of `wire`, including membership of a
    /// key variant in the key's variant list.
    #[must_use]
    pub fn conforms_to(&self, wire: &WireType) -> bool {
        match (self, wire) {
            (Self::Quantity(_), WireType::Quantity(_))
            | (Self::Int(_), WireType::Int)
            | (Self::Bool(_), WireType::Bool) => true,
            (Self::Key(variant), WireType::Key(key)) => key.position(variant.as_str()).is_some(),
            _ => false,
        }
    }
}

/// One borrowed value of a supported wire type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ValueRef<'a> {
    /// A finite real quantity in the port's wire unit.
    Quantity(FiniteF64),
    /// A signed integer.
    Int(i64),
    /// A Boolean.
    Bool(bool),
    /// A categorical variant.
    Key(&'a VariantName),
}

impl ValueRef<'_> {
    /// An owned copy of this value.
    #[must_use]
    pub fn to_owned_value(self) -> Value {
        match self {
            Self::Quantity(value) => Value::Quantity(value),
            Self::Int(value) => Value::Int(value),
            Self::Bool(value) => Value::Bool(value),
            Self::Key(variant) => Value::Key(variant.clone()),
        }
    }
}
