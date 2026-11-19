//! The project type store's canonical owners for imported definitions.

use super::*;

#[test]
fn diamond_imports_install_one_canonical_shared_definition() {
    use graphcal_compiler::resolved_name::{ResolvedDimName, ResolvedUnitName};
    use graphcal_compiler::syntax::dimension::{DimName, UnitName};

    let (_directory, root) = write_pipeline_project(
        &[
            (
                "shared.gcl",
                "pub base dim SharedMeasure;\npub base unit su: SharedMeasure;\n",
            ),
            (
                "left.gcl",
                "import pipeline.shared as left_shared;\nconst node left_value: left_shared::SharedMeasure = 1.0 left_shared::su;\n",
            ),
            (
                "right.gcl",
                "import pipeline.shared as right_shared;\nconst node right_value: right_shared::SharedMeasure = 2.0 right_shared::su;\n",
            ),
            (
                "main.gcl",
                "import pipeline.left as left;\nimport pipeline.right as right;\nnode result: Dimensionless = 1.0;\n",
            ),
        ],
        "main.gcl",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let shared = loaded_file_dag_id(&project, "shared.gcl");
    let aliases = [
        loaded_file_dag_id(&project, "left.gcl"),
        loaded_file_dag_id(&project, "right.gcl"),
        tir.root_dag_id().clone(),
    ];
    let dimension = DimName::expect_valid("SharedMeasure");
    let unit = UnitName::expect_valid("su");

    assert!(
        tir.dimension(&ResolvedDimName::for_test(
            shared.clone(),
            dimension.clone()
        ))
        .is_some()
    );
    assert!(
        tir.unit_info(&ResolvedUnitName::for_test(shared, unit.clone()))
            .is_some()
    );
    for alias_owner in aliases {
        assert!(
            tir.dimension(&ResolvedDimName::for_test(
                alias_owner.clone(),
                dimension.clone(),
            ))
            .is_none()
        );
        assert!(
            tir.unit_info(&ResolvedUnitName::for_test(alias_owner, unit.clone()))
                .is_none()
        );
    }
}

#[test]
fn same_leaf_definitions_from_distinct_modules_keep_distinct_canonical_owners() {
    use graphcal_compiler::resolved_name::{ResolvedDimName, ResolvedUnitName};
    use graphcal_compiler::syntax::dimension::{DimName, UnitName};

    let (_directory, root) = write_pipeline_project(
        &[
            (
                "left.gcl",
                "pub base dim Measure;\npub base unit u: Measure;\n",
            ),
            (
                "right.gcl",
                "pub base dim Measure;\npub base unit u: Measure;\n",
            ),
            (
                "main.gcl",
                "import pipeline.left as left;\nimport pipeline.right as right;\nnode left_value: left::Measure = 1.0 left::u;\nnode right_value: right::Measure = 2.0 right::u;\n",
            ),
        ],
        "main.gcl",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let left = loaded_file_dag_id(&project, "left.gcl");
    let right = loaded_file_dag_id(&project, "right.gcl");
    let root_owner = tir.root_dag_id().clone();
    let dimension = DimName::expect_valid("Measure");
    let unit = UnitName::expect_valid("u");

    for owner in [left, right] {
        assert!(
            tir.dimension(&ResolvedDimName::for_test(owner.clone(), dimension.clone()))
                .is_some()
        );
        assert!(
            tir.unit_info(&ResolvedUnitName::for_test(owner, unit.clone()))
                .is_some()
        );
    }
    assert!(
        tir.dimension(&ResolvedDimName::for_test(root_owner.clone(), dimension,))
            .is_none()
    );
    assert!(
        tir.unit_info(&ResolvedUnitName::for_test(root_owner, unit))
            .is_none()
    );
}

#[test]
fn checked_project_preparation_does_not_recompile_dependencies() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "import plugin \"graphcal:session-test\" as test {\n    fn count(x: Dimensionless) -> Dimensionless;\n}\npub node output: Dimensionless = test::count(2.0);\n",
            ),
            (
                "main.gcl",
                "include pipeline.lib()::{ output };\nnode result: Dimensionless = @output;\n",
            ),
        ],
        "main.gcl",
    );
    let project = crate::loader::load_project(&root, None, &fs()).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = Arc::clone(&calls);
    let mut host_fns = graphcal_eval::host_fns::HostFunctionRegistry::new();
    register_test_host_fn(
        &mut host_fns,
        "graphcal:session-test",
        "count",
        &["x"],
        graphcal_compiler::function_signature::NamedParamKind::dimensionless(),
        move |arguments| {
            observed_calls.fetch_add(1, Ordering::SeqCst);
            Ok(graphcal_eval::host_fns::HostFnValue::from_argument(
                &arguments[0],
            ))
        },
    );

    let checked = ProjectCompiler::new(&project)
        .host_fns(&host_fns)
        .check()
        .unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "static checking invoked a callable host function"
    );
    let prepared = checked.prepare_with_host_fns(&host_fns).unwrap();
    let row = prepared.binding_builder().finish().unwrap();
    let result = prepared.evaluate(&row).unwrap();
    assert_quantity_value(&result, "result", 2.0);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the explicit include should invoke its host function exactly once"
    );
}
