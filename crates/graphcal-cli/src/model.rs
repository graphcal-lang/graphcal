//! Persistent external-model CLI shell.

use std::path::Path;

use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_eval::eval::{CompileError, ModelDefinitionError, prepare_from_project_with_host_fns};
use graphcal_eval::loader::{build_rooted_filesystem, load_project};
use thiserror::Error;

/// Prepare and serve one Graphcal/Tenax model.
///
/// Setup errors occur before any stdout bytes. A [`ModelServeError::Protocol`]
/// may occur after the normative Arrow stream headers have been written.
pub fn serve(file: &Path, outputs: &[String], root: Option<&Path>) -> Result<(), ModelServeError> {
    let output_names = outputs
        .iter()
        .map(|name| DeclName::try_new(name.clone()))
        .collect::<Result<Vec<_>, _>>()?;
    let fs = build_rooted_filesystem(file, root);
    let project = load_project(file, root, &fs)?;
    let mut host_fns = graphcal_eval::host_fns::demo_registry();
    graphcal_plugin_host::register_project_plugins(
        &graphcal_plugin_host::PluginHost::new(),
        &project,
        &mut host_fns,
    );
    let prepared = prepare_from_project_with_host_fns(&project, &host_fns)?;
    let model = prepared.tenax_v2_model(&output_names)?;
    graphcal_tenax::serve_stdio(&prepared, &model).map_err(ModelServeError::Protocol)
}

/// Model-server setup or process-lifetime failure.
#[derive(Debug, Error)]
pub enum ModelServeError {
    #[error("invalid --output name: {0}")]
    OutputName(#[from] graphcal_compiler::syntax::names::NameAtomError),
    #[error(transparent)]
    Compile(#[from] CompileError),
    #[error("cannot expose model through Tenax schema v2: {0}")]
    Definition(#[from] ModelDefinitionError),
    #[error("Tenax stdio model server failed: {0}")]
    Protocol(graphcal_tenax::TenaxProtocolError),
}

impl ModelServeError {
    /// Exit status class: setup/configuration errors use 2; a started protocol
    /// process that becomes unusable uses 1.
    #[must_use]
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::Protocol(_) => 1,
            Self::OutputName(_) | Self::Compile(_) | Self::Definition(_) => 2,
        }
    }
}
