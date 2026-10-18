//! Arrow types and arrays of one wire type, including nested values.
//!
//! A nested array is never null below its top level: under a null row every
//! child slot holds a placeholder of its type, so non-nullable child fields
//! stay valid. The one nullable child is a constructor struct inside a
//! tagged union, which is non-null exactly in the rows that use that
//! constructor.

use std::sync::Arc;

use arrow_array::builder::NullBufferBuilder;
use arrow_array::types::Int32Type;
use arrow_array::{
    Array, ArrayRef, BooleanArray, DictionaryArray, FixedSizeListArray, Float64Array, Int32Array,
    Int64Array, StringArray, StructArray, UInt32Array,
};
use arrow_schema::{ArrowError, DataType, Field, Fields};
use thiserror::Error;

use crate::axis::{AxisKind, KeyRef, KeyValue};
use crate::finite::FiniteF64;
use crate::label::VariantName;
use crate::value::{ComplexValue, Value};
use crate::wire_type::{AlgebraicType, ConstructorPosition, ConstructorType};
use crate::wire_type::{KeyPosition, KeyType, WireType};

const LIST_ITEM_FIELD: &str = "item";
const REAL_FIELD: &str = "re";
const IMAGINARY_FIELD: &str = "im";
const CONSTRUCTOR_FIELD: &str = "constructor";
const VARIANTS_FIELD: &str = "variants";

/// Why an Arrow array does not hold values of a wire type.
///
/// Rows are rows of the top-level column.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ValueDecodeError {
    #[error("does not have its Arrow type")]
    ArrowType,
    #[error("has a non-finite value at row {row}")]
    NonFinite { row: usize },
    #[error("has a null inside its value at row {row}")]
    Null { row: usize },
    #[error("has unknown variant `{name}` at row {row}")]
    UnknownVariant { name: String, row: usize },
    #[error("has position {position} outside its axis at row {row}")]
    UnknownPosition { position: u32, row: usize },
    #[error("has coordinate {coordinate} that is not on its axis at row {row}")]
    UnknownCoordinate { coordinate: f64, row: usize },
    #[error("has unknown constructor `{name}` at row {row}")]
    UnknownConstructor { name: String, row: usize },
    #[error("has an invalid dictionary key at row {row}")]
    DictionaryKey { row: usize },
    #[error("does not mark exactly its constructor's fields as present at row {row}")]
    ConstructorPresence { row: usize },
}

impl ValueDecodeError {
    /// The same error with its row mapped by `outer`.
    fn map_row(self, outer: impl Fn(usize) -> usize) -> Self {
        match self {
            Self::ArrowType => Self::ArrowType,
            Self::NonFinite { row } => Self::NonFinite { row: outer(row) },
            Self::Null { row } => Self::Null { row: outer(row) },
            Self::UnknownVariant { name, row } => Self::UnknownVariant {
                name,
                row: outer(row),
            },
            Self::UnknownPosition { position, row } => Self::UnknownPosition {
                position,
                row: outer(row),
            },
            Self::UnknownCoordinate { coordinate, row } => Self::UnknownCoordinate {
                coordinate,
                row: outer(row),
            },
            Self::UnknownConstructor { name, row } => Self::UnknownConstructor {
                name,
                row: outer(row),
            },
            Self::DictionaryKey { row } => Self::DictionaryKey { row: outer(row) },
            Self::ConstructorPresence { row } => Self::ConstructorPresence { row: outer(row) },
        }
    }
}

fn dictionary_type() -> DataType {
    DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))
}

fn complex_fields() -> Fields {
    Fields::from(vec![
        Field::new(REAL_FIELD, DataType::Float64, false),
        Field::new(IMAGINARY_FIELD, DataType::Float64, false),
    ])
}

fn list_item_field(element: &WireType) -> Arc<Field> {
    Arc::new(Field::new(LIST_ITEM_FIELD, data_type(element), false))
}

fn constructor_fields(constructor: &ConstructorType) -> Fields {
    constructor
        .fields()
        .iter()
        .map(|field| Field::new(field.name().as_str(), data_type(field.wire()), false))
        .collect()
}

fn variant_fields(algebraic: &AlgebraicType) -> Fields {
    algebraic
        .constructors()
        .iter()
        .map(|constructor| {
            Field::new(
                constructor.name().as_str(),
                DataType::Struct(constructor_fields(constructor)),
                true,
            )
        })
        .collect()
}

fn union_fields(algebraic: &AlgebraicType) -> Fields {
    Fields::from(vec![
        Field::new(CONSTRUCTOR_FIELD, dictionary_type(), false),
        Field::new(
            VARIANTS_FIELD,
            DataType::Struct(variant_fields(algebraic)),
            false,
        ),
    ])
}

/// The Arrow type of `wire`'s values.
pub(super) fn data_type(wire: &WireType) -> DataType {
    match wire {
        WireType::Quantity(_) | WireType::Datetime(_) => DataType::Float64,
        WireType::Complex(_) => DataType::Struct(complex_fields()),
        WireType::Int => DataType::Int64,
        WireType::Bool => DataType::Boolean,
        WireType::Key(key) => match key.axis().kind() {
            AxisKind::Named { .. } => dictionary_type(),
            AxisKind::Finite { .. } => DataType::UInt32,
            AxisKind::Coordinate { .. } => DataType::Float64,
        },
        WireType::Indexed(indexed) => DataType::FixedSizeList(
            list_item_field(indexed.element()),
            axis_size(indexed.axis().len()),
        ),
        WireType::Algebraic(algebraic) => match algebraic.constructors() {
            [record] => DataType::Struct(constructor_fields(record)),
            _ => DataType::Struct(union_fields(algebraic)),
        },
    }
}

/// `Axis` and `AlgebraicType` guarantee that their sizes fit in `i32`, so
/// also in `u32`.
fn axis_size(len: usize) -> i32 {
    i32::try_from(len).unwrap_or(i32::MAX)
}

/// A value of `wire` that stands in under a null row.
fn placeholder(wire: &WireType) -> Value {
    let zero = FiniteF64::ZERO;
    match wire {
        WireType::Quantity(_) => Value::Quantity(zero),
        WireType::Datetime(_) => Value::Datetime(zero),
        WireType::Complex(_) => Value::Complex(ComplexValue { re: zero, im: zero }),
        WireType::Int => Value::Int(0),
        WireType::Bool => Value::Bool(false),
        WireType::Key(key) => Value::Key(
            key.positions()
                .next()
                .and_then(|position| key.key(position))
                .map_or(KeyValue::Position(0), KeyRef::to_key_value),
        ),
        WireType::Indexed(indexed) => {
            Value::Indexed(vec![placeholder(indexed.element()); indexed.axis().len()])
        }
        WireType::Algebraic(algebraic) => Value::Algebraic {
            constructor: ConstructorPosition::new(0),
            fields: algebraic.constructors()[0]
                .fields()
                .iter()
                .map(|field| placeholder(field.wire()))
                .collect(),
        },
    }
}

/// A builder of the null buffer for `len` rows marked by `valid` (all valid
/// when `None`); `finish` yields no buffer when every row is valid.
fn nulls(len: usize, valid: Option<&[bool]>) -> NullBufferBuilder {
    let mut builder = NullBufferBuilder::new(len);
    match valid {
        None => builder.append_n_non_nulls(len),
        Some(valid) => valid.iter().for_each(|valid| builder.append(*valid)),
    }
    builder
}

/// An Arrow array of `values`, each conforming to `wire`, null where `valid`
/// is `false`. Invalid rows still hold a conforming value.
///
/// # Errors
///
/// Returns an [`ArrowError`] when a value does not conform to `wire`, which
/// callers rule out beforehand.
pub(super) fn encode(
    wire: &WireType,
    values: &[Value],
    valid: Option<&[bool]>,
) -> Result<ArrayRef, ArrowError> {
    let mismatch =
        || ArrowError::InvalidArgumentError("a value does not conform to its wire type".to_owned());
    let is_valid = |row: usize| valid.is_none_or(|valid| valid[row]);
    Ok(match wire {
        WireType::Quantity(_) | WireType::Datetime(_) => {
            let numbers = values
                .iter()
                .enumerate()
                .map(|(row, value)| match value {
                    Value::Quantity(value) | Value::Datetime(value) => {
                        Ok(is_valid(row).then_some(value.get()))
                    }
                    _ => Err(mismatch()),
                })
                .collect::<Result<Float64Array, _>>()?;
            Arc::new(numbers)
        }
        WireType::Int => {
            let numbers = values
                .iter()
                .enumerate()
                .map(|(row, value)| match value {
                    Value::Int(value) => Ok(is_valid(row).then_some(*value)),
                    _ => Err(mismatch()),
                })
                .collect::<Result<Int64Array, _>>()?;
            Arc::new(numbers)
        }
        WireType::Bool => {
            let booleans = values
                .iter()
                .enumerate()
                .map(|(row, value)| match value {
                    Value::Bool(value) => Ok(is_valid(row).then_some(*value)),
                    _ => Err(mismatch()),
                })
                .collect::<Result<BooleanArray, _>>()?;
            Arc::new(booleans)
        }
        WireType::Key(key) => {
            let positions = values
                .iter()
                .enumerate()
                .map(|(row, value)| match value {
                    Value::Key(value) => key
                        .position(value.as_key_ref())
                        .map(|position| is_valid(row).then_some(position))
                        .ok_or_else(mismatch),
                    _ => Err(mismatch()),
                })
                .collect::<Result<Vec<_>, _>>()?;
            key_array(key, positions.into_iter())?
        }
        WireType::Complex(_) => {
            let (re, im): (Vec<f64>, Vec<f64>) = values
                .iter()
                .map(|value| match value {
                    Value::Complex(value) => Ok((value.re.get(), value.im.get())),
                    _ => Err(mismatch()),
                })
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .unzip();
            Arc::new(StructArray::try_new_with_length(
                complex_fields(),
                vec![
                    Arc::new(Float64Array::from(re)),
                    Arc::new(Float64Array::from(im)),
                ],
                nulls(values.len(), valid).finish(),
                values.len(),
            )?)
        }
        WireType::Indexed(indexed) => {
            let mut entries = Vec::with_capacity(values.len() * indexed.axis().len());
            for value in values {
                match value {
                    Value::Indexed(row) if row.len() == indexed.axis().len() => {
                        entries.extend(row.iter().cloned());
                    }
                    _ => return Err(mismatch()),
                }
            }
            let child = encode(indexed.element(), &entries, None)?;
            Arc::new(FixedSizeListArray::try_new_with_length(
                list_item_field(indexed.element()),
                axis_size(indexed.axis().len()),
                child,
                nulls(values.len(), valid).finish(),
                values.len(),
            )?)
        }
        WireType::Algebraic(algebraic) => encode_algebraic(algebraic, values, valid)?,
    })
}

/// A record's struct of fields, or a union's struct of a constructor tag and
/// one nullable struct per constructor.
fn encode_algebraic(
    algebraic: &AlgebraicType,
    values: &[Value],
    valid: Option<&[bool]>,
) -> Result<ArrayRef, ArrowError> {
    let mismatch =
        || ArrowError::InvalidArgumentError("a value does not conform to its wire type".to_owned());
    let rows = values
        .iter()
        .map(|value| match value {
            Value::Algebraic {
                constructor,
                fields,
            } => Ok((*constructor, fields.as_slice())),
            _ => Err(mismatch()),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(match algebraic.constructors() {
        [record] => Arc::new(encode_constructor(
            record,
            &rows
                .iter()
                .map(|(_, fields)| Some(*fields))
                .collect::<Vec<_>>(),
            valid,
        )?),
        constructors => {
            let tags = DictionaryArray::<Int32Type>::try_new(
                rows.iter()
                    .map(|(constructor, _)| i32::try_from(constructor.get()).ok())
                    .collect::<Int32Array>(),
                Arc::new(StringArray::from_iter_values(
                    constructors
                        .iter()
                        .map(|constructor| constructor.name().as_str()),
                )),
            )?;
            let variants = constructors
                .iter()
                .enumerate()
                .map(|(position, constructor)| {
                    let active = rows
                        .iter()
                        .map(|(chosen, fields)| (chosen.get() == position).then_some(*fields))
                        .collect::<Vec<_>>();
                    let present = active.iter().map(Option::is_some).collect::<Vec<_>>();
                    encode_constructor(constructor, &active, Some(&present))
                        .map(|array| Arc::new(array) as ArrayRef)
                })
                .collect::<Result<Vec<_>, _>>()?;
            Arc::new(StructArray::try_new_with_length(
                union_fields(algebraic),
                vec![
                    Arc::new(tags),
                    Arc::new(StructArray::try_new_with_length(
                        variant_fields(algebraic),
                        variants,
                        None,
                        values.len(),
                    )?),
                ],
                nulls(values.len(), valid).finish(),
                values.len(),
            )?)
        }
    })
}

/// The struct of `constructor`'s fields over `rows`, null where `valid` is
/// `false`; a `None` row holds placeholders.
fn encode_constructor(
    constructor: &ConstructorType,
    rows: &[Option<&[Value]>],
    valid: Option<&[bool]>,
) -> Result<StructArray, ArrowError> {
    let columns = constructor
        .fields()
        .iter()
        .enumerate()
        .map(|(position, field)| {
            let values = rows
                .iter()
                .map(|row| {
                    row.map_or_else(
                        || Ok(placeholder(field.wire())),
                        |fields| {
                            fields.get(position).cloned().ok_or_else(|| {
                                ArrowError::InvalidArgumentError(
                                    "a constructor value lacks a field".to_owned(),
                                )
                            })
                        },
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            encode(field.wire(), &values, None)
        })
        .collect::<Result<Vec<_>, _>>()?;
    StructArray::try_new_with_length(
        constructor_fields(constructor),
        columns,
        nulls(rows.len(), valid).finish(),
        rows.len(),
    )
}

/// The array of `key`'s entries at `positions`: a `Dictionary<Int32,
/// Utf8>` over every variant of a named axis, the `UInt32` positions of a
/// `Fin` axis, or the `Float64` SI coordinates of a coordinate axis.
pub(super) fn key_array(
    key: &KeyType,
    positions: impl Iterator<Item = Option<KeyPosition>>,
) -> Result<ArrayRef, ArrowError> {
    let positions = positions
        .map(|position| {
            position
                .map(|position| {
                    (position.get() < key.axis().len())
                        .then(|| position.get())
                        .ok_or_else(|| {
                            ArrowError::InvalidArgumentError(format!(
                                "key position {} is outside its axis",
                                position.get()
                            ))
                        })
                })
                .transpose()
        })
        .collect::<Result<Vec<_>, _>>()?;
    // `Axis` guarantees that every position fits in `i32`, so also in `u32`.
    Ok(match key.axis().kind() {
        AxisKind::Named { variants } => Arc::new(DictionaryArray::<Int32Type>::try_new(
            positions
                .iter()
                .map(|position| {
                    position.map(|position| i32::try_from(position).unwrap_or(i32::MAX))
                })
                .collect::<Int32Array>(),
            Arc::new(StringArray::from_iter_values(
                variants.iter().map(VariantName::as_str),
            )),
        )?),
        AxisKind::Finite { .. } => Arc::new(
            positions
                .iter()
                .map(|position| {
                    position.map(|position| u32::try_from(position).unwrap_or(u32::MAX))
                })
                .collect::<UInt32Array>(),
        ),
        AxisKind::Coordinate { coordinates, .. } => Arc::new(
            positions
                .iter()
                .map(|position| position.map(|position| coordinates[position].get()))
                .collect::<Float64Array>(),
        ),
    })
}

/// The positions of the keys in `array`, a [`key_array`] of `key`; a null
/// row decodes as `None`.
///
/// # Errors
///
/// Returns [`ValueDecodeError`] for another Arrow type, an invalid
/// dictionary code, or a key that selects no entry of the axis.
pub(super) fn decode_key_positions(
    key: &KeyType,
    array: &dyn Array,
) -> Result<Vec<Option<KeyPosition>>, ValueDecodeError> {
    let lookup = |row: usize, entry: KeyRef<'_>| {
        key.position(entry).ok_or_else(|| match entry {
            KeyRef::Variant(variant) => ValueDecodeError::UnknownVariant {
                name: variant.as_str().to_owned(),
                row,
            },
            KeyRef::Position(position) => ValueDecodeError::UnknownPosition {
                position: u32::try_from(position).unwrap_or(u32::MAX),
                row,
            },
            KeyRef::Coordinate(coordinate) => ValueDecodeError::UnknownCoordinate {
                coordinate: coordinate.get(),
                row,
            },
        })
    };
    match key.axis().kind() {
        AxisKind::Named { .. } => Ok(decode_dictionary(array, |label| {
            VariantName::new(label)
                .ok()
                .and_then(|variant| key.position(KeyRef::Variant(&variant)))
                .map(KeyPosition::get)
        })
        .map_err(|error| match error {
            DictionaryError::ArrowType => ValueDecodeError::ArrowType,
            DictionaryError::Key { row } => ValueDecodeError::DictionaryKey { row },
            DictionaryError::Unknown { name, row } => {
                ValueDecodeError::UnknownVariant { name, row }
            }
        })?
        .into_iter()
        .map(|position| position.map(KeyPosition::new))
        .collect()),
        AxisKind::Finite { .. } => downcast::<UInt32Array>(array)?
            .iter()
            .enumerate()
            .map(|(row, position)| {
                position
                    .map(|position| {
                        lookup(
                            row,
                            KeyRef::Position(usize::try_from(position).unwrap_or(usize::MAX)),
                        )
                    })
                    .transpose()
            })
            .collect(),
        AxisKind::Coordinate { .. } => downcast::<Float64Array>(array)?
            .iter()
            .enumerate()
            .map(|(row, coordinate)| {
                coordinate
                    .map(|coordinate| {
                        let coordinate = FiniteF64::new(coordinate)
                            .map_err(|_| ValueDecodeError::NonFinite { row })?;
                        lookup(row, KeyRef::Coordinate(coordinate))
                    })
                    .transpose()
            })
            .collect(),
    }
}

/// An entry of a `Dictionary<Int32, Utf8>` that resolves to no label.
pub(super) enum DictionaryError {
    /// The array is not a string dictionary.
    ArrowType,
    /// A code at `row` points outside the dictionary.
    Key { row: usize },
    /// The label at `row` is not resolvable.
    Unknown { name: String, row: usize },
}

/// Resolve every entry of a string dictionary array by its label; a null row
/// decodes as `None`.
pub(super) fn decode_dictionary(
    array: &dyn Array,
    resolve: impl Fn(&str) -> Option<usize>,
) -> Result<Vec<Option<usize>>, DictionaryError> {
    let dictionary = array
        .as_any()
        .downcast_ref::<DictionaryArray<Int32Type>>()
        .ok_or(DictionaryError::ArrowType)?;
    let labels = dictionary
        .values()
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or(DictionaryError::ArrowType)?;
    // Each dictionary entry is resolved once.
    let resolved = (0..labels.len())
        .map(|entry| {
            (!labels.is_null(entry))
                .then(|| resolve(labels.value(entry)))
                .flatten()
        })
        .collect::<Vec<_>>();
    dictionary
        .keys()
        .iter()
        .enumerate()
        .map(|(row, code)| {
            let Some(code) = code else {
                return Ok(None);
            };
            let entry = usize::try_from(code)
                .ok()
                .filter(|entry| *entry < labels.len())
                .ok_or(DictionaryError::Key { row })?;
            resolved[entry]
                .ok_or_else(|| DictionaryError::Unknown {
                    name: if labels.is_null(entry) {
                        "null".to_owned()
                    } else {
                        labels.value(entry).to_owned()
                    },
                    row,
                })
                .map(Some)
        })
        .collect()
}

fn downcast<T: 'static>(array: &dyn Array) -> Result<&T, ValueDecodeError> {
    array
        .as_any()
        .downcast_ref::<T>()
        .ok_or(ValueDecodeError::ArrowType)
}

/// Require a value in every row the parent marks valid.
fn require_values(
    values: Vec<Option<Value>>,
    parent_valid: impl Fn(usize) -> bool,
) -> Result<Vec<Value>, ValueDecodeError> {
    values
        .into_iter()
        .enumerate()
        .map(|(row, value)| match value {
            Some(value) => Ok(value),
            None if parent_valid(row) => Err(ValueDecodeError::Null { row }),
            // Placeholder slots under a null parent are never read.
            None => Ok(Value::Bool(false)),
        })
        .collect()
}

/// The values of `array` as values of `wire`, `None` in null rows.
///
/// # Errors
///
/// Returns [`ValueDecodeError`] for another Arrow type, a non-finite number,
/// a null inside a value, an unknown variant or constructor, or a tagged
/// union whose present variant does not match its tag.
pub(super) fn decode(
    wire: &WireType,
    array: &dyn Array,
) -> Result<Vec<Option<Value>>, ValueDecodeError> {
    let finite = |row: usize, value: f64| {
        FiniteF64::new(value).map_err(|_| ValueDecodeError::NonFinite { row })
    };
    Ok(match wire {
        WireType::Quantity(_) | WireType::Datetime(_) => {
            let datetime = matches!(wire, WireType::Datetime(_));
            downcast::<Float64Array>(array)?
                .iter()
                .enumerate()
                .map(|(row, value)| {
                    value
                        .map(|value| {
                            finite(row, value).map(|value| {
                                if datetime {
                                    Value::Datetime(value)
                                } else {
                                    Value::Quantity(value)
                                }
                            })
                        })
                        .transpose()
                })
                .collect::<Result<_, _>>()?
        }
        WireType::Int => downcast::<Int64Array>(array)?
            .iter()
            .map(|value| value.map(Value::Int))
            .collect(),
        WireType::Bool => downcast::<BooleanArray>(array)?
            .iter()
            .map(|value| value.map(Value::Bool))
            .collect(),
        WireType::Key(key) => decode_key_positions(key, array)?
            .into_iter()
            .map(|position| {
                position
                    .and_then(|position| key.key(position))
                    .map(|key| Value::Key(key.to_key_value()))
            })
            .collect(),
        WireType::Complex(_) => {
            let parts = downcast::<StructArray>(array)?;
            let [re, im] = parts.columns() else {
                return Err(ValueDecodeError::ArrowType);
            };
            let (re, im) = (downcast::<Float64Array>(re)?, downcast::<Float64Array>(im)?);
            (0..parts.len())
                .map(|row| {
                    if parts.is_null(row) {
                        return Ok(None);
                    }
                    if re.is_null(row) || im.is_null(row) {
                        return Err(ValueDecodeError::Null { row });
                    }
                    Ok(Some(Value::Complex(ComplexValue {
                        re: finite(row, re.value(row))?,
                        im: finite(row, im.value(row))?,
                    })))
                })
                .collect::<Result<_, _>>()?
        }
        WireType::Indexed(indexed) => {
            let list = downcast::<FixedSizeListArray>(array)?;
            let size = indexed.axis().len();
            if usize::try_from(list.value_length()).ok() != Some(size) {
                return Err(ValueDecodeError::ArrowType);
            }
            let entries = decode(indexed.element(), list.values().as_ref())
                .map_err(|error| error.map_row(|row| row / size))?;
            let entries = require_values(entries, |entry| list.is_valid(entry / size))
                .map_err(|error| error.map_row(|row| row / size))?;
            entries
                .chunks(size)
                .enumerate()
                .map(|(row, chunk)| list.is_valid(row).then(|| Value::Indexed(chunk.to_vec())))
                .collect()
        }
        WireType::Algebraic(algebraic) => decode_algebraic(algebraic, array)?,
    })
}

/// The values of a record's or a union's struct array.
fn decode_algebraic(
    algebraic: &AlgebraicType,
    array: &dyn Array,
) -> Result<Vec<Option<Value>>, ValueDecodeError> {
    let structure = downcast::<StructArray>(array)?;
    Ok(match algebraic.constructors() {
        [record] => decode_constructor(record, structure, |row| structure.is_valid(row))?
            .into_iter()
            .enumerate()
            .map(|(row, fields)| {
                structure.is_valid(row).then(|| Value::Algebraic {
                    constructor: ConstructorPosition::new(0),
                    fields,
                })
            })
            .collect(),
        constructors => {
            let [tags, variants] = structure.columns() else {
                return Err(ValueDecodeError::ArrowType);
            };
            let tags = decode_dictionary(tags.as_ref(), |label| {
                algebraic.position(label).map(ConstructorPosition::get)
            })
            .map_err(|error| match error {
                DictionaryError::ArrowType => ValueDecodeError::ArrowType,
                DictionaryError::Key { row } => ValueDecodeError::DictionaryKey { row },
                DictionaryError::Unknown { name, row } => {
                    ValueDecodeError::UnknownConstructor { name, row }
                }
            })?;
            let variants = downcast::<StructArray>(variants.as_ref())?;
            if variants.num_columns() != constructors.len() {
                return Err(ValueDecodeError::ArrowType);
            }
            let mut decoded = Vec::with_capacity(constructors.len());
            for (constructor, column) in constructors.iter().zip(variants.columns()) {
                let column = downcast::<StructArray>(column.as_ref())?;
                decoded.push((
                    column,
                    decode_constructor(constructor, column, |row| column.is_valid(row))?,
                ));
            }
            (0..structure.len())
                .map(|row| {
                    if structure.is_null(row) {
                        return Ok(None);
                    }
                    let tag = tags[row].ok_or(ValueDecodeError::Null { row })?;
                    let present = decoded
                        .iter()
                        .enumerate()
                        .filter(|(_, (column, _))| column.is_valid(row))
                        .map(|(position, _)| position)
                        .collect::<Vec<_>>();
                    if present != [tag] {
                        return Err(ValueDecodeError::ConstructorPresence { row });
                    }
                    Ok(Some(Value::Algebraic {
                        constructor: ConstructorPosition::new(tag),
                        fields: decoded[tag].1[row].clone(),
                    }))
                })
                .collect::<Result<_, _>>()?
        }
    })
}

/// The field values of `constructor` in every row of `structure`, requiring
/// values in the rows `valid` marks.
fn decode_constructor(
    constructor: &ConstructorType,
    structure: &StructArray,
    valid: impl Fn(usize) -> bool,
) -> Result<Vec<Vec<Value>>, ValueDecodeError> {
    if structure.num_columns() != constructor.fields().len() {
        return Err(ValueDecodeError::ArrowType);
    }
    let mut rows = vec![Vec::with_capacity(constructor.fields().len()); structure.len()];
    for (field, column) in constructor.fields().iter().zip(structure.columns()) {
        let values = require_values(decode(field.wire(), column.as_ref())?, &valid)?;
        for (row, value) in rows.iter_mut().zip(values) {
            row.push(value);
        }
    }
    Ok(rows)
}

/// Values for a top-level column of `wire`, with placeholders and a
/// validity mask for `None` rows.
pub(super) fn with_placeholders(
    wire: &WireType,
    values: impl Iterator<Item = Option<Value>>,
) -> (Vec<Value>, Vec<bool>) {
    values
        .map(|value| value.map_or_else(|| (placeholder(wire), false), |value| (value, true)))
        .unzip()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::axis::{Axis, AxisKind};
    use crate::batch::{InputColumn, RequestBatch, ResultBatch, ResultBatchBuilder};
    use crate::datetime_type::{DatetimeType, TimeScale};
    use crate::domain::{Bounds, Domain};
    use crate::interface::tests::{length, name};
    use crate::interface::{ModelInterface, OutputDescription, ParameterDescription};
    use crate::label::{ConstructorName, FieldName, IndexName, PortName, TypeName, UnitSpelling};
    use crate::outcome::{RowFailure, RowOutcome};
    use crate::session::{SessionError, SessionSchema, SessionSpec};
    use crate::unit_scale::SiScale;
    use crate::wire_type::PortWire;
    use crate::wire_type::{AlgebraicType, ConstructorType, FieldType, IndexedType};

    fn finite(value: f64) -> FiniteF64 {
        FiniteF64::new(value).unwrap()
    }

    fn phases() -> Axis {
        Axis::new(
            IndexName::new("Phase").unwrap(),
            AxisKind::Named {
                variants: vec![
                    VariantName::new("Climb").unwrap(),
                    VariantName::new("Cruise").unwrap(),
                ],
            },
        )
        .unwrap()
    }

    /// `Length[Phase][Fin(3)]`.
    fn profile() -> WireType {
        let steps = Axis::new(
            IndexName::new("Fin(3)").unwrap(),
            AxisKind::Finite { size: 3 },
        )
        .unwrap();
        WireType::Indexed(IndexedType::new(
            steps,
            WireType::Indexed(IndexedType::new(phases(), length())),
        ))
    }

    /// `Shape = Circle(r: Length) | Dot`.
    fn shape() -> WireType {
        WireType::Algebraic(
            AlgebraicType::new(
                TypeName::new("Shape").unwrap(),
                vec![
                    ConstructorType::new(
                        ConstructorName::new("Circle").unwrap(),
                        vec![FieldType::new(FieldName::new("r").unwrap(), length())],
                    ),
                    ConstructorType::new(ConstructorName::new("Dot").unwrap(), Vec::new()),
                ],
            )
            .unwrap(),
        )
    }

    /// `Pair { at: Datetime<TAI>, z: Complex }`.
    fn pair() -> WireType {
        WireType::Algebraic(
            AlgebraicType::new(
                TypeName::new("Pair").unwrap(),
                vec![ConstructorType::new(
                    ConstructorName::new("Pair").unwrap(),
                    vec![
                        FieldType::new(
                            FieldName::new("at").unwrap(),
                            WireType::Datetime(DatetimeType::new(TimeScale::Tai)),
                        ),
                        FieldType::new(
                            FieldName::new("z").unwrap(),
                            WireType::Complex(
                                crate::wire_type::QuantityType::new(
                                    crate::dimension::Dimension::dimensionless(),
                                    None,
                                )
                                .unwrap(),
                            ),
                        ),
                    ],
                )],
            )
            .unwrap(),
        )
    }

    fn configure(units: &[(&str, &str)]) -> Result<SessionSchema, SessionError> {
        let parameter = |port: &str, wire: WireType, domain| {
            ParameterDescription::new(name(port), port, PortWire::Supported(wire), domain, false)
                .unwrap()
        };
        let interface = ModelInterface::new(
            vec![
                parameter(
                    "profile",
                    profile(),
                    Some(Domain::Quantity(
                        Bounds::new(Some(finite(0.0)), None).unwrap(),
                    )),
                ),
                parameter("shape", shape(), None),
                parameter(
                    "at",
                    WireType::Datetime(DatetimeType::new(TimeScale::Utc)),
                    Some(Domain::Datetime(
                        Bounds::new(None, Some(finite(1e9))).unwrap(),
                    )),
                ),
            ],
            vec![
                OutputDescription::new(name("profile_out"), "p", PortWire::Supported(profile())),
                OutputDescription::new(name("shape_out"), "s", PortWire::Supported(shape())),
                OutputDescription::new(name("pair_out"), "q", PortWire::Supported(pair())),
            ],
        )
        .unwrap();
        let spec = units.iter().fold(
            SessionSpec::new(
                vec![name("profile"), name("shape"), name("at")],
                vec![name("profile_out"), name("shape_out"), name("pair_out")],
            ),
            |spec, (port, unit)| spec.with_unit(name(port), UnitSpelling::new(*unit).unwrap()),
        );
        interface.configure(&spec, |_: &PortName, _: &UnitSpelling| {
            SiScale::new(1000.0).map_err(|error| error.to_string())
        })
    }

    fn session(units: &[(&str, &str)]) -> Arc<SessionSchema> {
        Arc::new(configure(units).unwrap())
    }

    fn profile_value(offset: f64) -> Value {
        Value::Indexed(
            (0..3)
                .map(|step| {
                    Value::Indexed(vec![
                        Value::Quantity(finite(offset + f64::from(step))),
                        Value::Quantity(finite(offset + 10.0 + f64::from(step))),
                    ])
                })
                .collect(),
        )
    }

    fn circle(radius: f64) -> Value {
        Value::Algebraic {
            constructor: ConstructorPosition::new(0),
            fields: vec![Value::Quantity(finite(radius))],
        }
    }

    fn dot() -> Value {
        Value::Algebraic {
            constructor: ConstructorPosition::new(1),
            fields: Vec::new(),
        }
    }

    #[test]
    fn rich_requests_round_trip() {
        let schema = session(&[]);
        let batch = RequestBatch::new(
            Arc::clone(&schema),
            vec![
                InputColumn::Structured(vec![profile_value(0.0), profile_value(100.0)]),
                InputColumn::Structured(vec![circle(2.0), dot()]),
                InputColumn::Datetime(vec![finite(-5.5), finite(8.0e8)]),
            ],
        )
        .unwrap();
        let encoded = batch.to_record_batch().unwrap();
        assert!(matches!(
            encoded.schema().field(0).data_type(),
            DataType::FixedSizeList(_, 3)
        ));
        let decoded = RequestBatch::from_record_batch(schema, &encoded).unwrap();
        assert_eq!(decoded, batch);
    }

    #[test]
    fn rich_results_round_trip_with_failed_rows() {
        let schema = session(&[]);
        let pair_value = Value::Algebraic {
            constructor: ConstructorPosition::new(0),
            fields: vec![
                Value::Datetime(finite(12.5)),
                Value::Complex(ComplexValue {
                    re: finite(1.0),
                    im: finite(-2.0),
                }),
            ],
        };
        let mut builder = ResultBatchBuilder::new(Arc::clone(&schema));
        builder
            .push(RowOutcome::Success(vec![
                profile_value(1.0),
                dot(),
                pair_value.clone(),
            ]))
            .unwrap();
        builder
            .push(RowOutcome::Failure(RowFailure::EvaluationError {
                message: "boom".to_owned(),
            }))
            .unwrap();
        builder
            .push(RowOutcome::Success(vec![
                profile_value(2.0),
                circle(3.0),
                pair_value,
            ]))
            .unwrap();
        let batch = builder.finish().unwrap();
        let encoded = batch.to_record_batch().unwrap();
        let decoded = ResultBatch::from_record_batch(schema, &encoded).unwrap();
        assert_eq!(decoded, batch);
    }

    #[test]
    fn fin_and_coordinate_keys_travel_as_positions_and_coordinates() {
        let fin = KeyType::new(
            Axis::new(
                IndexName::new("Fin(3)").unwrap(),
                AxisKind::Finite { size: 3 },
            )
            .unwrap(),
        );
        let grid = KeyType::new(
            Axis::new(
                IndexName::new("Step").unwrap(),
                AxisKind::Coordinate {
                    dimension: crate::dimension::Dimension::dimensionless(),
                    coordinates: vec![finite(0.0), finite(0.25)],
                },
            )
            .unwrap(),
        );
        let fin_wire = WireType::Key(fin);
        let grid_wire = WireType::Key(grid);
        assert_eq!(data_type(&fin_wire), DataType::UInt32);
        assert_eq!(data_type(&grid_wire), DataType::Float64);

        let fin_values = [
            Value::Key(KeyValue::Position(2)),
            Value::Key(KeyValue::Position(0)),
        ];
        let encoded = encode(&fin_wire, &fin_values, Some(&[true, false])).unwrap();
        assert_eq!(
            encoded.as_any().downcast_ref::<UInt32Array>().unwrap(),
            &UInt32Array::from(vec![Some(2), None])
        );
        assert_eq!(
            decode(&fin_wire, encoded.as_ref()).unwrap(),
            [Some(fin_values[0].clone()), None]
        );

        let grid_values = [Value::Key(KeyValue::Coordinate(finite(0.25)))];
        let encoded = encode(&grid_wire, &grid_values, None).unwrap();
        assert_eq!(
            decode(&grid_wire, encoded.as_ref()).unwrap(),
            [Some(grid_values[0].clone())]
        );

        assert!(encode(&fin_wire, &[Value::Key(KeyValue::Position(3))], None).is_err());
        assert_eq!(
            decode(&fin_wire, &UInt32Array::from(vec![3])),
            Err(ValueDecodeError::UnknownPosition {
                position: 3,
                row: 0
            })
        );
        assert_eq!(
            decode(&grid_wire, &Float64Array::from(vec![0.0, 0.2])),
            Err(ValueDecodeError::UnknownCoordinate {
                coordinate: 0.2,
                row: 1
            })
        );
        assert_eq!(
            decode(&grid_wire, &Float64Array::from(vec![f64::NAN])),
            Err(ValueDecodeError::NonFinite { row: 0 })
        );
        assert!(!Value::Key(KeyValue::Position(0)).conforms_to(&grid_wire));
    }

    #[test]
    fn decoding_rejects_nulls_inside_values_and_mismatched_constructors() {
        let element = Arc::new(Float64Array::from(vec![Some(1.0), None]));
        let list = FixedSizeListArray::try_new_with_length(
            Arc::new(Field::new(LIST_ITEM_FIELD, DataType::Float64, true)),
            2,
            element,
            None,
            1,
        )
        .unwrap();
        let wire = WireType::Indexed(IndexedType::new(phases(), length()));
        assert_eq!(decode(&wire, &list), Err(ValueDecodeError::Null { row: 0 }));

        let encoded = encode(&shape(), &[circle(1.0)], None).unwrap();
        let union = encoded.as_any().downcast_ref::<StructArray>().unwrap();
        let tags = DictionaryArray::<Int32Type>::try_new(
            Int32Array::from(vec![1]),
            Arc::new(StringArray::from(vec!["Circle", "Dot"])),
        )
        .unwrap();
        let mislabeled = StructArray::try_new(
            union.fields().clone(),
            vec![Arc::new(tags), Arc::clone(union.column(1))],
            None,
        )
        .unwrap();
        assert_eq!(
            decode(&shape(), &mislabeled),
            Err(ValueDecodeError::ConstructorPresence { row: 0 })
        );
    }

    #[test]
    fn units_rescale_indexed_quantities_and_reject_algebraic_ports() {
        let schema = session(&[("profile", "km")]);
        assert_eq!(
            schema.inputs()[0]
                .wire()
                .quantity()
                .and_then(|quantity| quantity.unit())
                .map(UnitSpelling::as_str),
            Some("km")
        );
        let Some(Domain::Quantity(bounds)) = schema.inputs()[0].domain() else {
            panic!("profile has an element domain");
        };
        assert_eq!(bounds.min().map(|min| min.get()), Some(0.0));
        assert_eq!(
            configure(&[("shape", "km")]),
            Err(SessionError::UnitForNonQuantity {
                name: name("shape")
            })
        );
    }
}
