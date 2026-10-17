//! Value kinds that type port handles.

use std::fmt;

use graphcal_model_protocol::{
    FiniteF64, InputColumn, KeyType, PortName, QuantityType, ValueRef, VariantName,
    VariantPosition, WireType,
};

use crate::error::ClientError;
use crate::identity::PortIdentity;

mod sealed {
    pub trait Sealed {}
}

/// The value kind of a port, chosen by a handle's type parameter.
///
/// The kinds are [`Quantity`], [`Int`], [`Bool`], and [`Key`]; the trait is
/// sealed. A handle can only be obtained for a port of its kind, so the
/// Rust type of every supplied and returned value is fixed at compile time.
pub trait Kind: sealed::Sealed + Sized {
    /// Wire details of the port that a handle keeps.
    type Detail: Clone + fmt::Debug;
    /// The value supplied for one request row.
    type Input;
    /// The value read from one successful result row.
    type Output;

    /// The kind's name, for diagnostics.
    const NAME: &'static str;

    #[doc(hidden)]
    fn detail(wire: &WireType) -> Option<Self::Detail>;

    #[doc(hidden)]
    fn input_column(
        port: &PortIdentity,
        detail: &Self::Detail,
        values: Vec<Self::Input>,
    ) -> Result<InputColumn, ClientError>;

    #[doc(hidden)]
    fn output(value: ValueRef<'_>) -> Option<Self::Output>;
}

/// A real quantity, supplied and returned as `f64` in the port's wire unit.
#[derive(Debug, Clone, Copy)]
pub enum Quantity {}

/// A signed 64-bit integer.
#[derive(Debug, Clone, Copy)]
pub enum Int {}

/// A Boolean.
#[derive(Debug, Clone, Copy)]
pub enum Bool {}

/// A categorical key: supplied as a [`KeyVariant`] of the same port,
/// returned as the variant name.
#[derive(Debug, Clone, Copy)]
pub enum Key {}

impl sealed::Sealed for Quantity {}
impl sealed::Sealed for Int {}
impl sealed::Sealed for Bool {}
impl sealed::Sealed for Key {}

impl Kind for Quantity {
    type Detail = QuantityType;
    type Input = f64;
    type Output = f64;

    const NAME: &'static str = "quantity";

    fn detail(wire: &WireType) -> Option<Self::Detail> {
        match wire {
            WireType::Quantity(quantity) => Some(quantity.clone()),
            WireType::Int | WireType::Bool | WireType::Key(_) => None,
        }
    }

    fn input_column(
        port: &PortIdentity,
        _detail: &Self::Detail,
        values: Vec<Self::Input>,
    ) -> Result<InputColumn, ClientError> {
        values
            .into_iter()
            .enumerate()
            .map(|(row, value)| {
                FiniteF64::new(value).map_err(|_| ClientError::NonFinite {
                    input: port.name().clone(),
                    row,
                })
            })
            .collect::<Result<_, _>>()
            .map(InputColumn::Quantity)
    }

    fn output(value: ValueRef<'_>) -> Option<Self::Output> {
        match value {
            ValueRef::Quantity(value) => Some(value.get()),
            ValueRef::Int(_) | ValueRef::Bool(_) | ValueRef::Key(_) => None,
        }
    }
}

impl Kind for Int {
    type Detail = ();
    type Input = i64;
    type Output = i64;

    const NAME: &'static str = "int";

    fn detail(wire: &WireType) -> Option<Self::Detail> {
        matches!(wire, WireType::Int).then_some(())
    }

    fn input_column(
        _port: &PortIdentity,
        _detail: &Self::Detail,
        values: Vec<Self::Input>,
    ) -> Result<InputColumn, ClientError> {
        Ok(InputColumn::Int(values))
    }

    fn output(value: ValueRef<'_>) -> Option<Self::Output> {
        match value {
            ValueRef::Int(value) => Some(value),
            ValueRef::Quantity(_) | ValueRef::Bool(_) | ValueRef::Key(_) => None,
        }
    }
}

impl Kind for Bool {
    type Detail = ();
    type Input = bool;
    type Output = bool;

    const NAME: &'static str = "bool";

    fn detail(wire: &WireType) -> Option<Self::Detail> {
        matches!(wire, WireType::Bool).then_some(())
    }

    fn input_column(
        _port: &PortIdentity,
        _detail: &Self::Detail,
        values: Vec<Self::Input>,
    ) -> Result<InputColumn, ClientError> {
        Ok(InputColumn::Bool(values))
    }

    fn output(value: ValueRef<'_>) -> Option<Self::Output> {
        match value {
            ValueRef::Bool(value) => Some(value),
            ValueRef::Quantity(_) | ValueRef::Int(_) | ValueRef::Key(_) => None,
        }
    }
}

/// One variant of a key parameter, validated against that parameter.
///
/// Obtain it from [`InputHandle::variant`](crate::InputHandle::variant); a
/// variant of one parameter cannot be supplied for another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyVariant {
    port: PortIdentity,
    position: VariantPosition,
}

impl KeyVariant {
    pub(crate) const fn new(port: PortIdentity, position: VariantPosition) -> Self {
        Self { port, position }
    }

    /// The parameter the variant belongs to.
    #[must_use]
    pub const fn parameter(&self) -> &PortName {
        self.port.name()
    }
}

impl Kind for Key {
    type Detail = KeyType;
    type Input = KeyVariant;
    type Output = VariantName;

    const NAME: &'static str = "key";

    fn detail(wire: &WireType) -> Option<Self::Detail> {
        match wire {
            WireType::Key(key) => Some(key.clone()),
            WireType::Quantity(_) | WireType::Int | WireType::Bool => None,
        }
    }

    fn input_column(
        port: &PortIdentity,
        _detail: &Self::Detail,
        values: Vec<Self::Input>,
    ) -> Result<InputColumn, ClientError> {
        values
            .into_iter()
            .map(|variant| {
                if variant.port == *port {
                    Ok(variant.position)
                } else {
                    Err(ClientError::ForeignVariant {
                        input: port.name().clone(),
                        owner: variant.port.name().clone(),
                    })
                }
            })
            .collect::<Result<_, _>>()
            .map(InputColumn::Key)
    }

    fn output(value: ValueRef<'_>) -> Option<Self::Output> {
        match value {
            ValueRef::Key(variant) => Some(variant.clone()),
            ValueRef::Quantity(_) | ValueRef::Int(_) | ValueRef::Bool(_) => None,
        }
    }
}
