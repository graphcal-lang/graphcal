//! Tests of the client's typed, process-free parts.

use graphcal_model_protocol::{
    Axis, AxisKind, BaseDimensionName, DescribeDocument, Dimension, Exponent, IndexName, KeyType,
    ModelInterface, OutputDescription, ParameterDescription, PortName, PortWire, QuantityType,
    UnitSpelling, VariantName, WireType,
};

use crate::{Bool, ClientError, GraphcalCommand, Int, Key, Model, Quantity};

fn name(name: &str) -> PortName {
    PortName::new(name).unwrap()
}

fn model() -> Model {
    let length = WireType::Quantity(
        QuantityType::new(
            Dimension::from_iter([(
                BaseDimensionName::new("Length").unwrap(),
                Exponent::new(1, 1).unwrap(),
            )]),
            Some(UnitSpelling::new("m").unwrap()),
        )
        .unwrap(),
    );
    let mode = WireType::Key(KeyType::new(
        Axis::new(
            IndexName::new("Mode").unwrap(),
            AxisKind::Named {
                variants: vec![
                    VariantName::new("A").unwrap(),
                    VariantName::new("B").unwrap(),
                ],
            },
        )
        .unwrap(),
    ));
    let interface = ModelInterface::new(
        vec![
            ParameterDescription::new(
                name("load"),
                "Length",
                PortWire::Supported(length),
                None,
                false,
            )
            .unwrap(),
            ParameterDescription::new(
                name("mode"),
                "Key<Mode>",
                PortWire::Supported(mode),
                None,
                false,
            )
            .unwrap(),
            ParameterDescription::new(
                name("at"),
                "Datetime<UTC>",
                PortWire::Unsupported {
                    reason: "not supported".to_owned(),
                },
                None,
                true,
            )
            .unwrap(),
        ],
        vec![OutputDescription::new(
            name("failure"),
            "Bool",
            PortWire::Supported(WireType::Bool),
        )],
    )
    .unwrap();
    Model::from_document(
        GraphcalCommand::new("model.gcl"),
        DescribeDocument::new("test", interface),
    )
}

#[test]
fn handles_are_typed_by_port_kind() {
    let model = model();
    assert!(model.input::<Quantity>("load").is_ok());
    assert!(matches!(
        model.input::<Int>("load"),
        Err(ClientError::KindMismatch {
            expected: "int",
            ..
        })
    ));
    assert!(matches!(
        model.input::<Quantity>("at"),
        Err(ClientError::UnsupportedPort { .. })
    ));
    assert!(matches!(
        model.input::<Quantity>("nope"),
        Err(ClientError::UnknownInput { .. })
    ));
    assert!(model.output::<Bool>("failure").is_ok());
    assert!(matches!(
        model.output::<Bool>("load"),
        Err(ClientError::UnknownOutput { .. })
    ));
}

#[test]
fn key_entries_belong_to_their_parameter() {
    let model = model();
    let mode = model.input::<Key>("mode").unwrap();
    assert_eq!(mode.entries().len(), 2);
    assert!(matches!(
        mode.variant("C"),
        Err(ClientError::UnknownVariant { .. })
    ));
    let other = Model::from_document(GraphcalCommand::new("model.gcl"), model.document().clone());
    let foreign = other.input::<Key>("mode").unwrap().variant("A").unwrap();
    let error = <Key as crate::Kind>::input_column(
        &crate::PortIdentity::clone(mode_port(&mode)),
        mode.detail(),
        vec![mode.variant("A").unwrap(), foreign],
    )
    .unwrap_err();
    assert!(matches!(error, ClientError::ForeignKey { .. }));
    assert!(matches!(
        mode.position(0),
        Err(ClientError::UnknownKey { .. })
    ));
}

fn mode_port(handle: &crate::InputHandle<Key>) -> &crate::PortIdentity {
    handle.port()
}

#[test]
fn sessions_can_read_every_supported_output() {
    let interface = ModelInterface::new(
        vec![
            ParameterDescription::new(
                name("load"),
                "Int",
                PortWire::Supported(WireType::Int),
                None,
                false,
            )
            .unwrap(),
        ],
        vec![
            OutputDescription::new(name("failure"), "Bool", PortWire::Supported(WireType::Bool)),
            OutputDescription::new(
                name("trace"),
                "Trace",
                PortWire::Unsupported {
                    reason: "not supported".to_owned(),
                },
            ),
            OutputDescription::new(name("count"), "Int", PortWire::Supported(WireType::Int)),
        ],
    )
    .unwrap();
    let model = Model::from_document(
        GraphcalCommand::new("model.gcl"),
        DescribeDocument::new("test", interface),
    );
    let load = model.input::<Int>("load").unwrap();
    let schema = model
        .session()
        .vary(&load)
        .output_all_supported()
        .schema()
        .unwrap();
    let outputs = schema
        .outputs()
        .iter()
        .map(|output| output.name().as_str())
        .collect::<Vec<_>>();
    assert_eq!(outputs, ["failure", "count"]);

    let failure = model.output::<Bool>("failure").unwrap();
    assert!(matches!(
        model
            .session()
            .vary(&load)
            .output(&failure)
            .output_all_supported()
            .schema(),
        Err(ClientError::Session(
            graphcal_model_protocol::SessionError::DuplicateOutput { .. }
        ))
    ));
}

#[test]
fn sessions_reject_handles_of_another_model() {
    let model = model();
    let other = Model::from_document(GraphcalCommand::new("model.gcl"), model.document().clone());
    let failure = other.output::<Bool>("failure").unwrap();
    let load = model.input::<Quantity>("load").unwrap();
    let mode = model.input::<Key>("mode").unwrap();
    let builder = model.session().vary(&load).vary(&mode).output(&failure);
    assert!(matches!(
        builder.schema(),
        Err(ClientError::ForeignHandle { .. })
    ));
    let builder = model
        .session()
        .vary(&load)
        .output(&model.output::<Bool>("failure").unwrap());
    assert!(matches!(
        builder.schema(),
        Err(ClientError::Session(
            graphcal_model_protocol::SessionError::MissingDefault { .. }
        ))
    ));
}

#[test]
fn batches_need_one_valid_column_per_varied_parameter() {
    use std::sync::Arc;

    use crate::batch::BatchBuilder;
    use crate::identity::SessionId;

    let model = model();
    let load = model.input::<Quantity>("load").unwrap();
    let mode = model.input::<Key>("mode").unwrap();
    let failure = model.output::<Bool>("failure").unwrap();
    let schema = Arc::new(
        model
            .session()
            .vary(&load)
            .vary(&mode)
            .output(&failure)
            .schema()
            .unwrap(),
    );
    let builder = || BatchBuilder::new(SessionId::fresh(), model.id(), &schema);
    let a = mode.variant("A").unwrap();

    let request = builder()
        .column(&mode, [a.clone(), a.clone()])
        .unwrap()
        .column(&load, [1.0, 2.0])
        .unwrap()
        .build()
        .unwrap();
    assert_eq!(request.rows(), 2);

    assert!(matches!(
        builder().column(&load, [1.0]).unwrap().build(),
        Err(ClientError::MissingColumn { .. })
    ));
    assert!(matches!(
        builder().column(&load, [1.0]).unwrap().column(&load, [2.0]),
        Err(ClientError::DuplicateColumn { .. })
    ));
    assert!(matches!(
        builder().column(&load, [f64::NAN]),
        Err(ClientError::NonFinite { row: 0, .. })
    ));
    assert!(matches!(
        builder()
            .column(&load, [1.0, 2.0])
            .unwrap()
            .column(&mode, [a])
            .unwrap()
            .build(),
        Err(ClientError::Request(_))
    ));
}
