//! Resolution of same-leaf names to their owning modules at check and run time.

use super::*;

fn write_same_leaf_index_project(main_source: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub index Phase = { Burn, Coast };\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub index Phase = { Warm, Cold };\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

fn write_same_leaf_same_variant_index_project(
    main_source: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub index Phase = { Burn, Coast };\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub index Phase = { Burn, Coast };\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

fn write_same_leaf_range_index_project(
    main_source: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub index Step = range(0.0 s, 1.0 s, step: 1.0 s);\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub index Step = range(0.0 s, 2.0 s, step: 1.0 s);\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

fn write_same_leaf_struct_type_project(
    main_source: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub type Item { Pick(distance: Length), Idle }\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub type Item { Pick(duration: Time), Idle }\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

fn write_same_leaf_record_type_project(
    main_source: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub type Item { Item(distance: Length) }\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub type Item { Item(duration: Time) }\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

fn write_same_leaf_constrained_record_type_project(
    main_source: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub type Item { Item(distance: Length(min: 1.0 m)) }\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub type Item { Item(duration: Time(min: 1.0 s)) }\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

fn write_same_leaf_same_field_constrained_record_type_project(
    main_source: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub type Item { Item(value: Length(min: 1.0 m)) }\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub type Item { Item(value: Length(min: 10.0 m)) }\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

#[test]
fn project_match_pattern_uses_resolved_constructor_and_binding() {
    let (_dir, root) = write_same_leaf_constructor_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node action: a::Action = a::Pick(distance: 2.0 m);\n\
         node distance: Length = match @action {\n\
             a::Pick(distance: d) => d,\n\
             a::Idle => 0.0 m,\n\
         };\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn project_struct_type_uses_resolved_owner_with_same_leaf_types() {
    let (_dir, root) = write_same_leaf_struct_type_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node action: a::Item = a::Pick(distance: 2.0 m);\n\
         node command: b::Item = b::Pick(duration: 3.0 s);\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn project_struct_type_rejects_same_leaf_wrong_owner_constructor() {
    let (_dir, root) = write_same_leaf_struct_type_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node bad: a::Item = b::Pick(duration: 3.0 s);\n",
    );

    match compile_to_tir_project(&root, None, &fs()) {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Dimension(DimensionError::DimensionMismatchInAnnotation {
                            ..
                        }),
                    ..
                }),
            ..
        })) => {}
        other => panic!("expected DimensionMismatchInAnnotation, got {other:?}"),
    }
}

#[test]
fn project_field_access_uses_resolved_struct_type_def_with_same_leaf_types() {
    let (_dir, root) = write_same_leaf_record_type_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node item: a::Item = a::Item(distance: 2.0 m);\n\
         node distance: Length = @item.distance;\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn eval_constructor_calls_preserve_same_leaf_struct_owners() {
    let (_dir, root) = write_same_leaf_constructor_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node action: a::Action = a::Pick(distance: 2.0 m);\n\
         node command: b::Command = b::Pick(duration: 3.0 s);\n",
    );

    let (_tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let a_id = loaded_file_dag_id(&project, "a.gcl");
    let b_id = loaded_file_dag_id(&project, "b.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();

    let owner_of_struct = |name: &str| {
        let value = result
            .nodes()
            .find(|(n, _)| n.to_string() == name)
            .unwrap_or_else(|| panic!("node `{name}` not found"))
            .1
            .as_ref()
            .unwrap_or_else(|e| panic!("node `{name}` failed: {e}"));
        let Value::Struct {
            type_name,
            constructor,
            ..
        } = value
        else {
            panic!("expected struct value for `{name}`, got {value:?}");
        };
        assert_eq!(constructor.as_str(), "Pick");
        type_name.resolved().clone()
    };

    assert_eq!(owner_of_struct("action").owner(), &a_id);
    assert_eq!(owner_of_struct("command").owner(), &b_id);
}

#[test]
fn eval_constructor_match_uses_resolved_owner_and_binding() {
    let (_dir, root) = write_same_leaf_constructor_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node action: a::Action = a::Pick(distance: 2.0 m);\n\
         node command: b::Command = b::Pick(duration: 3.0 s);\n\
         node distance: Length = match @action {\n\
             a::Pick(distance: d) => d,\n\
             a::Idle => 0.0 m,\n\
         };\n\
         node duration: Time = match @command {\n\
             b::Pick(duration: t) => t,\n\
             b::Idle => 0.0 s,\n\
         };\n",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!((find_value(&result, "distance") - 2.0).abs() < f64::EPSILON);
    assert!((find_value(&result, "duration") - 3.0).abs() < f64::EPSILON);
}

#[test]
fn eval_field_access_uses_resolved_struct_type_def_with_same_leaf_types() {
    let (_dir, root) = write_same_leaf_record_type_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node item: a::Item = a::Item(distance: 2.0 m);\n\
         node other: b::Item = b::Item(duration: 3.0 s);\n\
         node distance: Length = @item.distance;\n",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!((find_value(&result, "distance") - 2.0).abs() < f64::EPSILON);
}

#[test]
fn eval_constructor_match_rejects_runtime_owner_mismatch_with_same_leaf_constructor() {
    let (_dir, root) = write_same_leaf_constructor_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node action: a::Action = a::Pick(distance: 2.0 m);\n\
         node distance: Length = match @action {\n\
             a::Pick(distance: d) => d,\n\
             a::Idle => 0.0 m,\n\
         };\n",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let expr_key = tir
        .root()
        .body_for_test()
        .bound_decl_identity(&scoped_name("distance"))
        .unwrap()
        .clone();
    let expr = tir
        .declaration_body(&expr_key)
        .unwrap()
        .runtime_expression()
        .unwrap();
    let b_owner = graphcal_compiler::resolved_name::ResolvedName::for_test(
        loaded_file_dag_id(&project, "b.gcl"),
        graphcal_compiler::syntax::type_name::StructTypeName::expect_valid("Command"),
    );
    let fields = vec![(
        graphcal_compiler::syntax::type_name::FieldName::expect_valid("distance"),
        graphcal_compiler::semantic::checked_type::CheckedType::Quantity(
            graphcal_compiler::dimension::Dimension::dimensionless(),
        ),
        graphcal_eval::eval_expr::RuntimeValue::quantity(9.0).unwrap(),
    )];
    let values = HashMap::from([(
        tir.root()
            .body_for_test()
            .lookup_decl_identity(&scoped_name("action"))
            .into_bound()
            .unwrap(),
        graphcal_eval::eval_expr::RuntimeValue::Struct(
            graphcal_eval::runtime_value::StructValue::for_test(
                b_owner,
                graphcal_compiler::syntax::type_name::ConstructorName::expect_valid("Pick"),
                fields,
            ),
        ),
    )]);
    let src = project.root_file().source_id();
    let sources = project.sources();
    let ctx = graphcal_eval::eval_expr::EvalSession::provisional_constants(
        &tir,
        src,
        sources,
        graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
    .for_decl(&expr_key);

    let err = graphcal_eval::eval_expr::eval_root(&ctx.executable(expr).unwrap(), &values, &ctx)
        .unwrap_err();
    // A value of another owner contradicts the checked type: no arm is
    // selected by leaf name, and the violation is an internal error.
    match err {
        Outcome::Failed(SemanticError::Internal(internal)) => {
            assert!(
                internal
                    .message()
                    .contains("a union value one arm takes apart"),
                "{}",
                internal.message()
            );
        }
        other => panic!("expected InternalError, got {other:?}"),
    }
}

#[test]
fn eval_struct_field_constraints_use_resolved_owner_with_same_leaf_types_and_fields() {
    let (_dir, root) = write_same_leaf_same_field_constrained_record_type_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node a_ok: a::Item = a::Item(value: 2.0 m);\n\
         node b_bad: b::Item = b::Item(value: 2.0 m);\n",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let a_ok = result
        .nodes()
        .find(|(n, _)| n.to_string() == "a_ok")
        .expect("node a_ok")
        .1
        .as_ref();
    assert!(
        a_ok.is_ok(),
        "a_ok should satisfy a::Item's constraint: {a_ok:?}"
    );
    let b_bad = result
        .nodes()
        .find(|(n, _)| n.to_string() == "b_bad")
        .expect("node b_bad")
        .1
        .as_ref();
    match b_bad {
        Err(NodeUnavailable::EvalFailed { message }) => {
            assert!(message.contains("minimum"), "{message}");
        }
        other => panic!("expected b_bad constraint failure, got {other:?}"),
    }
}

#[test]
fn project_declared_type_preserves_same_leaf_index_owner() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a::Phase] = for p: a::Phase { 1.0 };\n",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let a_id = loaded_file_dag_id(&project, "a.gcl");

    let graphcal_compiler::semantic::checked_type::CheckedType::Indexed { index, .. } =
        root_decl_type(&tir, "series").declared()
    else {
        panic!("expected indexed declared type for `series`");
    };
    assert_eq!(index.display_name().to_string(), "Phase");
    assert_eq!(
        index
            .declared_resolved()
            .map(graphcal_compiler::resolved_name::ResolvedName::owner,),
        Some(&a_id)
    );
}

#[test]
fn project_declared_type_preserves_same_leaf_struct_owner() {
    let (_dir, root) = write_same_leaf_record_type_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node item: a::Item = a::Item(distance: 2.0 m);\n\
         node other: b::Item = b::Item(duration: 3.0 s);\n",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let a_id = loaded_file_dag_id(&project, "a.gcl");
    let b_id = loaded_file_dag_id(&project, "b.gcl");

    let graphcal_compiler::semantic::checked_type::CheckedType::Struct(item, _) =
        root_decl_type(&tir, "item").declared()
    else {
        panic!("expected struct declared type for `item`");
    };
    let graphcal_compiler::semantic::checked_type::CheckedType::Struct(other, _) =
        root_decl_type(&tir, "other").declared()
    else {
        panic!("expected struct declared type for `other`");
    };
    assert_eq!(item.name().as_str(), "Item");
    assert_eq!(other.name().as_str(), "Item");
    assert_eq!(item.resolved().owner(), &a_id);
    assert_eq!(other.resolved().owner(), &b_id);
}

#[test]
fn project_struct_field_constraints_preserve_same_leaf_struct_owner() {
    let (_dir, root) = write_same_leaf_constrained_record_type_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node item: a::Item = a::Item(distance: 2.0 m);\n\
         node other: b::Item = b::Item(duration: 3.0 s);\n",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let src = project.root_file().source_id();
    let sources = project.sources();
    let constraints = graphcal_eval::execution_check::resolve_struct_field_constraints(
        &tir,
        &HashMap::new(),
        src,
        sources,
    )
    .unwrap();
    let a_id = loaded_file_dag_id(&project, "a.gcl");
    let b_id = loaded_file_dag_id(&project, "b.gcl");

    assert!(constraints.keys().any(|key| {
        key.owning_type.resolved().owner() == &a_id
            && key.owning_type.name().as_str() == "Item"
            && key.constructor.as_str() == "Item"
            && key.field.as_str() == "distance"
    }));
    assert!(constraints.keys().any(|key| {
        key.owning_type.resolved().owner() == &b_id
            && key.owning_type.name().as_str() == "Item"
            && key.constructor.as_str() == "Item"
            && key.field.as_str() == "duration"
    }));
    assert!(
        constraints
            .keys()
            .all(|key| !key.owning_type.resolved().owner().segments().is_empty())
    );
}

#[test]
fn nat_generic_arguments_work_in_types_constructors_and_nested_expressions() {
    let source = r"
pub type Fixed<N: Nat> {
    Fixed(value: Dimensionless),
}

pub type Matrix<M: Nat, N: Nat> {
    Matrix(value: Fixed<M * N + 1>),
}

param value: Matrix<2, 3> = Matrix<2, 3>(
    value: Fixed<7>(value: 1.0),
);
";

    let result = compile_and_eval(source).unwrap();
    let value = result
        .params()
        .find(|(name, _)| name.to_string() == "value")
        .expect("value param")
        .1
        .as_ref();
    assert!(value.is_ok(), "Nat-generic value failed: {value:?}");
}

#[test]
fn project_index_access_uses_resolved_owner_with_same_leaf_indexes() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a::Phase] = for p: a::Phase { 1.0 };\n\
         node burn: Dimensionless = @series[a::Phase#Burn];\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn project_index_access_rejects_same_leaf_wrong_owner() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a::Phase] = for p: a::Phase { 1.0 };\n\
         node bad: Dimensionless = @series[b::Phase#Warm];\n",
    );

    match compile_to_tir_project(&root, None, &fs()) {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Index(IndexError::IndexMismatch { .. }),
                    ..
                }),
            ..
        })) => {}
        other => panic!("expected IndexMismatch, got {other:?}"),
    }
}

#[test]
fn project_for_comp_rejects_same_leaf_wrong_owner() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a::Phase] = for p: b::Phase { 1.0 };\n",
    );

    match compile_to_tir_project(&root, None, &fs()) {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Dimension(DimensionError::DimensionMismatchInAnnotation {
                            ..
                        }),
                    ..
                }),
            ..
        })) => {}
        other => panic!("expected DimensionMismatchInAnnotation, got {other:?}"),
    }
}

#[test]
fn project_map_literal_uses_resolved_owner_with_same_leaf_indexes() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a::Phase] = {\n\
             a::Phase#Burn: 1.0,\n\
             a::Phase#Coast: 2.0,\n\
         };\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn project_map_literal_rejects_same_leaf_wrong_owner_key() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a::Phase] = {\n\
             a::Phase#Burn: 1.0,\n\
             b::Phase#Warm: 2.0,\n\
         };\n",
    );

    match compile_to_tir_project(&root, None, &fs()) {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Index(IndexError::IndexMismatch { .. }),
                    ..
                }),
            ..
        })) => {}
        other => panic!("expected IndexMismatch, got {other:?}"),
    }
}

#[test]
fn project_map_literal_missing_variants_uses_resolved_owner() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a::Phase] = {\n\
             a::Phase#Burn: 1.0,\n\
         };\n",
    );

    match compile_to_tir_project(&root, None, &fs()) {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Index(IndexError::MissingVariants { missing, .. }),
                    ..
                }),
            ..
        })) => {
            assert_eq!(missing.len(), 1);
            assert_eq!(
                missing[0]
                    .as_named()
                    .expect("Phase is a named index")
                    .as_str(),
                "Coast"
            );
        }
        other => panic!("expected MissingVariants, got {other:?}"),
    }
}

#[test]
fn project_table_literal_uses_resolved_owner_with_same_leaf_indexes() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a::Phase] = table[a::Phase] {\n\
             Burn: 1.0;\n\
             Coast: 2.0;\n\
         };\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn project_variant_literal_uses_resolved_owner_with_same_leaf_indexes() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a::Phase] = {\n\
             a::Phase#Burn: 1.0,\n\
             a::Phase#Coast: 2.0,\n\
         };\n\
         node burn: Dimensionless = @series[a::Phase#Burn];\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn project_label_match_uses_resolved_owner_with_same_leaf_indexes() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node code: Dimensionless[a::Phase] = for p: a::Phase {\n\
             match p {\n\
                 a::Phase#Burn => 1.0,\n\
                 a::Phase#Coast => 2.0,\n\
             }\n\
         };\n\
         node burn_code: Dimensionless = @code[a::Phase#Burn];\n",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!((find_value(&result, "burn_code") - 1.0).abs() < f64::EPSILON);
}

#[test]
fn project_label_match_rejects_same_leaf_wrong_owner_pattern() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node code: Dimensionless[a::Phase] = for p: a::Phase {\n\
             match p {\n\
                 a::Phase#Burn => 1.0,\n\
                 b::Phase#Warm => 2.0,\n\
             }\n\
         };\n",
    );

    match compile_to_tir_project(&root, None, &fs()) {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Index(IndexError::IndexMismatch { .. }),
                    ..
                }),
            ..
        })) => {}
        other => panic!("expected IndexMismatch, got {other:?}"),
    }
}

#[test]
fn project_expected_fail_keys_accept_resolved_index_owner_with_same_leaf_indexes() {
    let (_dir, root) = write_same_leaf_same_variant_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node a_checks: Bool[a::Phase] = {\n\
             a::Phase#Burn: false,\n\
             a::Phase#Coast: true,\n\
         };\n\
         #[expected_fail(a::Phase#Burn)]\n\
         assert a_expected = @a_checks;\n\
         node b_checks: Bool[b::Phase] = {\n\
             b::Phase#Burn: false,\n\
             b::Phase#Coast: true,\n\
         };\n\
         #[expected_fail(b::Phase#Burn)]\n\
         assert b_expected = @b_checks;\n",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let assert_result = |name: &str| {
        result
            .assertions
            .iter()
            .find(|assertion| assertion.name.to_string() == name)
            .unwrap_or_else(|| panic!("assertion `{name}` not found"))
            .outcome
            .clone()
    };
    assert_eq!(assert_result("a_expected"), AssertResult::Pass);
    assert_eq!(assert_result("b_expected"), AssertResult::Pass);
}

#[test]
fn included_expected_fail_resolves_index_in_its_defining_module() {
    // Regression for #1200: include assembly renames the assertion, but its
    // expected-fail index key remains scoped to and sourced from lib.gcl.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/multi/include_expected_fail_scope/src/repro/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let assertion = result
        .assertions
        .iter()
        .find(|assertion| assertion.name.to_string().contains("check"))
        .expect("included assertion not found");
    assert_eq!(assertion.outcome, AssertResult::Pass);
}

#[test]
fn project_expected_fail_keys_reject_same_leaf_wrong_owner() {
    let (_dir, root) = write_same_leaf_same_variant_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node b_checks: Bool[b::Phase] = {\n\
             b::Phase#Burn: false,\n\
             b::Phase#Coast: true,\n\
         };\n\
         #[expected_fail(a::Phase#Burn)]\n\
         assert b_expected = @b_checks;\n",
    );

    match compile_and_eval_project(&root, &HashMap::new(), None, &fs()) {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Attribute(AttributeError::ExpectedFailKeyIndexMismatch {
                            ..
                        }),
                    ..
                }),
            ..
        })) => {}
        other => {
            panic!(
                "expected ExpectedFailKeyIndexMismatch for foreign expected_fail key, got {other:?}"
            )
        }
    }
}

#[test]
fn eval_index_collections_preserve_same_leaf_owners_across_runtime_boundaries() {
    let (_dir, root) = write_same_leaf_same_variant_index_project(
        "import collide.a::{ index Phase };\n\
         import collide.a as a;\n\
         import collide.b as b;\n\
         dag pick_a {\n\
             import collide.a::{ index Phase };\n\
             param series: Dimensionless[Phase];\n\
             pub node burn: Dimensionless = @series[Phase#Burn];\n\
             pub node echoed: Dimensionless[Phase] = for p: Phase { @series[p] };\n\
         }\n\
         node map_a: Dimensionless[a::Phase] = {\n\
             a::Phase#Burn: 10.0,\n\
             a::Phase#Coast: 20.0,\n\
         };\n\
         node map_b: Dimensionless[b::Phase] = {\n\
             b::Phase#Burn: 1.0,\n\
             b::Phase#Coast: 2.0,\n\
         };\n\
         node table_a: Dimensionless[Phase] = table[Phase] {\n\
             Burn: 3.0;\n\
             Coast: 4.0;\n\
         };\n\
         node for_a: Dimensionless[a::Phase] = for p: a::Phase {\n\
             match p {\n\
                 a::Phase#Burn => @map_a[p],\n\
                 a::Phase#Coast => @pick_a(series: @map_a)::echoed[p],\n\
             }\n\
         };\n\
         node scan_a: Dimensionless[a::Phase] = scan(@map_a, 0.0, |acc, val| acc + val);\n\
         node total: Dimensionless = @pick_a(series: @map_a)::burn\n\
             + @table_a[Phase#Burn]\n\
             + @for_a[a::Phase#Coast]\n\
             + @scan_a[a::Phase#Coast];\n",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!((find_value(&result, "total") - 63.0).abs() < f64::EPSILON);

    let owner_of_indexed = |name: &str| {
        let value = result
            .nodes()
            .find(|(n, _)| n.to_string() == name)
            .unwrap_or_else(|| panic!("node `{name}` not found"))
            .1
            .as_ref()
            .unwrap_or_else(|e| panic!("node `{name}` failed: {e}"));
        let Value::Indexed { index_name, .. } = value else {
            panic!("expected indexed value for `{name}`, got {value:?}");
        };
        index_name
            .declared_resolved()
            .cloned()
            .unwrap_or_else(|| panic!("expected declared index for `{name}`"))
    };

    let a_owner = owner_of_indexed("map_a");
    let b_owner = owner_of_indexed("map_b");
    assert_ne!(a_owner, b_owner);
    assert_eq!(owner_of_indexed("table_a"), a_owner);
    assert_eq!(owner_of_indexed("for_a"), a_owner);
    assert_eq!(owner_of_indexed("scan_a"), a_owner);
}

#[test]
fn eval_unfold_uses_resolved_explicit_range_index_owner_with_same_leaf_indexes() {
    let (_dir, root) = write_same_leaf_range_index_project(
        "import collide.b as b;\n\
         import collide.a as a;\n\
         node y: Dimensionless[a::Step] = unfold(a::Step, 0.0, |prev_y, prev_t, t| prev_y + 1.0);\n",
    );

    let (_tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let a_owner = loaded_file_dag_id(&project, "a.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let value = result
        .nodes()
        .find(|(name, _)| name.to_string() == "y")
        .expect("node y")
        .1
        .as_ref()
        .expect("node y value");
    let Value::Indexed {
        index_name,
        entries,
        ..
    } = value
    else {
        panic!("expected indexed value for `y`, got {value:?}");
    };
    assert_eq!(entries.len(), 2);
    assert_eq!(
        index_name
            .declared_resolved()
            .map(graphcal_compiler::resolved_name::ResolvedName::owner),
        Some(&a_owner)
    );
}

#[test]
fn eval_index_access_rejects_runtime_owner_mismatch_with_same_leaf_variant() {
    let (_dir, root) = write_same_leaf_same_variant_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a::Phase] = {\n\
             a::Phase#Burn: 1.0,\n\
             a::Phase#Coast: 2.0,\n\
         };\n\
         node burn: Dimensionless = @series[a::Phase#Burn];\n",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let expr_key = tir
        .root()
        .body_for_test()
        .bound_decl_identity(&scoped_name("burn"))
        .unwrap()
        .clone();
    let expr = tir
        .declaration_body(&expr_key)
        .unwrap()
        .runtime_expression()
        .unwrap();
    let b_owner = graphcal_compiler::resolved_name::ResolvedName::for_test(
        loaded_file_dag_id(&project, "b.gcl"),
        graphcal_compiler::syntax::index_name::IndexName::expect_valid("Phase"),
    );
    let b_axis = graphcal_eval::runtime_value::IndexAxis::resolve(
        &tir,
        &graphcal_compiler::semantic::checked_type::IndexTypeRef::from_resolved(b_owner),
    )
    .unwrap();
    let entries = graphcal_eval::runtime_value::IndexedValue::for_test(
        b_axis,
        vec![
            graphcal_eval::eval_expr::RuntimeValue::quantity(99.0).unwrap(),
            graphcal_eval::eval_expr::RuntimeValue::quantity(100.0).unwrap(),
        ],
    );
    let values = HashMap::from([(
        tir.root()
            .body_for_test()
            .lookup_decl_identity(&scoped_name("series"))
            .into_bound()
            .unwrap(),
        graphcal_eval::eval_expr::RuntimeValue::Indexed(entries),
    )]);
    let src = project.root_file().source_id();
    let sources = project.sources();
    let ctx = graphcal_eval::eval_expr::EvalSession::provisional_constants(
        &tir,
        src,
        sources,
        graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
    .for_decl(&expr_key);

    let err = graphcal_eval::eval_expr::eval_root(&ctx.executable(expr).unwrap(), &values, &ctx)
        .unwrap_err();
    // A key of another owner contradicts the checked type: no entry is
    // selected by leaf name, and the violation is an internal error.
    match err {
        Outcome::Failed(SemanticError::Internal(internal)) => {
            assert!(
                internal
                    .message()
                    .contains("an indexed value with the entry"),
                "{}",
                internal.message()
            );
        }
        other => panic!("expected InternalError, got {other:?}"),
    }
}

#[test]
fn eval_label_match_rejects_runtime_owner_mismatch_with_same_leaf_variant() {
    let (_dir, root) = write_same_leaf_same_variant_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node code: Dimensionless[a::Phase] = for p: a::Phase {\n\
             match p {\n\
                 a::Phase#Burn => 1.0,\n\
                 a::Phase#Coast => 2.0,\n\
             }\n\
         };\n",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let expr_key = tir
        .root()
        .body_for_test()
        .bound_decl_identity(&scoped_name("code"))
        .unwrap()
        .clone();
    let expr = tir
        .declaration_body(&expr_key)
        .unwrap()
        .runtime_expression()
        .unwrap();
    let root = expr.executable().unwrap();
    let tree = root.root();
    let graphcal_compiler::tir::typed::scoped_node::NodeKind::For {
        bindings,
        body: match_expr,
    } = tree.kind()
    else {
        panic!("expected `code` to be a for-comprehension, got {tree:?}");
    };
    let [binding] = bindings.as_slice() else {
        panic!("expected one for-comprehension binding, got {bindings:?}");
    };
    let binding = &binding.binding;
    let b_owner = graphcal_compiler::resolved_name::ResolvedName::for_test(
        loaded_file_dag_id(&project, "b.gcl"),
        graphcal_compiler::syntax::index_name::IndexName::expect_valid("Phase"),
    );
    let values = HashMap::new();
    let local_values = graphcal_eval::eval_expr::HirLocalValueMap::from_bindings(vec![(
        binding.local.id,
        graphcal_eval::runtime_presentation::EvaluatedRuntimeValue::plain(
            graphcal_eval::eval_expr::RuntimeValue::Key(
                graphcal_eval::runtime_value::KeyValue::for_entry(
                    graphcal_eval::runtime_value::IndexAxis::resolve(
                        &tir,
                        &graphcal_compiler::semantic::checked_type::IndexTypeRef::from_resolved(
                            b_owner,
                        ),
                    )
                    .unwrap(),
                    &graphcal_compiler::syntax::index_name::IndexEntryKey::named(
                        graphcal_compiler::syntax::index_name::IndexVariantName::expect_valid(
                            "Burn",
                        ),
                    ),
                )
                .unwrap(),
            ),
        ),
    )]);
    let src = project.root_file().source_id();
    let sources = project.sources();
    let ctx = graphcal_eval::eval_expr::EvalSession::provisional_constants(
        &tir,
        src,
        sources,
        graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
    .for_decl(&expr_key);

    let err =
        graphcal_eval::eval_expr::eval_subtree_for_test(match_expr, &values, &local_values, &ctx)
            .unwrap_err();
    // A value of another owner contradicts the checked type: no arm is
    // selected by leaf name, and the violation is an internal error.
    match err {
        Outcome::Failed(SemanticError::Internal(internal)) => {
            assert!(
                internal.message().contains("operand checked as a key of"),
                "{}",
                internal.message()
            );
        }
        other => panic!("expected InternalError, got {other:?}"),
    }
}
