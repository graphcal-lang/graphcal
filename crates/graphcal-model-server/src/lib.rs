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

use std::io::{Read, Write};

use arrow_ipc::reader::StreamReader;
use arrow_ipc::writer::StreamWriter;
use graphcal_model_protocol::arrow::{
    RequestDecodeError, SchemaMismatch, request_schema, result_schema, validate_schema,
};
use graphcal_model_protocol::{RequestBatch, ResultBatch};
use graphcal_project::prepare::{ModelExecutionError, ModelSession, PreparedProject};
use thiserror::Error;

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
pub fn serve_stdio(project: &PreparedProject, session: &ModelSession) -> Result<(), ServeError> {
    serve(
        project,
        session,
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
        let result: ResultBatch = project.evaluate_request(session, &request)?;
        results.write(&result.to_record_batch()?)?;
        results.get_mut().flush()?;
    }
    results.finish()?;
    results.get_mut().flush()?;
    Ok(())
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
            .model_interface()
            .unwrap()
            .configure(&SessionSpec::new(
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
        serve(&project, &session, Cursor::new(input), &mut output).unwrap();

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
        let error = serve(&project, &session, Cursor::new(input), &mut output).unwrap_err();
        assert!(matches!(error, ServeError::RequestSchema(_)));
        assert!(!output.is_empty(), "the result header precedes stdin reads");
    }
}
