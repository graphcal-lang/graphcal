//! The value types a port can carry on the wire.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::axis::{Axis, KeyRef};
use crate::datetime_type::DatetimeType;
use crate::dimension::Dimension;
use crate::label::{ConstructorName, FieldName, TypeName, UnitSpelling};

/// A real quantity: its dimension and the unit its values are expressed in.
///
/// A dimensioned quantity always names its unit. A dimensionless quantity
/// without a unit is expressed at scale one.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "QuantityTypeRepr", into = "QuantityTypeRepr")]
pub struct QuantityType {
    dimension: Dimension,
    unit: Option<UnitSpelling>,
}

/// Why a dimension and unit do not form a [`QuantityType`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum QuantityTypeError {
    #[error("a dimensioned quantity must name its unit")]
    MissingUnit,
}

impl QuantityType {
    /// A quantity of `dimension` whose values are expressed in `unit`.
    ///
    /// # Errors
    ///
    /// Returns [`QuantityTypeError::MissingUnit`] when a dimensioned quantity
    /// names no unit.
    pub fn new(
        dimension: Dimension,
        unit: Option<UnitSpelling>,
    ) -> Result<Self, QuantityTypeError> {
        if unit.is_none() && !dimension.is_dimensionless() {
            return Err(QuantityTypeError::MissingUnit);
        }
        Ok(Self { dimension, unit })
    }

    /// The physical dimension.
    #[must_use]
    pub const fn dimension(&self) -> &Dimension {
        &self.dimension
    }

    /// The unit values are expressed in; `None` only for a dimensionless
    /// quantity at scale one.
    #[must_use]
    pub const fn unit(&self) -> Option<&UnitSpelling> {
        self.unit.as_ref()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct QuantityTypeRepr {
    dimension: Dimension,
    unit: Option<UnitSpelling>,
}

impl TryFrom<QuantityTypeRepr> for QuantityType {
    type Error = QuantityTypeError;

    fn try_from(repr: QuantityTypeRepr) -> Result<Self, Self::Error> {
        Self::new(repr.dimension, repr.unit)
    }
}

impl From<QuantityType> for QuantityTypeRepr {
    fn from(quantity: QuantityType) -> Self {
        Self {
            dimension: quantity.dimension,
            unit: quantity.unit,
        }
    }
}

/// The position of an entry within a [`KeyType`]'s axis.
///
/// A position is only meaningful together with the key type it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyPosition(usize);

impl KeyPosition {
    pub(crate) const fn new(position: usize) -> Self {
        Self(position)
    }

    /// The zero-based position in axis order.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }
}

/// A key selecting one entry of a fixed axis.
///
/// Keys travel as the protocol identifies entries: a named axis's variant
/// by name, a `Fin` axis's position, or a coordinate axis's coordinate in
/// the canonical SI unit. Positions within a named or coordinate axis are
/// not semantic identities on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyType {
    axis: Axis,
}

impl KeyType {
    /// A key over the entries of `axis`.
    #[must_use]
    pub const fn new(axis: Axis) -> Self {
        Self { axis }
    }

    /// The axis whose entries the key selects.
    #[must_use]
    pub const fn axis(&self) -> &Axis {
        &self.axis
    }

    /// Every position, in axis order.
    pub fn positions(&self) -> impl ExactSizeIterator<Item = KeyPosition> + use<> {
        (0..self.axis.len()).map(KeyPosition::new)
    }

    /// The position of the entry `key` selects, if any.
    #[must_use]
    pub fn position(&self, key: KeyRef<'_>) -> Option<KeyPosition> {
        self.axis.position(key).map(KeyPosition::new)
    }

    /// The key of the entry at `position`, when it is in range.
    #[must_use]
    pub fn key(&self, position: KeyPosition) -> Option<KeyRef<'_>> {
        self.axis.key(position.get())
    }
}

/// One value per entry of a fixed axis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexedType {
    axis: Axis,
    element: Box<WireType>,
}

impl IndexedType {
    /// One `element` per entry of `axis`.
    #[must_use]
    pub fn new(axis: Axis, element: WireType) -> Self {
        Self {
            axis,
            element: Box::new(element),
        }
    }

    /// The outer axis.
    #[must_use]
    pub const fn axis(&self) -> &Axis {
        &self.axis
    }

    /// The type of each entry.
    #[must_use]
    pub fn element(&self) -> &WireType {
        &self.element
    }
}

/// One field of a constructor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldType {
    name: FieldName,
    wire: WireType,
}

impl FieldType {
    /// A field `name` of type `wire`.
    #[must_use]
    pub const fn new(name: FieldName, wire: WireType) -> Self {
        Self { name, wire }
    }

    /// The field name.
    #[must_use]
    pub const fn name(&self) -> &FieldName {
        &self.name
    }

    /// The field's wire type.
    #[must_use]
    pub const fn wire(&self) -> &WireType {
        &self.wire
    }
}

/// One constructor of an algebraic type, with its fields in declaration
/// order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConstructorType {
    name: ConstructorName,
    fields: Vec<FieldType>,
}

impl ConstructorType {
    /// A constructor `name` with `fields`.
    #[must_use]
    pub const fn new(name: ConstructorName, fields: Vec<FieldType>) -> Self {
        Self { name, fields }
    }

    /// The constructor name.
    #[must_use]
    pub const fn name(&self) -> &ConstructorName {
        &self.name
    }

    /// The fields in declaration order.
    #[must_use]
    pub fn fields(&self) -> &[FieldType] {
        &self.fields
    }
}

/// The position of a constructor within an [`AlgebraicType`].
///
/// A position is only meaningful together with the type it came from; on the
/// wire, constructors are resolved by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConstructorPosition(usize);

impl ConstructorPosition {
    /// The constructor at zero-based declaration-order `position`; values
    /// using it are checked against their type
    /// ([`Value::conforms_to`](crate::Value::conforms_to)).
    #[must_use]
    pub const fn new(position: usize) -> Self {
        Self(position)
    }

    /// The zero-based declaration-order position.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }
}

/// A concrete, non-recursive algebraic type: a record (one constructor) or
/// a tagged union (several).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "AlgebraicTypeRepr", into = "AlgebraicTypeRepr")]
pub struct AlgebraicType {
    name: TypeName,
    constructors: Vec<ConstructorType>,
}

/// Why an algebraic type description is invalid.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AlgebraicTypeError {
    #[error("type `{name}` has no constructors")]
    NoConstructors { name: TypeName },
    #[error("type `{name}` repeats constructor `{constructor}`")]
    DuplicateConstructor {
        name: TypeName,
        constructor: ConstructorName,
    },
    #[error("constructor `{constructor}` of type `{name}` repeats field `{field}`")]
    DuplicateField {
        name: TypeName,
        constructor: ConstructorName,
        field: FieldName,
    },
    #[error("type `{name}` has {count} constructors, more than an Int32 dictionary can encode")]
    TooManyConstructors { name: TypeName, count: usize },
}

impl AlgebraicType {
    /// A type `name` with `constructors` in declaration order.
    ///
    /// # Errors
    ///
    /// Returns [`AlgebraicTypeError`] for no constructors, a repeated
    /// constructor, or a repeated field within one constructor.
    pub fn new(
        name: TypeName,
        constructors: Vec<ConstructorType>,
    ) -> Result<Self, AlgebraicTypeError> {
        if constructors.is_empty() {
            return Err(AlgebraicTypeError::NoConstructors { name });
        }
        if i32::try_from(constructors.len()).is_err() {
            return Err(AlgebraicTypeError::TooManyConstructors {
                name,
                count: constructors.len(),
            });
        }
        let mut names = HashSet::with_capacity(constructors.len());
        for constructor in &constructors {
            if !names.insert(constructor.name()) {
                return Err(AlgebraicTypeError::DuplicateConstructor {
                    constructor: constructor.name().clone(),
                    name,
                });
            }
            let mut fields = HashSet::with_capacity(constructor.fields().len());
            if let Some(field) = constructor
                .fields()
                .iter()
                .find(|field| !fields.insert(field.name()))
            {
                return Err(AlgebraicTypeError::DuplicateField {
                    constructor: constructor.name().clone(),
                    field: field.name().clone(),
                    name,
                });
            }
        }
        Ok(Self { name, constructors })
    }

    /// The type name.
    #[must_use]
    pub const fn name(&self) -> &TypeName {
        &self.name
    }

    /// The constructors in declaration order.
    #[must_use]
    pub fn constructors(&self) -> &[ConstructorType] {
        &self.constructors
    }

    /// Every constructor with its position, in declaration order.
    pub fn entries(
        &self,
    ) -> impl ExactSizeIterator<Item = (ConstructorPosition, &ConstructorType)> {
        self.constructors
            .iter()
            .enumerate()
            .map(|(position, constructor)| (ConstructorPosition::new(position), constructor))
    }

    /// The position of the constructor named `name`.
    #[must_use]
    pub fn position(&self, name: &str) -> Option<ConstructorPosition> {
        self.constructors
            .iter()
            .position(|constructor| constructor.name().as_str() == name)
            .map(ConstructorPosition::new)
    }

    /// The constructor at `position`, when it is in range.
    #[must_use]
    pub fn constructor(&self, position: ConstructorPosition) -> Option<&ConstructorType> {
        self.constructors.get(position.get())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AlgebraicTypeRepr {
    name: TypeName,
    constructors: Vec<ConstructorType>,
}

impl TryFrom<AlgebraicTypeRepr> for AlgebraicType {
    type Error = AlgebraicTypeError;

    fn try_from(repr: AlgebraicTypeRepr) -> Result<Self, Self::Error> {
        Self::new(repr.name, repr.constructors)
    }
}

impl From<AlgebraicType> for AlgebraicTypeRepr {
    fn from(algebraic: AlgebraicType) -> Self {
        Self {
            name: algebraic.name,
            constructors: algebraic.constructors,
        }
    }
}

/// A value type the protocol can carry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "WireRepr", into = "WireRepr")]
pub enum WireType {
    /// A finite real quantity (Arrow `Float64`).
    Quantity(QuantityType),
    /// A finite complex quantity (Arrow `Struct{re: Float64, im: Float64}`),
    /// both parts in the same unit.
    Complex(QuantityType),
    /// A signed 64-bit integer (Arrow `Int64`).
    Int,
    /// A Boolean (Arrow `Boolean`).
    Bool,
    /// A categorical key (Arrow `Dictionary<Int32, Utf8>`).
    Key(KeyType),
    /// A datetime instant, seconds since J2000 in its time scale (Arrow
    /// `Float64`).
    Datetime(DatetimeType),
    /// One value per entry of a fixed axis (Arrow `FixedSizeList`).
    Indexed(IndexedType),
    /// A record or tagged union (Arrow `Struct`).
    Algebraic(AlgebraicType),
}

impl WireType {
    /// The quantity type of a quantity-like wire: a quantity, a complex
    /// quantity, or an indexed value whose innermost elements are one of
    /// those.
    #[must_use]
    pub fn quantity(&self) -> Option<&QuantityType> {
        match self {
            Self::Quantity(quantity) | Self::Complex(quantity) => Some(quantity),
            Self::Indexed(indexed) => indexed.element().quantity(),
            Self::Int | Self::Bool | Self::Key(_) | Self::Datetime(_) | Self::Algebraic(_) => None,
        }
    }

    /// This quantity-like wire with its quantities carried in `unit`.
    ///
    /// # Errors
    ///
    /// Returns `None` when the wire is not quantity-like (see
    /// [`Self::quantity`]), or [`QuantityTypeError`] when the quantity cannot
    /// name `unit`.
    pub fn with_unit(&self, unit: &UnitSpelling) -> Option<Result<Self, QuantityTypeError>> {
        let rewrap = |quantity: &QuantityType| {
            QuantityType::new(quantity.dimension().clone(), Some(unit.clone()))
        };
        Some(match self {
            Self::Quantity(quantity) => rewrap(quantity).map(Self::Quantity),
            Self::Complex(quantity) => rewrap(quantity).map(Self::Complex),
            Self::Indexed(indexed) => indexed
                .element()
                .with_unit(unit)?
                .map(|element| Self::Indexed(IndexedType::new(indexed.axis().clone(), element))),
            Self::Int | Self::Bool | Self::Key(_) | Self::Datetime(_) | Self::Algebraic(_) => {
                return None;
            }
        })
    }

    /// The innermost element of an indexed wire, or the wire itself.
    #[must_use]
    pub fn leaf(&self) -> &Self {
        match self {
            Self::Indexed(indexed) => indexed.element().leaf(),
            _ => self,
        }
    }
}

/// How one port of a model interface travels on the wire.
///
/// Ports whose type the protocol cannot carry are still listed, with the
/// reason, so they are never silently omitted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "WireRepr", into = "WireRepr")]
pub enum PortWire {
    /// The port carries values of this type.
    Supported(WireType),
    /// The protocol cannot carry the port's type.
    Unsupported {
        /// Why the type cannot be carried.
        reason: String,
    },
}

impl PortWire {
    /// The wire type, when the port is supported.
    #[must_use]
    pub const fn supported(&self) -> Option<&WireType> {
        match self {
            Self::Supported(wire) => Some(wire),
            Self::Unsupported { .. } => None,
        }
    }
}

/// The JSON shape shared by [`WireType`] and [`PortWire`]. Field-less
/// kinds are empty struct variants so unknown fields are rejected.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum WireRepr {
    Quantity(QuantityType),
    Complex(QuantityType),
    Int {},
    Bool {},
    Key(KeyType),
    Datetime(DatetimeType),
    Indexed(IndexedType),
    Algebraic(AlgebraicType),
    Unsupported { reason: String },
}

/// A port marked unsupported where a supported wire type is required.
#[derive(Debug, Error)]
#[error("expected a supported wire type, found an unsupported port: {reason}")]
pub struct UnsupportedWire {
    reason: String,
}

impl TryFrom<WireRepr> for WireType {
    type Error = UnsupportedWire;

    fn try_from(repr: WireRepr) -> Result<Self, Self::Error> {
        match PortWire::from(repr) {
            PortWire::Supported(wire) => Ok(wire),
            PortWire::Unsupported { reason } => Err(UnsupportedWire { reason }),
        }
    }
}

impl From<WireType> for WireRepr {
    fn from(wire: WireType) -> Self {
        PortWire::Supported(wire).into()
    }
}

impl From<WireRepr> for PortWire {
    fn from(repr: WireRepr) -> Self {
        match repr {
            WireRepr::Quantity(quantity) => Self::Supported(WireType::Quantity(quantity)),
            WireRepr::Complex(quantity) => Self::Supported(WireType::Complex(quantity)),
            WireRepr::Int {} => Self::Supported(WireType::Int),
            WireRepr::Bool {} => Self::Supported(WireType::Bool),
            WireRepr::Key(key) => Self::Supported(WireType::Key(key)),
            WireRepr::Datetime(datetime) => Self::Supported(WireType::Datetime(datetime)),
            WireRepr::Indexed(indexed) => Self::Supported(WireType::Indexed(indexed)),
            WireRepr::Algebraic(algebraic) => Self::Supported(WireType::Algebraic(algebraic)),
            WireRepr::Unsupported { reason } => Self::Unsupported { reason },
        }
    }
}

impl From<PortWire> for WireRepr {
    fn from(wire: PortWire) -> Self {
        match wire {
            PortWire::Supported(WireType::Quantity(quantity)) => Self::Quantity(quantity),
            PortWire::Supported(WireType::Complex(quantity)) => Self::Complex(quantity),
            PortWire::Supported(WireType::Int) => Self::Int {},
            PortWire::Supported(WireType::Bool) => Self::Bool {},
            PortWire::Supported(WireType::Key(key)) => Self::Key(key),
            PortWire::Supported(WireType::Datetime(datetime)) => Self::Datetime(datetime),
            PortWire::Supported(WireType::Indexed(indexed)) => Self::Indexed(indexed),
            PortWire::Supported(WireType::Algebraic(algebraic)) => Self::Algebraic(algebraic),
            PortWire::Unsupported { reason } => Self::Unsupported { reason },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::axis::AxisKind;
    use crate::dimension::Exponent;
    use crate::label::{BaseDimensionName, IndexName, VariantName};

    fn variant(name: &str) -> VariantName {
        VariantName::new(name).unwrap()
    }

    #[test]
    fn dimensioned_quantities_name_their_unit() {
        let length = Dimension::from_iter([(
            BaseDimensionName::new("Length").unwrap(),
            Exponent::new(1, 1).unwrap(),
        )]);
        assert_eq!(
            QuantityType::new(length, None),
            Err(QuantityTypeError::MissingUnit)
        );
        assert!(QuantityType::new(Dimension::dimensionless(), None).is_ok());
    }

    #[test]
    fn key_types_select_axis_entries_and_serialize_their_axis() {
        let axis = Axis::new(
            IndexName::new("Mode").unwrap(),
            AxisKind::Named {
                variants: vec![variant("B"), variant("A")],
            },
        )
        .unwrap();
        let key = KeyType::new(axis);
        let a = variant("A");
        let position = key.position(KeyRef::Variant(&a)).unwrap();
        assert_eq!(position.get(), 1);
        assert_eq!(key.key(position), Some(KeyRef::Variant(&a)));
        assert_eq!(key.positions().len(), 2);

        let json = serde_json::to_string(&WireType::Key(key.clone())).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"key","axis":{"kind":"named","index":"Mode","variants":["B","A"]}}"#
        );
        assert_eq!(
            serde_json::from_str::<WireType>(&json).unwrap(),
            WireType::Key(key)
        );
        assert!(
            serde_json::from_str::<WireType>(
                r#"{"kind":"key","axis":{"kind":"finite","index":"Fin(2)","size":2},"extra":1}"#
            )
            .is_err(),
            "unknown fields are rejected"
        );
    }

    #[test]
    fn port_wire_json_is_one_tagged_object() {
        let unsupported = PortWire::Unsupported {
            reason: "indexed".to_owned(),
        };
        let json = serde_json::to_string(&unsupported).unwrap();
        assert_eq!(json, r#"{"kind":"unsupported","reason":"indexed"}"#);
        assert_eq!(
            serde_json::from_str::<PortWire>(&json).unwrap(),
            unsupported
        );

        let quantity: PortWire =
            serde_json::from_str(r#"{"kind":"quantity","dimension":{"Length":1},"unit":"m"}"#)
                .unwrap();
        assert!(matches!(quantity.supported(), Some(WireType::Quantity(_))));
        assert!(
            serde_json::from_str::<PortWire>(r#"{"kind":"int","extra":1}"#).is_err(),
            "unknown fields are rejected"
        );
        assert_eq!(
            serde_json::to_string(&WireType::Int).unwrap(),
            r#"{"kind":"int"}"#
        );
        assert!(
            serde_json::from_str::<WireType>(r#"{"kind":"unsupported","reason":"x"}"#).is_err(),
            "a wire type is always supported"
        );
    }

    #[test]
    fn algebraic_types_have_unique_constructors_and_fields() {
        let name = TypeName::new("Shape").unwrap();
        let constructor = |name: &str, fields: &[&str]| {
            ConstructorType::new(
                ConstructorName::new(name).unwrap(),
                fields
                    .iter()
                    .map(|field| FieldType::new(FieldName::new(*field).unwrap(), WireType::Int))
                    .collect(),
            )
        };
        assert!(matches!(
            AlgebraicType::new(name.clone(), Vec::new()),
            Err(AlgebraicTypeError::NoConstructors { .. })
        ));
        assert!(matches!(
            AlgebraicType::new(
                name.clone(),
                vec![constructor("A", &[]), constructor("A", &[])]
            ),
            Err(AlgebraicTypeError::DuplicateConstructor { .. })
        ));
        assert!(matches!(
            AlgebraicType::new(name.clone(), vec![constructor("A", &["x", "x"])]),
            Err(AlgebraicTypeError::DuplicateField { .. })
        ));
        let shape = AlgebraicType::new(
            name,
            vec![constructor("Circle", &["r"]), constructor("Dot", &[])],
        )
        .unwrap();
        assert_eq!(shape.position("Dot").map(ConstructorPosition::get), Some(1));
    }
}
