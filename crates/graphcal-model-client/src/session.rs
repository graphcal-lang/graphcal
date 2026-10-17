//! Configuring and running one `graphcal model serve` process.

use std::io::{BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, ExitStatus, Stdio};
use std::sync::Arc;
use std::thread::JoinHandle;

use arrow_ipc::reader::StreamReader;
use arrow_ipc::writer::StreamWriter;
use graphcal_model_protocol::arrow::{request_schema, result_schema, validate_schema};
use graphcal_model_protocol::{PortName, RequestBatch, ResultBatch, SessionSchema, SessionSpec};

use crate::batch::{BatchBuilder, Request};
use crate::error::ClientError;
use crate::evaluation::Evaluation;
use crate::identity::{ModelId, SessionId};
use crate::kind::Kind;
use crate::model::{InputHandle, Model, OutputHandle};

impl Model {
    /// Start configuring a session of this model.
    pub const fn session(&self) -> SessionBuilder<'_> {
        SessionBuilder::new(self)
    }
}

/// Selects the parameters a session varies and the outputs it reads.
#[derive(Debug)]
#[must_use]
pub struct SessionBuilder<'m> {
    model: &'m Model,
    inputs: Vec<PortName>,
    outputs: Vec<PortName>,
    foreign: Option<PortName>,
}

impl<'m> SessionBuilder<'m> {
    pub(crate) const fn new(model: &'m Model) -> Self {
        Self {
            model,
            inputs: Vec::new(),
            outputs: Vec::new(),
            foreign: None,
        }
    }

    /// Vary `input` in every request.
    pub fn vary<K: Kind>(mut self, input: &InputHandle<K>) -> Self {
        self.record(input.port(), true);
        self
    }

    /// Return `output` in every result.
    pub fn output<K: Kind>(mut self, output: &OutputHandle<K>) -> Self {
        self.record(output.port(), false);
        self
    }

    fn record(&mut self, port: &crate::identity::PortIdentity, input: bool) {
        if port.ensure_model(self.model.id()).is_err() {
            self.foreign.get_or_insert_with(|| port.name().clone());
        } else if input {
            self.inputs.push(port.name().clone());
        } else {
            self.outputs.push(port.name().clone());
        }
    }

    /// The session schema this selection configures, without starting a
    /// server.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] for a handle of another model or an invalid
    /// selection.
    pub fn schema(&self) -> Result<SessionSchema, ClientError> {
        if let Some(name) = &self.foreign {
            return Err(ClientError::ForeignHandle { name: name.clone() });
        }
        Ok(self
            .model
            .interface()
            .configure(&SessionSpec::new(self.inputs.clone(), self.outputs.clone()))?)
    }

    /// Start `graphcal model serve` for this session.
    ///
    /// The server refuses to start unless the model's interface still has
    /// the described fingerprint.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] for an invalid selection, a server that cannot
    /// start, or an unexpected result-stream schema.
    pub fn spawn(self) -> Result<Session, ClientError> {
        let schema = Arc::new(self.schema()?);
        let mut child = self
            .model
            .command()
            .serve(&schema)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(ClientError::Spawn)?;
        let stderr = child.stderr.take().map(|mut stderr| {
            std::thread::spawn(move || {
                let mut captured = Vec::new();
                // A read failure only truncates the diagnostic text.
                let _ = stderr.read_to_end(&mut captured);
                captured
            })
        });
        let mut process = ServerProcess { child, stderr };
        match process.open_streams(&schema) {
            Ok((requests, results)) => Ok(Session {
                id: SessionId::fresh(),
                model: self.model.id(),
                schema,
                requests: Some(requests),
                results,
                process: Some(process),
            }),
            Err(error) => Err(process.failure(error)),
        }
    }
}

type Results = StreamReader<BufReader<ChildStdout>>;

struct ServerProcess {
    child: Child,
    stderr: Option<JoinHandle<Vec<u8>>>,
}

impl ServerProcess {
    fn open_streams(
        &mut self,
        schema: &SessionSchema,
    ) -> Result<(StreamWriter<ChildStdin>, Results), ClientError> {
        let stdout = self.child.stdout.take().ok_or_else(missing_pipe)?;
        let results = StreamReader::try_new(BufReader::new(stdout), None)?;
        validate_schema(&result_schema(schema), results.schema().as_ref())?;
        let stdin = self.child.stdin.take().ok_or_else(missing_pipe)?;
        let requests = StreamWriter::try_new(stdin, &request_schema(schema))?;
        Ok((requests, results))
    }

    /// Wait for the server to stop and report `error`, preferring the
    /// server's own diagnostic when it stopped unsuccessfully.
    fn failure(mut self, error: ClientError) -> ClientError {
        match self.finish() {
            Ok((status, stderr)) if !status.success() => {
                ClientError::ServerStopped { status, stderr }
            }
            Ok(_) | Err(_) => error,
        }
    }

    fn finish(&mut self) -> Result<(ExitStatus, String), ClientError> {
        let status = self.child.wait()?;
        let stderr = match self.stderr.take().map(JoinHandle::join) {
            Some(Ok(captured)) => String::from_utf8_lossy(&captured).into_owned(),
            Some(Err(_)) => "<the stderr reader of the model server panicked>".to_owned(),
            None => "<the model server's stderr was not captured>".to_owned(),
        };
        Ok((status, stderr))
    }
}

fn missing_pipe() -> ClientError {
    ClientError::Io(std::io::Error::other(
        "the model server pipe is not available",
    ))
}

/// One running `graphcal model serve` process.
///
/// Each [`Self::evaluate`] call sends one request and waits for its result.
/// Call [`Self::shutdown`] when done; dropping a session kills the server.
pub struct Session {
    id: SessionId,
    model: ModelId,
    schema: Arc<SessionSchema>,
    requests: Option<StreamWriter<ChildStdin>>,
    results: Results,
    process: Option<ServerProcess>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Session")
            .field("schema", &self.schema)
            .finish_non_exhaustive()
    }
}

impl Session {
    /// The session schema both peers share.
    #[must_use]
    pub const fn schema(&self) -> &Arc<SessionSchema> {
        &self.schema
    }

    /// Start building a request.
    pub fn batch(&self) -> BatchBuilder<'_> {
        BatchBuilder::new(self.id, self.model, &self.schema)
    }

    /// Wrap an already built protocol batch of this session as a request.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError::ForeignSession`] when `batch` was validated
    /// against another session schema.
    pub fn request(&self, batch: RequestBatch) -> Result<Request, ClientError> {
        if batch.schema().as_ref() == self.schema.as_ref() {
            Ok(Request::new(self.id, batch))
        } else {
            Err(ClientError::ForeignSession)
        }
    }

    /// Send `request` and wait for its result.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] for a request of another session, an I/O or
    /// IPC failure, or a stopped server; the session is unusable afterwards.
    pub fn evaluate(&mut self, request: &Request) -> Result<Evaluation, ClientError> {
        if request.session() != self.id {
            return Err(ClientError::ForeignSession);
        }
        let outcome = self.exchange(request);
        match outcome {
            Ok(result) => Ok(Evaluation::new(self.model, result)),
            Err(error) => {
                drop(self.requests.take());
                Err(match self.process.take() {
                    Some(process) => process.stopped(error),
                    None => error,
                })
            }
        }
    }

    fn exchange(&mut self, request: &Request) -> Result<ResultBatch, ClientError> {
        let requests = self.requests.as_mut().ok_or(ClientError::SessionClosed)?;
        requests.write(&request.batch().to_record_batch()?)?;
        requests.get_mut().flush()?;
        let received = self.results.next().ok_or_else(|| {
            ClientError::Io(std::io::Error::from(std::io::ErrorKind::UnexpectedEof))
        })??;
        let result = ResultBatch::from_record_batch(Arc::clone(&self.schema), &received)?;
        if result.rows() != request.batch().rows() {
            return Err(ClientError::RowCount {
                expected: request.batch().rows(),
                received: result.rows(),
            });
        }
        Ok(result)
    }

    /// End the request stream and wait for the server to exit.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] when the server does not end its result
    /// stream cleanly or exits unsuccessfully.
    pub fn shutdown(mut self) -> Result<(), ClientError> {
        let finished = self
            .requests
            .take()
            .map_or(Ok(()), |mut requests| requests.finish());
        let trailing = self.results.next();
        let Some(mut process) = self.process.take() else {
            return Err(ClientError::SessionClosed);
        };
        let (status, stderr) = process.finish()?;
        if !status.success() {
            return Err(ClientError::ServerStopped { status, stderr });
        }
        finished?;
        match trailing {
            None => Ok(()),
            Some(Err(error)) => Err(error.into()),
            Some(Ok(_)) => Err(ClientError::UnsolicitedResult),
        }
    }
}

impl ServerProcess {
    /// Report `error` after a failed exchange, preferring the server's own
    /// diagnostic when it already stopped.
    fn stopped(mut self, error: ClientError) -> ClientError {
        if let Ok(Some(_)) = self.child.try_wait() {
            self.failure(error)
        } else {
            // The server is still running but out of step; stop it.
            let _ = self.child.kill();
            let _ = self.finish();
            error
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(mut process) = self.process.take() {
            drop(self.requests.take());
            let _ = process.child.kill();
            let _ = process.finish();
        }
    }
}
