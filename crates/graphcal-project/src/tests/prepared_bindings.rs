//! Prepared-project parameter bindings and their typed errors.

use super::*;

/// A value expression synthesized without text of its own (JSON, editor).
fn parse_synthesized_expr(s: &str) -> graphcal_compiler::desugar::desugared_ast::Expr {
    parse_expr(s).expr().clone()
}

#[test]
fn prepared_project_binds_coordinate_and_finite_keys() {
    let project = crate::loader::LoadedProject::from_source(
        "pub index TimeStep = range(0.0 s, 2.0 s, step: 1.0 s); \
         param at: Key<TimeStep>; param slot: Key<Fin(3)>;",
        "key-bindings.gcl",
    )
    .unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    assert!(matches!(
        prepared.parameter_ports()[0].value_schema(),
        ModelValueSchema::Key(index)
            if matches!(index.kind(), ModelIndexKind::Coordinate { .. })
    ));
    assert!(matches!(
        prepared.parameter_ports()[1].value_schema(),
        ModelValueSchema::Key(index)
            if matches!(index.kind(), ModelIndexKind::Finite { index } if index.cardinality().get() == 3)
    ));

    let mut bindings = prepared.binding_builder();
    bindings
        .bind_expression(
            &DeclName::expect_valid("at"),
            &parse_expr("nearest_key(TimeStep, 1.1 s)"),
        )
        .unwrap();
    bindings
        .bind_expression(
            &DeclName::expect_valid("slot"),
            &parse_expr("fin_key(Fin(3), 2)"),
        )
        .unwrap();
    let result = prepared.evaluate(&bindings.finish().unwrap()).unwrap();
    assert!(result.params().all(|(_, value)| value.is_ok()));
}

#[test]
fn prepared_binding_literals_use_the_expected_numeric_type_exactly() {
    let project = crate::loader::LoadedProject::from_source(
        "param ratio: Dimensionless; param sample_count: Int;",
        "numeric-bindings.gcl",
    )
    .unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let mut bindings = prepared.binding_builder();
    bindings
        .bind_expression(&DeclName::expect_valid("ratio"), &parse_expr("1"))
        .unwrap();
    bindings
        .bind_expression(&DeclName::expect_valid("sample_count"), &parse_expr("2.0"))
        .unwrap();
    let result = prepared.evaluate(&bindings.finish().unwrap()).unwrap();
    assert!((find_value(&result, "ratio") - 1.0).abs() < f64::EPSILON);
    assert_eq!(find_int_value(&result, "sample_count"), 2);
}

#[test]
fn prepared_bindings_reject_computations_and_cross_plan_positions() {
    let first_project = crate::loader::LoadedProject::from_source(
        "param x: Int; pub node positive: Bool = @x > 0;",
        "first.gcl",
    )
    .unwrap();
    let second_project = crate::loader::LoadedProject::from_source(
        "param x: Int; pub node positive: Bool = @x > 0;",
        "second.gcl",
    )
    .unwrap();
    let first = prepare_from_project(&first_project).unwrap();
    let second = prepare_from_project(&second_project).unwrap();

    let mut bindings = first.binding_builder();
    let error = bindings
        .bind_expression(&DeclName::expect_valid("x"), &parse_expr("1 + 2"))
        .unwrap_err();
    assert!(error.to_string().contains("not a closed value"));

    let mut wrong_plan = second.binding_builder();
    let error = wrong_plan
        .bind_integer(first.parameter_ports()[0].position(), 1)
        .unwrap_err();
    assert!(error.to_string().contains("another prepared project"));
}

fn binding_error_code(error: &CompileError) -> String {
    miette::Diagnostic::code(error)
        .expect("binding diagnostics have codes")
        .to_string()
}

#[test]
fn rejected_expression_bindings_report_typed_binding_errors() {
    let project = crate::loader::LoadedProject::from_source(
        "pub index I = { a, b };\nparam n: Dimensionless = 1.0;\nparam x: Length = 1.0 m;\n\
         param d: Length(min: 0.0 m) = 1.0 m;\nparam k: Int = 1;\nparam values: Length[I];",
        "bindings.gcl",
    )
    .unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let bind = |name: &str, value: &str| {
        prepared
            .binding_builder()
            .bind_expression(&DeclName::expect_valid(name), &parse_expr(value))
            .unwrap_err()
    };

    let error = bind("x", "@n");
    assert!(
        matches!(&error, CompileError::Binding(BindingError::NotClosed { name, .. }) if name.as_str() == "x"),
        "{error:?}"
    );
    assert_eq!(binding_error_code(&error), "graphcal::O006");
    // Value diagnostics are drawn against the value's own text.
    let source = error.registered_source().expect("located in the value");
    assert_eq!(source.name(), "<--param>");
    assert_eq!(source.text().as_str(), "@n");
    // Declaration diagnostics stay in the entry source.
    let error = bind("d", "-1.0 m");
    assert_eq!(
        error
            .registered_source()
            .map(graphcal_compiler::source_registry::RegisteredSource::name),
        Some("bindings.gcl")
    );
    assert!(ExternalValue::parse("<--param>", "1.0 +").is_err());

    let error = bind("k", "1.5");
    assert!(
        matches!(
            &error,
            CompileError::Binding(BindingError::InvalidLiteral {
                reason: crate::binding_error::BindingLiteralError::InexactInt,
                ..
            })
        ),
        "{error:?}"
    );
    assert_eq!(binding_error_code(&error), "graphcal::O005");

    let error = bind("d", "-1.0 m");
    assert!(
        matches!(
            &error,
            CompileError::Binding(BindingError::DomainViolation { name, violation, .. })
                if name.as_str() == "d"
                    && violation.entries().is_empty()
                    && matches!(
                        violation.bound(),
                        graphcal_compiler::semantic_error::domain::DomainBoundViolation::BelowMinimum(_)
                    )
        ),
        "{error:?}"
    );
    assert_eq!(error.to_string(), "below minimum (0 m)");
    assert_eq!(binding_error_code(&error), "graphcal::O010");

    // The value's kind is checked before its shape: a map for a scalar is a
    // kind mismatch, not an incomplete map.
    let error = bind("x", "{I#a: 1.0 m}");
    assert!(
        matches!(
            &error,
            CompileError::Binding(BindingError::KindMismatch {
                actual: crate::binding_error::BindingValueKind::MapLiteral,
                ..
            })
        ),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "cannot bind a map literal to `x` of type `Length`"
    );
    assert_eq!(binding_error_code(&error), "graphcal::O007");

    let mut bindings = prepared.binding_builder();
    bindings
        .bind_expression(&DeclName::expect_valid("x"), &parse_expr("2.0 m"))
        .unwrap();
    let error = bindings
        .bind_expression(&DeclName::expect_valid("x"), &parse_expr("3.0 m"))
        .unwrap_err();
    assert!(
        matches!(
            &error,
            CompileError::Binding(BindingError::BoundMoreThanOnce { .. })
        ),
        "{error:?}"
    );
    assert_eq!(binding_error_code(&error), "graphcal::O009");
}

#[test]
fn typed_scalar_bindings_report_typed_binding_errors() {
    let project = crate::loader::LoadedProject::from_source(
        "pub index Mode = { fast, slow };\nparam x: Length = 1.0 m;\n\
         param mode: Key<Mode> = Mode#fast;\nparam step: Key<Fin(2)>;",
        "scalars.gcl",
    )
    .unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let ports = prepared.parameter_ports();
    let position = |name: &str| {
        ports
            .iter()
            .find(|port| port.name().as_str() == name)
            .unwrap()
            .position()
    };

    let error = prepared
        .binding_builder()
        .bind_integer(position("x"), 1)
        .unwrap_err();
    assert_eq!(error.to_string(), "cannot bind Int to `x` of type `Length`");
    assert_eq!(binding_error_code(&error), "graphcal::O007");

    let error = prepared
        .binding_builder()
        .bind_quantity(position("x"), f64::INFINITY)
        .unwrap_err();
    assert_eq!(binding_error_code(&error), "graphcal::O008");

    let error = prepared
        .binding_builder()
        .bind_named_key(position("mode"), &IndexVariantName::expect_valid("medium"))
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "unknown category `medium` for index `Mode`"
    );
    assert_eq!(binding_error_code(&error), "graphcal::O012");

    let error = prepared
        .binding_builder()
        .bind_named_key(position("step"), &IndexVariantName::expect_valid("first"))
        .unwrap_err();
    assert_eq!(binding_error_code(&error), "graphcal::O011");
}

/// P3-3: binding a coordinate-indexed value entry by entry is out of scope;
/// the structured binding reports that clearly instead of failing elsewhere.
#[test]
fn structured_binding_of_coordinate_indexed_entries_is_rejected_clearly() {
    let project = crate::loader::LoadedProject::from_source(
        "pub index T = range(0.0 s, 1.0 s, step: 0.5 s);\nparam v: Length[T];",
        "coordinates.gcl",
    )
    .unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let value = StructuredValueExpr::Indexed {
        entries: (0..3)
            .map(|_| {
                StructuredValueExpr::Literal(
                    crate::prepare::StructuredLiteral::parse("1.0 m").unwrap(),
                )
            })
            .collect(),
    };

    let error = prepared
        .binding_builder()
        .bind_structured_expression(&DeclName::expect_valid("v"), &value)
        .unwrap_err();

    assert!(
        matches!(error.kind(), StructuredBindingErrorKind::CoordinateEntries),
        "{error:?}"
    );
    assert_eq!(error.path(), []);
    assert_eq!(
        error.to_string(),
        "coordinate-indexed values cannot be bound entry by entry"
    );
}

#[test]
fn external_map_bindings_reject_entries_deeper_than_the_declared_schema() {
    let project = crate::loader::LoadedProject::from_source(
        "pub index Axis = { X }; pub index Extra = { Only }; param samples: Int[Axis];",
        "model.gcl",
    )
    .unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let input = miette::NamedSource::new(
        "input.json",
        std::sync::Arc::new("{\n  \"samples\": {}\n}".to_string()),
    );
    let mut bindings = prepared.binding_builder();

    let error = bindings
        .bind_external_expression(
            &DeclName::expect_valid("samples"),
            &parse_synthesized_expr("{ (Axis#X, Extra#Only): 1 }"),
            &input,
            (4usize, 9usize).into(),
        )
        .unwrap_err();

    match error {
        CompileError::ExternalBinding { name, reason, .. } => {
            assert_eq!(name.as_str(), "samples");
            assert!(
                reason.contains("map entry has more keys than the declared value has indexed axes"),
                "unexpected reason: {reason}"
            );
        }
        other => panic!("expected external binding diagnostic, got {other:?}"),
    }
}

#[test]
fn external_binding_errors_retain_boundary_source_and_parameter() {
    let project =
        crate::loader::LoadedProject::from_source("param sample_count: Int;", "model.gcl").unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let input = miette::NamedSource::new(
        "input.json",
        std::sync::Arc::new("{\n  \"sample_count\": true\n}".to_string()),
    );
    let mut bindings = prepared.binding_builder();

    let error = bindings
        .bind_external_expression(
            &DeclName::expect_valid("sample_count"),
            &parse_synthesized_expr("true"),
            &input,
            (4usize, 7usize).into(),
        )
        .unwrap_err();

    match error {
        CompileError::ExternalBinding {
            name, reason, span, ..
        } => {
            assert_eq!(name.as_str(), "sample_count");
            assert!(
                reason.contains("declared Int, inferred Bool"),
                "unexpected reason: {reason}"
            );
            assert_eq!(span.offset(), 4);
            assert_eq!(span.len(), 7);
        }
        other => panic!("expected external binding diagnostic, got {other:?}"),
    }
}

#[test]
fn model_interface_describes_wire_types_units_and_domains() {
    use graphcal_model_protocol::{Domain, PortWire, WireType};

    let source = r#"
        pub index Mode = { Zulu, Alpha };
        param length: Length(min: 0.0 m, max: 10.0 m);
        param ratio: Dimensionless(min: -1.0, max: 1.0);
        param sample_count: Int(min: -2, max: 2);
        param mode: Key<Mode>;
        param force: Force = 1.0 N;
        param enabled: Bool = true;
        param at: Datetime<UTC> = datetime("2026-01-01T00:00:00Z");
        pub node selected: Bool = @enabled;
        node hidden: Bool = true;
    "#;
    let project = crate::loader::LoadedProject::from_source(source, "model.gcl").unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let interface = prepared.model_interface().unwrap();

    let names = interface
        .parameters()
        .iter()
        .map(|parameter| parameter.name().as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "length",
            "ratio",
            "sample_count",
            "mode",
            "force",
            "enabled",
            "at"
        ]
    );
    let wire = |name: &str| interface.parameter(name).unwrap().wire().clone();
    let PortWire::Supported(WireType::Quantity(length)) = wire("length") else {
        panic!("length is a quantity");
    };
    assert_eq!(
        length
            .unit()
            .map(graphcal_model_protocol::UnitSpelling::as_str),
        Some("m")
    );
    assert_eq!(
        serde_json::to_string(length.dimension()).unwrap(),
        r#"[{"base":{"kind":"prelude","name":"Length"},"exponent":1}]"#
    );
    let Some(Domain::Quantity(bounds)) = interface.parameter("length").unwrap().domain() else {
        panic!("length has a quantity domain");
    };
    assert_eq!(bounds.max().map(|max| max.get()), Some(10.0));

    let PortWire::Supported(WireType::Quantity(ratio)) = wire("ratio") else {
        panic!("ratio is a quantity");
    };
    assert!(ratio.unit().is_none() && ratio.dimension().is_dimensionless());

    let PortWire::Supported(WireType::Quantity(force)) = wire("force") else {
        panic!("force is a quantity");
    };
    assert_eq!(
        force
            .unit()
            .map(graphcal_model_protocol::UnitSpelling::as_str),
        Some("m*kg/s^2")
    );
    assert!(interface.parameter("force").unwrap().has_default());

    assert!(matches!(
        interface.parameter("sample_count").unwrap().domain(),
        Some(Domain::Int(_))
    ));
    let PortWire::Supported(WireType::Key(mode)) = wire("mode") else {
        panic!("mode is a key");
    };
    let graphcal_model_protocol::AxisKind::Named { variants, .. } = mode.axis().kind() else {
        panic!("mode ranges over a named index");
    };
    let variants = variants
        .iter()
        .map(graphcal_model_protocol::VariantName::as_str)
        .collect::<Vec<_>>();
    assert_eq!(variants, ["Zulu", "Alpha"], "declaration order");
    assert_eq!(wire("enabled"), PortWire::Supported(WireType::Bool));
    assert!(matches!(
        wire("at"),
        PortWire::Supported(WireType::Datetime(_))
    ));

    let outputs = interface
        .outputs()
        .iter()
        .map(|output| output.name().as_str())
        .collect::<Vec<_>>();
    assert_eq!(outputs, ["selected"], "private nodes are not outputs");
}
