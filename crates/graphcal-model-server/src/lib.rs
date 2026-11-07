//! Arrow IPC transport of the batch model-evaluation protocol.
//!
//! This crate is the imperative shell around
//! [`PreparedProject::evaluate_request`]: it reads request batches from one
//! Arrow IPC stream and writes one result batch per request to another. It
//! contains no Graphcal compilation logic.
//!
//! The lifecycle is fixed so neither pipe can deadlock:
//!
//! 1. the result-stream schema is written and flushed;
//! 2. the request stream is opened and its schema validated;
//! 3. each request batch is evaluated and its result written and flushed;
//! 4. the request stream's end finishes the result stream.
//!
//! A [`RequestDeadline`] bounds how long one request batch may evaluate:
//! evaluation has no operation budget, so a request whose work never
//! finishes in time ends the session instead of blocking it forever.

use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use arrow_ipc::reader::StreamReader;
use arrow_ipc::writer::StreamWriter;
use graphcal_compiler::cancellation::CancellationSource;
use graphcal_compiler::outcome::Outcome;
use graphcal_model_protocol::arrow::{
    RequestDecodeError, SchemaMismatch, request_schema, result_schema, validate_schema,
};
use graphcal_model_protocol::{RequestBatch, ResultBatch};
use graphcal_project::prepare::{ModelExecutionError, ModelSession, PreparedProject};
use thiserror::Error;

/// How long one request batch may evaluate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestDeadline {
    /// A request evaluates until it finishes.
    Unbounded,
    /// A request still evaluating after this long ends the session with
    /// [`ServeError::DeadlineExceeded`].
    After(Duration),
}

/// A failure that ends a serving session.
#[derive(Debug, Error)]
pub enum ServeError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Arrow(#[from] arrow_schema::ArrowError),
    #[error("invalid request-stream schema: {0}")]
    RequestSchema(#[from] SchemaMismatch),
    #[error("invalid request batch: {0}")]
    Request(#[from] RequestDecodeError),
    #[error(transparent)]
    Execution(Box<ModelExecutionError>),
    /// A request batch was still evaluating at its deadline; it has no result.
    #[error("a request batch did not finish evaluating within {0:?}")]
    DeadlineExceeded(Duration),
}

impl From<ModelExecutionError> for ServeError {
    fn from(error: ModelExecutionError) -> Self {
        Self::Execution(Box::new(error))
    }
}

/// Serve `session` over process stdin and stdout.
///
/// Stdout carries only Arrow IPC; callers report diagnostics on stderr.
///
/// # Errors
///
/// Returns a [`ServeError`] for an I/O, IPC, or protocol failure, or an
/// internal evaluation failure.
pub fn serve_stdio(
    project: &PreparedProject,
    session: &ModelSession,
    deadline: RequestDeadline,
) -> Result<(), ServeError> {
    serve(
        project,
        session,
        deadline,
        std::io::stdin().lock(),
        std::io::stdout().lock(),
    )
}

/// Serve `session` over the given byte streams.
///
/// # Errors
///
/// Returns a [`ServeError`] for an I/O, IPC, or protocol failure, or an
/// internal evaluation failure. The result-stream schema has been written by
/// the time a request-side failure is reported.
fn serve<R: Read, W: Write>(
    project: &PreparedProject,
    session: &ModelSession,
    deadline: RequestDeadline,
    input: R,
    output: W,
) -> Result<(), ServeError> {
    let schema = session.schema();
    let mut results = StreamWriter::try_new(output, &result_schema(schema))?;
    results.get_mut().flush()?;

    let requests = StreamReader::try_new(input, None)?;
    validate_schema(&request_schema(schema), requests.schema().as_ref())?;
    for batch in requests {
        let request = RequestBatch::from_record_batch(std::sync::Arc::clone(schema), &batch?)?;
        let result = evaluate_within(project, session, &request, deadline)?;
        results.write(&result.to_record_batch()?)?;
        results.get_mut().flush()?;
    }
    results.finish()?;
    results.get_mut().flush()?;
    Ok(())
}

/// Evaluate `request`, cancelling it once `deadline` passes.
fn evaluate_within(
    project: &PreparedProject,
    session: &ModelSession,
    request: &RequestBatch,
    deadline: RequestDeadline,
) -> Result<ResultBatch, ServeError> {
    match deadline {
        RequestDeadline::Unbounded => Ok(project.evaluate_request(session, request)?),
        RequestDeadline::After(timeout) => {
            evaluate_until(project, session, request, &Expiry::starting_now(timeout))
        }
    }
}

/// The monotonic instant a request's deadline expires, fixed before the
/// request starts evaluating.
struct Expiry {
    timeout: Duration,
    /// `None` when the instant is too far away to represent: it never comes.
    at: Option<Instant>,
}

impl Expiry {
    fn starting_now(timeout: Duration) -> Self {
        Self {
            timeout,
            at: Instant::now().checked_add(timeout),
        }
    }

    /// How long from now until the deadline; `None` when it never comes.
    fn remaining(&self) -> Option<Duration> {
        self.at
            .map(|at| at.saturating_duration_since(Instant::now()))
    }

    fn has_passed(&self) -> bool {
        self.at.is_some_and(|at| Instant::now() >= at)
    }
}

/// Evaluate `request` against `expiry`: a watchdog cancels it when the
/// deadline passes, and a result completed after the deadline is rejected
/// even when the watchdog had not run yet.
fn evaluate_until(
    project: &PreparedProject,
    session: &ModelSession,
    request: &RequestBatch,
    expiry: &Expiry,
) -> Result<ResultBatch, ServeError> {
    let source = CancellationSource::new();
    let token = source.token();
    let (finished, watched) = mpsc::channel::<()>();
    let remaining = expiry.remaining();
    std::thread::scope(|scope| {
        // The watchdog cancels at the deadline unless evaluation finishes
        // first, which drops `finished` and wakes it. Only the end of
        // evaluation wakes a deadline that never comes.
        scope.spawn(move || {
            let expired = remaining.map_or_else(
                || {
                    let _ = watched.recv();
                    false
                },
                |remaining| {
                    matches!(
                        watched.recv_timeout(remaining),
                        Err(mpsc::RecvTimeoutError::Timeout)
                    )
                },
            );
            if expired {
                source.cancel();
            }
        });
        let outcome = project.evaluate_request_with_cancellation(session, request, &token);
        drop(finished);
        match outcome {
            // A result that completed after the deadline has no place in the
            // stream, whether or not the watchdog had cancelled yet.
            Ok(_) if expiry.has_passed() => Err(ServeError::DeadlineExceeded(expiry.timeout)),
            Ok(result) => Ok(result),
            Err(Outcome::Cancelled) => Err(ServeError::DeadlineExceeded(expiry.timeout)),
            Err(Outcome::Failed(error)) => Err(error.into()),
        }
    })
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::Arc;

    use graphcal_model_protocol::{
        FiniteF64, InputColumn, PortName, RowOutcome, SessionSpec, ValueRef,
    };
    use graphcal_project::loader::LoadedProject;
    use graphcal_project::prepare::prepare_from_project;

    use super::*;

    const MODEL: &str = r"
        param load: Length(min: 0.0 m, max: 10.0 m);
        param factor: Dimensionless = 2.0;
        pub node failure: Bool = @load * @factor > 8.0 m;
    ";

    fn session(project: &PreparedProject) -> ModelSession {
        let schema = project
            .session_schema(&SessionSpec::new(
                vec![PortName::new("load").unwrap()],
                vec![PortName::new("failure").unwrap()],
            ))
            .unwrap();
        project.model_session(Arc::new(schema)).unwrap()
    }

    fn request_stream(batches: &[RequestBatch], schema: &arrow_array::RecordBatch) -> Vec<u8> {
        let mut writer = StreamWriter::try_new(Vec::new(), &schema.schema()).unwrap();
        for batch in batches {
            writer.write(&batch.to_record_batch().unwrap()).unwrap();
        }
        writer.finish().unwrap();
        writer.into_inner().unwrap()
    }

    #[test]
    fn answers_each_request_with_one_result_in_order() {
        let project =
            prepare_from_project(&LoadedProject::from_source(MODEL, "model.gcl").unwrap()).unwrap();
        let session = session(&project);
        let load = |values: &[f64]| {
            RequestBatch::new(
                Arc::clone(session.schema()),
                vec![InputColumn::Quantity(
                    values
                        .iter()
                        .map(|value| FiniteF64::new(*value).unwrap())
                        .collect(),
                )],
            )
            .unwrap()
        };
        let batches = [load(&[1.0, 5.0]), load(&[11.0])];
        let input = request_stream(&batches, &batches[0].to_record_batch().unwrap());

        let mut output = Vec::new();
        serve(
            &project,
            &session,
            RequestDeadline::Unbounded,
            Cursor::new(input),
            &mut output,
        )
        .unwrap();

        let reader = StreamReader::try_new(Cursor::new(output), None).unwrap();
        validate_schema(&result_schema(session.schema()), reader.schema().as_ref()).unwrap();
        let results = reader
            .map(|batch| {
                ResultBatch::from_record_batch(Arc::clone(session.schema()), &batch.unwrap())
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(results.len(), 2);
        let failure = session.schema().output_slot("failure").unwrap();
        let values = results[0]
            .outcomes()
            .map(|outcome| match outcome {
                RowOutcome::Success(row) => row.get(failure),
                RowOutcome::Failure(_) => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            values,
            [Some(ValueRef::Bool(false)), Some(ValueRef::Bool(true))]
        );
        assert!(matches!(
            results[1].row(0),
            Some(RowOutcome::Failure(
                graphcal_model_protocol::RowFailure::InputOutOfDomain { .. }
            ))
        ));
    }

    #[test]
    fn rejects_a_request_stream_of_another_schema_after_writing_the_header() {
        let project =
            prepare_from_project(&LoadedProject::from_source(MODEL, "model.gcl").unwrap()).unwrap();
        let session = session(&project);
        let foreign = arrow_array::RecordBatch::try_from_iter([(
            "load",
            Arc::new(arrow_array::Float64Array::from(vec![1.0])) as arrow_array::ArrayRef,
        )])
        .unwrap();
        let mut writer = StreamWriter::try_new(Vec::new(), &foreign.schema()).unwrap();
        writer.write(&foreign).unwrap();
        writer.finish().unwrap();
        let input = writer.into_inner().unwrap();

        let mut output = Vec::new();
        let error = serve(
            &project,
            &session,
            RequestDeadline::Unbounded,
            Cursor::new(input),
            &mut output,
        )
        .unwrap_err();
        assert!(matches!(error, ServeError::RequestSchema(_)));
        assert!(!output.is_empty(), "the result header precedes stdin reads");
    }

    #[test]
    fn a_request_past_its_deadline_ends_the_session_promptly() {
        use std::fmt::Write as _;

        // Each DAG calls the previous one twice: 2^48 additions per row.
        let mut model = String::from(
            "param load: Length(min: 0.0 m, max: 10.0 m);\n\
             dag d0 { pub node out: Dimensionless = 1.0; }\n",
        );
        for level in 1..=48 {
            let previous = level - 1;
            writeln!(
                model,
                "dag d{level} {{ pub node out: Dimensionless = @d{previous}()::out + @d{previous}()::out; }}"
            )
            .unwrap();
        }
        model.push_str("pub node failure: Bool = @d48()::out > 0.0 && @load > 1.0 m;\n");
        let project =
            prepare_from_project(&LoadedProject::from_source(&model, "model.gcl").unwrap())
                .unwrap();
        let session = session(&project);
        let request = RequestBatch::new(
            Arc::clone(session.schema()),
            vec![InputColumn::Quantity(vec![FiniteF64::new(5.0).unwrap()])],
        )
        .unwrap();
        let input = request_stream(
            std::slice::from_ref(&request),
            &request.to_record_batch().unwrap(),
        );

        let started = std::time::Instant::now();
        let mut output = Vec::new();
        let error = serve(
            &project,
            &session,
            RequestDeadline::After(Duration::from_millis(100)),
            Cursor::new(input),
            &mut output,
        )
        .unwrap_err();
        assert!(
            matches!(error, ServeError::DeadlineExceeded(timeout) if timeout == Duration::from_millis(100)),
            "{error:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the deadline must stop evaluation promptly"
        );
        assert!(!output.is_empty(), "the result header precedes evaluation");
    }

    #[test]
    fn a_result_completed_after_the_deadline_is_rejected() {
        let project =
            prepare_from_project(&LoadedProject::from_source(MODEL, "model.gcl").unwrap()).unwrap();
        let session = session(&project);
        let request = RequestBatch::new(
            Arc::clone(session.schema()),
            vec![InputColumn::Quantity(vec![FiniteF64::new(5.0).unwrap()])],
        )
        .unwrap();
        // A deadline that has already passed: the watchdog cancels at once,
        // and even an evaluation that finishes before observing it is
        // rejected at completion.
        let timeout = Duration::from_millis(5);
        let expired = Expiry {
            timeout,
            at: Instant::now().checked_sub(Duration::from_secs(1)),
        };
        assert!(expired.at.is_some());
        let error = evaluate_until(&project, &session, &request, &expired).unwrap_err();
        assert!(
            matches!(error, ServeError::DeadlineExceeded(reported) if reported == timeout),
            "{error:?}"
        );
    }

    #[test]
    fn a_request_within_its_deadline_is_answered() {
        let project =
            prepare_from_project(&LoadedProject::from_source(MODEL, "model.gcl").unwrap()).unwrap();
        let session = session(&project);
        let request = RequestBatch::new(
            Arc::clone(session.schema()),
            vec![InputColumn::Quantity(vec![FiniteF64::new(5.0).unwrap()])],
        )
        .unwrap();
        let input = request_stream(
            std::slice::from_ref(&request),
            &request.to_record_batch().unwrap(),
        );
        let mut output = Vec::new();
        serve(
            &project,
            &session,
            RequestDeadline::After(Duration::from_secs(60)),
            Cursor::new(input),
            &mut output,
        )
        .unwrap();
        let mut reader = StreamReader::try_new(Cursor::new(output), None).unwrap();
        let result = ResultBatch::from_record_batch(
            Arc::clone(session.schema()),
            &reader.next().unwrap().unwrap(),
        )
        .unwrap();
        assert!(matches!(result.row(0), Some(RowOutcome::Success(_))));
    }
}
