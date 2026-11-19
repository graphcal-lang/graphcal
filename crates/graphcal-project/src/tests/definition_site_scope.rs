//! Imported definitions keep their definition-site scope.

use super::*;

fn write_custom_unit_constrained_record_type_project(
    main_source: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/record_scope");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"record_scope\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("schema.gcl"),
        "pub base dim Currency;\n\
         pub base unit credit: Currency;\n\
         pub dim BaseRate = Length / Time;\n\
         pub dim WeightedRate = BaseRate * Mass;\n\
         pub dim ScaledRate = WeightedRate / Time;\n\
         pub type Price { Price(amount: Currency(min: 0.0 credit)) }\n\
         pub type Basket { Basket(price: Price) }\n\
         pub type Receipt { Receipt(basket: Basket) }\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

fn write_imported_unit_dependency_project(
    main_source: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/unit_dependencies");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"unit_dependencies\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("foundation.gcl"),
        "pub base dim Score;\n\
         pub base unit point: Score;\n\
         pub type Measurement { Measurement(value: Score) }\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("rates.gcl"),
        "import unit_dependencies.foundation as foundation;\n\
         pub dim ScoreRate = foundation::Score / Time;\n\
         pub const unit point_per_second: ScoreRate = 1.0 foundation::point / s;\n\
         pub type RateMeasurement { RateMeasurement(value: ScoreRate) }\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

// #1087: imported type constraints retain the defining module's unit scope.
#[test]
fn imported_record_field_constraint_uses_defining_unit_scope_through_module_alias() {
    let (_dir, root) = write_custom_unit_constrained_record_type_project(
        "import record_scope.schema as schema;\n\
         base dim ConsumerCurrency;\n\
         base unit credit: ConsumerCurrency;\n\
         param price: schema::Price;\n\
         node result: Dimensionless = 1.0;\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn imported_record_field_constraint_uses_defining_unit_scope_through_selective_type_import() {
    let (_dir, root) = write_custom_unit_constrained_record_type_project(
        "import record_scope.schema::{type Price};\n\
         base dim ConsumerCurrency;\n\
         base unit credit: ConsumerCurrency;\n\
         param price: Price;\n\
         node result: Dimensionless = 1.0;\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn prepared_imported_record_binding_uses_canonical_nested_constructors_and_units() {
    let (_dir, root) = write_custom_unit_constrained_record_type_project(
        "import record_scope.schema::{type Receipt};\n\
         param receipt: Receipt;\n\
         pub node accepted: Bool = true;\n",
    );
    let project = crate::loader::load_project(&root, None, &fs()).unwrap();
    let prepared = prepare_from_project(&project).unwrap();

    let mut bindings = prepared.binding_builder();
    bindings
        .bind_expression(
            &graphcal_compiler::syntax::decl_name::DeclName::expect_valid("receipt"),
            &parse_expr("Receipt(basket: Basket(price: Price(amount: 1.0 credit)))"),
        )
        .unwrap();
    let result = prepared.evaluate(&bindings.finish().unwrap()).unwrap();
    assert!(
        result
            .nodes()
            .any(|(name, value)| name.leaf().as_str() == "accepted"
                && matches!(value, Ok(Value::Bool(true))))
    );
}

#[test]
fn prepared_imported_record_binding_enforces_nested_definition_site_constraint() {
    let (_dir, root) = write_custom_unit_constrained_record_type_project(
        "import record_scope.schema::{type Receipt};\n\
         param receipt: Receipt;\n\
         pub node accepted: Bool = true;\n",
    );
    let project = crate::loader::load_project(&root, None, &fs()).unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let mut bindings = prepared.binding_builder();

    let error = bindings
        .bind_expression(
            &graphcal_compiler::syntax::decl_name::DeclName::expect_valid("receipt"),
            &parse_expr("Receipt(basket: Basket(price: Price(amount: -1.0 credit)))"),
        )
        .unwrap_err();
    assert!(
        error.to_string().contains("Price.amount") && error.to_string().contains("below minimum"),
        "unexpected error: {error}"
    );
}

#[test]
fn selectively_imported_dimension_retains_transitive_definition_site_dependencies() {
    let (_dir, root) = write_custom_unit_constrained_record_type_project(
        "import record_scope.schema::{dim ScaledRate};\n\
         param rate: ScaledRate;\n\
         pub node accepted: Bool = true;\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn module_imported_dimension_retains_transitive_definition_site_dependencies() {
    let (_dir, root) = write_custom_unit_constrained_record_type_project(
        "import record_scope.schema as schema;\n\
         param rate: schema::ScaledRate;\n\
         pub node accepted: Bool = true;\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn selectively_imported_record_retains_transitive_definition_site_field_types() {
    let (_dir, root) = write_custom_unit_constrained_record_type_project(
        "import record_scope.schema::{type Receipt};\n\
         param receipt: Receipt;\n\
         pub node accepted: Bool = true;\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn selectively_imported_base_unit_retains_custom_dimension_dependency() {
    let (_dir, root) = write_imported_unit_dependency_project(
        "import unit_dependencies.foundation::{type Measurement, unit point};\n\
         param measurement: Measurement;\n\
         assert nonnegative = @measurement.value >= 0.0 point;\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn selectively_imported_derived_unit_retains_aliased_dependencies() {
    let (_dir, root) = write_imported_unit_dependency_project(
        "import unit_dependencies.rates::{type RateMeasurement, unit point_per_second};\n\
         param rate: RateMeasurement;\n\
         assert nonnegative = @rate.value >= 0.0 point_per_second;\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn module_imported_unit_retains_custom_dimension_dependency() {
    let (_dir, root) = write_imported_unit_dependency_project(
        "import unit_dependencies.rates as rates;\n\
         param rate: rates::RateMeasurement;\n\
         assert nonnegative = @rate.value >= 0.0 rates::point_per_second;\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn selectively_imported_unit_does_not_expose_its_backing_dimension_name() {
    let (_dir, root) = write_imported_unit_dependency_project(
        "import unit_dependencies.foundation::{unit point};\n\
         param score: Score;\n\
         assert nonnegative = @score >= 0.0 point;\n",
    );

    match compile_to_tir_project(&root, None, &fs()) {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Dimension(DimensionError::UnknownDimension { name, .. }),
                    ..
                }),
            ..
        })) => {
            assert_eq!(name.to_string(), "Score");
        }
        other => panic!("expected UnknownDimension for Score, got {other:?}"),
    }
}

#[test]
fn project_constructor_call_uses_resolved_owner_with_same_leaf_constructors() {
    let (_dir, root) = write_same_leaf_constructor_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node action: a::Action = a::Pick(distance: 2.0 m);\n\
         node command: b::Command = b::Pick(duration: 3.0 s);\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}
