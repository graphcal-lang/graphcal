//! End-to-end tests of `graphcal-model-client` against the built CLI.
#![cfg(test)]

use graphcal_model_client::{Bool, ClientError, GraphcalCommand, Key, Model, Quantity};
use graphcal_model_protocol::{RowFailure, RowOutcome};

const MODEL: &str = "pub index Mode = { A, B };\n\
    param load: Length(min: 0.0 m, max: 10.0 m);\n\
    param mode: Key<Mode>;\n\
    param factor: Dimensionless = 2.0;\n\
    assert positive = @load > 0.5 m;\n\
    pub node failure: Bool = @load * @factor > 8.0 m && @mode == Mode#B;\n\
    pub node margin: Length = 10.0 m - @load * @factor;\n\
    pub node same_mode: Key<Mode> = @mode;\n";

fn command(dir: &std::path::Path) -> GraphcalCommand {
    let file = dir.join("model.gcl");
    std::fs::write(&file, MODEL).unwrap();
    GraphcalCommand::new(file).program(env!("CARGO_BIN_EXE_graphcal"))
}

#[test]
fn client_evaluates_typed_batches_in_one_server_process() {
    let dir = tempfile::tempdir().unwrap();
    let model = Model::describe(command(dir.path())).unwrap();
    let load = model.input::<Quantity>("load").unwrap();
    let mode = model.input::<Key>("mode").unwrap();
    let failure = model.output::<Bool>("failure").unwrap();
    let margin = model.output::<Quantity>("margin").unwrap();
    let same_mode = model.output::<Key>("same_mode").unwrap();

    let mut session = model
        .session()
        .vary(&mode)
        .vary(&load)
        .output(&failure)
        .output(&margin)
        .output(&same_mode)
        .spawn()
        .unwrap();
    let b = mode.variant("B").unwrap();

    for _ in 0..3 {
        let request = session
            .batch()
            .column(&load, [1.0, 5.0, 0.2, 11.0])
            .unwrap()
            .column(&mode, std::iter::repeat_n(b.clone(), 4))
            .unwrap()
            .build()
            .unwrap();
        let result = session.evaluate(&request).unwrap();
        // The same batch can be resubmitted as a plain protocol batch.
        let again = session
            .evaluate(&session.request(request.batch().clone()).unwrap())
            .unwrap();
        assert_eq!(again.batch(), result.batch());
        assert_eq!(
            result.column(&failure).unwrap()[..2],
            [Some(false), Some(true)]
        );
        let RowOutcome::Success(row) = result.row(1).unwrap() else {
            panic!("row 1 succeeds");
        };
        assert_eq!(row.get(&margin).unwrap(), 0.0);
        assert_eq!(row.get(&same_mode).unwrap().as_str(), "B");
        assert!(matches!(
            result.failure(2),
            Some(RowFailure::AssertionFailed { assertion, .. }) if assertion == "positive"
        ));
        assert!(matches!(
            result.failure(3),
            Some(RowFailure::InputOutOfDomain { parameter, .. }) if parameter.as_str() == "load"
        ));
    }
    session.shutdown().unwrap();
}

#[test]
fn client_reports_a_changed_interface_from_the_server() {
    let dir = tempfile::tempdir().unwrap();
    let command = command(dir.path());
    let model = Model::describe(command.clone()).unwrap();
    let load = model.input::<Quantity>("load").unwrap();
    let mode = model.input::<Key>("mode").unwrap();
    let failure = model.output::<Bool>("failure").unwrap();
    // The model gains a parameter after it was described.
    std::fs::write(
        command.file(),
        format!("{MODEL}param extra: Dimensionless = 1.0;\n"),
    )
    .unwrap();
    let error = model
        .session()
        .vary(&load)
        .vary(&mode)
        .output(&failure)
        .spawn()
        .unwrap_err();
    assert!(
        matches!(&error, ClientError::ServerStopped { stderr, .. } if stderr.contains("describe the model again")),
        "{error}"
    );
}

#[test]
fn client_sessions_carry_quantities_in_server_resolved_units() {
    use graphcal_model_protocol::{Domain, UnitSpelling};

    let dir = tempfile::tempdir().unwrap();
    let model = Model::describe(command(dir.path())).unwrap();
    let load = model.input::<Quantity>("load").unwrap();
    let mode = model.input::<Key>("mode").unwrap();
    let margin = model.output::<Quantity>("margin").unwrap();
    let unit = |spelling: &str| UnitSpelling::new(spelling).unwrap();
    let mut session = model
        .session()
        .vary_in(&load, unit("cm"))
        .vary(&mode)
        .output_in(&margin, unit("mm"))
        .spawn()
        .unwrap();
    let Some(Domain::Quantity(bounds)) = session.schema().inputs()[0].domain() else {
        panic!("load has a quantity domain");
    };
    assert_eq!(bounds.max().map(|max| max.get()), Some(1000.0));
    let request = session
        .batch()
        .column(&load, [100.0, 2000.0])
        .unwrap()
        .column(&mode, std::iter::repeat_n(mode.variant("A").unwrap(), 2))
        .unwrap()
        .build()
        .unwrap();
    let result = session.evaluate(&request).unwrap();
    assert_eq!(result.column(&margin).unwrap()[0], Some(8000.0));
    assert!(matches!(
        result.failure(1),
        Some(RowFailure::InputOutOfDomain { parameter, .. }) if parameter.as_str() == "load"
    ));
    session.shutdown().unwrap();

    let error = model
        .session()
        .vary_in(&load, unit("s"))
        .vary(&mode)
        .output(&margin)
        .spawn()
        .unwrap_err();
    assert!(
        matches!(&error, ClientError::ServerStopped { stderr, .. } if stderr.contains("cannot carry port `load`")),
        "{error}"
    );
    assert!(matches!(
        model
            .session()
            .vary_in(&load, unit("cm"))
            .vary(&mode)
            .output(&margin)
            .schema(),
        Err(ClientError::Session(_))
    ));
}

const RICH_MODEL: &str = "pub index Phase = { Climb, Cruise };\n\
    pub type Shape {\n\
        Circle(r: Length),\n\
        Dot,\n\
    }\n\
    param profile: Length[Phase];\n\
    param modes: Key<Phase>[Phase];\n\
    param at: Datetime<TAI>;\n\
    param z: Complex<Length>;\n\
    param shape: Shape;\n\
    pub node doubled: Length[Phase] = for p: Phase { @profile[p] * 2.0 };\n\
    pub node modes_out: Key<Phase>[Phase] = @modes;\n\
    pub node later: Datetime<TAI> = @at + 60.0 s;\n\
    pub node z2: Complex<Length> = @z * 2.0;\n\
    pub node shape_out: Shape = @shape;\n";

#[test]
fn client_carries_indexed_datetime_complex_and_algebraic_values() {
    use graphcal_model_client::{Algebraic, Complex, Datetime, Indexed};
    use graphcal_model_protocol::{FiniteF64, UnitSpelling, Value, WireType};

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("rich.gcl");
    std::fs::write(&file, RICH_MODEL).unwrap();
    let model = Model::describe(GraphcalCommand::new(file).program(env!("CARGO_BIN_EXE_graphcal")))
        .unwrap();
    let profile = model.input::<Indexed<Quantity>>("profile").unwrap();
    let phase_keys = model.input::<Indexed<Key>>("modes").unwrap();
    let at = model.input::<Datetime>("at").unwrap();
    let z = model.input::<Complex>("z").unwrap();
    let shape = model.input::<Algebraic>("shape").unwrap();
    let doubled = model.output::<Indexed<Quantity>>("doubled").unwrap();
    let phase_keys_out = model.output::<Indexed<Key>>("modes_out").unwrap();
    let later = model.output::<Datetime>("later").unwrap();
    let z2 = model.output::<Complex>("z2").unwrap();
    let shape_out = model.output::<Algebraic>("shape_out").unwrap();

    let mut session = model
        .session()
        .vary_in(&profile, UnitSpelling::new("km").unwrap())
        .vary(&phase_keys)
        .vary(&at)
        .vary(&z)
        .vary(&shape)
        .output(&doubled)
        .output(&phase_keys_out)
        .output(&later)
        .output(&z2)
        .output(&shape_out)
        .spawn()
        .unwrap();
    let circle = Value::Algebraic {
        constructor: shape.detail().position("Circle").unwrap(),
        fields: vec![Value::Quantity(FiniteF64::new(0.5).unwrap())],
    };
    let cruise = phase_keys.variant("Cruise").unwrap();
    let climb = phase_keys.variant("Climb").unwrap();
    let request = session
        .batch()
        .column(&profile, [vec![1.0, 2.0]])
        .unwrap()
        .column(&phase_keys, [vec![cruise, climb]])
        .unwrap()
        .column(&at, [100.0])
        .unwrap()
        .column(&z, [(1.0, -1.0)])
        .unwrap()
        .column(&shape, [circle.clone()])
        .unwrap()
        .build()
        .unwrap();
    let result = session.evaluate(&request).unwrap();
    let RowOutcome::Success(row) = result.row(0).unwrap() else {
        panic!("row 0 succeeds: {:?}", result.failure(0));
    };
    assert_eq!(
        row.get(&doubled).unwrap(),
        [2000.0, 4000.0],
        "km inputs, SI output in metres"
    );
    assert_eq!(
        row.get(&phase_keys_out)
            .unwrap()
            .iter()
            .map(graphcal_model_protocol::VariantName::as_str)
            .collect::<Vec<_>>(),
        ["Cruise", "Climb"]
    );
    assert_eq!(row.get(&later).unwrap(), 160.0);
    assert_eq!(row.get(&z2).unwrap(), (2.0, -2.0));
    assert_eq!(row.get(&shape_out).unwrap(), circle);
    assert!(matches!(
        session.schema().inputs()[0].wire(),
        WireType::Indexed(_)
    ));
    assert!(matches!(
        session.batch().column(&profile, [vec![1.0]]),
        Err(ClientError::EntryCount {
            expected: 2,
            found: 1,
            ..
        })
    ));
    session.shutdown().unwrap();
}
