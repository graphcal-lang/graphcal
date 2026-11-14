//! Model schemas, interfaces, and sessions.

use super::*;

#[test]
fn recursive_type_across_an_inline_dag_import_is_rejected() {
    let (_directory, root) = write_pipeline_project(
        &[(
            "main.gcl",
            "import pipeline.main.inner::{type Y};\n\
             pub type X { X(y: Y) }\n\
             pub dag inner {\n\
             \x20   import pipeline.main::{type X};\n\
             \x20   pub type Y { Y(x: X) }\n\
             }\n",
        )],
        "main.gcl",
    );
    let project = crate::loader::load_project(&root, None, &fs()).unwrap();
    match prepare_from_project(&project) {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Struct(error @ StructError::RecursiveType { .. }),
                    ..
                }),
            ..
        // The inline DAG's module is inserted last, so it closes the cycle.
        })) => assert_eq!(
            error.to_string(),
            "type `Y` refers to itself through `Y.x` -> `X` -> `X.y`"
        ),
        other => panic!("expected a recursive type error, got {other:?}"),
    }
}

#[test]
fn model_schema_graph_describes_a_shared_type_once() {
    let source = r"
        pub type Leaf { Leaf(x: Int) }
        pub type Pair { Pair(left: Leaf, right: Leaf) }
        param pair: Pair;
        pub node echo: Pair = @pair;
    ";
    let project = crate::loader::LoadedProject::from_source(source, "shared.gcl").unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let ModelValueSchema::Algebraic(pair_id) = prepared.parameter_ports()[0].value_schema() else {
        panic!("Pair parameter must reference an algebraic definition");
    };
    assert_eq!(prepared.schema_graph().definitions().len(), 2);
    let fields = prepared
        .schema_graph()
        .definition(pair_id)
        .unwrap()
        .constructors()[0]
        .fields();
    assert!(matches!(
        (fields[0].value(), fields[1].value()),
        (ModelValueSchema::Algebraic(left), ModelValueSchema::Algebraic(right)) if left == right
    ));
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end row-binding test covers every recursive value family"
)]
fn prepared_project_binds_complete_recursive_values_and_reuses_the_plan() {
    let source = r"
        pub type Choice {
            A,
            B(value: Int),
        }
        pub index Axis = { X, Y };
        param distance: Length;
        param sample_count: Int;
        param enabled: Bool;
        param axis_key: Key<Axis>;
        param when: Datetime;
        param impedance: Complex<Length>;
        param choice: Choice;
        param samples: Int[Axis];
        param choices: Choice[Axis];
        node total: Int = @samples[Axis#X] + @samples[Axis#Y];
        pub node accepted: Bool = @total == 3;
        pub node echo: Choice = @choice;
    ";
    let project = crate::loader::LoadedProject::from_source(source, "prepared.gcl").unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    assert_eq!(prepared.parameter_ports().len(), 9);
    assert!(matches!(
        prepared.parameter_ports()[3].value_schema(),
        ModelValueSchema::Key(index)
            if matches!(index.kind(), ModelIndexKind::Named { .. })
    ));

    let mut first = prepared.binding_builder();
    first
        .bind_expression(&DeclName::expect_valid("distance"), &parse_expr("2.0 m"))
        .unwrap();
    first
        .bind_expression(&DeclName::expect_valid("sample_count"), &parse_expr("5"))
        .unwrap();
    first
        .bind_expression(&DeclName::expect_valid("enabled"), &parse_expr("true"))
        .unwrap();
    first
        .bind_expression(&DeclName::expect_valid("axis_key"), &parse_expr("Axis#X"))
        .unwrap();
    first
        .bind_expression(
            &DeclName::expect_valid("when"),
            &parse_expr("datetime(\"2025-01-02T03:04:05Z\")"),
        )
        .unwrap();
    first
        .bind_expression(
            &DeclName::expect_valid("impedance"),
            &parse_expr("complex(3.0 m, 4.0 m)"),
        )
        .unwrap();
    first
        .bind_expression(
            &DeclName::expect_valid("choice"),
            &parse_expr("B(value: 2)"),
        )
        .unwrap();
    first
        .bind_expression(
            &DeclName::expect_valid("samples"),
            &parse_expr("{ Axis#X: 1, Axis#Y: 2 }"),
        )
        .unwrap();
    first
        .bind_expression(
            &DeclName::expect_valid("choices"),
            &parse_expr("{ Axis#X: A, Axis#Y: B(value: 9) }"),
        )
        .unwrap();
    let first_row = first.finish().unwrap();
    let first_result = prepared.evaluate(&first_row).unwrap();
    assert_eq!(find_int_value(&first_result, "total"), 3);
    let recursive_model = prepared.model(&[DeclName::expect_valid("echo")]).unwrap();
    let ModelValueSchema::Algebraic(type_id) = recursive_model.outputs()[0].value_schema() else {
        panic!("echo schema was not algebraic");
    };
    let definition = recursive_model
        .schema_graph()
        .definition(type_id)
        .expect("algebraic schema reference must have a definition");
    assert_eq!(definition.constructors().len(), 2);
    assert_eq!(
        definition.constructors()[1].fields()[0].value(),
        &ModelValueSchema::Int
    );
    let ModelRowOutcome::Success(recursive_outputs) = prepared
        .evaluate_model_row(&first_row, &recursive_model)
        .unwrap()
    else {
        panic!("recursive model row unexpectedly failed");
    };
    assert!(matches!(
        recursive_outputs.as_slice(),
        [Value::Struct { .. }]
    ));

    let mut second = prepared.binding_builder();
    second
        .bind_expression(&DeclName::expect_valid("distance"), &parse_expr("4.0 m"))
        .unwrap();
    second
        .bind_expression(&DeclName::expect_valid("sample_count"), &parse_expr("6"))
        .unwrap();
    second
        .bind_expression(&DeclName::expect_valid("enabled"), &parse_expr("false"))
        .unwrap();
    second
        .bind_expression(&DeclName::expect_valid("axis_key"), &parse_expr("Axis#Y"))
        .unwrap();
    second
        .bind_expression(
            &DeclName::expect_valid("when"),
            &parse_expr("datetime(\"2026-01-02T03:04:05Z\")"),
        )
        .unwrap();
    second
        .bind_expression(
            &DeclName::expect_valid("impedance"),
            &parse_expr("complex(5.0 m, 12.0 m)"),
        )
        .unwrap();
    second
        .bind_expression(&DeclName::expect_valid("choice"), &parse_expr("A"))
        .unwrap();
    second
        .bind_expression(
            &DeclName::expect_valid("samples"),
            &parse_expr("{ Axis#X: 10, Axis#Y: 20 }"),
        )
        .unwrap();
    second
        .bind_expression(
            &DeclName::expect_valid("choices"),
            &parse_expr("{ Axis#X: B(value: 8), Axis#Y: A }"),
        )
        .unwrap();
    let second_result = prepared.evaluate(&second.finish().unwrap()).unwrap();
    assert_eq!(find_int_value(&second_result, "total"), 30);
}

/// The model interface is the file's public value surface: `pub` node items
/// of selective includes are output ports in source order, under their local
/// names and through multi-hop facades. Private items, consts, and
/// whole-instance members are not.
#[test]
fn model_interface_lists_selectively_reexported_nodes_as_outputs() {
    use graphcal_model_protocol::{FiniteF64, InputColumn, RequestBatch, RowOutcome, ValueRef};

    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "param load: Force;\n\
                 pub const node limit: Force = 10.0 N;\n\
                 pub node margin: Force = @limit - @load;\n\
                 pub node failure: Bool = @margin < 0.0 N;\n\
                 pub node doubled: Force = @load * 2.0;\n",
            ),
            (
                "facade.gcl",
                "param load: Force;\n\
                 include pipeline.lib(load: @load)::{ pub margin };\n",
            ),
            (
                "main.gcl",
                "param load: Force;\n\
                 pub node first: Force = @load;\n\
                 include pipeline.lib(load: @load)::{ pub failure, pub doubled as twice, pub limit, margin as hidden_margin };\n\
                 include pipeline.facade(load: @load)::{ pub margin as facade_margin };\n\
                 include pipeline.lib(load: @load) as whole;\n\
                 pub node last: Force = @hidden_margin;\n",
            ),
        ],
        "main.gcl",
    );
    let project = crate::loader::load_project(&root, None, &fs()).unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let interface = prepared.model_interface().unwrap();
    let outputs = interface
        .outputs()
        .iter()
        .map(|output| output.name().as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        outputs,
        ["first", "failure", "twice", "facade_margin", "last"]
    );
    assert_eq!(interface.output("twice").unwrap().graphcal_type(), "Force");
    assert!(prepared.model(&[DeclName::expect_valid("twice")]).is_ok());
    assert!(matches!(
        prepared.model(&[DeclName::expect_valid("hidden_margin")]),
        Err(ModelDefinitionError::PrivateOutput { .. })
    ));

    let schema = session_over(&prepared, &["load"], &["twice", "facade_margin", "failure"]);
    let session = prepared
        .model_session(std::sync::Arc::clone(&schema))
        .unwrap();
    let request = RequestBatch::new(
        std::sync::Arc::clone(&schema),
        vec![InputColumn::Quantity(vec![FiniteF64::new(3.0).unwrap()])],
    )
    .unwrap();
    let result = prepared.evaluate_request(&session, &request).unwrap();
    let RowOutcome::Success(values) = result.row(0).unwrap() else {
        panic!("the row succeeds");
    };
    let get = |name: &str| values.get(schema.output_slot(name).unwrap()).unwrap();
    assert_eq!(
        get("twice"),
        ValueRef::Quantity(FiniteF64::new(6.0).unwrap())
    );
    assert_eq!(
        get("facade_margin"),
        ValueRef::Quantity(FiniteF64::new(7.0).unwrap())
    );
    assert_eq!(get("failure"), ValueRef::Bool(false));
}

const SESSION_MODEL: &str = r"
    pub index Mode = { A, B };
    param x: Length(min: 0.0 m, max: 10.0 m);
    param mode: Key<Mode>;
    param scale: Dimensionless = 2.0;
    assert large_enough = @x > 1.0 m;
    pub node scaled: Length = @x * @scale;
    pub node reciprocal: Dimensionless = 1.0 m / (@x - 5.0 m);
    pub node is_b: Bool = @mode == Mode#B;
    pub node same_mode: Key<Mode> = @mode;
    pub node trials: Int = 3;
";

const UNIT_MODEL: &str = r"
    const unit kilonewton: Force = 1000.0 N;
    unit league: Length = 4828.032 m;
    param load: Force(min: 0.0 N, max: 10000.0 N);
    pub node margin: Force = 10000.0 N - @load;
    pub node reach: Length = 2.0 m;
    pub node trials: Int = 3;
";

fn unit_spec(units: &[(&str, &str)]) -> graphcal_model_protocol::SessionSpec {
    use graphcal_model_protocol::{PortName, SessionSpec, UnitSpelling};

    units.iter().fold(
        SessionSpec::new(
            vec![PortName::new("load").unwrap()],
            vec![
                PortName::new("margin").unwrap(),
                PortName::new("reach").unwrap(),
                PortName::new("trials").unwrap(),
            ],
        ),
        |spec, (port, unit)| {
            spec.with_unit(
                PortName::new(*port).unwrap(),
                UnitSpelling::new(*unit).unwrap(),
            )
        },
    )
}

#[test]
fn model_sessions_carry_quantities_in_chosen_units() {
    use graphcal_model_protocol::{
        Domain, FiniteF64, InputColumn, RequestBatch, RowOutcome, ValueRef,
    };

    let project = crate::loader::LoadedProject::from_source(UNIT_MODEL, "model.gcl").unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let schema = std::sync::Arc::new(
        prepared
            .session_schema(&unit_spec(&[
                ("load", "kilonewton"),
                ("margin", "kN"),
                ("reach", "cm"),
            ]))
            .unwrap(),
    );
    let Some(Domain::Quantity(bounds)) = schema.inputs()[0].domain() else {
        panic!("load has a quantity domain");
    };
    assert_eq!(bounds.max().map(|max| max.get()), Some(10.0));

    let session = prepared
        .model_session(std::sync::Arc::clone(&schema))
        .unwrap();
    let x = |value: f64| FiniteF64::new(value).unwrap();
    let request = RequestBatch::new(
        std::sync::Arc::clone(&schema),
        vec![InputColumn::Quantity(vec![x(3.0), x(f64::MAX)])],
    )
    .unwrap();
    let result = prepared.evaluate_request(&session, &request).unwrap();
    let RowOutcome::Success(values) = result.row(0).unwrap() else {
        panic!("row 0 succeeds");
    };
    let get = |name: &str| values.get(schema.output_slot(name).unwrap()).unwrap();
    assert_eq!(get("margin"), ValueRef::Quantity(x(7.0)));
    assert_eq!(get("reach"), ValueRef::Quantity(x(200.0)));
    assert_eq!(get("trials"), ValueRef::Int(3));
    assert!(matches!(
        result.row(1),
        Some(RowOutcome::Failure(
            graphcal_model_protocol::RowFailure::InputOutOfDomain { .. }
        ))
    ));
}

#[test]
fn an_output_overflowing_in_its_chosen_unit_fails_only_its_row() {
    use graphcal_model_protocol::{
        FiniteF64, InputColumn, PortName, RequestBatch, RowFailure, RowOutcome, SessionSpec,
        UnitSpelling,
    };

    let project = crate::loader::LoadedProject::from_source(
        "param x: Dimensionless;\npub node y: Length = @x * 1.0e306 m;\n",
        "model.gcl",
    )
    .unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let schema = std::sync::Arc::new(
        prepared
            .session_schema(
                &SessionSpec::new(
                    vec![PortName::new("x").unwrap()],
                    vec![PortName::new("y").unwrap()],
                )
                .with_unit(
                    PortName::new("y").unwrap(),
                    UnitSpelling::new("mm").unwrap(),
                ),
            )
            .unwrap(),
    );
    let session = prepared
        .model_session(std::sync::Arc::clone(&schema))
        .unwrap();
    let x = |value: f64| FiniteF64::new(value).unwrap();
    let request = RequestBatch::new(
        std::sync::Arc::clone(&schema),
        vec![InputColumn::Quantity(vec![x(1e-10), x(1.0)])],
    )
    .unwrap();
    let result = prepared.evaluate_request(&session, &request).unwrap();
    assert!(matches!(result.row(0), Some(RowOutcome::Success(_))));
    assert!(matches!(
        result.row(1),
        Some(RowOutcome::Failure(RowFailure::EvaluationError { message }))
            if message.contains("`y`") && message.contains("`mm`")
    ));
}

#[test]
fn model_unit_choices_must_resolve_to_fixed_scale_units_of_the_port_dimension() {
    use graphcal_model_protocol::{PortName, SessionError, UnitSpelling};

    let project = crate::loader::LoadedProject::from_source(UNIT_MODEL, "model.gcl").unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let scale = |port: &str, unit: &str| {
        prepared.model_unit_scale(
            &PortName::new(port).unwrap(),
            &UnitSpelling::new(unit).unwrap(),
        )
    };
    assert_eq!(
        scale("load", "kilonewton").map(graphcal_model_protocol::SiScale::get),
        Ok(1000.0)
    );
    assert_eq!(
        scale("reach", "km").map(graphcal_model_protocol::SiScale::get),
        Ok(1000.0)
    );
    assert_eq!(
        scale("load", "kg*m/s^2").map(graphcal_model_protocol::SiScale::get),
        Ok(1.0)
    );
    assert!(matches!(
        scale("load", "m"),
        Err(ModelUnitError::DimensionMismatch { .. })
    ));
    assert!(matches!(
        scale("reach", "league"),
        Err(ModelUnitError::RuntimeUnit { .. })
    ));
    assert!(matches!(
        scale("load", "parsec_x"),
        Err(ModelUnitError::UnknownUnit { .. })
    ));
    assert!(matches!(
        scale("load", "kN *"),
        Err(ModelUnitError::Syntax { .. })
    ));
    assert!(matches!(
        scale("trials", "kN"),
        Err(ModelUnitError::NotQuantityPort { .. })
    ));
    assert!(matches!(
        prepared.session_schema(&unit_spec(&[("load", "m")])),
        Err(ModelSessionError::Session(
            SessionError::UnresolvedUnit { .. }
        ))
    ));

    let interface = prepared.model_interface().unwrap();
    let forged = interface
        .configure(
            &unit_spec(&[("load", "kilonewton")]),
            |_: &PortName, _: &UnitSpelling| {
                graphcal_model_protocol::SiScale::new(1.0).map_err(|error| error.to_string())
            },
        )
        .unwrap();
    assert!(matches!(
        prepared.model_session(std::sync::Arc::new(forged)),
        Err(ModelSessionError::SchemaMismatch)
    ));
}

#[test]
fn model_units_resolve_to_their_dimension_and_scale() {
    use graphcal_model_protocol::UnitSpelling;

    let project = crate::loader::LoadedProject::from_source(UNIT_MODEL, "model.gcl").unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let resolve = |unit: &str| prepared.resolve_model_unit(&UnitSpelling::new(unit).unwrap());
    let interface = prepared.model_interface().unwrap();
    let wire = |port: &str| {
        interface
            .output(port)
            .unwrap()
            .wire()
            .supported()
            .unwrap()
            .quantity()
            .unwrap()
            .clone()
    };

    let kilonewton = resolve("kilonewton").unwrap();
    assert_eq!(kilonewton.si_scale().get(), 1000.0);
    assert!(kilonewton.measures(&wire("margin")));
    assert!(!kilonewton.measures(&wire("reach")));
    let hour = resolve("h").unwrap();
    assert_eq!(hour.si_scale().get(), 3600.0);
    assert_eq!(resolve("km/h").unwrap().si_scale().get(), 1000.0 / 3600.0);
    assert!(matches!(
        resolve("league"),
        Err(ModelUnitError::RuntimeUnit { .. })
    ));
    assert!(matches!(
        resolve("parsec_x"),
        Err(ModelUnitError::UnknownUnit { .. })
    ));
}

#[test]
fn model_session_reports_each_row_outcome_kind() {
    use graphcal_model_protocol::{
        FiniteF64, InputColumn, KeyRef, RequestBatch, RowFailure, RowOutcome, ValueRef,
        VariantName, WireType,
    };

    let project = crate::loader::LoadedProject::from_source(SESSION_MODEL, "model.gcl").unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let schema = session_over(
        &prepared,
        &["x", "mode"],
        &["trials", "same_mode", "is_b", "reciprocal", "scaled"],
    );
    let session = prepared
        .model_session(std::sync::Arc::clone(&schema))
        .unwrap();
    let WireType::Key(mode) = schema.inputs()[1].wire() else {
        panic!("mode is a key");
    };
    let b = mode
        .position(KeyRef::Variant(&VariantName::new("B").unwrap()))
        .unwrap();
    let x = |value: f64| FiniteF64::new(value).unwrap();
    let request = RequestBatch::new(
        std::sync::Arc::clone(&schema),
        vec![
            InputColumn::Quantity(vec![x(3.0), x(0.5), x(12.0), x(5.0)]),
            InputColumn::Key(vec![b; 4]),
        ],
    )
    .unwrap();
    let result = prepared.evaluate_request(&session, &request).unwrap();
    assert_eq!(result.rows(), 4);

    let RowOutcome::Success(values) = result.row(0).unwrap() else {
        panic!("row 0 succeeds");
    };
    let get = |name: &str| values.get(schema.output_slot(name).unwrap()).unwrap();
    assert_eq!(get("scaled"), ValueRef::Quantity(x(6.0)));
    assert_eq!(get("reciprocal"), ValueRef::Quantity(x(-0.5)));
    assert_eq!(get("is_b"), ValueRef::Bool(true));
    assert_eq!(get("trials"), ValueRef::Int(3));
    assert!(matches!(
        get("same_mode"),
        ValueRef::Key(KeyRef::Variant(variant)) if variant.as_str() == "B"
    ));

    assert!(matches!(
        result.row(1),
        Some(RowOutcome::Failure(RowFailure::AssertionFailed { ref assertion, .. }))
            if assertion == "large_enough"
    ));
    assert!(matches!(
        result.row(2),
        Some(RowOutcome::Failure(RowFailure::InputOutOfDomain { ref parameter, .. }))
            if parameter.as_str() == "x"
    ));
    assert!(matches!(
        result.row(3),
        Some(RowOutcome::Failure(RowFailure::EvaluationError { .. }))
    ));
}

#[test]
fn model_session_rejects_a_schema_of_another_interface() {
    let project = crate::loader::LoadedProject::from_source(SESSION_MODEL, "model.gcl").unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let other = crate::loader::LoadedProject::from_source(
        "param x: Length; pub node y: Length = @x;",
        "other.gcl",
    )
    .unwrap();
    let other = prepare_from_project(&other).unwrap();
    assert!(matches!(
        prepared.model_session(session_over(&other, &["x"], &["y"])),
        Err(ModelSessionError::FingerprintMismatch { .. })
    ));
}

#[test]
fn model_sessions_bind_fixed_parameters_in_every_row() {
    use graphcal_model_protocol::{
        FiniteF64, InputColumn, PortName, RequestBatch, RowOutcome, SessionError, SessionSpec,
        ValueRef,
    };

    let project = crate::loader::LoadedProject::from_source(
        r"
        pub type Beam { Beam(length: Length, factor: Dimensionless), }
        param beam: Beam;
        param load: Force(min: 0.0 N, max: 100.0 N);
        pub node moment: Force * Length = @load * @beam.length * @beam.factor;
        ",
        "model.gcl",
    )
    .unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let name = |name: &str| PortName::new(name).unwrap();
    let spec = SessionSpec::new(vec![name("load")], vec![name("moment")]);
    assert!(matches!(
        prepared.session_schema(&spec),
        Err(ModelSessionError::Session(
            SessionError::MissingDefault { .. }
        ))
    ));

    let schema = std::sync::Arc::new(
        prepared
            .session_schema(&spec.with_fixed(name("beam")))
            .unwrap(),
    );
    let mut bindings = prepared.binding_builder();
    bindings
        .bind_expression(
            &DeclName::expect_valid("beam"),
            &ExternalValue::parse("beam", "Beam(length: 2.0 m, factor: 3.0)").unwrap(),
        )
        .unwrap();
    let fixed = bindings.into_fixed();
    assert!(matches!(
        prepared.model_session(std::sync::Arc::clone(&schema)),
        Err(ModelSessionError::Session(
            SessionError::MissingDefault { .. }
        ))
    ));
    let session = prepared
        .model_session_with(std::sync::Arc::clone(&schema), fixed)
        .unwrap();
    let x = |value: f64| FiniteF64::new(value).unwrap();
    let request = RequestBatch::new(
        std::sync::Arc::clone(&schema),
        vec![InputColumn::Quantity(vec![x(1.0), x(10.0)])],
    )
    .unwrap();
    let result = prepared.evaluate_request(&session, &request).unwrap();
    let moments = result
        .outcomes()
        .map(|outcome| match outcome {
            RowOutcome::Success(values) => values.get(schema.output_slot("moment").unwrap()),
            RowOutcome::Failure(_) => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        moments,
        [
            Some(ValueRef::Quantity(x(6.0))),
            Some(ValueRef::Quantity(x(60.0)))
        ]
    );

    let other = prepare_from_project(&project).unwrap();
    assert!(matches!(
        prepared.model_session_with(schema, other.binding_builder().into_fixed()),
        Err(ModelSessionError::ForeignFixedParameters)
    ));
}

#[test]
fn generic_model_interface_accepts_types_the_batch_protocol_cannot_carry() {
    let project = crate::loader::LoadedProject::from_source(
        "pub base dim Money;\n\
         node unpriced: Money = todo<Money> {};\n\
         param at: Money = @unpriced; pub node same: Money = @at;",
        "generic-model.gcl",
    )
    .unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    assert!(prepared.model(&[DeclName::expect_valid("same")]).is_ok());
    assert!(matches!(
        prepared
            .model_interface()
            .unwrap()
            .output("same")
            .map(graphcal_model_protocol::OutputDescription::wire),
        Some(graphcal_model_protocol::PortWire::Unsupported { .. })
    ));
}

/// Evaluate `pipeline.main`'s `ok` model output for one bound `x`.
fn evaluate_included_model_row(leaf: &str, x: &str) -> ModelRowOutcome {
    let (_directory, root) = write_pipeline_project(
        &[
            ("leaf.gcl", leaf),
            (
                "main.gcl",
                "param x: Dimensionless(min: -10.0, max: 10.0);\n\
                 include pipeline.leaf(input: @x) as leaf;\n\
                 pub node ok: Bool = @x < 100.0;",
            ),
        ],
        "main.gcl",
    );
    let project = crate::loader::load_project(&root, None, &fs()).unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let model = prepared.model(&[DeclName::expect_valid("ok")]).unwrap();
    let mut bindings = prepared.binding_builder();
    bindings
        .bind_expression(&DeclName::expect_valid("x"), &parse_expr(x))
        .unwrap();
    let row = bindings.finish().unwrap();
    prepared.evaluate_model_row(&row, &model).unwrap()
}

#[test]
fn model_row_fails_when_an_included_assertion_fails() {
    let leaf = "param input: Dimensionless;\n\
                pub node output: Dimensionless = @input;\n\
                assert positive = @output > 0.0;";
    assert_eq!(
        evaluate_included_model_row(leaf, "1.0"),
        ModelRowOutcome::Success(vec![Value::Bool(true)])
    );
    match evaluate_included_model_row(leaf, "-1.0") {
        ModelRowOutcome::Failure(failure) => assert!(
            failure.to_string().contains("positive"),
            "unexpected failure: {failure}"
        ),
        ModelRowOutcome::Success(values) => panic!("expected assertion failure, got {values:?}"),
    }
}

#[test]
fn model_row_reports_runtime_errors_inside_included_dags_as_row_failures() {
    let leaf = "param input: Dimensionless;\n\
                node reciprocal: Dimensionless = 1.0 / @input;\n\
                pub node output: Dimensionless = @input;";
    assert_eq!(
        evaluate_included_model_row(leaf, "1.0"),
        ModelRowOutcome::Success(vec![Value::Bool(true)])
    );
    match evaluate_included_model_row(leaf, "0.0") {
        ModelRowOutcome::Failure(failure) => assert!(
            failure.to_string().contains("division by zero"),
            "unexpected failure: {failure}"
        ),
        ModelRowOutcome::Success(values) => panic!("expected runtime failure, got {values:?}"),
    }
}

/// A project whose root includes `pipeline.leaf` under `include`, where
/// `leaf` fails in its private `reciprocal` when `x` is zero.
fn write_private_failure_project(include: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    write_pipeline_project(
        &[
            (
                "leaf.gcl",
                "param input: Dimensionless;\n\
                 node reciprocal: Dimensionless = 1.0 / @input;\n\
                 pub node output: Dimensionless = @input;",
            ),
            (
                "main.gcl",
                &format!(
                    "param x: Dimensionless(min: -10.0, max: 10.0);\n\
                     {include}\n\
                     pub node ok: Bool = @x < 100.0;"
                ),
            ),
        ],
        "main.gcl",
    )
}

#[test]
fn private_include_failures_are_labelled_by_the_include_scope() {
    // B2: a failure private to an included DAG is named by the scope the
    // root gives the include (its alias, or the module name of an anonymous
    // include), in both the model-row and the full evaluation result,
    // never by the internal identity or by the module name behind an alias.
    for (include, label) in [
        ("include pipeline.leaf(input: @x) as l2;", "l2::reciprocal"),
        (
            "include pipeline.leaf(input: @x)::{ output as o };",
            "leaf::reciprocal",
        ),
    ] {
        let (_directory, root) = write_private_failure_project(include);
        let project = crate::loader::load_project(&root, None, &fs()).unwrap();
        let prepared = prepare_from_project(&project).unwrap();
        let model = prepared.model(&[DeclName::expect_valid("ok")]).unwrap();
        let mut bindings = prepared.binding_builder();
        bindings
            .bind_expression(&DeclName::expect_valid("x"), &parse_expr("0.0"))
            .unwrap();
        let row = bindings.finish().unwrap();
        match prepared.evaluate_model_row(&row, &model).unwrap() {
            ModelRowOutcome::Failure(failure) => assert!(
                failure
                    .to_string()
                    .starts_with(&format!("{label}: division by zero")),
                "unexpected failure for `{include}`: {failure}"
            ),
            ModelRowOutcome::Success(values) => {
                panic!("expected a runtime failure for `{include}`, got {values:?}")
            }
        }

        let result = prepared.evaluate(&row).unwrap();
        let failed = result
            .entries
            .iter()
            .filter(|EvalEntry { outcome: value, .. }| value.is_err())
            .map(|EvalEntry { name, .. }| name.to_string())
            .collect::<Vec<_>>();
        assert_eq!(failed, [label], "for `{include}`");
    }
}

#[test]
fn inclusive_bounds_are_ordered_intervals() {
    assert!(InclusiveBounds::try_new(Some(1.0), Some(2.0)).is_ok());
    assert!(InclusiveBounds::try_new(Some(2.0), Some(2.0)).is_ok());
    assert!(InclusiveBounds::<f64>::try_new(None, None).is_ok());
    let lower_only = InclusiveBounds::try_new(Some(3_i64), None).unwrap();
    assert_eq!((lower_only.lower(), lower_only.upper()), (Some(&3), None));
    assert_eq!(
        InclusiveBounds::try_new(Some(2.0), Some(1.0)),
        Err(InclusiveBoundsError::Inverted)
    );
    assert_eq!(
        InclusiveBounds::try_new(Some(f64::NAN), Some(1.0)),
        Err(InclusiveBoundsError::Unordered)
    );
    assert_eq!(
        InclusiveBounds::try_new(None, Some(f64::NAN)),
        Err(InclusiveBoundsError::Unordered)
    );
}

#[test]
fn required_param_without_override_errors() {
    let source = "param x: Dimensionless;\nnode y: Dimensionless = @x + 1.0;";
    let result = compile_and_eval_with_overrides(source, "test.gcl", &HashMap::new());
    match result {
        Err(CompileError::Binding(BindingError::RequiredParamNotProvided { name, .. })) => {
            assert_eq!(name.as_str(), "x");
        }
        other => panic!("expected RequiredParamNotProvided, got {other:?}"),
    }
}

const RICH_MODEL: &str = r"
    pub index Phase = { Climb, Cruise };
    pub index Step = linspace(0.0 s, 1.0 s, points: 3);
    pub type Shape {
        Circle(r: Length),
        Dot,
    }
    param profile: Length(min: 0.0 m)[Phase];
    param samples: Length[Step];
    param shape: Shape;
    param at: Datetime<TAI>;
    param z: Complex<Length>;
    pub node doubled: Length[Phase] = for p: Phase { @profile[p] * 2.0 };
    pub node samples_out: Length[Step] = @samples;
    pub node shape_out: Shape = @shape;
    pub node later: Datetime<TAI> = @at + 60.0 s;
    pub node z2: Complex<Length> = @z * 2.0;
";

#[test]
fn model_interface_carries_indexed_datetime_complex_and_algebraic_ports() {
    use graphcal_model_protocol::{AxisKind, Domain, PortWire, WireType};

    let project = crate::loader::LoadedProject::from_source(RICH_MODEL, "model.gcl").unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let interface = prepared.model_interface().unwrap();
    let parameter = |name: &str| interface.parameter(name).unwrap();

    let PortWire::Supported(WireType::Indexed(profile)) = parameter("profile").wire() else {
        panic!("profile is indexed");
    };
    assert!(
        matches!(profile.axis().kind(), AxisKind::Named { variants, .. } if variants.len() == 2)
    );
    assert!(matches!(profile.element(), WireType::Quantity(_)));
    assert!(matches!(
        parameter("profile").domain(),
        Some(Domain::Quantity(_))
    ));
    let PortWire::Supported(WireType::Indexed(samples)) = parameter("samples").wire() else {
        panic!("samples is indexed");
    };
    assert!(matches!(
        samples.axis().kind(),
        AxisKind::Coordinate { coordinates, .. } if coordinates.len() == 3
    ));
    assert!(matches!(
        parameter("shape").wire(),
        PortWire::Supported(WireType::Algebraic(_))
    ));
    assert!(matches!(
        parameter("at").wire(),
        PortWire::Supported(WireType::Datetime(_))
    ));
    assert!(matches!(
        parameter("z").wire(),
        PortWire::Supported(WireType::Complex(_))
    ));
}

#[test]
fn model_interface_names_the_base_dimension_that_needs_a_base_unit() {
    use graphcal_model_protocol::PortWire;

    let source = r"
        pub base dim Money;
        param budget: Money * Length;
    ";
    let project = crate::loader::LoadedProject::from_source(source, "model.gcl").unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let interface = prepared.model_interface().unwrap();
    let PortWire::Unsupported { reason, message } = interface.parameter("budget").unwrap().wire()
    else {
        panic!("a dimension without a canonical unit is unsupported");
    };
    assert_eq!(
        *reason,
        graphcal_model_protocol::UnsupportedReason::NoCanonicalUnit
    );
    assert!(
        message.ends_with("declare a `base unit` for base dimension `Money`"),
        "{message}"
    );
}

/// The interface of `model.gcl` in package `finance`, placed under
/// `source_dir` inside a fresh project directory.
fn finance_interface(source_dir: &str) -> graphcal_model_protocol::ModelInterface {
    let directory = tempfile::tempdir().unwrap();
    let package = directory.path().join(source_dir).join("finance");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        directory.path().join("graphcal.toml"),
        format!("[package]\nname = \"finance\"\nsource_dir = \"{source_dir}\"\n"),
    )
    .unwrap();
    std::fs::write(
        package.join("units.gcl"),
        "pub base dim Money;\npub base unit usd: Money;\npub index Mode = { A, B };\n",
    )
    .unwrap();
    std::fs::write(
        package.join("model.gcl"),
        "import finance.units::{ dim Money, unit usd, index Mode };\n\
         pub index Phase = { Climb, Cruise };\n\
         param budget: Money = 1.0 usd;\n\
         param mode: Key<Mode>;\n\
         pub node by_phase: Money[Phase] = for p: Phase { @budget };\n",
    )
    .unwrap();
    let project = crate::loader::load_project(&package.join("model.gcl"), None, &fs()).unwrap();
    ProjectCompiler::new(&project)
        .prepare()
        .unwrap()
        .model_interface()
        .unwrap()
}

#[test]
fn model_wire_identities_name_declarations_as_the_user_writes_them() {
    use graphcal_model_protocol::{
        AxisKind, BaseDimension, InterfaceFingerprint, PortWire, WireType,
    };

    let interface = finance_interface("src");
    let PortWire::Supported(WireType::Quantity(budget)) =
        interface.parameter("budget").unwrap().wire()
    else {
        panic!("budget is a quantity");
    };
    let (base, _) = budget.dimension().exponents().next().unwrap();
    let BaseDimension::Declared(money) = base else {
        panic!("Money is a declared base dimension");
    };
    assert_eq!(money.to_string(), "finance.units.Money");
    assert_eq!(money.package().as_str(), "finance");

    let PortWire::Supported(WireType::Key(mode)) = interface.parameter("mode").unwrap().wire()
    else {
        panic!("mode is a key");
    };
    assert_eq!(
        mode.axis().index().unwrap().to_string(),
        "finance.units.Mode"
    );
    let PortWire::Supported(WireType::Indexed(by_phase)) =
        interface.output("by_phase").unwrap().wire()
    else {
        panic!("by_phase is indexed");
    };
    assert!(matches!(
        by_phase.axis().kind(),
        AxisKind::Named { index, .. } if index.to_string() == "finance.model.Phase"
    ));

    // The source directory is file layout, not identity.
    assert_eq!(
        InterfaceFingerprint::of(&interface),
        InterfaceFingerprint::of(&finance_interface("lib"))
    );
}

#[test]
fn indexes_with_one_name_in_two_modules_have_distinct_wire_identities() {
    use graphcal_model_protocol::{PortWire, WireType};

    let (_directory, root) = write_pipeline_project(
        &[
            ("left.gcl", "pub index Mode = { A, B };\n"),
            ("right.gcl", "pub index Mode = { A, B };\n"),
            (
                "main.gcl",
                "import pipeline.left as left;\nimport pipeline.right as right;\n\
                 param l: Key<left::Mode>;\nparam r: Key<right::Mode>;\n\
                 pub node same: Bool = true;\n",
            ),
        ],
        "main.gcl",
    );
    let project = crate::loader::load_project(&root, None, &fs()).unwrap();
    let interface = ProjectCompiler::new(&project)
        .prepare()
        .unwrap()
        .model_interface()
        .unwrap();
    let axis = |name: &str| match interface.parameter(name).unwrap().wire() {
        PortWire::Supported(WireType::Key(key)) => key.axis().clone(),
        other => panic!("{name} is a key, not {other:?}"),
    };
    assert_ne!(axis("l"), axis("r"));
    assert_eq!(axis("l").index().unwrap().to_string(), "pipeline.left.Mode");
}

#[test]
fn model_sessions_round_trip_rich_values_through_evaluation() {
    use graphcal_model_protocol::{
        ComplexValue, FiniteF64, InputColumn, RequestBatch, RowFailure, RowOutcome, Value,
        ValueRef, WireType,
    };

    let project = crate::loader::LoadedProject::from_source(RICH_MODEL, "model.gcl").unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let schema = std::sync::Arc::new(
        prepared
            .session_schema(
                &unit_names(
                    &["profile", "samples", "shape", "at", "z"],
                    &["doubled", "samples_out", "shape_out", "later", "z2"],
                )
                .with_unit(
                    graphcal_model_protocol::PortName::new("doubled").unwrap(),
                    graphcal_model_protocol::UnitSpelling::new("cm").unwrap(),
                ),
            )
            .unwrap(),
    );
    let session = prepared
        .model_session(std::sync::Arc::clone(&schema))
        .unwrap();
    let x = |value: f64| FiniteF64::new(value).unwrap();
    let lengths = |values: &[f64]| {
        Value::Indexed(
            values
                .iter()
                .map(|value| Value::Quantity(x(*value)))
                .collect(),
        )
    };
    let WireType::Algebraic(shape) = schema.inputs()[2].wire() else {
        panic!("shape is algebraic");
    };
    let circle = |radius: f64| Value::Algebraic {
        constructor: shape.position("Circle").unwrap(),
        fields: vec![Value::Quantity(x(radius))],
    };
    let request = RequestBatch::new(
        std::sync::Arc::clone(&schema),
        vec![
            InputColumn::Structured(vec![lengths(&[1.0, 2.0]), lengths(&[-1.0, 2.0])]),
            InputColumn::Structured(vec![lengths(&[0.1, 0.2, 0.3]), lengths(&[0.0, 0.0, 0.0])]),
            InputColumn::Structured(vec![circle(4.0), circle(5.0)]),
            InputColumn::Datetime(vec![x(100.0), x(0.0)]),
            InputColumn::Complex(vec![
                ComplexValue {
                    re: x(1.0),
                    im: x(-1.5)
                };
                2
            ]),
        ],
    )
    .unwrap();
    let result = prepared.evaluate_request(&session, &request).unwrap();
    let RowOutcome::Success(values) = result.row(0).unwrap() else {
        panic!("row 0 succeeds: {:?}", result.failure(0));
    };
    let get = |name: &str| {
        values
            .get(schema.output_slot(name).unwrap())
            .unwrap()
            .to_owned_value()
    };
    assert_eq!(get("doubled"), lengths(&[200.0, 400.0]), "in centimetres");
    assert_eq!(get("samples_out"), lengths(&[0.1, 0.2, 0.3]));
    assert_eq!(get("shape_out"), circle(4.0));
    assert_eq!(get("later"), Value::Datetime(x(160.0)));
    assert_eq!(
        get("z2"),
        Value::Complex(ComplexValue {
            re: x(2.0),
            im: x(-3.0)
        })
    );
    assert!(matches!(
        values.get(schema.output_slot("shape_out").unwrap()),
        Some(ValueRef::Algebraic { constructor, .. }) if constructor.get() == 0
    ));
    assert!(matches!(
        result.row(1),
        Some(RowOutcome::Failure(RowFailure::InputOutOfDomain { ref parameter, .. }))
            if parameter.as_str() == "profile"
    ));
}

const KEY_MODEL: &str = r"
    pub index Step = linspace(0.0 s, 1.0 s, points: 3);
    param channel: Key<Fin(3)>;
    param when: Key<Step>;
    pub node channel_number: Int = to_int(@channel);
    pub node next_channel: Key<Fin(4)> = @channel + 1;
    pub node when_time: Time = coord(@when);
    pub node when_out: Key<Step> = @when;
";

#[test]
fn model_sessions_carry_fin_keys_as_positions_and_coordinate_keys_as_coordinates() {
    use graphcal_model_protocol::{
        AxisKind, FiniteF64, InputColumn, KeyRef, KeyValue, RequestBatch, RowOutcome, Value,
        WireType,
    };

    let project = crate::loader::LoadedProject::from_source(KEY_MODEL, "model.gcl").unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let schema = std::sync::Arc::new(
        prepared
            .session_schema(&unit_names(
                &["channel", "when"],
                &["channel_number", "next_channel", "when_time", "when_out"],
            ))
            .unwrap(),
    );
    let (WireType::Key(channel), WireType::Key(when)) =
        (schema.inputs()[0].wire(), schema.inputs()[1].wire())
    else {
        panic!("channel and when are keys");
    };
    assert!(matches!(
        channel.axis().kind(),
        AxisKind::Finite { size: 3 }
    ));
    assert!(matches!(
        when.axis().kind(),
        AxisKind::Coordinate { coordinates, .. } if coordinates.len() == 3
    ));
    let x = |value: f64| FiniteF64::new(value).unwrap();
    let request = RequestBatch::new(
        std::sync::Arc::clone(&schema),
        vec![
            InputColumn::Key(vec![channel.position(KeyRef::Position(2)).unwrap()]),
            InputColumn::Key(vec![when.position(KeyRef::Coordinate(x(0.5))).unwrap()]),
        ],
    )
    .unwrap();
    let session = prepared
        .model_session(std::sync::Arc::clone(&schema))
        .unwrap();
    let result = prepared.evaluate_request(&session, &request).unwrap();
    let RowOutcome::Success(values) = result.row(0).unwrap() else {
        panic!("row 0 succeeds: {:?}", result.failure(0));
    };
    let get = |name: &str| {
        values
            .get(schema.output_slot(name).unwrap())
            .unwrap()
            .to_owned_value()
    };
    assert_eq!(get("channel_number"), Value::Int(2));
    assert_eq!(get("next_channel"), Value::Key(KeyValue::Position(3)));
    assert_eq!(get("when_time"), Value::Quantity(x(0.5)));
    assert_eq!(get("when_out"), Value::Key(KeyValue::Coordinate(x(0.5))));
}

fn unit_names(inputs: &[&str], outputs: &[&str]) -> graphcal_model_protocol::SessionSpec {
    let names = |names: &[&str]| {
        names
            .iter()
            .map(|name| graphcal_model_protocol::PortName::new(*name).unwrap())
            .collect()
    };
    graphcal_model_protocol::SessionSpec::new(names(inputs), names(outputs))
}

#[test]
fn model_row_failures_name_the_failures_they_originate_from() {
    use graphcal_model_protocol::{FiniteF64, InputColumn, RequestBatch, RowFailure, RowOutcome};

    let project = crate::loader::LoadedProject::from_source(
        r"
        param x: Length(min: 0.0 m, max: 10.0 m);
        dag stage {
            param x: Length;
            node ratio: Dimensionless = 1.0 m / (@x - 5.0 m);
            node doubled: Dimensionless = @ratio * 2.0;
            pub node out: Dimensionless = @doubled + 1.0;
        }
        include stage(x: @x) as inner;
        pub node result: Dimensionless = @inner::out;
        ",
        "model.gcl",
    )
    .unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let schema = session_over(&prepared, &["x"], &["result"]);
    let session = prepared
        .model_session(std::sync::Arc::clone(&schema))
        .unwrap();
    let request = RequestBatch::new(
        std::sync::Arc::clone(&schema),
        vec![InputColumn::Quantity(vec![FiniteF64::new(5.0).unwrap()])],
    )
    .unwrap();
    let result = prepared.evaluate_request(&session, &request).unwrap();
    let Some(RowOutcome::Failure(RowFailure::EvaluationError { message })) = result.row(0) else {
        panic!("row 0 fails to evaluate");
    };
    assert_eq!(
        message,
        "result: dependency failed: inner::out; originating failure: inner::ratio (division by zero)"
    );
}
