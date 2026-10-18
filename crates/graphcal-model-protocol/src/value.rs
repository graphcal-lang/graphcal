//! Single values carried by the protocol.

use crate::axis::{KeyRef, KeyValue};
use crate::finite::FiniteF64;
use crate::wire_type::ConstructorPosition;
use crate::wire_type::WireType;

/// A finite complex number in Cartesian form.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ComplexValue {
    /// The real part.
    pub re: FiniteF64,
    /// The imaginary part.
    pub im: FiniteF64,
}

/// One owned value of a supported wire type.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A finite real quantity in the port's wire unit.
    Quantity(FiniteF64),
    /// A finite complex quantity in the port's wire unit.
    Complex(ComplexValue),
    /// A signed integer.
    Int(i64),
    /// A Boolean.
    Bool(bool),
    /// A key of an axis entry.
    Key(KeyValue),
    /// Seconds since J2000 in the port's time scale.
    Datetime(FiniteF64),
    /// One value per axis entry, in axis order.
    Indexed(Vec<Self>),
    /// A constructor of an algebraic type with its fields in declaration
    /// order.
    Algebraic {
        /// The constructor's position within the type.
        constructor: ConstructorPosition,
        /// The field values.
        fields: Vec<Self>,
    },
}

impl Value {
    /// Whether this value has the shape of `wire`: the same kind at every
    /// level, a key of an entry of the key's axis, one entry per axis entry,
    /// and a constructor of the type with one value per field.
    #[must_use]
    pub fn conforms_to(&self, wire: &WireType) -> bool {
        match (self, wire) {
            (Self::Quantity(_), WireType::Quantity(_))
            | (Self::Complex(_), WireType::Complex(_))
            | (Self::Int(_), WireType::Int)
            | (Self::Bool(_), WireType::Bool)
            | (Self::Datetime(_), WireType::Datetime(_)) => true,
            (Self::Key(value), WireType::Key(key)) => key.position(value.as_key_ref()).is_some(),
            (Self::Indexed(entries), WireType::Indexed(indexed)) => {
                entries.len() == indexed.axis().len()
                    && entries
                        .iter()
                        .all(|entry| entry.conforms_to(indexed.element()))
            }
            (
                Self::Algebraic {
                    constructor,
                    fields,
                },
                WireType::Algebraic(algebraic),
            ) => algebraic
                .constructor(*constructor)
                .is_some_and(|constructor| {
                    constructor.fields().len() == fields.len()
                        && constructor
                            .fields()
                            .iter()
                            .zip(fields)
                            .all(|(field, value)| value.conforms_to(field.wire()))
                }),
            _ => false,
        }
    }

    /// A borrowed view of this value.
    #[must_use]
    pub fn as_value_ref(&self) -> ValueRef<'_> {
        match self {
            Self::Quantity(value) => ValueRef::Quantity(*value),
            Self::Complex(value) => ValueRef::Complex(*value),
            Self::Int(value) => ValueRef::Int(*value),
            Self::Bool(value) => ValueRef::Bool(*value),
            Self::Key(key) => ValueRef::Key(key.as_key_ref()),
            Self::Datetime(value) => ValueRef::Datetime(*value),
            Self::Indexed(entries) => ValueRef::Indexed(entries),
            Self::Algebraic {
                constructor,
                fields,
            } => ValueRef::Algebraic {
                constructor: *constructor,
                fields,
            },
        }
    }
}

/// One borrowed value of a supported wire type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ValueRef<'a> {
    /// A finite real quantity in the port's wire unit.
    Quantity(FiniteF64),
    /// A finite complex quantity in the port's wire unit.
    Complex(ComplexValue),
    /// A signed integer.
    Int(i64),
    /// A Boolean.
    Bool(bool),
    /// A key of an axis entry.
    Key(KeyRef<'a>),
    /// Seconds since J2000 in the port's time scale.
    Datetime(FiniteF64),
    /// One value per axis entry, in axis order.
    Indexed(&'a [Value]),
    /// A constructor with its field values.
    Algebraic {
        /// The constructor's position within the type.
        constructor: ConstructorPosition,
        /// The field values.
        fields: &'a [Value],
    },
}

impl ValueRef<'_> {
    /// An owned copy of this value.
    #[must_use]
    pub fn to_owned_value(self) -> Value {
        match self {
            Self::Quantity(value) => Value::Quantity(value),
            Self::Complex(value) => Value::Complex(value),
            Self::Int(value) => Value::Int(value),
            Self::Bool(value) => Value::Bool(value),
            Self::Key(key) => Value::Key(key.to_key_value()),
            Self::Datetime(value) => Value::Datetime(value),
            Self::Indexed(entries) => Value::Indexed(entries.to_vec()),
            Self::Algebraic {
                constructor,
                fields,
            } => Value::Algebraic {
                constructor,
                fields: fields.to_vec(),
            },
        }
    }
}
