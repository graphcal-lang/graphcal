//! Proc-macro implementation of the graphcal plugin authoring SDK.
//!
//! This crate only hosts the macro machinery; depend on `graphcal-plugin`
//! and invoke the macro as `graphcal_plugin::plugin!` — the generated code
//! calls back into that crate's runtime support, and the user-facing
//! documentation lives on the re-export.
//!
//! The pipeline is parse (`parse`) → validate/lower (`lower`) → manifest
//! bytes via the ABI crate's own codec (`manifest`) → item emission
//! (`codegen`). Signature validation intentionally reproduces the
//! compiler's `FunctionSignature::try_new` invariants so a signature the
//! macro accepts is always one the graphcal loader accepts.

mod codegen;
mod dims;
mod lower;
mod manifest;
mod parse;
mod rational;

/// Declare a graphcal plugin's exported functions.
///
/// Documented on the re-export: see `graphcal_plugin::plugin!`.
#[proc_macro]
pub fn plugin(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    expand(input.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand(input: proc_macro2::TokenStream) -> syn::Result<proc_macro2::TokenStream> {
    let ast: parse::PluginInput = syn::parse2(input)?;
    let ir = lower::lower(&ast)?;
    let manifest_json = manifest::build_manifest_json(&ir)?;
    Ok(codegen::generate(&ir, &manifest_json))
}

#[cfg(test)]
mod tests {
    use graphcal_plugin_abi::{ManifestValueKind, PluginManifest};
    use quote::quote;

    use super::*;

    /// Run the front half of the pipeline and decode the manifest with the
    /// ABI crate's validating decoder, so every accepted signature is also
    /// proven wire-valid.
    fn manifest_of(input: proc_macro2::TokenStream) -> PluginManifest {
        let ast: parse::PluginInput = syn::parse2(input).expect("parse");
        let ir = lower::lower(&ast).expect("lower");
        let json = manifest::build_manifest_json(&ir).expect("manifest");
        PluginManifest::from_json(json.as_bytes()).expect("decode")
    }

    fn error_of(input: proc_macro2::TokenStream) -> String {
        expand(input).expect_err("expected an error").to_string()
    }

    fn scalar(kind: &ManifestValueKind) -> &graphcal_plugin_abi::ManifestMonomial {
        match kind {
            ManifestValueKind::Scalar(monomial) => monomial,
            other => panic!("expected a scalar kind, got {other:?}"),
        }
    }

    #[test]
    fn dim_generic_signature_roundtrips() {
        let manifest = manifest_of(quote! {
            fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D { a + (b - a) * t }
        });
        assert_eq!(manifest.abi_version, graphcal_plugin_abi::ABI_VERSION);
        let function = &manifest.functions[0];
        assert_eq!(function.name, "lerp");
        assert_eq!(function.dim_vars, ["D"]);
        assert_eq!(function.params.len(), 3);
        let a = scalar(&function.params[0].kind);
        assert_eq!(a.vars.len(), 1);
        assert_eq!(a.vars[0].var, "D");
        assert_eq!((a.vars[0].pow.num, a.vars[0].pow.den), (1, 1));
        assert!(a.fixed.is_empty());
        let t = scalar(&function.params[2].kind);
        assert!(t.vars.is_empty() && t.fixed.is_empty());
    }

    #[test]
    fn derived_dimensions_expand_to_base_exponents() {
        let manifest = manifest_of(quote! {
            fn density(p: Pressure, t: Temperature) -> Mass / Volume { p / t }
        });
        let function = &manifest.functions[0];
        let pressure = scalar(&function.params[0].kind);
        let factors: Vec<(&str, i32, i32)> = pressure
            .fixed
            .iter()
            .map(|f| (f.dim.as_str(), f.pow.num, f.pow.den))
            .collect();
        assert_eq!(
            factors,
            [("Length", -1, 1), ("Time", -2, 1), ("Mass", 1, 1)]
        );
        let result = scalar(&function.result);
        let factors: Vec<(&str, i32, i32)> = result
            .fixed
            .iter()
            .map(|f| (f.dim.as_str(), f.pow.num, f.pow.den))
            .collect();
        assert_eq!(factors, [("Length", -3, 1), ("Mass", 1, 1)]);
    }

    #[test]
    fn rational_powers_and_division_fold() {
        let manifest = manifest_of(quote! {
            fn geometric_mean<D1: Dim, D2: Dim>(x: D1, y: D2) -> (D1 * D2)^(1/2) { (x * y).sqrt() }
            fn cancel<D: Dim>(x: D, y: D^2) -> D^2 / D * Dimensionless { y / x * 1.0 }
        });
        let mean = scalar(&manifest.functions[0].result);
        let powers: Vec<(&str, i32, i32)> = mean
            .vars
            .iter()
            .map(|v| (v.var.as_str(), v.pow.num, v.pow.den))
            .collect();
        assert_eq!(powers, [("D1", 1, 2), ("D2", 1, 2)]);

        let cancelled = scalar(&manifest.functions[1].result);
        assert_eq!(cancelled.vars.len(), 1);
        assert_eq!(
            (cancelled.vars[0].pow.num, cancelled.vars[0].pow.den),
            (1, 1)
        );
    }

    #[test]
    fn bool_and_int_kinds_map_directly() {
        let manifest = manifest_of(quote! {
            fn step(n: Int, up: Bool) -> Int { if up { n + 1 } else { n - 1 } }
        });
        let function = &manifest.functions[0];
        assert_eq!(function.params[0].kind, ManifestValueKind::Int);
        assert_eq!(function.params[1].kind, ManifestValueKind::Bool);
        assert_eq!(function.result, ManifestValueKind::Int);
    }

    #[test]
    fn negative_and_parenthesized_exponents_parse() {
        let manifest = manifest_of(quote! {
            fn f(x: Length^-3, y: Time^(-1/2)) -> Length^(2) { x + y }
        });
        let function = &manifest.functions[0];
        let x = scalar(&function.params[0].kind);
        assert_eq!((x.fixed[0].pow.num, x.fixed[0].pow.den), (-3, 1));
        let y = scalar(&function.params[1].kind);
        assert_eq!((y.fixed[0].pow.num, y.fixed[0].pow.den), (-1, 2));
    }

    #[test]
    fn use_before_binding_is_rejected() {
        let message = error_of(quote! {
            fn sq<D: Dim>(x: D^2) -> D { x.sqrt() }
        });
        assert!(message.contains("before it is bound"), "got: {message}");
    }

    #[test]
    fn unbound_result_variable_is_rejected() {
        let message = error_of(quote! {
            fn f<D: Dim>(x: Dimensionless) -> D { x }
        });
        assert!(message.contains("no parameter binds"), "got: {message}");
    }

    #[test]
    fn never_bound_variable_is_rejected() {
        let message = error_of(quote! {
            fn f<D: Dim>(x: Dimensionless) -> Dimensionless { x }
        });
        assert!(message.contains("never bound"), "got: {message}");
    }

    #[test]
    fn vocabulary_misuse_is_rejected() {
        let message = error_of(quote! {
            fn f(x: Density) -> Dimensionless { x }
        });
        assert!(
            message.contains("unknown dimension `Density`"),
            "got: {message}"
        );
        assert!(message.contains("Velocity"), "got: {message}");

        let message = error_of(quote! {
            fn f(x: Bool * Length) -> Dimensionless { x }
        });
        assert!(
            message.contains("value kind, not a dimension"),
            "got: {message}"
        );

        let message = error_of(quote! {
            fn f(x: Datetime) -> Dimensionless { x }
        });
        assert!(message.contains("plugin ABI v1"), "got: {message}");

        let message = error_of(quote! {
            fn f<Length: Dim>(x: Length) -> Length { x }
        });
        assert!(
            message.contains("shadows the prelude name"),
            "got: {message}"
        );
    }

    #[test]
    fn zero_exponents_are_rejected() {
        let message = error_of(quote! {
            fn f(x: Length^0) -> Dimensionless { x }
        });
        assert!(message.contains("erase its term"), "got: {message}");

        let message = error_of(quote! {
            fn f(x: Length^(1/0)) -> Dimensionless { x }
        });
        assert!(
            message.contains("denominator cannot be zero"),
            "got: {message}"
        );
    }

    #[test]
    fn duplicates_are_rejected() {
        let message = error_of(quote! {
            fn f(x: Length) -> Length { x }
            fn f(y: Time) -> Time { y }
        });
        assert!(
            message.contains("declared more than once"),
            "got: {message}"
        );

        let message = error_of(quote! {
            fn f(x: Length, x: Time) -> Length { x }
        });
        assert!(message.contains("parameter `x`"), "got: {message}");

        let message = error_of(quote! {
            fn f<D: Dim, D: Dim>(x: D) -> D { x }
        });
        assert!(
            message.contains("dimension variable `D` is declared more than once"),
            "got: {message}"
        );
    }

    #[test]
    fn declaration_shapes_are_guided() {
        let message = error_of(quote! {
            fn f(x: Dimensionless) -> Dimensionless;
        });
        assert!(message.contains("need a Rust body"), "got: {message}");

        let message = error_of(quote! {
            pub fn f(x: Dimensionless) -> Dimensionless { x }
        });
        assert!(message.contains("drop `pub`"), "got: {message}");

        let message = error_of(quote! {
            #[inline]
            fn f(x: Dimensionless) -> Dimensionless { x }
        });
        assert!(message.contains("only doc comments"), "got: {message}");

        let message = error_of(quote! {});
        assert!(message.contains("declares no functions"), "got: {message}");

        let message = error_of(quote! {
            fn f<>(x: Dimensionless) -> Dimensionless { x }
        });
        assert!(
            message.contains("empty dimension-variable binder"),
            "got: {message}"
        );

        let message = error_of(quote! {
            fn f<N: Nat>(x: Dimensionless) -> Dimensionless { x }
        });
        assert!(
            message.contains("unsupported binder constraint `Nat`"),
            "got: {message}"
        );
    }

    #[test]
    fn array_signatures_roundtrip_to_the_manifest() {
        let manifest = manifest_of(quote! {
            fn smooth<D: Dim, I: Index>(xs: D[I], window: Dimensionless) -> D[I] {
                let _ = window;
                xs.to_vec()
            }
        });
        let function = &manifest.functions[0];
        assert_eq!(function.dim_vars, ["D"]);
        assert_eq!(function.index_vars, ["I"]);
        assert!(matches!(
            &function.params[0].kind,
            ManifestValueKind::Array { index, .. } if index == "I"
        ));
        assert!(matches!(
            &function.result,
            ManifestValueKind::Array { index, .. } if index == "I"
        ));
    }

    #[test]
    fn struct_results_roundtrip_to_the_manifest() {
        let manifest = manifest_of(quote! {
            fn span<D: Dim, I: Index>(xs: D[I]) -> { lo: Pressure, ok: Bool, n: Int } {
                SpanOutput { lo: xs[0], ok: true, n: xs.len() as i64 }
            }
        });
        let function = &manifest.functions[0];
        let ManifestValueKind::Struct { fields } = &function.result else {
            panic!("expected a struct result, got {:?}", function.result);
        };
        assert_eq!(fields.len(), 3);
        assert_eq!(fields[0].name, "lo");
        assert!(matches!(
            &fields[0].kind,
            graphcal_plugin_abi::ManifestFieldKind::Scalar(monomial) if monomial.vars.is_empty()
        ));
        assert!(matches!(
            &fields[1].kind,
            graphcal_plugin_abi::ManifestFieldKind::Bool
        ));
        assert!(matches!(
            &fields[2].kind,
            graphcal_plugin_abi::ManifestFieldKind::Int
        ));
    }

    #[test]
    fn struct_discipline_is_enforced() {
        let message = error_of(quote! {
            fn f(x: Dimensionless) -> { } { unreachable!() }
        });
        assert!(message.contains("at least one field"), "got: {message}");

        let message = error_of(quote! {
            fn f(x: Dimensionless) -> { a: Dimensionless, a: Int } { unreachable!() }
        });
        assert!(
            message.contains("field `a` is declared more than once"),
            "got: {message}"
        );

        // Struct fields have no dimension variables in scope.
        let message = error_of(quote! {
            fn f<D: Dim>(x: D) -> { lo: D } { unreachable!() }
        });
        assert!(message.contains("unknown dimension `D`"), "got: {message}");
    }

    #[test]
    fn array_discipline_is_enforced() {
        let message = error_of(quote! {
            fn f<D: Dim>(xs: D[I]) -> D { xs.iter().sum() }
        });
        assert!(
            message.contains("unknown index variable `I`"),
            "got: {message}"
        );

        let message = error_of(quote! {
            fn f<D: Dim, I: Index>(x: D) -> D[I] { vec![x] }
        });
        assert!(
            message.contains("cannot invent its output length"),
            "got: {message}"
        );

        let message = error_of(quote! {
            fn f<D: Dim, I: Index>(x: D) -> D { x }
        });
        assert!(
            message.contains("indexes no array parameter"),
            "got: {message}"
        );

        let message = error_of(quote! {
            fn f<I: Index>(xs: Bool[I]) -> Dimensionless { 0.0 }
        });
        assert!(
            message.contains("array elements must be scalars"),
            "got: {message}"
        );

        let message = error_of(quote! {
            fn f<D: Dim, D: Index>(xs: D) -> D { xs }
        });
        assert!(
            message.contains("generic binder `D` is declared more than once"),
            "got: {message}"
        );
    }

    #[test]
    fn generated_items_are_present() {
        let expansion = expand(quote! {
            /// Docs survive.
            fn identity<D: Dim>(x: D) -> D { x }
        })
        .expect("expansion")
        .to_string();
        assert!(
            expansion.contains("GRAPHCAL_PLUGIN_MANIFEST"),
            "got: {expansion}"
        );
        assert!(
            expansion.contains("GRAPHCAL_PLUGIN_MANIFEST_SECTION_IS_UNIQUE"),
            "got: {expansion}"
        );
        assert!(
            expansion.contains("extern \"C-unwind\" fn identity"),
            "got: {expansion}"
        );
        assert!(
            expansion.contains("install_failure_hook"),
            "got: {expansion}"
        );
        assert!(expansion.contains("Docs survive."), "got: {expansion}");
    }
}
