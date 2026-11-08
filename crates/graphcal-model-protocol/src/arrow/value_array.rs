//! Arrow types and arrays of one wire type, including nested values.
//!
//! A null value has no placeholder: every child of a null slot is null too,
//! so a reader that looks at a child array without its parent's validity
//! (pyarrow's `StructArray.field`, `FixedSizeListArray.values`) sees nulls,
//! never values that look valid. Below a column that holds a value in every
//! row, a child is null only inside a tagged union's constructor structs,
//! which are non-null exactly in the rows that use that constructor.

use std::sync::Arc;

use arrow_array::builder::NullBufferBuilder;
use arrow_array::types::Int32Type;
use arrow_array::{
    Array, ArrayRef, BooleanArray, DictionaryArray, FixedSizeListArray, Float64Array, Int32Array,
    Int64Array, StringArray, StructArray, UInt32Array,
};
use arrow_schema::{ArrowError, DataType, Field, Fields};
use thiserror::Error;

use crate::axis::{AxisKind, KeyRef};
use crate::finite::FiniteF64;
use crate::label::VariantName;
use crate::value::{ComplexValue, Value, ValueRef};
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
    #[error("has a value below a null at row {row}")]
    ValueUnderNull { row: usize },
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
            Self::ValueUnderNull { row } => Self::ValueUnderNull { row: outer(row) },
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

/// Whether a column holds a value in every row, or may be null.
///
/// A null value has no placeholder below it: every child of a null slot is
/// null too, so nested children of a column that may be null are nullable.
/// The constructor structs of a tagged union may always be null (every row
/// uses one constructor), and so may everything below them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Presence {
    /// A value in every row (request columns).
    Always,
    /// Null in some rows (output columns, null in rows without values).
    Nullable,
}

impl Presence {
    const fn nullable(self) -> bool {
        matches!(self, Self::Nullable)
    }
}

fn dictionary_type() -> DataType {
    DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))
}

fn complex_fields(presence: Presence) -> Fields {
    Fields::from(vec![
        Field::new(REAL_FIELD, DataType::Float64, presence.nullable()),
        Field::new(IMAGINARY_FIELD, DataType::Float64, presence.nullable()),
    ])
}

fn list_item_field(element: &WireType, presence: Presence) -> Arc<Field> {
    Arc::new(Field::new(
        LIST_ITEM_FIELD,
        data_type(element, presence),
        presence.nullable(),
    ))
}

fn constructor_fields(constructor: &ConstructorType, presence: Presence) -> Fields {
    constructor
        .fields()
        .iter()
        .map(|field| {
            Field::new(
                field.name().as_str(),
                data_type(field.wire(), presence),
                presence.nullable(),
            )
        })
        .collect()
}

fn variant_fields(algebraic: &AlgebraicType) -> Fields {
    algebraic
        .constructors()
        .iter()
        .map(|constructor| {
            Field::new(
                constructor.name().as_str(),
                DataType::Struct(constructor_fields(constructor, Presence::Nullable)),
                true,
            )
        })
        .collect()
}

fn union_fields(algebraic: &AlgebraicType, presence: Presence) -> Fields {
    Fields::from(vec![
        Field::new(CONSTRUCTOR_FIELD, dictionary_type(), presence.nullable()),
        Field::new(
            VARIANTS_FIELD,
            DataType::Struct(variant_fields(algebraic)),
            presence.nullable(),
        ),
    ])
}

/// The Arrow type of `wire`'s values in a column of `presence`.
pub(super) fn data_type(wire: &WireType, presence: Presence) -> DataType {
    match wire {
        WireType::Quantity(_) | WireType::Datetime(_) => DataType::Float64,
        WireType::Complex(_) => DataType::Struct(complex_fields(presence)),
        WireType::Int => DataType::Int64,
        WireType::Bool => DataType::Boolean,
        WireType::Key(key) => match key.axis().kind() {
            AxisKind::Named { .. } => dictionary_type(),
            AxisKind::Finite { .. } => DataType::UInt32,
            AxisKind::Coordinate { .. } => DataType::Float64,
        },
        WireType::Indexed(indexed) => DataType::FixedSizeList(
            list_item_field(indexed.element(), presence),
            axis_size(indexed.axis().len()),
        ),
        WireType::Algebraic(algebraic) => match algebraic.constructors() {
            [record] => DataType::Struct(constructor_fields(record, presence)),
            _ => DataType::Struct(union_fields(algebraic, presence)),
        },
    }
}

/// `Axis` and `AlgebraicType` guarantee that their sizes fit in `i32`, so
/// also in `u32`.
fn axis_size(len: usize) -> i32 {
    i32::try_from(len).unwrap_or(i32::MAX)
}

/// A builder of the null buffer of `rows`; `finish` yields no buffer when
/// every row holds a value.
fn nulls<T>(rows: &[Option<T>]) -> NullBufferBuilder {
    let mut builder = NullBufferBuilder::new(rows.len());
    for row in rows {
        builder.append(row.is_some());
    }
    builder
}

fn mismatch() -> ArrowError {
    ArrowError::InvalidArgumentError("a value does not conform to its wire type".to_owned())
}

/// An Arrow array of `rows`, each a value conforming to `wire` or `None` for
/// a null row, in a column of `presence`. Every child of a null row is null.
///
/// # Errors
///
/// Returns an [`ArrowError`] when a value does not conform to `wire`, or a
/// row is null in a column that holds a value in every row; callers rule
/// both out beforehand.
pub(super) fn encode(
    wire: &WireType,
    rows: &[Option<ValueRef<'_>>],
    presence: Presence,
) -> Result<ArrayRef, ArrowError> {
    Ok(match wire {
        WireType::Quantity(_) | WireType::Datetime(_) => {
            let numbers = rows
                .iter()
                .map(|row| match row {
                    Some(ValueRef::Quantity(value) | ValueRef::Datetime(value)) => {
                        Ok(Some(value.get()))
                    }
                    None => Ok(None),
                    Some(_) => Err(mismatch()),
                })
                .collect::<Result<Float64Array, _>>()?;
            Arc::new(numbers)
        }
        WireType::Int => {
            let numbers = rows
                .iter()
                .map(|row| match row {
                    Some(ValueRef::Int(value)) => Ok(Some(*value)),
                    None => Ok(None),
                    Some(_) => Err(mismatch()),
                })
                .collect::<Result<Int64Array, _>>()?;
            Arc::new(numbers)
        }
        WireType::Bool => {
            let booleans = rows
                .iter()
                .map(|row| match row {
                    Some(ValueRef::Bool(value)) => Ok(Some(*value)),
                    None => Ok(None),
                    Some(_) => Err(mismatch()),
                })
                .collect::<Result<BooleanArray, _>>()?;
            Arc::new(booleans)
        }
        WireType::Key(key) => {
            let positions = rows
                .iter()
                .map(|row| match row {
                    Some(ValueRef::Key(value)) => {
                        key.position(*value).map(Some).ok_or_else(mismatch)
                    }
                    None => Ok(None),
                    Some(_) => Err(mismatch()),
                })
                .collect::<Result<Vec<_>, _>>()?;
            key_array(key, positions.into_iter())?
        }
        WireType::Complex(_) => {
            let parts = rows
                .iter()
                .map(|row| match row {
                    Some(ValueRef::Complex(value)) => Ok(Some(*value)),
                    None => Ok(None),
                    Some(_) => Err(mismatch()),
                })
                .collect::<Result<Vec<_>, _>>()?;
            let part = |get: fn(&ComplexValue) -> FiniteF64| -> ArrayRef {
                Arc::new(
                    parts
                        .iter()
                        .map(|value| value.as_ref().map(|value| get(value).get()))
                        .collect::<Float64Array>(),
                )
            };
            Arc::new(StructArray::try_new_with_length(
                complex_fields(presence),
                vec![part(|value| value.re), part(|value| value.im)],
                nulls(rows).finish(),
                rows.len(),
            )?)
        }
        WireType::Indexed(indexed) => {
            let size = indexed.axis().len();
            let mut entries = Vec::with_capacity(rows.len() * size);
            for row in rows {
                match row {
                    Some(ValueRef::Indexed(values)) if values.len() == size => {
                        entries.extend(values.iter().map(|value| Some(value.as_value_ref())));
                    }
                    None => entries.extend(std::iter::repeat_n(None, size)),
                    Some(_) => return Err(mismatch()),
                }
            }
            let child = encode(indexed.element(), &entries, presence)?;
            Arc::new(FixedSizeListArray::try_new_with_length(
                list_item_field(indexed.element(), presence),
                axis_size(size),
                child,
                nulls(rows).finish(),
                rows.len(),
            )?)
        }
        WireType::Algebraic(algebraic) => encode_algebraic(algebraic, rows, presence)?,
    })
}

/// A record's struct of fields, or a union's struct of a constructor tag and
/// one nullable struct per constructor.
fn encode_algebraic(
    algebraic: &AlgebraicType,
    rows: &[Option<ValueRef<'_>>],
    presence: Presence,
) -> Result<ArrayRef, ArrowError> {
    let rows = rows
        .iter()
        .map(|row| match row {
            Some(ValueRef::Algebraic {
                constructor,
                fields,
            }) => Ok(Some((*constructor, *fields))),
            None => Ok(None),
            Some(_) => Err(mismatch()),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(match algebraic.constructors() {
        [record] => Arc::new(encode_constructor(
            record,
            &rows
                .iter()
                .map(|row| row.map(|(_, fields)| fields))
                .collect::<Vec<_>>(),
            presence,
        )?),
        constructors => {
            let tags = DictionaryArray::<Int32Type>::try_new(
                rows.iter()
                    .map(|row| {
                        row.and_then(|(constructor, _)| i32::try_from(constructor.get()).ok())
                    })
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
                        .map(|row| {
                            row.and_then(|(chosen, fields)| {
                                (chosen.get() == position).then_some(fields)
                            })
                        })
                        .collect::<Vec<_>>();
                    encode_constructor(constructor, &active, Presence::Nullable)
                        .map(|array| Arc::new(array) as ArrayRef)
                })
                .collect::<Result<Vec<_>, _>>()?;
            Arc::new(StructArray::try_new_with_length(
                union_fields(algebraic, presence),
                vec![
                    Arc::new(tags),
                    Arc::new(StructArray::try_new_with_length(
                        variant_fields(algebraic),
                        variants,
                        nulls(&rows).finish(),
                        rows.len(),
                    )?),
                ],
                nulls(&rows).finish(),
                rows.len(),
            )?)
        }
    })
}

/// The struct of `constructor`'s fields over `rows`, null in `None` rows,
/// whose fields are then null too.
fn encode_constructor(
    constructor: &ConstructorType,
    rows: &[Option<&[Value]>],
    presence: Presence,
) -> Result<StructArray, ArrowError> {
    let columns = constructor
        .fields()
        .iter()
        .enumerate()
        .map(|(position, field)| {
            let values = rows
                .iter()
                .map(|row| {
                    row.map(|fields| {
                        fields
                            .get(position)
                            .map(Value::as_value_ref)
                            .ok_or_else(|| {
                                ArrowError::InvalidArgumentError(
                                    "a constructor value lacks a field".to_owned(),
                                )
                            })
                    })
                    .transpose()
                })
                .collect::<Result<Vec<_>, _>>()?;
            encode(field.wire(), &values, presence)
        })
        .collect::<Result<Vec<_>, _>>()?;
    StructArray::try_new_with_length(
        constructor_fields(constructor, presence),
        columns,
        nulls(rows).finish(),
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
        AxisKind::Named { variants, .. } => Arc::new(DictionaryArray::<Int32Type>::try_new(
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

/// `values`, the decoded children of a parent, checked to hold a value
/// exactly where `expected` says the parent does: a child is null under a
/// null parent and holds a value under a valid one.
fn expect_presence(
    values: Vec<Option<Value>>,
    expected: impl Fn(usize) -> bool,
) -> Result<Vec<Option<Value>>, ValueDecodeError> {
    for (row, value) in values.iter().enumerate() {
        match (value.is_some(), expected(row)) {
            (false, true) => return Err(ValueDecodeError::Null { row }),
            (true, false) => return Err(ValueDecodeError::ValueUnderNull { row }),
            _ => {}
        }
    }
    Ok(values)
}

/// The values of `array` as values of `wire` in a column of `presence`,
/// `None` in null rows.
///
/// # Errors
///
/// Returns [`ValueDecodeError`] for another Arrow type, a non-finite number,
/// a null inside a value, a value below a null, an unknown variant or
/// constructor, or a tagged union whose present variant does not match its
/// tag.
pub(super) fn decode(
    wire: &WireType,
    array: &dyn Array,
    presence: Presence,
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
            let part = |column: &ArrayRef| {
                let column = downcast::<Float64Array>(column)?;
                expect_presence(
                    column
                        .iter()
                        .enumerate()
                        .map(|(row, value)| {
                            value
                                .map(|value| finite(row, value).map(Value::Quantity))
                                .transpose()
                        })
                        .collect::<Result<_, _>>()?,
                    |row| parts.is_valid(row),
                )
            };
            part(re)?
                .into_iter()
                .zip(part(im)?)
                .map(|parts| match parts {
                    (Some(Value::Quantity(re)), Some(Value::Quantity(im))) => {
                        Some(Value::Complex(ComplexValue { re, im }))
                    }
                    _ => None,
                })
                .collect()
        }
        WireType::Indexed(indexed) => {
            let list = downcast::<FixedSizeListArray>(array)?;
            let size = indexed.axis().len();
            if usize::try_from(list.value_length()).ok() != Some(size) {
                return Err(ValueDecodeError::ArrowType);
            }
            let entries = decode(indexed.element(), list.values().as_ref(), presence)
                .and_then(|entries| expect_presence(entries, |entry| list.is_valid(entry / size)))
                .map_err(|error| error.map_row(|row| row / size))?;
            entries
                .chunks(size)
                .enumerate()
                .map(|(row, chunk)| {
                    list.is_valid(row)
                        .then(|| Value::Indexed(chunk.iter().flatten().cloned().collect()))
                })
                .collect()
        }
        WireType::Algebraic(algebraic) => decode_algebraic(algebraic, array, presence)?,
    })
}

/// The values of a record's or a union's struct array.
fn decode_algebraic(
    algebraic: &AlgebraicType,
    array: &dyn Array,
    presence: Presence,
) -> Result<Vec<Option<Value>>, ValueDecodeError> {
    let structure = downcast::<StructArray>(array)?;
    Ok(match algebraic.constructors() {
        [record] => decode_constructor(record, structure, presence)?
            .into_iter()
            .map(|fields| {
                fields.map(|fields| Value::Algebraic {
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
                decoded.push(decode_constructor(constructor, column, Presence::Nullable)?);
            }
            (0..structure.len())
                .map(|row| {
                    let valid = structure.is_valid(row);
                    match (valid, tags[row], variants.is_valid(row)) {
                        (false, None, false) | (true, Some(_), true) => {}
                        (true, _, _) => return Err(ValueDecodeError::Null { row }),
                        (false, _, _) => return Err(ValueDecodeError::ValueUnderNull { row }),
                    }
                    let present = decoded
                        .iter()
                        .enumerate()
                        .filter(|(_, rows)| rows[row].is_some())
                        .map(|(position, _)| position)
                        .collect::<Vec<_>>();
                    let Some(tag) = tags[row] else {
                        // A null union row has no constructor struct either.
                        return if present.is_empty() {
                            Ok(None)
                        } else {
                            Err(ValueDecodeError::ValueUnderNull { row })
                        };
                    };
                    // Exactly the tagged constructor's struct holds the fields.
                    let fields = match (present.as_slice(), &decoded[tag][row]) {
                        ([present], Some(fields)) if *present == tag => fields.clone(),
                        _ => return Err(ValueDecodeError::ConstructorPresence { row }),
                    };
                    Ok(Some(Value::Algebraic {
                        constructor: ConstructorPosition::new(tag),
                        fields,
                    }))
                })
                .collect::<Result<_, _>>()?
        }
    })
}

/// The field values of `constructor` in every row of `structure`: `None` in
/// its null rows, whose fields must be null too.
fn decode_constructor(
    constructor: &ConstructorType,
    structure: &StructArray,
    presence: Presence,
) -> Result<Vec<Option<Vec<Value>>>, ValueDecodeError> {
    if structure.num_columns() != constructor.fields().len() {
        return Err(ValueDecodeError::ArrowType);
    }
    let mut rows = (0..structure.len())
        .map(|row| {
            structure
                .is_valid(row)
                .then(|| Vec::with_capacity(constructor.fields().len()))
        })
        .collect::<Vec<_>>();
    for (field, column) in constructor.fields().iter().zip(structure.columns()) {
        let values = expect_presence(decode(field.wire(), column.as_ref(), presence)?, |row| {
            structure.is_valid(row)
        })?;
        for (row, value) in rows.iter_mut().zip(values) {
            if let (Some(row), Some(value)) = (row.as_mut(), value) {
                row.push(value);
            }
        }
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::axis::{Axis, AxisKind, KeyValue};
    use crate::batch::{InputColumn, OutputValues, RequestBatch, ResultBatch, ResultBatchBuilder};
    use crate::datetime_type::{DatetimeType, TimeScale};
    use crate::domain::{Bounds, Domain};
    use crate::interface::tests::{length, name};
    use crate::interface::{ModelInterface, OutputDescription, ParameterDescription};
    use crate::label::{ConstructorName, FieldName, PortName, TypeName, UnitSpelling};
    use crate::outcome::{RowFailure, RowOutcome};
    use crate::session::{SessionError, SessionSchema, SessionSpec};
    use crate::unit_scale::SiScale;
    use crate::wire_type::PortWire;
    use crate::wire_type::{AlgebraicType, ConstructorType, FieldType, IndexedType};

    fn finite(value: f64) -> FiniteF64 {
        FiniteF64::new(value).unwrap()
    }

    fn phases() -> Axis {
        Axis::new(AxisKind::Named {
            index: crate::axis::tests::declared_index("Phase"),
            variants: vec![
                VariantName::new("Climb").unwrap(),
                VariantName::new("Cruise").unwrap(),
            ],
        })
        .unwrap()
    }

    /// `Length[Phase][Fin(3)]`.
    fn profile() -> WireType {
        let steps = Axis::new(AxisKind::Finite { size: 3 }).unwrap();
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
        let decoded = RequestBatch::from_record_batch(Arc::clone(&schema), &encoded).unwrap();
        assert_eq!(decoded, batch);
        let encoded = batch.to_struct_array().unwrap();
        let decoded = RequestBatch::from_struct_array(schema, &encoded).unwrap();
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
        let decoded = ResultBatch::from_record_batch(Arc::clone(&schema), &encoded).unwrap();
        assert_eq!(decoded, batch);
        let values = batch.values().to_struct_array().unwrap();
        let decoded = OutputValues::from_struct_array(schema, &values).unwrap();
        assert_eq!(&decoded, batch.values());
    }

    #[test]
    fn every_child_of_a_failed_row_is_null() {
        let schema = session(&[]);
        let pair_value = Value::Algebraic {
            constructor: ConstructorPosition::new(0),
            fields: vec![
                Value::Datetime(finite(12.5)),
                Value::Complex(ComplexValue {
                    re: finite(3.0),
                    im: finite(-2.0),
                }),
            ],
        };
        let mut builder = ResultBatchBuilder::new(Arc::clone(&schema));
        builder
            .push(RowOutcome::Success(vec![
                profile_value(1.0),
                circle(2.0),
                pair_value,
            ]))
            .unwrap();
        builder
            .push(RowOutcome::Failure(RowFailure::EvaluationError {
                message: "boom".to_owned(),
            }))
            .unwrap();
        let outputs = builder
            .finish()
            .unwrap()
            .values()
            .to_struct_array()
            .unwrap();
        let struct_column = |array: &ArrayRef| {
            array
                .as_any()
                .downcast_ref::<StructArray>()
                .cloned()
                .unwrap()
        };

        // A reader that ignores the parent's validity sees nulls, not zeros.
        let profile = outputs
            .column(0)
            .as_any()
            .downcast_ref::<FixedSizeListArray>()
            .unwrap();
        let steps = profile
            .values()
            .as_any()
            .downcast_ref::<FixedSizeListArray>()
            .unwrap();
        assert_eq!(steps.null_count(), 3);
        assert!((3..6).all(|step| steps.is_null(step)));
        assert_eq!(steps.values().null_count(), 6);

        let shape = struct_column(outputs.column(1));
        assert!(shape.column(0).is_null(1) && shape.column(1).is_null(1));
        let variants = struct_column(shape.column(1));
        let circle_fields = struct_column(variants.column(0));
        assert!(circle_fields.is_null(1) && circle_fields.column(0).is_null(1));

        let pair = struct_column(outputs.column(2));
        assert!(pair.column(0).is_null(1));
        let z = struct_column(pair.column(1));
        assert!(z.column(0).is_null(1) && z.column(1).is_null(1));
        assert!(z.column(0).is_valid(0));
    }

    #[test]
    fn decoding_rejects_a_value_below_a_null() {
        let wire = WireType::Indexed(IndexedType::new(phases(), length()));
        // Row 0 is null but still carries a value, as a v2 placeholder did.
        let list = FixedSizeListArray::try_new_with_length(
            list_item_field(&length(), Presence::Nullable),
            2,
            Arc::new(Float64Array::from(vec![Some(0.0), None])),
            Some(vec![false].into()),
            1,
        )
        .unwrap();
        assert_eq!(
            decode(&wire, &list, Presence::Nullable),
            Err(ValueDecodeError::ValueUnderNull { row: 0 })
        );
    }

    #[test]
    fn only_columns_that_may_be_null_have_nullable_children() {
        let request = data_type(&profile(), Presence::Always);
        let output = data_type(&profile(), Presence::Nullable);
        let item = |data_type: &DataType| match data_type {
            DataType::FixedSizeList(item, _) => item.as_ref().clone(),
            _ => panic!("indexed values are lists"),
        };
        assert!(!item(&request).is_nullable());
        assert!(!item(item(&request).data_type()).is_nullable());
        assert!(item(&output).is_nullable());
        assert!(item(item(&output).data_type()).is_nullable());
        // A union's constructor structs and their fields may always be null.
        let DataType::Struct(union) = data_type(&shape(), Presence::Always) else {
            panic!("a union is a struct");
        };
        assert!(!union[0].is_nullable() && !union[1].is_nullable());
        let DataType::Struct(variants) = union[1].data_type() else {
            panic!("variants is a struct");
        };
        let DataType::Struct(circle) = variants[0].data_type() else {
            panic!("a constructor is a struct");
        };
        assert!(variants[0].is_nullable() && circle[0].is_nullable());
    }

    #[test]
    fn fin_and_coordinate_keys_travel_as_positions_and_coordinates() {
        let fin = KeyType::new(Axis::new(AxisKind::Finite { size: 3 }).unwrap());
        let grid = KeyType::new(
            Axis::new(AxisKind::Coordinate {
                index: crate::axis::tests::declared_index("Step"),
                dimension: crate::dimension::Dimension::dimensionless(),
                coordinates: vec![finite(0.0), finite(0.25)],
            })
            .unwrap(),
        );
        let fin_wire = WireType::Key(fin);
        let grid_wire = WireType::Key(grid);
        assert_eq!(data_type(&fin_wire, Presence::Always), DataType::UInt32);
        assert_eq!(data_type(&grid_wire, Presence::Nullable), DataType::Float64);

        let fin_values = [
            Value::Key(KeyValue::Position(2)),
            Value::Key(KeyValue::Position(0)),
        ];
        let encoded = encode(
            &fin_wire,
            &[Some(fin_values[0].as_value_ref()), None],
            Presence::Nullable,
        )
        .unwrap();
        assert_eq!(
            encoded.as_any().downcast_ref::<UInt32Array>().unwrap(),
            &UInt32Array::from(vec![Some(2), None])
        );
        assert_eq!(
            decode(&fin_wire, encoded.as_ref(), Presence::Nullable).unwrap(),
            [Some(fin_values[0].clone()), None]
        );

        let grid_values = [Value::Key(KeyValue::Coordinate(finite(0.25)))];
        let encoded = encode(
            &grid_wire,
            &[Some(grid_values[0].as_value_ref())],
            Presence::Always,
        )
        .unwrap();
        assert_eq!(
            decode(&grid_wire, encoded.as_ref(), Presence::Always).unwrap(),
            [Some(grid_values[0].clone())]
        );

        assert!(
            encode(
                &fin_wire,
                &[Some(ValueRef::Key(KeyRef::Position(3)))],
                Presence::Always
            )
            .is_err()
        );
        assert_eq!(
            decode(&fin_wire, &UInt32Array::from(vec![3]), Presence::Always),
            Err(ValueDecodeError::UnknownPosition {
                position: 3,
                row: 0
            })
        );
        assert_eq!(
            decode(
                &grid_wire,
                &Float64Array::from(vec![0.0, 0.2]),
                Presence::Always
            ),
            Err(ValueDecodeError::UnknownCoordinate {
                coordinate: 0.2,
                row: 1
            })
        );
        assert_eq!(
            decode(
                &grid_wire,
                &Float64Array::from(vec![f64::NAN]),
                Presence::Always
            ),
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
        assert_eq!(
            decode(&wire, &list, Presence::Nullable),
            Err(ValueDecodeError::Null { row: 0 })
        );

        let encoded = encode(
            &shape(),
            &[Some(circle(1.0).as_value_ref())],
            Presence::Always,
        )
        .unwrap();
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
            decode(&shape(), &mislabeled, Presence::Always),
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
