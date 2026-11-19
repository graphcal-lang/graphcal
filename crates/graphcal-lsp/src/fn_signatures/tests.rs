use graphcal_compiler::builtin::BuiltinFn;
use graphcal_compiler::function_signature::{
    FunctionParam, FunctionSignature, ParamKind, ScalarValueKind,
};
use graphcal_compiler::syntax::function_name::{FnName, FnParamName};
use graphcal_compiler::syntax::index_name::IndexVarName;
use graphcal_compiler::syntax::module_name::ModuleAliasName;
use graphcal_compiler::syntax::non_empty::NonEmpty;

use crate::analysis_pipeline::{run_analysis, test_plugin_host};
use crate::fn_signatures::{ExternCallee, build_fn_signatures};

use tower_lsp::lsp_types::Url;

#[test]
fn extern_signature_labels_use_the_shared_signature_renderer() {
    let uri = Url::parse("file:///extern-labels.gcl").unwrap();
    let source = concat!(
        "type DvRange { DvRange(min: Velocity, max: Velocity) }\n",
        "import plugin \"graphcal:demo\" as demo {\n",
        "    fn inverse<D: Dim>(x: D) -> D^-1;\n",
        "    fn geometric_mean<D1: Dim, D2: Dim>(x: D1, y: D2) -> D1^(1/2) * D2^(1/2);\n",
        "    fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D;\n",
        "    fn dv_range<I: Index>(xs: Velocity[I]) -> DvRange;\n",
        "}\n",
        "node y: Dimensionless = 1.0;\n",
    );
    let analysis = run_analysis(&uri, source, &[], test_plugin_host());
    let signature = |name: &str| {
        let callee = ExternCallee {
            alias: ModuleAliasName::expect_valid("demo"),
            name: FnName::expect_valid(name),
        };
        analysis
            .extern_fn_signatures
            .get(&callee)
            .map(|info| (info.label.as_str(), info.parameters.join("; ")))
    };
    assert_eq!(
        signature("inverse"),
        Some(("fn demo::inverse<D: Dim>(x: D) -> D^-1", "x: D".to_string()))
    );
    assert_eq!(
        signature("geometric_mean"),
        Some((
            "fn demo::geometric_mean<D1: Dim, D2: Dim>(x: D1, y: D2) -> D1^(1/2) * D2^(1/2)",
            "x: D1; y: D2".to_string()
        ))
    );
    assert_eq!(
        signature("lerp"),
        Some((
            "fn demo::lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D",
            "a: D; b: D; t: Dimensionless".to_string()
        ))
    );
    assert_eq!(
        signature("dv_range"),
        Some((
            "fn demo::dv_range<I: Index>(xs: Velocity[I]) -> DvRange",
            "xs: Velocity[I]".to_string()
        ))
    );
}

#[test]
fn lsp_signature_display_preserves_bool_and_int_array_kinds() {
    let index = IndexVarName::expect_valid("I");
    for (element, expected) in [
        (ScalarValueKind::Bool, "Bool[I]"),
        (ScalarValueKind::Int, "Int[I]"),
    ] {
        let kind = ParamKind::Indexed {
            element,
            indexes: NonEmpty::singleton(index.clone()),
        };
        let signature = FunctionSignature::try_new(
            Vec::new(),
            vec![index.clone()],
            vec![FunctionParam {
                name: FnParamName::expect_valid("values"),
                kind: kind.clone(),
            }],
            kind.into(),
        )
        .unwrap();
        assert_eq!(
            signature.params()[0].format_with(&mut |_| String::new()),
            format!("values: {expected}")
        );
        assert!(signature.format_with(|_| String::new()).contains(expected));
    }
}

fn builtin(name: &str) -> BuiltinFn {
    BuiltinFn::parse(name).unwrap()
}

#[test]
fn complex_signature_help_uses_dimension_aware_signatures() {
    let signatures = build_fn_signatures();
    assert_eq!(
        signatures
            .get(&builtin("complex"))
            .map(|signature| signature.label.as_str()),
        Some("fn complex<D: Dim>(re: D, im: D) -> Complex<D>")
    );
    assert_eq!(
        signatures
            .get(&builtin("polar"))
            .map(|signature| signature.parameters.as_slice()),
        Some(["magnitude: D".to_string(), "phase: Angle".to_string()].as_slice())
    );
    assert_eq!(
        signatures
            .get(&builtin("abs"))
            .map(|signature| signature.label.as_str()),
        Some("fn abs<D: Dim>(x: D | Complex<D>) -> D")
    );
}

#[test]
fn aggregation_signature_help_includes_product_and_rss() {
    let signatures = build_fn_signatures();
    assert_eq!(
        signatures
            .get(&builtin("product"))
            .map(|signature| signature.label.as_str()),
        Some("fn product<D: Dim, I: Index>(values: D[I]) -> D^|I|")
    );
    assert_eq!(
        signatures
            .get(&builtin("rss"))
            .map(|signature| signature.label.as_str()),
        Some("fn rss<D: Dim, I: Index>(values: D[I]) -> D")
    );
}

#[test]
fn signature_help_covers_exactly_the_documented_builtins() {
    let signatures = build_fn_signatures();
    let documented = BuiltinFn::all()
        .filter(|function| function.entry().documented_arity().is_some())
        .map(BuiltinFn::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    let keys = signatures
        .keys()
        .copied()
        .map(BuiltinFn::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(keys, documented);
    for function in BuiltinFn::all() {
        let Some(arity) = function.entry().documented_arity() else {
            continue;
        };
        assert_eq!(
            signatures[&function].parameters.len(),
            arity,
            "`{function}`"
        );
    }
    assert!(!signatures.contains_key(&builtin("datetime")));
    assert!(!signatures.contains_key(&builtin("to_float")));
    let label = |name: &str| signatures[&builtin(name)].label.as_str();
    assert_eq!(label("sqrt"), "fn sqrt<D: Dim>(x: D) -> D^(1/2)");
    assert_eq!(label("atan2"), "fn atan2<D: Dim>(y: D, x: D) -> Angle");
    assert_eq!(label("abs"), "fn abs<D: Dim>(x: D | Complex<D>) -> D");
    assert_eq!(
        label("cross"),
        "fn cross<D1: Dim, D2: Dim, I: Index>(a: D1[I], b: D2[I]) -> (D1 * D2)[I] where |I| = 3"
    );
}
