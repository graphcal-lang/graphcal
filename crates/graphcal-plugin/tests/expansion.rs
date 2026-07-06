//! Native exercises of `plugin!` expansions.
//!
//! The generated wrappers are plain Rust on non-wasm targets, so kernels,
//! ABI conversions, and failure paths are testable with `cargo test` —
//! exactly how plugin authors will test their own crates. The embedded
//! manifest is validated with the ABI crate's decoder and converted
//! through the host's boundary, so everything the macro accepts is proven
//! loadable.
#![cfg(test)]

use graphcal_compiler::function_signature::{
    DimMonomial, FunctionParam, FunctionSignature, ValueKind,
};
use graphcal_compiler::syntax::dimension::DimVarName;
use graphcal_compiler::syntax::function_name::FnParamName;
use graphcal_plugin_abi::PluginManifest;

graphcal_plugin::plugin! {
    /// Linear interpolation between `a` and `b`.
    fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D {
        (b - a).mul_add(t, a)
    }

    /// Reciprocal with an explicit domain failure.
    fn checked_sqrt(x: Dimensionless) -> Dimensionless {
        if x < 0.0 {
            graphcal_plugin::fail!("sqrt of a negative value: {x}");
        }
        x.sqrt()
    }

    /// Bool and Int parameters arrive typed in the body.
    fn step(n: Int, up: Bool) -> Int {
        if up { n + 1 } else { n - 1 }
    }

    /// Bool results cross back as 1.0/0.0.
    fn is_probability(x: Dimensionless) -> Bool {
        (0.0..=1.0).contains(&x)
    }

    /// An Int result the ABI cannot represent exactly.
    fn unrepresentable() -> Int {
        (1_i64 << 53) + 1
    }
}

fn decoded_manifest() -> PluginManifest {
    PluginManifest::from_json(&GRAPHCAL_PLUGIN_MANIFEST).expect("embedded manifest must decode")
}

#[test]
fn kernels_run_natively() {
    assert!((lerp(1.0, 3.0, 0.5) - 2.0).abs() < 1e-12);
    assert!((checked_sqrt(9.0) - 3.0).abs() < 1e-12);
}

#[test]
fn bool_and_int_values_convert_at_the_boundary() {
    assert!((step(5.0, 1.0) - 6.0).abs() < f64::EPSILON);
    assert!((step(5.0, 0.0) - 4.0).abs() < f64::EPSILON);
    assert!((is_probability(0.5) - 1.0).abs() < f64::EPSILON);
    assert!((is_probability(1.5) - 0.0).abs() < f64::EPSILON);
}

#[test]
#[should_panic(expected = "sqrt of a negative value: -1")]
fn fail_macro_aborts_with_the_message() {
    let _ = checked_sqrt(-1.0);
}

#[test]
#[should_panic(expected = "parameter `up`: expected a Bool encoded as 1.0 or 0.0, got 0.5")]
fn corrupt_bool_arguments_are_rejected() {
    let _ = step(5.0, 0.5);
}

#[test]
#[should_panic(expected = "parameter `n`: expected an Int encoded as an exactly-representable")]
fn corrupt_int_arguments_are_rejected() {
    let _ = step(5.5, 1.0);
}

#[test]
#[should_panic(expected = "not exactly representable as an f64")]
fn unrepresentable_int_results_are_rejected() {
    let _ = unrepresentable();
}

#[test]
fn manifest_matches_the_declarations() {
    let manifest = decoded_manifest();
    let names: Vec<&str> = manifest
        .functions
        .iter()
        .map(|function| function.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "lerp",
            "checked_sqrt",
            "step",
            "is_probability",
            "unrepresentable"
        ]
    );
}

#[test]
fn manifest_converts_to_the_compiler_signature_ir() {
    let manifest = decoded_manifest();
    let functions = graphcal_plugin_host::convert_manifest(&manifest)
        .expect("macro-produced manifests must convert");

    let var = || DimVarName::expect_valid("D");
    let expected_lerp = FunctionSignature::try_new(
        vec![var()],
        vec![
            FunctionParam {
                name: FnParamName::expect_valid("a"),
                kind: ValueKind::Scalar(DimMonomial::var(var())),
            },
            FunctionParam {
                name: FnParamName::expect_valid("b"),
                kind: ValueKind::Scalar(DimMonomial::var(var())),
            },
            FunctionParam {
                name: FnParamName::expect_valid("t"),
                kind: ValueKind::dimensionless(),
            },
        ],
        ValueKind::Scalar(DimMonomial::var(var())),
    )
    .expect("expected signature is valid");

    let lerp_signature = &functions
        .iter()
        .find(|(name, _)| name.as_str() == "lerp")
        .expect("lerp is in the manifest")
        .1;
    assert!(lerp_signature.structurally_equivalent(&expected_lerp));

    let step_signature = &functions
        .iter()
        .find(|(name, _)| name.as_str() == "step")
        .expect("step is in the manifest")
        .1;
    let expected_step = FunctionSignature::try_new(
        Vec::new(),
        vec![
            FunctionParam {
                name: FnParamName::expect_valid("n"),
                kind: ValueKind::Int,
            },
            FunctionParam {
                name: FnParamName::expect_valid("up"),
                kind: ValueKind::Bool,
            },
        ],
        ValueKind::Int,
    )
    .expect("expected signature is valid");
    assert!(step_signature.structurally_equivalent(&expected_step));
}
