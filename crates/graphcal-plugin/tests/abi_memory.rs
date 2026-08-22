//! Direct native harnesses for the plugin SDK's raw memory protocol.
//!
//! These tests intentionally call every unsafe helper with valid storage. Miri
//! and `AddressSanitizer` therefore execute the same pointer construction, copy,
//! and deallocation paths used by generated Wasm wrappers.
#![expect(
    clippy::float_cmp,
    unsafe_code,
    reason = "this focused ABI harness compares exactly copied bits and executes unsafe contracts"
)]

use std::alloc::Layout;
use std::mem;
use std::panic::{AssertUnwindSafe, catch_unwind};

use graphcal_plugin::{__rt, Array};

const BAD_POINTER_ALIGNMENT: usize = 1;
const BAD_BUFFER_LENGTH: u32 = 1;

#[test]
fn sdk_abi_memory_round_trip() {
    let size = u32::try_from(mem::size_of::<[f64; 3]>()).unwrap();
    let pointer = __rt::buffer_alloc(size);
    assert!(!pointer.is_null());
    assert_eq!((pointer as usize) % mem::align_of::<f64>(), 0);

    let values = [1.25, -2.5, 9.0];
    // SAFETY: `pointer` names the live SDK allocation above, which has the
    // exact byte size, alignment, and lifetime required by the copied array.
    unsafe {
        std::ptr::copy_nonoverlapping(values.as_ptr().cast::<u8>(), pointer, size as usize);
        let view = __rt::quantity_array_view_from_abi(pointer.cast(), 3, &[3], "values");
        assert_eq!(view.values(), values);
        __rt::buffer_free(pointer, size);
    }
}

#[test]
fn sdk_abi_result_writers_fill_exact_storage() {
    let array = Array::new(vec![2, 2], vec![1.0, 2.0, 3.0, 4.0]).unwrap();
    let mut array_output = [f64::NAN; 4];
    let slots = [7.0, 8.0, 9.0];
    let mut slot_output = [f64::NAN; 3];

    // SAFETY: both output pointers name the exact initialized, writable slot
    // counts declared to the helpers and remain live for the calls.
    unsafe {
        __rt::write_quantity_array_result(
            &array,
            array_output.as_mut_ptr(),
            &[2, 2],
            "array_result",
        );
        __rt::write_slots(&slots, slot_output.as_mut_ptr(), 3, "struct_result");
    }

    assert_eq!(array_output, [1.0, 2.0, 3.0, 4.0]);
    assert_eq!(slot_output, slots);
}

#[test]
fn sdk_abi_bool_and_int_arrays_are_typed_and_validated() {
    let bool_raw = [1.0, -0.0, 0.0];
    // SAFETY: `bool_raw` supplies exactly the three initialized, aligned slots.
    let bools = unsafe { __rt::bool_array_view_from_abi(bool_raw.as_ptr(), 3, &[3], "bools") };
    assert_eq!(bools.values(), [true, false, false]);

    let int_raw = [1.0, -0.0, 18_014_398_509_481_984.0];
    // SAFETY: `int_raw` supplies exactly the three initialized, aligned slots.
    let ints = unsafe { __rt::int_array_view_from_abi(int_raw.as_ptr(), 3, &[3], "ints") };
    assert_eq!(ints.values(), [1, 0, 1_i64 << 54]);

    let invalid_bool = [0.0, 0.5];
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: the pointer and shape are valid; only the semantic element is invalid.
            unsafe { __rt::bool_array_view_from_abi(invalid_bool.as_ptr(), 2, &[2], "bools") }
        }))
        .is_err()
    );
    let invalid_int = [1.0, 2.5];
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: the pointer and shape are valid; only the semantic element is invalid.
            unsafe { __rt::int_array_view_from_abi(invalid_int.as_ptr(), 2, &[2], "ints") }
        }))
        .is_err()
    );

    let bool_result = Array::vector(vec![true, false]).unwrap();
    let int_result = Array::vector(vec![1_i64, -2]).unwrap();
    let mut bool_output = [f64::NAN; 2];
    let mut int_output = [f64::NAN; 2];
    // SAFETY: each output has exactly the two writable slots declared by its shape.
    unsafe {
        __rt::write_bool_array_result(&bool_result, bool_output.as_mut_ptr(), &[2], "bools");
        __rt::write_int_array_result(&int_result, int_output.as_mut_ptr(), &[2], "ints");
    }
    assert_eq!(bool_output, [1.0, 0.0]);
    assert_eq!(int_output, [1.0, -2.0]);
}

#[test]
fn sdk_abi_rejects_misaligned_nonempty_pointers_before_dereference() {
    let layout = Layout::from_size_align(32, 8).unwrap();
    // SAFETY: `layout` has nonzero size and valid power-of-two alignment.
    let allocation = unsafe { std::alloc::alloc(layout) };
    assert!(!allocation.is_null());
    // Offset by one byte while remaining inside the allocation. The SDK must
    // reject this address before constructing an `f64` slice or writing it.
    // SAFETY: adding one stays within the 32-byte allocation.
    let misaligned = unsafe { allocation.add(BAD_POINTER_ALIGNMENT) };

    let read = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: this deliberately violates the documented alignment
        // precondition to verify the boundary's pre-dereference guard.
        unsafe { __rt::quantity_array_view_from_abi(misaligned.cast(), 1, &[1], "bad") }
    }));
    assert!(read.is_err());

    let write = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: as above, the deliberate misalignment must be rejected
        // before the helper performs a raw-pointer write.
        unsafe { __rt::write_slots(&[1.0], misaligned.cast(), 1, "bad") };
    }));
    assert!(write.is_err());

    // SAFETY: `allocation` is the exact pointer returned for `layout` and has
    // not been deallocated or exposed to a successful raw write.
    unsafe { std::alloc::dealloc(allocation, layout) };
}

#[test]
fn sdk_abi_rejects_shape_and_slot_length_mismatches_before_copy() {
    let input = [1.0, 2.0];
    let bad_shape = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: the pointer itself is valid for both elements; the deliberate
        // shape mismatch is rejected before the slice is exposed.
        unsafe { __rt::quantity_array_view_from_abi(input.as_ptr(), 2, &[3], "bad") }
    }));
    assert!(bad_shape.is_err());

    let mut output = [f64::NAN; 2];
    let bad_slots = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: the output has two slots. The deliberate declared-length
        // mismatch must fail before any copy occurs.
        unsafe { __rt::write_slots(&[1.0, 2.0], output.as_mut_ptr(), BAD_BUFFER_LENGTH, "bad") };
    }));
    assert!(bad_slots.is_err());
    assert!(output.iter().all(|value| value.is_nan()));
}
