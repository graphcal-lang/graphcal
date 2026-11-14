//! Include and call instances: bindings, identity, dynamic units, and failure naming.

use super::*;

fn assert_missing_dag_bindings(
    error: CompileError,
    expected_dag_name: &str,
    expected_missing: &[&str],
) {
    match error {
        CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Graph(GraphError::MissingDagBindings {
                            missing,
                            dag_name,
                            ..
                        }),
                    ..
                }),
            ..
        }) => {
            assert_eq!(dag_name.to_string(), expected_dag_name);
            assert_eq!(
                missing.iter().map(ToString::to_string).collect::<Vec<_>>(),
                expected_missing
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            );
        }
        other => panic!("expected MissingDagBindings, got {other:?}"),
    }
}

#[test]
fn prepared_interface_contains_only_direct_hir_source_declarations() {
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "param nested_input: Dimensionless = 2.0;\n\
                 pub node nested_output: Dimensionless = @nested_input;\n",
            ),
            (
                "main.gcl",
                "include pipeline.lib() as nested;\n\
                 param direct_input: Dimensionless = 3.0;\n\
                 node private_output: Dimensionless = @direct_input;\n\
                 pub node public_output: Dimensionless = @private_output;\n",
            ),
        ],
        "main.gcl",
    );
    let project = crate::loader::load_project(&root, None, &fs()).unwrap();
    let prepared = prepare_from_project(&project).unwrap();

    let [parameter] = prepared.parameter_ports() else {
        panic!("expected exactly one direct entry parameter");
    };
    assert_eq!(parameter.name().as_str(), "direct_input");
    assert!(parameter.has_default());

    let outputs = prepared.output_ports();
    assert_eq!(
        outputs
            .iter()
            .map(|output| output.name().as_str())
            .collect::<Vec<_>>(),
        ["private_output", "public_output"]
    );
    assert!(!outputs[0].is_public());
    assert!(outputs[1].is_public());
}

#[test]
fn empty_and_default_equivalent_includes_share_instance_semantics() {
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "param input: Dimensionless = 2.0;\npub node output: Dimensionless = @input * 3.0;\nassert positive = @output > 0.0;\n",
            ),
            (
                "main.gcl",
                "include pipeline.lib() as defaults;\ninclude pipeline.lib(input: 2.0) as configured;\n",
            ),
        ],
        "main.gcl",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert_quantity_value(&result, "defaults::output", 6.0);
    assert_quantity_value(&result, "configured::output", 6.0);
    assert_eq!(
        result
            .assertions
            .iter()
            .map(|assertion| (assertion.name.to_string(), &assertion.outcome))
            .collect::<Vec<_>>(),
        [
            ("defaults::positive".to_string(), &AssertResult::Pass),
            ("configured::positive".to_string(), &AssertResult::Pass),
        ]
    );
}

#[test]
fn repeated_empty_includes_keep_distinct_output_names() {
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "param input: Dimensionless = 2.0;\npub node output: Dimensionless = @input;\n",
            ),
            (
                "main.gcl",
                "include pipeline.lib() as first;\ninclude pipeline.lib() as second;\n",
            ),
        ],
        "main.gcl",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert_quantity_value(&result, "first::output", 2.0);
    assert_quantity_value(&result, "second::output", 2.0);
}

#[test]
fn check_rejects_all_unbound_required_params_in_same_file_include() {
    let error = compile_to_tir(
        "dag scale {\n\
         param factor: Dimensionless;\n\
         param value: Dimensionless;\n\
         pub node out: Dimensionless = @factor * @value;\n\
         }\n\
         include scale()::{ out };\n",
        "test.gcl",
    )
    .expect_err("an include must bind every required param");

    assert_missing_dag_bindings(error, "scale", &["factor", "value"]);
}

#[test]
fn check_rejects_partially_bound_required_params_in_same_file_include() {
    let error = compile_to_tir(
        "dag scale {\n\
         param factor: Dimensionless;\n\
         param value: Dimensionless;\n\
         pub node out: Dimensionless = @factor * @value;\n\
         }\n\
         include scale(value: 3.0)::{ out };\n",
        "test.gcl",
    )
    .expect_err("a partial include must report its remaining required params");

    assert_missing_dag_bindings(error, "scale", &["factor"]);
}

#[test]
fn check_rejects_unbound_required_params_in_cross_file_include() {
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "param factor: Dimensionless;\n\
                 pub node out: Dimensionless = @factor * 2.0;\n",
            ),
            ("main.gcl", "include pipeline.lib()::{ out };\n"),
        ],
        "main.gcl",
    );

    let error = compile_to_tir_project(&root, None, &fs())
        .expect_err("a cross-file include must bind every required param");
    assert_missing_dag_bindings(error, "pipeline.lib", &["factor"]);
}

#[test]
fn nested_configured_includes_keep_instance_local_bindings_and_assertions() {
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "leaf.gcl",
                "param input: Dimensionless;\npub node output: Dimensionless = @input;\nassert positive = @output > 0.0;\n",
            ),
            (
                "middle.gcl",
                "param input: Dimensionless;\ninclude pipeline.leaf(input: @input) as leaf;\npub node output: Dimensionless = @leaf::output;\n",
            ),
            (
                "main.gcl",
                "include pipeline.middle(input: 2.0) as first;\ninclude pipeline.middle(input: 5.0) as second;\n",
            ),
        ],
        "main.gcl",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert_quantity_value(&result, "first::output", 2.0);
    assert_quantity_value(&result, "second::output", 5.0);
    assert_eq!(
        result
            .assertions
            .iter()
            .filter(|assertion| assertion.outcome == AssertResult::Pass)
            .count(),
        2
    );
}

#[test]
fn pure_imports_reject_assertion_outcomes_and_runtime_unit_scales() {
    let (_directory, assertion_root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "param rate: Dimensionless = 2.0;\npub assert rate_positive = @rate > 0.0;\n",
            ),
            (
                "main.gcl",
                "import pipeline.lib::{ rate_positive };\n#[assumes(rate_positive)]\nnode checked: Bool = true;\n",
            ),
        ],
        "main.gcl",
    );
    let assertion_error =
        compile_to_tir_project(&assertion_root, None, &fs()).expect_err("assert import must fail");
    assert!(
        matches!(
            assertion_error,
            CompileError::Eval(RenderedSemanticError {
                error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Module(ModuleError::ImportAssertionItem { .. }),
                    ..
                }),
                ..
            })
        ),
        "unexpected assertion-import error: {assertion_error:?}"
    );

    let (_directory, unit_root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "pub base dim Money;\npub base unit USD: Money;\nparam rate: Dimensionless = 2.0;\npub unit EUR: Money = (@rate) USD;\npub unit DoubleUSD: Money = 2.0 USD;\n",
            ),
            (
                "main.gcl",
                "import pipeline.lib as lib;\nnode price: lib::Money = 3.0 lib::EUR;\n",
            ),
        ],
        "main.gcl",
    );
    let unit_error =
        compile_to_tir_project(&unit_root, None, &fs()).expect_err("runtime unit import must fail");
    assert!(
        matches!(
            unit_error,
            CompileError::Eval(RenderedSemanticError {
                error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Module(ModuleError::ImportRuntimeUnit { .. }),
                    ..
                }),
                ..
            })
        ),
        "unexpected runtime-unit import error: {unit_error:?}"
    );

    std::fs::write(
        &unit_root,
        "import pipeline.lib as lib;\nconst unit LocalEUR: lib::Money = 1.0 lib::EUR;\n",
    )
    .unwrap();
    let unit_definition_error = compile_to_tir_project(&unit_root, None, &fs())
        .expect_err("runtime unit in a static unit definition must fail");
    assert!(
        matches!(
            unit_definition_error,
            CompileError::Eval(RenderedSemanticError {
                error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Module(ModuleError::ImportRuntimeUnit { .. }),
                    ..
                }),
                ..
            })
        ),
        "unexpected unit-definition import error: {unit_definition_error:?}"
    );

    std::fs::write(
        &unit_root,
        "import pipeline.lib::{ unit DoubleUSD };\nnode price: Money = 1.0 DoubleUSD;\n",
    )
    .unwrap();
    let constant_scale_runtime_error = compile_to_tir_project(&unit_root, None, &fs())
        .expect_err("plain units remain runtime even with a constant scale expression");
    assert!(
        matches!(
            constant_scale_runtime_error,
            CompileError::Eval(RenderedSemanticError {
                error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Module(ModuleError::ImportRuntimeUnit { .. }),
                    ..
                }),
                ..
            })
        ),
        "unexpected constant-scale runtime-unit error: {constant_scale_runtime_error:?}"
    );
}

#[test]
fn pure_module_import_can_call_an_instance_with_dynamic_units() {
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "pub base dim Money;\npub base unit USD: Money;\nparam rate: Dimensionless = 2.0;\npub unit EUR: Money = (@rate) USD;\nparam amount: Money = 100.0 EUR;\npub node converted: Money = @amount -> USD;\n",
            ),
            (
                "main.gcl",
                "import pipeline.lib as lib;\nnode result: lib::Money = @lib()::converted;\n",
            ),
        ],
        "main.gcl",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert_quantity_value(&result, "result", 200.0);
}

#[test]
fn repeated_dag_calls_keep_dynamic_unit_scales_instance_scoped() {
    // Regression: expression evaluation must retain each concrete DAG scope;
    // resolving these scales against the root DAG would either fail or conflate calls.
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "pub base dim Money;\npub base unit USD: Money;\nparam rate: Dimensionless = 2.0;\npub unit EUR: Money = (@rate) USD;\nparam amount: Money = 100.0 EUR;\npub node converted: Money = @amount -> USD;\n",
            ),
            (
                "main.gcl",
                "import pipeline.lib as lib;\nnode low: lib::Money = @lib(rate: 1.5)::converted;\nnode high: lib::Money = @lib(rate: 3.0)::converted;\n",
            ),
        ],
        "main.gcl",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert_quantity_value(&result, "low", 150.0);
    assert_quantity_value(&result, "high", 300.0);
}

#[test]
fn shared_modules_keep_equal_static_instances_and_dynamic_units_independent() {
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "pub(bind) dim Measure; param measured: Measure; pub node measurement: Measure = @measured; param scale: Dimensionless; pub unit step: Length = (@scale) m; param amount: Length = 3.0 step; pub node out: Length = @amount -> step;",
            ),
            (
                "main.gcl",
                "include pipeline.lib(dim Measure: Length, measured: 1.0 m, scale: 1.0) as first; include pipeline.lib(dim Measure: Length, measured: 2.0 m, scale: 2.0) as second; node low: Length = @first::out; node high: Length = @second::out; node first_measurement: Length = @first::measurement; node second_measurement: Length = @second::measurement;",
            ),
        ],
        "main.gcl",
    );
    let project = crate::loader::load_project(&root, None, &fs()).unwrap();
    let checked = ProjectCompiler::new(&project).check().unwrap();
    let instances = checked.tir().root().semantic_instances();
    assert_eq!(instances.len(), 2);
    assert_eq!(
        instances[0].instance.specialization(),
        instances[1].instance.specialization()
    );
    assert_ne!(instances[0].instance.id(), instances[1].instance.id());
    let prepared = ProjectCompiler::new(&project).prepare().unwrap();
    let row = prepared.binding_builder().finish().unwrap();
    let result = prepared.evaluate(&row).unwrap();
    assert!(!result.has_errors(), "{result:?}");
    assert_quantity_value(&result, "low", 3.0);
    assert_quantity_value(&result, "high", 6.0);
    assert_quantity_value(&result, "first_measurement", 1.0);
    assert_quantity_value(&result, "second_measurement", 2.0);
    for (name, expected_scale) in [("low", 1.0), ("high", 2.0)] {
        let value = result
            .nodes()
            .find(|(key, _)| *key == &scoped_name(name))
            .unwrap()
            .1
            .as_ref()
            .unwrap();
        match value {
            Value::Quantity {
                display_unit: Some(unit),
                ..
            } => assert!((unit.scale.get() - expected_scale).abs() < f64::EPSILON),
            other => panic!("expected instance-owned display unit, got {other:?}"),
        }
    }
}

/// A selective include item is materialized as a local alias in the
/// including DAG, and the include edge records that, so the exposed name
/// denotes the alias rather than the instance declaration; a whole-module
/// include member denotes the instance declaration.
#[test]
fn include_edges_record_which_exposed_values_have_local_alias_bodies() {
    use graphcal_compiler::ir::entry::ValueOrigin;
    use graphcal_compiler::ir::instance::ExposedValueBody;

    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "param factor: Dimensionless;\npub node output: Dimensionless = @factor * 2.0;\n",
            ),
            (
                "main.gcl",
                "include pipeline.lib(factor: 2.0)::{output as renamed};\ninclude pipeline.lib(factor: 5.0) as high;\nnode total: Dimensionless = @renamed + @high::output;\n",
            ),
        ],
        "main.gcl",
    );

    let (tir, _project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let body = tir.root().body_for_test();
    let bodies = body
        .semantic_instances()
        .iter()
        .flat_map(|record| {
            tir.dag_registry()
                .semantic_instance(record)
                .expect("materialized instance")
                .output_projections()
                .map(|resolved| {
                    (
                        record.instance.exposed_name(resolved.projection),
                        resolved.projection.body(),
                        resolved.target,
                    )
                })
        })
        .collect::<Vec<_>>();
    let renamed = scoped_name("renamed");
    let (_, renamed_body, _) = bodies
        .iter()
        .find(|(name, ..)| name == &renamed)
        .expect("the selected projection");
    assert_eq!(*renamed_body, ExposedValueBody::LocalAlias);
    let alias = body.bound_decl_identity(&renamed).unwrap();
    assert_eq!(alias.owner(), tir.root_dag_id());
    // The generated alias carries a typed origin; the declared-type check
    // trusts its body by that origin, never by its name.
    let origin_of = |name: &str| {
        body.nodes()
            .find(|node| node.name().as_str() == name)
            .map(graphcal_compiler::ir::entry::NodeEntry::origin)
    };
    assert_eq!(origin_of("renamed"), Some(ValueOrigin::IncludeAlias));
    assert_eq!(origin_of("total"), Some(ValueOrigin::Source));

    let member = member_name(&["high"], "output");
    let (_, member_body, member_target) = bodies
        .iter()
        .find(|(name, ..)| name == &member)
        .expect("the member projection");
    assert_eq!(*member_body, ExposedValueBody::Instance);
    assert_eq!(body.bound_decl_identity(&member), Some(member_target));

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert_quantity_value(&result, "total", 14.0);
}

/// Unavailability reasons name declarations as the output does: instance
/// members by their scopes, a private include scope by its readable name,
/// and an inline DAG a call invokes by its scope below the root.
#[test]
fn unavailability_reasons_name_declarations_as_the_output_does() {
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "node pending: Dimensionless = todo<Dimensionless> {};\nnode bad: Dimensionless = 1.0 / 0.0;\npub node output: Dimensionless = @pending;\npub node broken: Dimensionless = @bad;\n",
            ),
            (
                "main.gcl",
                "include pipeline.lib() as l;\ninclude pipeline.lib()::{output as picked, broken as busted};\ndag helper {\n    node inner: Dimensionless = todo<Dimensionless> {};\n    pub node out: Dimensionless = @inner;\n}\nnode total: Dimensionless = @l::output + @picked;\nnode sad: Dimensionless = @l::broken + @busted;\nnode called: Dimensionless = @helper()::out;\nassert waits = @l::output > 0.0;\nassert fails = @sad > 0.0;\n",
            ),
        ],
        "main.gcl",
    );
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let reason = |name: &str| {
        result
            .entries
            .iter()
            .find(
                |EvalEntry {
                     name: candidate, ..
                 }| candidate.to_string() == name,
            )
            .unwrap_or_else(|| panic!("missing `{name}`"))
            .outcome
            .as_ref()
            .expect_err("unavailable")
            .to_string()
    };
    assert_eq!(
        reason("total"),
        "BLOCKED — unfinished dependencies: lib::pending, l::pending"
    );
    assert_eq!(reason("sad"), "dependency failed: busted, l::broken");
    assert_eq!(reason("busted"), "dependency failed: lib::broken");
    assert_eq!(
        reason("l::output"),
        "BLOCKED — unfinished dependencies: l::pending"
    );
    assert_eq!(
        reason("called"),
        "BLOCKED — unfinished dependencies: helper::inner"
    );
    let assertion = |name: &str| {
        result
            .assertions
            .iter()
            .find(|assertion| assertion.name.to_string() == name)
            .unwrap_or_else(|| panic!("missing assertion `{name}`"))
            .outcome
            .clone()
    };
    let AssertResult::Blocked { reason } = assertion("waits") else {
        panic!("`waits` must be blocked");
    };
    assert_eq!(
        reason.to_string(),
        "BLOCKED — unfinished dependencies: l::pending"
    );
    let AssertResult::Error { message } = assertion("fails") else {
        panic!("`fails` must report its failed dependency");
    };
    assert_eq!(message, "dependency failed: sad");
}

#[test]
fn checked_tir_records_typed_template_instance_bindings() {
    use graphcal_compiler::resolved_name::ResolvedDeclName;

    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "param factor: Dimensionless;\npub node output: Dimensionless = @factor * 2.0;\n",
            ),
            (
                "main.gcl",
                "include pipeline.lib(factor: 2.0) as low;\ninclude pipeline.lib(factor: 5.0) as high;\n",
            ),
        ],
        "main.gcl",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let template = loaded_file_dag_id(&project, "lib.gcl");
    let template_param =
        ResolvedDeclName::for_test(template.clone(), DeclName::expect_valid("factor"));
    let records = tir
        .root()
        .semantic_instances()
        .iter()
        .filter(|record| record.instance.id().template() == &template)
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);
    assert_ne!(
        records[0].instance.id().owner(),
        records[1].instance.id().owner()
    );

    for record in records {
        let instance = &record.instance;
        assert_eq!(instance.id().parent(), tir.root_dag_id());
        assert!(record.value_bindings.contains_key(&template_param));
        let concrete = instance.value_port(&template_param).unwrap();
        assert_eq!(concrete.owner(), instance.id().owner());
        assert_eq!(concrete.as_str(), "factor");

        let output_name = ScopedName::in_scope(
            instance.id().scope().clone(),
            DeclName::expect_valid("output"),
        );
        let output = &tir.root().body_for_test().semantic().decl_bindings[&output_name];
        assert_eq!(output.owner(), instance.id().owner());

        // Projections are read resolved in the instance's own frame.
        let checked = tir
            .dag_registry()
            .semantic_instance(record)
            .expect("materialized instance");
        assert_eq!(checked.dag().dag_id(), instance.id().owner());
        let targets = checked
            .output_projections()
            .map(|projection| projection.target)
            .collect::<Vec<_>>();
        assert_eq!(targets, vec![concrete, output.clone()]);
    }
}

#[test]
fn nested_instances_retain_template_and_concrete_parent_identity() {
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "leaf.gcl",
                "param input: Dimensionless;\npub node output: Dimensionless = @input;\n",
            ),
            (
                "middle.gcl",
                "param input: Dimensionless;\ninclude pipeline.leaf(input: @input) as leaf;\npub node output: Dimensionless = @leaf::output;\n",
            ),
            (
                "main.gcl",
                "include pipeline.middle(input: 2.0) as first;\ninclude pipeline.middle(input: 5.0) as second;\n",
            ),
        ],
        "main.gcl",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let middle_template = loaded_file_dag_id(&project, "middle.gcl");
    let leaf_template = loaded_file_dag_id(&project, "leaf.gcl");
    let outer_owners = tir
        .root()
        .semantic_instances()
        .iter()
        .filter(|record| record.instance.id().template() == &middle_template)
        .map(|record| record.instance.id().owner().clone())
        .collect::<HashSet<_>>();
    let nested = tir
        .root()
        .semantic_instances()
        .iter()
        .flat_map(|outer| {
            tir.dag_registry()
                .get(outer.instance.id().owner())
                .expect("materialized outer instance")
                .semantic_instances()
                .iter()
                .map(|record| &record.instance)
        })
        .filter(|record| record.id().template() == &leaf_template)
        .collect::<Vec<_>>();

    assert_eq!(outer_owners.len(), 2);
    assert_eq!(nested.len(), 2);
    assert!(nested.iter().all(|record| {
        outer_owners.contains(record.id().parent())
            && record.id().owner().parent().as_ref() == Some(record.id().parent())
    }));
    assert_ne!(nested[0].id().owner(), nested[1].id().owner());
}

#[test]
fn instance_declarations_lead_back_to_their_template_and_include_site() {
    use graphcal_compiler::resolved_name::ResolvedDeclName;

    let middle = "param input: Dimensionless;\ninclude pipeline.leaf(input: @input) as leaf;\npub node output: Dimensionless = @leaf::output;\n";
    let main = "node x: Dimensionless = 1.0;\ninclude pipeline.middle(input: 2.0) as first;\n";
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "leaf.gcl",
                "param input: Dimensionless;\npub node output: Dimensionless = @input;\n",
            ),
            ("middle.gcl", middle),
            ("main.gcl", main),
        ],
        "main.gcl",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let registry = tir.dag_registry();
    let middle_template = loaded_file_dag_id(&project, "middle.gcl");
    let leaf_template = loaded_file_dag_id(&project, "leaf.gcl");
    let spelled = |source: &'static str, span: graphcal_compiler::syntax::span::Span| {
        &source[span.offset()..span.offset() + span.len()]
    };
    let output = |owner: &graphcal_compiler::dag_id::DagId| {
        ResolvedDeclName::for_test(owner.clone(), DeclName::expect_valid("output"))
    };

    let [first] = tir.root().semantic_instances() else {
        panic!("one include instance");
    };
    let first_owner = first.instance.id().owner();
    let edge = registry.instance_edge(first_owner).expect("include edge");
    assert_eq!(edge.instance.id(), first.instance.id());
    assert_eq!(
        spelled(main, edge.include_span),
        "include pipeline.middle(input: 2.0) as first;"
    );
    assert_eq!(
        registry.template_declaration_of(&output(first_owner)),
        Some(output(&middle_template))
    );

    // A nested edge keeps the include span of the template that writes it.
    let [nested] = registry
        .get(first_owner)
        .expect("materialized instance")
        .semantic_instances()
    else {
        panic!("one nested include instance");
    };
    let nested_owner = nested.instance.id().owner();
    let nested_edge = registry.instance_edge(nested_owner).expect("nested edge");
    assert_eq!(
        spelled(middle, nested_edge.include_span),
        "include pipeline.leaf(input: @input) as leaf;"
    );
    assert_eq!(
        registry.template_declaration_of(&output(nested_owner)),
        Some(output(&leaf_template))
    );

    // A declaration no instance owns has no template.
    assert_eq!(
        registry.template_declaration_of(&output(&middle_template)),
        None
    );
    assert!(registry.instance_edge(tir.root_dag_id()).is_none());
}

#[test]
fn project_type_store_keeps_imported_definitions_under_their_canonical_owner() {
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "pub base dim Measure;\npub base unit u: Measure;\npub index Axis = { A };\npub type Item { Item(value: Measure) }\n",
            ),
            (
                "main.gcl",
                "import pipeline.lib as lib;\nnode value: lib::Measure = 1.0 lib::u;\n",
            ),
        ],
        "main.gcl",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let dependency = loaded_file_dag_id(&project, "lib.gcl");
    let importer = tir.root_dag_id().clone();

    let dependency_dimension = graphcal_compiler::resolved_name::ResolvedDimName::for_test(
        dependency.clone(),
        graphcal_compiler::syntax::dimension::DimName::expect_valid("Measure"),
    );
    let importer_dimension = graphcal_compiler::resolved_name::ResolvedDimName::for_test(
        importer.clone(),
        graphcal_compiler::syntax::dimension::DimName::expect_valid("Measure"),
    );
    assert!(tir.dimension(&dependency_dimension).is_some());
    assert!(tir.dimension(&importer_dimension).is_none());

    let dependency_unit = graphcal_compiler::resolved_name::ResolvedUnitName::for_test(
        dependency.clone(),
        graphcal_compiler::syntax::dimension::UnitName::expect_valid("u"),
    );
    let importer_unit = graphcal_compiler::resolved_name::ResolvedUnitName::for_test(
        importer.clone(),
        graphcal_compiler::syntax::dimension::UnitName::expect_valid("u"),
    );
    assert!(tir.unit_info(&dependency_unit).is_some());
    assert!(tir.unit_info(&importer_unit).is_none());

    let dependency_index = graphcal_compiler::resolved_name::ResolvedIndexName::for_test(
        dependency.clone(),
        graphcal_compiler::syntax::index_name::IndexName::expect_valid("Axis"),
    );
    let importer_index = graphcal_compiler::resolved_name::ResolvedIndexName::for_test(
        importer.clone(),
        graphcal_compiler::syntax::index_name::IndexName::expect_valid("Axis"),
    );
    assert!(tir.declared_index_def(&dependency_index).is_some());
    assert!(tir.declared_index_def(&importer_index).is_none());

    let dependency_type = graphcal_compiler::resolved_name::ResolvedStructTypeName::for_test(
        dependency,
        graphcal_compiler::syntax::type_name::StructTypeName::expect_valid("Item"),
    );
    let importer_type = graphcal_compiler::resolved_name::ResolvedStructTypeName::for_test(
        importer,
        graphcal_compiler::syntax::type_name::StructTypeName::expect_valid("Item"),
    );
    assert!(tir.struct_type_def(&dependency_type).is_some());
    assert!(tir.struct_type_def(&importer_type).is_none());
}

#[test]
fn assumes_inside_an_instance_name_private_assumers_and_skip_unreported_assertions() {
    // Before 7-1 an assumer the root does not expose, or an assertion the
    // root does not report, aborted evaluation with an internal error.
    let source = "dag producer {\n\
                      param v: Length;\n\
                      assert positive = @v > 0.0 m;\n\
                      #[assumes(positive)]\n\
                      node hidden: Length = @v * 2.0;\n\
                      pub node result: Length = @hidden;\n\
                  }\n\
                  include producer(v: 1.0 m)::{ result as r1 };\n\
                  include producer(v: -1.0 m) as other;\n\
                  node y: Length = @other::result;\n";
    let result = compile_and_eval_named(source, "test.gcl").unwrap();
    let assumes = result
        .assumes_map
        .iter()
        .map(|(assertion, assumers)| {
            (
                assertion.to_string(),
                assumers.iter().map(ToString::to_string).collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    // The selective include does not report its instance's assertion, so
    // only the aliased include's assertion has an entry.
    assert_eq!(
        assumes,
        [(
            "other::positive".to_owned(),
            vec!["other::hidden".to_owned()]
        )]
    );
}

#[test]
fn projected_assertions_name_failed_dependencies_as_the_root_does() {
    // Before 7-5 an assertion an include site exposes reported a failed
    // dependency by its runtime identity (`test.other.bad`).
    let source = "dag producer {\n\
                      param v: Length;\n\
                      node bad: Length = @v / 0.0;\n\
                      pub assert positive = @bad > 0.0 m;\n\
                  }\n\
                  include producer(v: 2.0 m) as other;\n";
    let result = compile_and_eval_named(source, "test.gcl").unwrap();
    let [
        graphcal_eval::eval::EvalAssertion {
            name,
            outcome: graphcal_eval::eval::types::AssertResult::Error { message },
            ..
        },
    ] = result.assertions.as_slice()
    else {
        panic!("expected one errored assertion: {:?}", result.assertions);
    };
    assert_eq!(name.to_string(), "other::positive");
    assert_eq!(message, "dependency failed: other::bad (division by zero)");
}

#[test]
fn invoked_modules_name_unavailable_declarations_by_source_module_path() {
    // Before #1941 a declaration of an invoked module outside the root's
    // subtree was reported by its identity (`src.pipeline.lib.d.pending`).
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "pub dag d {\n    node pending: Dimensionless = todo<Dimensionless> {};\n    pub node out: Dimensionless = @pending;\n}\n\
                 dag helper {\n    node inner: Dimensionless = todo<Dimensionless> {};\n    pub node out: Dimensionless = @inner;\n}\n\
                 node pending: Dimensionless = todo<Dimensionless> {};\n\
                 pub node output: Dimensionless = @pending;\n\
                 pub node via_helper: Dimensionless = @helper()::out;\n",
            ),
            (
                "main.gcl",
                "import pipeline.lib as lib;\nimport pipeline.lib.d as d;\n\
                 node from_file: Dimensionless = @lib()::output;\n\
                 node from_inline: Dimensionless = @d()::out;\n\
                 node from_nested: Dimensionless = @lib()::via_helper;\n\
                 assert waits = @d()::out > 0.0;\n",
            ),
        ],
        "main.gcl",
    );
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let reason = |name: &str| {
        result
            .entries
            .iter()
            .find(
                |EvalEntry {
                     name: candidate, ..
                 }| candidate.to_string() == name,
            )
            .unwrap_or_else(|| panic!("missing `{name}`"))
            .outcome
            .as_ref()
            .expect_err("unavailable")
            .to_string()
    };
    assert_eq!(
        reason("from_file"),
        "BLOCKED — unfinished dependencies: pipeline.lib::pending"
    );
    assert_eq!(
        reason("from_inline"),
        "BLOCKED — unfinished dependencies: pipeline.lib.d::pending"
    );
    // An inline DAG no file imports is named under its file's path.
    assert_eq!(
        reason("from_nested"),
        "BLOCKED — unfinished dependencies: pipeline.lib.helper::inner"
    );
    let [
        graphcal_eval::eval::EvalAssertion {
            name,
            outcome: graphcal_eval::eval::types::AssertResult::Blocked { reason },
            ..
        },
    ] = result.assertions.as_slice()
    else {
        panic!("expected one blocked assertion: {:?}", result.assertions);
    };
    assert_eq!(name.to_string(), "waits");
    assert_eq!(
        reason.to_string(),
        "BLOCKED — unfinished dependencies: pipeline.lib.d::pending"
    );
}

#[test]
fn inline_dags_without_required_inputs_run_on_their_own_defaults() {
    // The root is a library, and an include binds the DAG differently: neither
    // changes what the DAG itself evaluates to.
    let source = "
        param root_input: Length;
        dag ready {
            param p: Length = 2.0 m;
            pub node twice: Length = 2.0 * @p;
        }
        dag needs {
            param q: Length;
            pub node same: Length = @q;
        }
        include ready(p: 5.0 m)::{ twice };
        node total: Length = @twice + @root_input;
    ";
    let project = crate::loader::LoadedProject::from_source(source, "defaults.gcl").unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let results = prepared
        .evaluate_dag_defaults_with_cancellation(
            &graphcal_compiler::cancellation::CancellationToken::unbounded(),
        )
        .unwrap();
    let [result] = results.as_slice() else {
        panic!("only `ready` runs on its defaults: {results:?}");
    };
    let ready = project
        .root_id()
        .inline_dag_child(DeclName::expect_valid("ready"));
    let entries = result
        .entries
        .iter()
        .map(|entry| {
            (
                entry.name.to_string(),
                entry.identity.owner().clone(),
                entry.outcome.as_ref().unwrap().si_value().unwrap().get(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        entries,
        [
            ("p".to_string(), ready.clone(), 2.0),
            ("twice".to_string(), ready, 4.0),
        ]
    );
}
