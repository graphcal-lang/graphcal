#![no_main]

//! Model-protocol request decoding: an untrusted Arrow IPC request stream
//! goes through the steps `graphcal model serve` applies to it.

use std::io::Cursor;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, LazyLock};

use arrow_ipc::reader::StreamReader;
use graphcal_model_protocol::arrow::{request_schema, validate_schema};
use graphcal_model_protocol::{PortName, RequestBatch, ResultBatch, SessionSpec};
use graphcal_project::loader::LoadedProject;
use graphcal_project::prepare::{ModelSession, PreparedProject, prepare_from_project};
use libfuzzer_sys::fuzz_target;

const MAX_INPUT_BYTES: usize = 256 * 1024;
/// Requests with more rows are decoded but not evaluated, so one input cannot
/// spend the smoke run's time budget.
const MAX_EVALUATED_ROWS: usize = 64;

/// One varied parameter of every input wire type the protocol carries.
const MODEL: &str = r"
pub index Phase = { Climb, Cruise };
pub index Step = linspace(0.0 s, 1.0 s, points: 3);
pub type Shape {
    Circle(r: Length),
    Dot,
}
param load: Length(min: 0.0 m, max: 10.0 m);
param cycles: Int;
param enabled: Bool;
param profile: Length(min: 0.0 m)[Phase];
param shape: Shape;
param at: Datetime<TAI>;
param z: Complex<Length>;
param channel: Key<Fin(3)>;
param when: Key<Step>;
pub node load_out: Length = @load * 2.0;
pub node cycles_out: Int = @cycles;
pub node enabled_out: Bool = @enabled;
pub node profile_out: Length[Phase] = @profile;
pub node shape_out: Shape = @shape;
pub node later: Datetime<TAI> = @at + 60.0 s;
pub node z_out: Complex<Length> = @z;
pub node channel_out: Key<Fin(3)> = @channel;
pub node when_out: Key<Step> = @when;
";

const INPUTS: [&str; 9] = [
    "load", "cycles", "enabled", "profile", "shape", "at", "z", "channel", "when",
];
const OUTPUTS: [&str; 9] = [
    "load_out",
    "cycles_out",
    "enabled_out",
    "profile_out",
    "shape_out",
    "later",
    "z_out",
    "channel_out",
    "when_out",
];

struct Model {
    project: PreparedProject,
    session: ModelSession,
}

static MODEL_SESSION: LazyLock<Model> = LazyLock::new(|| {
    let project = prepare_from_project(
        &LoadedProject::from_source(MODEL, "model.gcl").expect("the fuzz model loads"),
    )
    .expect("the fuzz model prepares");
    let names = |names: &[&str]| {
        names
            .iter()
            .map(|name| PortName::new(*name).expect("a valid port name"))
            .collect()
    };
    let schema = project
        .session_schema(&SessionSpec::new(names(&INPUTS), names(&OUTPUTS)))
        .expect("the fuzz session is valid");
    let session = project
        .model_session(Arc::new(schema))
        .expect("the fuzz session opens");
    Model { project, session }
});

/// Run one step of `arrow-ipc`'s stream reader, treating a panic inside it as
/// a rejected stream.
///
/// `arrow-ipc` 60.1 panics instead of erroring on some malformed messages
/// (for example a buffer whose offset lies outside the message body). That is
/// an upstream defect, reported separately; containing it here keeps this
/// target exercising Graphcal's own decoders behind the reader. Panics in
/// Graphcal code are not contained and still crash the target.
fn arrow_read<T>(step: impl FnOnce() -> T) -> Option<T> {
    // libfuzzer-sys installs a hook that aborts on any panic; silence it for
    // this step only. Fuzzing runs one input at a time on one thread.
    let fuzzer_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = catch_unwind(AssertUnwindSafe(step));
    std::panic::set_hook(fuzzer_hook);
    result.ok()
}

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_BYTES {
        return;
    }
    let Model { project, session } = &*MODEL_SESSION;
    let schema = session.schema();

    let Some(Ok(mut requests)) = arrow_read(|| StreamReader::try_new(Cursor::new(data), None))
    else {
        return;
    };
    if validate_schema(&request_schema(schema), requests.schema().as_ref()).is_err() {
        return;
    }
    while let Some(batch) = arrow_read(|| requests.next()).flatten() {
        let Ok(batch) = batch else {
            return;
        };
        let Ok(request) = RequestBatch::from_record_batch(Arc::clone(schema), &batch) else {
            continue;
        };

        let encoded = request
            .to_record_batch()
            .expect("an accepted request must re-encode");
        let decoded = RequestBatch::from_record_batch(Arc::clone(schema), &encoded)
            .expect("a re-encoded request must decode");
        assert_eq!(decoded, request);

        if request.rows() > MAX_EVALUATED_ROWS {
            continue;
        }
        let result = project
            .evaluate_request(session, &request)
            .expect("a decoded request must evaluate without an internal failure");
        let encoded = result
            .to_record_batch()
            .expect("a result must encode in its session's result schema");
        let decoded = ResultBatch::from_record_batch(Arc::clone(schema), &encoded)
            .expect("an encoded result must decode");
        assert_eq!(decoded, result);
    }
});
