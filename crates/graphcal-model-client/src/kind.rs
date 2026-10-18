//! Value kinds that type port handles.

use std::fmt;
use std::marker::PhantomData;

use graphcal_model_protocol::{
    AlgebraicType, Axis, ComplexValue, DatetimeType, FiniteF64, InputColumn, KeyType, PortName,
    QuantityType, Value, ValueRef, VariantName, VariantPosition, WireType,
};

use crate::error::ClientError;
use crate::identity::PortIdentity;

mod sealed {
    pub trait Sealed {}
}

/// The value kind of a port, chosen by a handle's type parameter.
///
/// The kinds are [`Quantity`], [`Complex`], [`Int`], [`Bool`], [`Key`],
/// [`Datetime`], [`Indexed`] of an element kind, and [`Algebraic`]; the trait
/// is sealed. A handle can only be obtained for a port of its kind, so the
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

    /// The protocol value of one input, at `row` of the column of `port`.
    #[doc(hidden)]
    fn value(
        port: &PortIdentity,
        detail: &Self::Detail,
        input: Self::Input,
        row: usize,
    ) -> Result<Value, ClientError>;

    /// The output of one protocol value of this kind.
    #[doc(hidden)]
    fn read(value: ValueRef<'_>) -> Option<Self::Output>;

    #[doc(hidden)]
    fn input_column(
        port: &PortIdentity,
        detail: &Self::Detail,
        values: Vec<Self::Input>,
    ) -> Result<InputColumn, ClientError> {
        values
            .into_iter()
            .enumerate()
            .map(|(row, input)| Self::value(port, detail, input, row))
            .collect::<Result<_, _>>()
            .map(InputColumn::Structured)
    }

    #[doc(hidden)]
    #[must_use]
    fn output(value: ValueRef<'_>) -> Option<Self::Output> {
        Self::read(value)
    }
}

/// A kind whose values are quantities, so a session may carry them in a
/// chosen unit: [`Quantity`], [`Complex`], and [`Indexed`] of those.
pub trait QuantityLike: Kind {}

impl QuantityLike for Quantity {}
impl QuantityLike for Complex {}
impl<K: QuantityLike> QuantityLike for Indexed<K> {}

fn finite(port: &PortIdentity, row: usize, value: f64) -> Result<FiniteF64, ClientError> {
    FiniteF64::new(value).map_err(|_| ClientError::NonFinite {
        input: port.name().clone(),
        row,
    })
}

/// A real quantity, supplied and returned as `f64` in the port's wire unit.
#[derive(Debug, Clone, Copy)]
pub enum Quantity {}

/// A complex quantity, supplied and returned as `(re, im)` in the port's
/// wire unit.
#[derive(Debug, Clone, Copy)]
pub enum Complex {}

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

/// A datetime, supplied and returned as seconds since J2000 read in the
/// port's time scale.
#[derive(Debug, Clone, Copy)]
pub enum Datetime {}

/// One value of kind `K` per entry of a fixed axis, supplied and returned
/// as a `Vec` in axis order.
#[derive(Debug, Clone, Copy)]
pub struct Indexed<K>(PhantomData<fn() -> K>);

/// A record or tagged union, supplied and returned as a protocol [`Value`];
/// requests check each value against the port's type.
#[derive(Debug, Clone, Copy)]
pub enum Algebraic {}

impl sealed::Sealed for Quantity {}
impl sealed::Sealed for Complex {}
impl sealed::Sealed for Int {}
impl sealed::Sealed for Bool {}
impl sealed::Sealed for Key {}
impl sealed::Sealed for Datetime {}
impl<K: Kind> sealed::Sealed for Indexed<K> {}
impl sealed::Sealed for Algebraic {}

impl Kind for Quantity {
    type Detail = QuantityType;
    type Input = f64;
    type Output = f64;

    const NAME: &'static str = "quantity";

    fn detail(wire: &WireType) -> Option<Self::Detail> {
        match wire {
            WireType::Quantity(quantity) => Some(quantity.clone()),
            _ => None,
        }
    }

    fn value(
        port: &PortIdentity,
        _detail: &Self::Detail,
        input: Self::Input,
        row: usize,
    ) -> Result<Value, ClientError> {
        finite(port, row, input).map(Value::Quantity)
    }

    fn read(value: ValueRef<'_>) -> Option<Self::Output> {
        match value {
            ValueRef::Quantity(value) => Some(value.get()),
            _ => None,
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
            .map(|(row, value)| finite(port, row, value))
            .collect::<Result<_, _>>()
            .map(InputColumn::Quantity)
    }
}

impl Kind for Complex {
    type Detail = QuantityType;
    type Input = (f64, f64);
    type Output = (f64, f64);

    const NAME: &'static str = "complex";

    fn detail(wire: &WireType) -> Option<Self::Detail> {
        match wire {
            WireType::Complex(quantity) => Some(quantity.clone()),
            _ => None,
        }
    }

    fn value(
        port: &PortIdentity,
        _detail: &Self::Detail,
        (re, im): Self::Input,
        row: usize,
    ) -> Result<Value, ClientError> {
        Ok(Value::Complex(ComplexValue {
            re: finite(port, row, re)?,
            im: finite(port, row, im)?,
        }))
    }

    fn read(value: ValueRef<'_>) -> Option<Self::Output> {
        match value {
            ValueRef::Complex(value) => Some((value.re.get(), value.im.get())),
            _ => None,
        }
    }

    fn input_column(
        port: &PortIdentity,
        detail: &Self::Detail,
        values: Vec<Self::Input>,
    ) -> Result<InputColumn, ClientError> {
        values
            .into_iter()
            .enumerate()
            .map(
                |(row, input)| match Self::value(port, detail, input, row)? {
                    Value::Complex(value) => Ok(value),
                    _ => Err(ClientError::KindMismatch {
                        name: port.name().clone(),
                        expected: Self::NAME,
                    }),
                },
            )
            .collect::<Result<_, _>>()
            .map(InputColumn::Complex)
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

    fn value(
        _port: &PortIdentity,
        _detail: &Self::Detail,
        input: Self::Input,
        _row: usize,
    ) -> Result<Value, ClientError> {
        Ok(Value::Int(input))
    }

    fn read(value: ValueRef<'_>) -> Option<Self::Output> {
        match value {
            ValueRef::Int(value) => Some(value),
            _ => None,
        }
    }

    fn input_column(
        _port: &PortIdentity,
        _detail: &Self::Detail,
        values: Vec<Self::Input>,
    ) -> Result<InputColumn, ClientError> {
        Ok(InputColumn::Int(values))
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

    fn value(
        _port: &PortIdentity,
        _detail: &Self::Detail,
        input: Self::Input,
        _row: usize,
    ) -> Result<Value, ClientError> {
        Ok(Value::Bool(input))
    }

    fn read(value: ValueRef<'_>) -> Option<Self::Output> {
        match value {
            ValueRef::Bool(value) => Some(value),
            _ => None,
        }
    }

    fn input_column(
        _port: &PortIdentity,
        _detail: &Self::Detail,
        values: Vec<Self::Input>,
    ) -> Result<InputColumn, ClientError> {
        Ok(InputColumn::Bool(values))
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

impl Key {
    fn position(port: &PortIdentity, variant: &KeyVariant) -> Result<VariantPosition, ClientError> {
        if variant.port == *port {
            Ok(variant.position)
        } else {
            Err(ClientError::ForeignVariant {
                input: port.name().clone(),
                owner: variant.port.name().clone(),
            })
        }
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
            _ => None,
        }
    }

    fn value(
        port: &PortIdentity,
        detail: &Self::Detail,
        input: Self::Input,
        _row: usize,
    ) -> Result<Value, ClientError> {
        let position = Self::position(port, &input)?;
        detail
            .variant(position)
            .cloned()
            .map(Value::Key)
            .ok_or_else(|| ClientError::KindMismatch {
                name: port.name().clone(),
                expected: Self::NAME,
            })
    }

    fn read(value: ValueRef<'_>) -> Option<Self::Output> {
        match value {
            ValueRef::Key(variant) => Some(variant.clone()),
            _ => None,
        }
    }

    fn input_column(
        port: &PortIdentity,
        _detail: &Self::Detail,
        values: Vec<Self::Input>,
    ) -> Result<InputColumn, ClientError> {
        values
            .iter()
            .map(|variant| Self::position(port, variant))
            .collect::<Result<_, _>>()
            .map(InputColumn::Key)
    }
}

impl Kind for Datetime {
    type Detail = DatetimeType;
    type Input = f64;
    type Output = f64;

    const NAME: &'static str = "datetime";

    fn detail(wire: &WireType) -> Option<Self::Detail> {
        match wire {
            WireType::Datetime(datetime) => Some(*datetime),
            _ => None,
        }
    }

    fn value(
        port: &PortIdentity,
        _detail: &Self::Detail,
        input: Self::Input,
        row: usize,
    ) -> Result<Value, ClientError> {
        finite(port, row, input).map(Value::Datetime)
    }

    fn read(value: ValueRef<'_>) -> Option<Self::Output> {
        match value {
            ValueRef::Datetime(value) => Some(value.get()),
            _ => None,
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
            .map(|(row, value)| finite(port, row, value))
            .collect::<Result<_, _>>()
            .map(InputColumn::Datetime)
    }
}

/// The wire details of an indexed port: its outer axis and the details of
/// its elements.
pub struct IndexedDetail<K: Kind> {
    axis: Axis,
    element: K::Detail,
}

impl<K: Kind> Clone for IndexedDetail<K> {
    fn clone(&self) -> Self {
        Self {
            axis: self.axis.clone(),
            element: self.element.clone(),
        }
    }
}

impl<K: Kind> fmt::Debug for IndexedDetail<K> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IndexedDetail")
            .field("axis", &self.axis)
            .field("element", &self.element)
            .finish()
    }
}

impl<K: Kind> IndexedDetail<K> {
    /// The outer axis.
    #[must_use]
    pub const fn axis(&self) -> &Axis {
        &self.axis
    }

    /// The details of each element.
    #[must_use]
    pub const fn element(&self) -> &K::Detail {
        &self.element
    }
}

impl<K: Kind> Kind for Indexed<K> {
    type Detail = IndexedDetail<K>;
    type Input = Vec<K::Input>;
    type Output = Vec<K::Output>;

    const NAME: &'static str = "indexed";

    fn detail(wire: &WireType) -> Option<Self::Detail> {
        match wire {
            WireType::Indexed(indexed) => Some(IndexedDetail {
                axis: indexed.axis().clone(),
                element: K::detail(indexed.element())?,
            }),
            _ => None,
        }
    }

    fn value(
        port: &PortIdentity,
        detail: &Self::Detail,
        input: Self::Input,
        row: usize,
    ) -> Result<Value, ClientError> {
        if input.len() != detail.axis.len() {
            return Err(ClientError::EntryCount {
                input: port.name().clone(),
                row,
                expected: detail.axis.len(),
                found: input.len(),
            });
        }
        input
            .into_iter()
            .map(|entry| K::value(port, &detail.element, entry, row))
            .collect::<Result<_, _>>()
            .map(Value::Indexed)
    }

    fn read(value: ValueRef<'_>) -> Option<Self::Output> {
        match value {
            ValueRef::Indexed(entries) => entries
                .iter()
                .map(|entry| K::read(entry.as_value_ref()))
                .collect(),
            _ => None,
        }
    }
}

impl Kind for Algebraic {
    type Detail = AlgebraicType;
    type Input = Value;
    type Output = Value;

    const NAME: &'static str = "algebraic";

    fn detail(wire: &WireType) -> Option<Self::Detail> {
        match wire {
            WireType::Algebraic(algebraic) => Some(algebraic.clone()),
            _ => None,
        }
    }

    fn value(
        _port: &PortIdentity,
        _detail: &Self::Detail,
        input: Self::Input,
        _row: usize,
    ) -> Result<Value, ClientError> {
        Ok(input)
    }

    fn read(value: ValueRef<'_>) -> Option<Self::Output> {
        match value {
            ValueRef::Algebraic { .. } => Some(value.to_owned_value()),
            _ => None,
        }
    }
}
