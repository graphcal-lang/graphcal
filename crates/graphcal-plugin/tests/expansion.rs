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

    /// Array parameters carry a shape; array results preserve it explicitly.
    fn rescale<D: Dim, I: Index>(xs: D[I], k: Dimensionless) -> D[I] {
        let values = xs.iter().map(|x| x * k).collect();
        graphcal_plugin::Array::new(xs.shape().to_vec(), values)
            .unwrap_or_else(|error| graphcal_plugin::fail!("{error}"))
    }

    /// Multi-axis arrays can reorder axes and values.
    fn matrix_transpose<D: Dim, I: Index, J: Index>(xs: D[I, J]) -> D[J, I] {
        let [rows, columns] = xs.shape() else {
            graphcal_plugin::fail!("matrix_transpose requires rank two");
        };
        let values = (0..*columns)
            .flat_map(|column| {
                (0..*rows).map(move |row| {
                    let offset = row
                        .checked_mul(*columns)
                        .and_then(|offset| offset.checked_add(column))
                        .unwrap_or_else(|| graphcal_plugin::fail!("matrix offset overflow"));
                    *xs.values()
                        .get(offset)
                        .unwrap_or_else(|| graphcal_plugin::fail!("matrix shape mismatch"))
                })
            })
            .collect();
        graphcal_plugin::Array::new(vec![*columns, *rows], values)
            .unwrap_or_else(|error| graphcal_plugin::fail!("{error}"))
    }

    /// Arrays can collapse to quantities.
    fn total<D: Dim, I: Index>(xs: D[I]) -> D {
        xs.iter().sum()
    }

    /// Struct results are declared structurally (concrete field types) and
    /// returned through a generated named output type (issue #25 Phase D).
    fn span<I: Index>(xs: Pressure[I]) -> { lo: Pressure, hi: Pressure } {
        let lo = xs.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = xs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        SpanOutput { lo, hi }
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
fn array_kernels_run_natively_with_explicit_shapes() {
    let values = [1.0, 2.5, -4.0];
    let xs = graphcal_plugin::ArrayView::new(&[3], &values).unwrap();
    assert_eq!(
        rescale(xs, 2.0),
        graphcal_plugin::Array::vector(vec![2.0, 5.0, -8.0]).unwrap()
    );
    assert!((total(xs) + 0.5).abs() < 1e-12);

    let matrix = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
    let matrix = graphcal_plugin::ArrayView::new(&[2, 3], &matrix).unwrap();
    assert_eq!(
        matrix_transpose(matrix),
        graphcal_plugin::Array::new(vec![3, 2], vec![1.0, 4.0, 2.0, 5.0, 3.0, 6.0]).unwrap()
    );
}

#[test]
fn struct_kernels_return_the_generated_output_type() {
    let values = [3.0, -1.5, 2.0];
    let span = span(graphcal_plugin::ArrayView::new(&[3], &values).unwrap());
    assert_eq!(span, SpanOutput { lo: -1.5, hi: 3.0 });
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
            "unrepresentable",
            "rescale",
            "matrix_transpose",
            "total",
            "span"
        ]
    );

    let rescale = &manifest.functions[5];
    assert_eq!(rescale.index_vars, ["I"]);
    let span = &manifest.functions[8];
    assert!(matches!(
        &span.result,
        graphcal_plugin_abi::ManifestResultKind::Struct { fields }
            if fields.len() == 2 && fields[0].name == "lo" && fields[1].name == "hi"
    ));
    assert!(matches!(
        &rescale.params[0].kind,
        graphcal_plugin_abi::ManifestParamKind::Array { indexes, .. }
            if indexes == &["I"]
    ));
    assert!(matches!(
        &rescale.result,
        graphcal_plugin_abi::ManifestResultKind::Array { indexes, .. }
            if indexes == &["I"]
    ));
}

#[test]
fn manifest_converts_to_the_compiler_signature_ir() {
    let manifest = decoded_manifest();
    let functions = graphcal_plugin_host::convert_manifest(&manifest)
        .expect("macro-produced manifests must convert");

    let var = || DimVarName::expect_valid("D");
    let expected_lerp = FunctionSignature::try_new(
        vec![var()],
        Vec::new(),
        vec![
            FunctionParam {
                name: FnParamName::expect_valid("a"),
                kind: ValueKind::Quantity(DimMonomial::var(var())),
            },
            FunctionParam {
                name: FnParamName::expect_valid("b"),
                kind: ValueKind::Quantity(DimMonomial::var(var())),
            },
            FunctionParam {
                name: FnParamName::expect_valid("t"),
                kind: ValueKind::dimensionless(),
            },
        ],
        ValueKind::Quantity(DimMonomial::var(var())),
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
