//! Evaluator-side validation of host function results at the plugin ABI.

use std::collections::HashMap;

use graphcal_compiler::syntax::function_name::FnName;
use graphcal_compiler::syntax::plugin::PluginPath;
use graphcal_eval::host_fns::{HostArray, HostFnValue, HostFunctionRegistry};

#[test]
fn evaluator_and_cli_boundary_share_scalar_and_composite_abi_policy() {
    let plugin = PluginPath::new("graphcal:test-abi-policy");
    let mut registry = HostFunctionRegistry::new();
    for (name, value) in [
        ("signed_zero_bool", HostFnValue::F64(-0.0)),
        ("signed_zero_int", HostFnValue::F64(-0.0)),
        ("invalid_bool", HostFnValue::F64(0.5)),
        ("non_finite_quantity", HostFnValue::F64(f64::NAN)),
        (
            "non_finite_record",
            HostFnValue::Record(vec![f64::INFINITY]),
        ),
    ] {
        registry.register_for_test(plugin.clone(), FnName::expect_valid(name), move |_| {
            Ok(value.clone())
        });
    }
    registry.register_for_test(plugin, FnName::expect_valid("non_finite_array"), |_| {
        Ok(HostFnValue::Array(
            HostArray::vector(vec![1.0, f64::NEG_INFINITY]).unwrap(),
        ))
    });
    let source = r#"
pub index Axis = { A, B };
type QuantityResult {
    QuantityResult(value: Dimensionless),
}
import plugin "graphcal:test-abi-policy" as test {
    fn signed_zero_bool() -> Bool;
    fn signed_zero_int() -> Int;
    fn invalid_bool() -> Bool;
    fn non_finite_quantity() -> Dimensionless;
    fn non_finite_array<D: Dim, I: Index>(values: D[I]) -> D[I];
    fn non_finite_record() -> QuantityResult;
}
param values: Dimensionless[Axis] = { Axis#A: 1.0, Axis#B: 2.0 };
node valid_bool: Bool = test::signed_zero_bool();
node valid_int: Int = test::signed_zero_int();
node bad_bool: Bool = test::invalid_bool();
node bad_quantity: Dimensionless = test::non_finite_quantity();
node bad_array: Dimensionless[Axis] = test::non_finite_array(@values);
node bad_record: QuantityResult = test::non_finite_record();
"#;
    let project = graphcal_project::loader::LoadedProject::from_source(source, "test.gcl").unwrap();
    let result = graphcal_project::project_compiler::ProjectCompiler::new(&project)
        .host_fns(&registry)
        .eval(&HashMap::new())
        .unwrap();
    let outcome = |name: &str| {
        result
            .nodes()
            .find(|(candidate, _)| candidate.to_string() == name)
            .unwrap_or_else(|| panic!("{name} node should exist"))
            .1
            .as_ref()
    };

    assert!(matches!(
        outcome("valid_bool"),
        Ok(graphcal_eval::eval::Value::Bool(false))
    ));
    assert!(matches!(
        outcome("valid_int"),
        Ok(graphcal_eval::eval::Value::Int(0))
    ));
    for (name, expected) in [
        ("bad_bool", "Bool slot must be 0.0 or 1.0"),
        ("bad_quantity", "quantity must be finite"),
        ("bad_array", "array element #1"),
        ("bad_record", "field `value`"),
    ] {
        let error = outcome(name).expect_err("invalid ABI result should fail");
        let graphcal_eval::eval::NodeUnavailable::EvalFailed { message } = error else {
            panic!("expected EvalFailed, got {error:?}");
        };
        assert!(message.contains(expected), "{message}");
    }
}

#[test]
fn evaluator_rejects_fractional_host_int_results() {
    let plugin = PluginPath::new("graphcal:test-exact-int");
    let mut registry = HostFunctionRegistry::new();
    registry.register_for_test(
        plugin.clone(),
        FnName::expect_valid("fractional_scalar"),
        |_| Ok(HostFnValue::F64(3.7)),
    );
    registry.register_for_test(plugin, FnName::expect_valid("fractional_record"), |_| {
        Ok(HostFnValue::Record(vec![3.7]))
    });
    let source = r#"
type IntResult {
    IntResult(value: Int),
}
import plugin "graphcal:test-exact-int" as test {
    fn fractional_scalar() -> Int;
    fn fractional_record() -> IntResult;
}
node invalid_scalar: Int = test::fractional_scalar();
node invalid_record: IntResult = test::fractional_record();
"#;
    let project = graphcal_project::loader::LoadedProject::from_source(source, "test.gcl").unwrap();
    let result = graphcal_project::project_compiler::ProjectCompiler::new(&project)
        .host_fns(&registry)
        .eval(&HashMap::new())
        .unwrap();

    for name in ["invalid_scalar", "invalid_record"] {
        let error = result
            .nodes()
            .find(|(candidate, _)| candidate.to_string() == name)
            .unwrap_or_else(|| panic!("{name} node should exist"))
            .1
            .as_ref()
            .expect_err("fractional Int result should fail");
        let graphcal_eval::eval::NodeUnavailable::EvalFailed { message } = error else {
            panic!("expected EvalFailed, got {error:?}");
        };
        assert!(message.contains("is not integer-valued"), "{message}");
    }
}
