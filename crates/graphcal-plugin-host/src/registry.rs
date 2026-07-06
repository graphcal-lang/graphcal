//! Bridging loaded projects to the evaluator's host function registry.
//!
//! This is the one wiring point embedders (CLI, language server) call:
//! it walks the wasm plugin files the project loader read, loads each
//! through the [`PluginHost`] (hitting the content-hash cache on
//! re-analysis), and registers every manifest function as a host closure
//! carrying its manifest signature. Failures are recorded *into the
//! registry* rather than returned: the evaluation pipeline owns all plugin
//! diagnostics and reports them with the declaring import's span, so
//! embedders need no error plumbing of their own.

use std::sync::Arc;

use graphcal_eval::host_fns::{
    HostFnError, HostFnValue, HostFunctionRegistry, PluginRegistrationError,
};
use graphcal_eval::loader::LoadedProject;

use crate::host::PluginHost;
use crate::module::{PluginLoadError, PluginModule};

/// Load every wasm plugin the project references and register its functions.
///
/// File-level read failures recorded by the loader are left in
/// `project.plugins` (the pipeline reports them); module-level validation
/// failures are recorded in the registry via
/// [`HostFunctionRegistry::record_plugin_failure`]. Successfully loaded
/// modules register one closure per manifest function, each carrying the
/// manifest signature the pipeline verifies declarations against.
pub fn register_project_plugins(
    host: &PluginHost,
    project: &LoadedProject,
    registry: &mut HostFunctionRegistry,
) {
    for (plugin_path, entry) in &project.plugins {
        let Ok(plugin) = entry else {
            // The loader recorded why the file is unavailable; the pipeline
            // reports it from `project.plugins` directly.
            continue;
        };
        match host.load(&plugin.bytes) {
            Ok(module) => register_module_functions(plugin_path, &module, registry),
            Err(PluginLoadError::ForbiddenImport { module, name }) => {
                registry.record_plugin_failure(
                    plugin_path.clone(),
                    PluginRegistrationError::ForbiddenImport { module, name },
                );
            }
            Err(other) => {
                registry.record_plugin_failure(
                    plugin_path.clone(),
                    PluginRegistrationError::LoadFailed {
                        reason: other.to_string(),
                    },
                );
            }
        }
    }
}

/// Register every manifest function of one loaded module.
fn register_module_functions(
    plugin_path: &graphcal_compiler::syntax::plugin::PluginPath,
    module: &Arc<PluginModule>,
    registry: &mut HostFunctionRegistry,
) {
    for (name, signature) in module.functions() {
        let module = Arc::clone(module);
        let function_name = name.clone();
        registry.register_with_signature(
            plugin_path.clone(),
            name.clone(),
            signature.clone(),
            move |args| {
                // ABI v1 modules are scalar-only; array-capable manifests
                // arrive with the buffer protocol (issue #25 Phase D).
                let scalars: Vec<f64> = args
                    .iter()
                    .enumerate()
                    .map(|(position, value)| value.expect_scalar(position))
                    .collect::<Result<_, _>>()?;
                module
                    .call(&function_name, &scalars)
                    .map(HostFnValue::Scalar)
                    .map_err(|err| HostFnError::new(err.to_string()))
            },
        );
    }
}
