#![allow(
    clippy::expect_used,
    reason = "test-only generated invariants should fail immediately and explicitly"
)]

use std::collections::HashMap;

use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::syntax::module_name::ModuleAliasName;
use graphcal_eval::eval::{CompileError, Value, compile_and_eval_project};
use graphcal_io::{InMemoryFileSystem, VirtualAbsolutePath};
use graphcal_test_support::project::{
    ExpectedArtifact, GeneratedProject, GenerationLimits, SemanticErrorClass,
    multi_owner_project_strategy, presentation_project_strategy, single_file_project_strategy,
};
use proptest::prelude::*;

#[expect(
    clippy::result_large_err,
    reason = "integration-test helper preserves the production error for typed matching"
)]
fn evaluate(project: &GeneratedProject) -> Result<graphcal_eval::eval::EvalResult, CompileError> {
    let rendered = project.render();
    let root = VirtualAbsolutePath::new("/project").expect("static virtual root");
    let mut fs = InMemoryFileSystem::new();
    for (path, source) in rendered.files() {
        fs.add_file(
            VirtualAbsolutePath::new(root.as_path().join(path))
                .expect("generated paths are relative and bounded"),
            source.to_string(),
        )
        .expect("generated files form a valid topology");
    }
    compile_and_eval_project(
        &root.as_path().join(rendered.root()),
        &HashMap::new(),
        Some(root.as_path()),
        &fs,
    )
}

fn assert_expected(project: &GeneratedProject) -> Result<(), TestCaseError> {
    let result = evaluate(project).map_err(|error| {
        TestCaseError::fail(format!(
            "generated project failed to reach evaluation:\n{}\n{error:?}",
            project.render().root_source()
        ))
    })?;
    match project.expected() {
        ExpectedArtifact::DimensionlessInteger { name, value } => {
            let evaluated = generated_node(&result, name)?;
            let Value::Quantity {
                si_value,
                dimension,
                ..
            } = evaluated
            else {
                return Err(TestCaseError::fail("generated result was not a quantity"));
            };
            prop_assert!(dimension.is_dimensionless());
            let expected = value.to_string().parse::<f64>().expect("bounded integer");
            prop_assert!((*si_value - expected).abs() < f64::EPSILON);
        }
        ExpectedArtifact::PresentedQuantity {
            name,
            si_value,
            display_unit,
            display_scale,
        } => {
            let evaluated = generated_node(&result, name)?;
            let Value::Quantity {
                si_value: actual_si,
                display_unit: Some(actual_unit),
                ..
            } = evaluated
            else {
                return Err(TestCaseError::fail(
                    "generated result lacked quantity presentation",
                ));
            };
            let expected_si = si_value
                .to_string()
                .parse::<f64>()
                .expect("bounded integer");
            let expected_scale = display_scale
                .to_string()
                .parse::<f64>()
                .expect("bounded positive integer");
            prop_assert!((*actual_si - expected_si).abs() < f64::EPSILON);
            prop_assert_eq!(actual_unit.label.as_str(), display_unit.as_str());
            prop_assert!((actual_unit.scale() - expected_scale).abs() < f64::EPSILON);
            let projected =
                graphcal_eval::eval::quantity_display_value(*actual_si, Some(actual_unit))
                    .map_err(|error| TestCaseError::fail(error.to_string()))?;
            prop_assert!((projected - expected_si / expected_scale).abs() < f64::EPSILON);
        }
    }
    Ok(())
}

fn generated_node<'a>(
    result: &'a graphcal_eval::eval::EvalResult,
    name: &graphcal_compiler::syntax::decl_name::DeclName,
) -> Result<&'a Value, TestCaseError> {
    result
        .nodes
        .iter()
        .find(|(candidate, _)| !candidate.is_qualified() && candidate.member() == name)
        .ok_or_else(|| TestCaseError::fail(format!("missing generated node `{name}`")))?
        .1
        .as_ref()
        .map_err(|error| TestCaseError::fail(format!("generated node failed: {error:?}")))
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn valid_single_file_projects_reach_tir_and_evaluation(
        project in single_file_project_strategy(GenerationLimits::SMOKE)
    ) {
        assert_expected(&project)?;
    }

    #[test]
    fn generated_presentation_matches_the_typed_scale_and_label(
        project in presentation_project_strategy(GenerationLimits::SMOKE)
    ) {
        assert_expected(&project)?;
    }

    #[test]
    fn owner_order_and_alpha_renaming_preserve_the_declared_artifact(
        project in multi_owner_project_strategy(GenerationLimits::SMOKE)
    ) {
        assert_expected(&project)?;
        assert_expected(&project.reverse_module_storage())?;
        let renamed = project.rename_alias(
            &ModuleAliasName::expect_valid("left_owner"),
            &ModuleAliasName::expect_valid("fresh_owner"),
        );
        assert_expected(&renamed)?;
    }

    #[test]
    fn controlled_dimension_mutations_have_the_expected_semantic_class(
        project in single_file_project_strategy(GenerationLimits::SMOKE)
    ) {
        let invalid = project.with_dimension_mismatch();
        prop_assert_eq!(
            invalid.expected_error(),
            SemanticErrorClass::DimensionMismatch
        );
        let rendered = invalid.render();
        let error = graphcal_eval::eval::compile_and_eval_named(
            rendered.root_source(),
            "main.gcl",
        )
        .expect_err("controlled dimension mutation must fail");
        prop_assert!(
            matches!(
                error,
                CompileError::Eval(
                    GraphcalError::DimensionMismatch { .. }
                        | GraphcalError::DimensionMismatchInAnnotation { .. }
                )
            ),
            "controlled mutation produced the wrong diagnostic: {error:?}"
        );
    }
}
