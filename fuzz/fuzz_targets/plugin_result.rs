#![no_main]

//! Plugin result marshalling: raw `f64` slots a plugin wrote are validated
//! against a declared result kind, as the evaluator does for every plugin
//! call.

use graphcal_compiler::dimension::Dimension;
use graphcal_compiler::function_signature::{
    DimMonomial, ParamKind, ResultKind, ScalarValueKind, StructFieldKind, StructShape,
    StructShapeField,
};
use graphcal_compiler::syntax::non_empty::NonEmpty;
use graphcal_compiler::syntax::type_name::FieldName;
use graphcal_eval::host_abi::{
    HostInt, HostScalar, ValidatedHostArrayValues, ValidatedHostFieldValue, ValidatedHostResult,
    decode_result,
};
use graphcal_eval::host_fns::{HostArray, HostFnValue};
use libfuzzer_sys::fuzz_target;

const MAX_INPUT_BYTES: usize = 64 * 1024;
const MAX_RANK: usize = 3;
const MAX_FIELDS: usize = 8;

/// Index binders are opaque to result validation; their position suffices.
type Kind = ResultKind<StructShape, graphcal_compiler::function_signature::DimBinder, usize>;

/// Untrusted bytes read front to back; exhausted input reads as zero.
struct Input<'a>(&'a [u8]);

impl Input<'_> {
    fn byte(&mut self) -> u8 {
        let Some((&first, rest)) = self.0.split_first() else {
            return 0;
        };
        self.0 = rest;
        first
    }

    /// Every remaining whole 8-byte chunk as an arbitrary `f64` bit pattern.
    fn slots(self) -> Vec<f64> {
        self.0
            .chunks_exact(8)
            .map(|chunk| f64::from_le_bytes(chunk.try_into().expect("an 8-byte chunk")))
            .collect()
    }
}

fn scalar_kind(selector: u8) -> ScalarValueKind {
    match selector % 3 {
        0 => ScalarValueKind::Bool,
        1 => ScalarValueKind::Int,
        _ => ScalarValueKind::Quantity(DimMonomial::fixed(Dimension::dimensionless())),
    }
}

fn field_kind(selector: u8) -> StructFieldKind {
    match selector % 3 {
        0 => StructFieldKind::Bool,
        1 => StructFieldKind::Int,
        _ => StructFieldKind::Quantity(Dimension::dimensionless()),
    }
}

/// A raw plugin return, with the slots and shape it was built from (the
/// evaluator's `HostArray` does not expose them outside its crate).
struct Returned {
    value: HostFnValue,
    slots: Vec<f64>,
    shape: Vec<usize>,
}

/// The declared result kind and the raw value a plugin returned for it.
fn result_and_value(input: &mut Input<'_>) -> Option<(Kind, Returned)> {
    let declared = match input.byte() % 3 {
        0 => ResultKind::Value(ParamKind::Scalar(scalar_kind(input.byte()))),
        1 => {
            let element = scalar_kind(input.byte());
            let rank = 1 + usize::from(input.byte()) % MAX_RANK;
            let indexes = NonEmpty::try_from_vec((0..rank).collect()).ok()?;
            ResultKind::Value(ParamKind::Indexed { element, indexes })
        }
        _ => {
            let count = 1 + usize::from(input.byte()) % MAX_FIELDS;
            let fields = (0..count)
                .map(|position| StructShapeField {
                    name: FieldName::expect_valid(&format!("f{position}")),
                    kind: field_kind(input.byte()),
                })
                .collect();
            ResultKind::Struct(StructShape::try_new(fields).ok()?)
        }
    };
    // The wire form is chosen independently: a plugin may return any form.
    let form = input.byte() % 3;
    let rank = 1 + usize::from(input.byte()) % MAX_RANK;
    let shape = (0..rank)
        .map(|_| usize::from(input.byte()) % 9)
        .collect::<Vec<_>>();
    let slots = std::mem::replace(input, Input(&[])).slots();
    let value = match form {
        0 => HostFnValue::F64(slots.first().copied().unwrap_or_default()),
        1 => HostFnValue::Array(HostArray::try_new(shape.clone(), slots.clone()).ok()?),
        _ => HostFnValue::Record(slots.clone()),
    };
    Some((
        declared,
        Returned {
            value,
            slots,
            shape,
        },
    ))
}

/// The slot an accepted `Bool` came from must encode it (either signed zero
/// encodes `false`).
fn assert_bool_round_trips(value: bool, slot: f64) {
    let expected = if value { 1.0 } else { 0.0 };
    assert!(slot == expected, "Bool {value} accepted from slot {slot}");
}

/// The slot an accepted quantity came from must be its exact bits.
fn assert_quantity_round_trips(value: f64, slot: f64) {
    assert_eq!(
        value.to_bits(),
        slot.to_bits(),
        "quantity {value} accepted from slot {slot}"
    );
}

/// The slot an accepted `Int` came from must encode it exactly.
fn assert_int_round_trips(value: i64, slot: f64) {
    let encoded = HostScalar::Int(
        HostInt::try_new(value).expect("an accepted Int must be exactly representable"),
    );
    assert!(
        encoded.abi_slot() == slot,
        "Int {value} accepted from slot {slot}"
    );
}

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_BYTES {
        return;
    }
    let mut input = Input(data);
    let Some((declared, returned)) = result_and_value(&mut input) else {
        return;
    };
    let Ok(decoded) = decode_result(&declared, &returned.value) else {
        return;
    };

    match (decoded, &returned.value) {
        (ValidatedHostResult::Bool(value), HostFnValue::F64(slot)) => {
            assert_bool_round_trips(value, *slot);
        }
        (ValidatedHostResult::Int(value), HostFnValue::F64(slot)) => {
            assert_int_round_trips(value, *slot);
        }
        (ValidatedHostResult::Quantity { value, .. }, HostFnValue::F64(slot)) => {
            assert_quantity_round_trips(value.get(), *slot);
        }
        (ValidatedHostResult::Array(array), HostFnValue::Array(_)) => {
            assert_eq!(array.shape(), returned.shape, "the array keeps its shape");
            let slots = &returned.slots;
            match array.values() {
                ValidatedHostArrayValues::Quantity { values, .. } => {
                    assert_eq!(values.len(), slots.len());
                    for (value, slot) in values.iter().zip(slots) {
                        assert_quantity_round_trips(value.get(), *slot);
                    }
                }
                ValidatedHostArrayValues::Bool(values) => {
                    assert_eq!(values.len(), slots.len());
                    for (value, slot) in values.iter().zip(slots) {
                        assert_bool_round_trips(*value, *slot);
                    }
                }
                ValidatedHostArrayValues::Int(values) => {
                    assert_eq!(values.len(), slots.len());
                    for (value, slot) in values.iter().zip(slots) {
                        assert_int_round_trips(*value, *slot);
                    }
                }
            }
        }
        (ValidatedHostResult::Struct { record, fields }, HostFnValue::Record(slots)) => {
            assert_eq!(fields.len(), record.fields().len());
            assert_eq!(fields.len(), slots.len());
            for (field, slot) in fields.iter().zip(slots) {
                match field.value() {
                    ValidatedHostFieldValue::Bool(value) => assert_bool_round_trips(*value, *slot),
                    ValidatedHostFieldValue::Int(value) => assert_int_round_trips(*value, *slot),
                    ValidatedHostFieldValue::Quantity(value) => {
                        assert_quantity_round_trips(value.get(), *slot);
                    }
                }
            }
        }
        (decoded, raw) => panic!("{decoded:?} was decoded from another wire form: {raw:?}"),
    }
});
