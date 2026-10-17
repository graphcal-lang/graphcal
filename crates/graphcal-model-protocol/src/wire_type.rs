//! The value types a port can carry on the wire.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::dimension::Dimension;
use crate::label::{IndexName, UnitSpelling, VariantName};

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

/// The position of a variant within a [`KeyType`]'s declaration-order list.
///
/// A position is only meaningful together with the key type it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VariantPosition(usize);

impl VariantPosition {
    pub(crate) const fn new(position: usize) -> Self {
        Self(position)
    }

    /// The zero-based declaration-order position.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }
}

/// A categorical key over the variants of one named index.
///
/// Variants are listed in declaration order. Their positions are not
/// semantic identities on the wire: values are always resolved by name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "KeyTypeRepr", into = "KeyTypeRepr")]
pub struct KeyType {
    index: IndexName,
    variants: Vec<VariantName>,
}

/// Why a variant list does not form a [`KeyType`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum KeyTypeError {
    #[error("index `{index}` has no variants")]
    Empty { index: IndexName },
    #[error("index `{index}` repeats variant `{variant}`")]
    DuplicateVariant {
        index: IndexName,
        variant: VariantName,
    },
    #[error("index `{index}` has {count} variants, more than an Int32 dictionary can encode")]
    TooManyVariants { index: IndexName, count: usize },
}

impl KeyType {
    /// A key over `variants` of `index`, in declaration order.
    ///
    /// # Errors
    ///
    /// Returns [`KeyTypeError`] for an empty or repeated variant list, or one
    /// too long for an Int32 dictionary.
    pub fn new(index: IndexName, variants: Vec<VariantName>) -> Result<Self, KeyTypeError> {
        if variants.is_empty() {
            return Err(KeyTypeError::Empty { index });
        }
        if i32::try_from(variants.len()).is_err() {
            return Err(KeyTypeError::TooManyVariants {
                index,
                count: variants.len(),
            });
        }
        let mut seen = HashSet::with_capacity(variants.len());
        if let Some(variant) = variants.iter().find(|variant| !seen.insert(*variant)) {
            return Err(KeyTypeError::DuplicateVariant {
                variant: variant.clone(),
                index,
            });
        }
        Ok(Self { index, variants })
    }

    /// The index the key ranges over.
    #[must_use]
    pub const fn index(&self) -> &IndexName {
        &self.index
    }

    /// The variants in declaration order.
    #[must_use]
    pub fn variants(&self) -> &[VariantName] {
        &self.variants
    }

    /// Every variant with its position, in declaration order.
    pub fn entries(&self) -> impl ExactSizeIterator<Item = (VariantPosition, &VariantName)> {
        self.variants
            .iter()
            .enumerate()
            .map(|(position, variant)| (VariantPosition::new(position), variant))
    }

    /// The position of the variant spelled `name`.
    #[must_use]
    pub fn position(&self, name: &str) -> Option<VariantPosition> {
        self.variants
            .iter()
            .position(|variant| variant.as_str() == name)
            .map(VariantPosition::new)
    }

    /// The variant at `position`, when it is in range.
    #[must_use]
    pub fn variant(&self, position: VariantPosition) -> Option<&VariantName> {
        self.variants.get(position.get())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyTypeRepr {
    index: IndexName,
    variants: Vec<VariantName>,
}

impl TryFrom<KeyTypeRepr> for KeyType {
    type Error = KeyTypeError;

    fn try_from(repr: KeyTypeRepr) -> Result<Self, Self::Error> {
        Self::new(repr.index, repr.variants)
    }
}

impl From<KeyType> for KeyTypeRepr {
    fn from(key: KeyType) -> Self {
        Self {
            index: key.index,
            variants: key.variants,
        }
    }
}

/// A value type the protocol can carry.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "WireRepr", into = "WireRepr")]
pub enum WireType {
    /// A finite real quantity (Arrow `Float64`).
    Quantity(QuantityType),
    /// A signed 64-bit integer (Arrow `Int64`).
    Int,
    /// A Boolean (Arrow `Boolean`).
    Bool,
    /// A categorical key (Arrow `Dictionary<Int32, Utf8>`).
    Key(KeyType),
}

/// How one port of a model interface travels on the wire.
///
/// Ports whose type the protocol cannot carry are still listed, with the
/// reason, so they are never silently omitted.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    Int {},
    Bool {},
    Key(KeyType),
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
            WireRepr::Int {} => Self::Supported(WireType::Int),
            WireRepr::Bool {} => Self::Supported(WireType::Bool),
            WireRepr::Key(key) => Self::Supported(WireType::Key(key)),
            WireRepr::Unsupported { reason } => Self::Unsupported { reason },
        }
    }
}

impl From<PortWire> for WireRepr {
    fn from(wire: PortWire) -> Self {
        match wire {
            PortWire::Supported(WireType::Quantity(quantity)) => Self::Quantity(quantity),
            PortWire::Supported(WireType::Int) => Self::Int {},
            PortWire::Supported(WireType::Bool) => Self::Bool {},
            PortWire::Supported(WireType::Key(key)) => Self::Key(key),
            PortWire::Unsupported { reason } => Self::Unsupported { reason },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dimension::Exponent;
    use crate::label::BaseDimensionName;

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
    fn key_types_reject_repeated_variants() {
        let index = IndexName::new("Mode").unwrap();
        assert!(matches!(
            KeyType::new(index.clone(), vec![variant("A"), variant("A")]),
            Err(KeyTypeError::DuplicateVariant { .. })
        ));
        assert!(matches!(
            KeyType::new(index.clone(), Vec::new()),
            Err(KeyTypeError::Empty { .. })
        ));
        let key = KeyType::new(index, vec![variant("B"), variant("A")]).unwrap();
        assert_eq!(key.position("A").map(VariantPosition::get), Some(1));
        let entries = key
            .entries()
            .map(|(position, variant)| (position.get(), variant.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(entries, [(0, "B"), (1, "A")]);
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
}
