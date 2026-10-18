//! Batch model-evaluation CLI shell: `graphcal model describe` and
//! `graphcal model serve`.

use std::path::Path;
use std::sync::Arc;

use graphcal_model_protocol::{
    DescribeDocument, InterfaceFingerprint, LabelError, PortName, SessionSpec, UnitSpelling,
};
use graphcal_project::compile_error::CompileError;
use graphcal_project::loader::{build_rooted_filesystem, load_project};
use graphcal_project::prepare::{ModelInterfaceError, ModelSessionError, PreparedProject};
use graphcal_project::project_compiler::ProjectCompiler;
use thiserror::Error;

/// How `serve` guards against serving a different interface than the client
/// configured its session over.
pub enum FingerprintCheck {
    /// Refuse to start unless the model's interface has this fingerprint.
    Expect(InterfaceFingerprint),
    /// Serve whatever interface the model has.
    Skip,
}

/// One `--unit NAME=UNIT` argument of `serve`.
#[derive(Debug, Clone)]
pub struct UnitChoiceArgument {
    port: PortName,
    unit: UnitSpelling,
}

/// Parse a `--unit NAME=UNIT` argument; the unit itself is resolved against
/// the model later.
pub fn parse_unit_choice(argument: &str) -> Result<UnitChoiceArgument, String> {
    let (port, unit) = argument
        .split_once('=')
        .ok_or_else(|| format!("expected NAME=UNIT, got `{argument}`"))?;
    Ok(UnitChoiceArgument {
        port: PortName::new(port.trim()).map_err(|error| error.to_string())?,
        unit: UnitSpelling::new(unit.trim()).map_err(|error| error.to_string())?,
    })
}

/// Print the model's describe document as JSON on stdout.
pub fn describe(file: &Path, root: Option<&Path>) -> Result<(), ModelServeError> {
    let prepared = prepare(file, root)?;
    let document = DescribeDocument::new(env!("CARGO_PKG_VERSION"), prepared.model_interface()?);
    let json = serde_json::to_string_pretty(&document).map_err(ModelServeError::Json)?;
    println!("{json}");
    Ok(())
}

/// Prepare and serve one session of the model over stdin and stdout.
///
/// Setup errors occur before any stdout bytes. A [`ModelServeError::Serve`]
/// may occur after the result-stream schema has been written.
pub fn serve(
    file: &Path,
    inputs: &[String],
    outputs: &[String],
    units: &[UnitChoiceArgument],
    fingerprint: &FingerprintCheck,
    root: Option<&Path>,
) -> Result<(), ModelServeError> {
    let names = |names: &[String]| {
        names
            .iter()
            .map(|name| PortName::new(name.as_str()))
            .collect::<Result<Vec<_>, _>>()
    };
    let spec = units.iter().fold(
        SessionSpec::new(names(inputs)?, names(outputs)?),
        |spec, choice| spec.with_unit(choice.port.clone(), choice.unit.clone()),
    );
    let prepared = prepare(file, root)?;
    let interface = prepared.model_interface()?;
    if let FingerprintCheck::Expect(expected) = fingerprint {
        let actual = InterfaceFingerprint::of(&interface);
        if actual != *expected {
            return Err(ModelServeError::FingerprintMismatch {
                expected: *expected,
                actual,
            });
        }
    }
    let schema = prepared.session_schema(&spec)?;
    let session = prepared.model_session(Arc::new(schema))?;
    graphcal_model_server::serve_stdio(&prepared, &session).map_err(ModelServeError::Serve)
}

fn prepare(file: &Path, root: Option<&Path>) -> Result<PreparedProject, ModelServeError> {
    let fs = build_rooted_filesystem(file, root)?;
    let project = load_project(file, root, &fs)?;
    let mut host_fns = graphcal_eval::host_fns::demo_registry();
    graphcal_plugin_host::register_project_plugins(
        &graphcal_plugin_host::PluginHost::new(),
        &project,
        &mut host_fns,
    );
    Ok(ProjectCompiler::new(&project)
        .host_fns(&host_fns)
        .prepare()?)
}

/// Model setup or process-lifetime failure.
#[derive(Debug, Error)]
pub enum ModelServeError {
    #[error("invalid port name: {0}")]
    PortName(#[from] LabelError),
    #[error(transparent)]
    Compile(#[from] CompileError),
    #[error(transparent)]
    Interface(#[from] ModelInterfaceError),
    #[error(
        "the model's interface fingerprint is {actual}, not the expected {expected}; describe the model again"
    )]
    FingerprintMismatch {
        expected: InterfaceFingerprint,
        actual: InterfaceFingerprint,
    },
    #[error(transparent)]
    ModelSession(#[from] ModelSessionError),
    #[error("cannot encode the describe document: {0}")]
    Json(serde_json::Error),
    #[error("model server failed: {0}")]
    Serve(graphcal_model_server::ServeError),
}

impl ModelServeError {
    /// Exit status class: setup/configuration errors use 2; a started server
    /// that becomes unusable uses 1.
    #[must_use]
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::Serve(_) => 1,
            Self::PortName(_)
            | Self::Compile(_)
            | Self::Interface(_)
            | Self::FingerprintMismatch { .. }
            | Self::ModelSession(_)
            | Self::Json(_) => 2,
        }
    }
}
