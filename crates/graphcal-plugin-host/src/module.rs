//! A validated, callable WASM plugin module.
//!
//! Loading a [`PluginModule`] performs every load-time check the ABI demands
//! — manifest extraction and typed conversion, the import ban (purity by
//! construction), the memory-export rule, and per-function wasm type
//! verification — before any plugin code runs. [`PluginModule::call`] then
//! executes one function under the configured fuel and memory bounds, with
//! failure messages, traps, and fuel exhaustion mapped to
//! [`PluginCallError`].

use std::sync::{Mutex, PoisonError};

use graphcal_compiler::function_signature::FunctionSignature;
use graphcal_compiler::syntax::function_name::FnName;
use graphcal_plugin_abi::{
    FAIL_IMPORT_MODULE, FAIL_IMPORT_NAME, MAX_FAIL_MESSAGE_BYTES, ManifestFromWasmError,
    PluginManifest,
};
use sha2::Digest as _;
use thiserror::Error;

use crate::convert::{ManifestConvertError, convert_manifest};
use crate::host::PluginLimits;

/// Host-side state carried by each plugin store.
struct CallState {
    /// Resource limiter enforcing the memory cap.
    limits: wasmi::StoreLimits,
    /// Message recorded by the `graphcal::fail` import during the current
    /// call, if any.
    fail_message: Option<String>,
}

/// One instantiated plugin, reused across successful calls.
struct LiveInstance {
    store: wasmi::Store<CallState>,
    instance: wasmi::Instance,
}

/// A compiled and fully validated plugin module.
///
/// Cheap to share; obtain through
/// [`PluginHost::load`](crate::host::PluginHost::load), which caches modules
/// by content hash. Calls reuse one instance and discard it after a failed
/// call (the instance may be arbitrarily damaged), so a failure in one graph
/// node cannot corrupt later calls.
pub struct PluginModule {
    engine: wasmi::Engine,
    module: wasmi::Module,
    manifest: PluginManifest,
    functions: Vec<(FnName, FunctionSignature)>,
    sha256: [u8; 32],
    limits: PluginLimits,
    instance: Mutex<Option<LiveInstance>>,
}

impl std::fmt::Debug for PluginModule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginModule")
            .field("sha256", &self.sha256_hex())
            .field(
                "functions",
                &self
                    .functions
                    .iter()
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl PluginModule {
    /// Compile and validate a plugin from its `.wasm` bytes.
    ///
    /// No plugin code runs: the manifest comes from the custom section, and
    /// all checks are static. Prefer
    /// [`PluginHost::load`](crate::host::PluginHost::load), which adds
    /// content-hash caching.
    ///
    /// # Errors
    ///
    /// Returns [`PluginLoadError`] when the bytes are not a valid module,
    /// the manifest is missing or malformed, the module imports anything
    /// beyond `graphcal::fail`, the memory-export rule is violated, or an
    /// exported function's wasm type does not match its manifest signature.
    pub(crate) fn new(
        engine: &wasmi::Engine,
        bytes: &[u8],
        limits: PluginLimits,
    ) -> Result<Self, PluginLoadError> {
        let manifest = PluginManifest::from_wasm(bytes)?;
        let functions = convert_manifest(&manifest)?;

        let module =
            wasmi::Module::new(engine, bytes).map_err(|err| PluginLoadError::InvalidModule {
                message: err.to_string(),
            })?;

        let mut imports_fail = false;
        for import in module.imports() {
            if import.module() != FAIL_IMPORT_MODULE || import.name() != FAIL_IMPORT_NAME {
                return Err(PluginLoadError::ForbiddenImport {
                    module: import.module().to_string(),
                    name: import.name().to_string(),
                });
            }
            match import.ty() {
                wasmi::ExternType::Func(ty)
                    if ty.params() == [wasmi::ValType::I32; 2] && ty.results().is_empty() =>
                {
                    imports_fail = true;
                }
                other => {
                    return Err(PluginLoadError::FailImportTypeMismatch {
                        found: describe_extern_type(other),
                    });
                }
            }
        }

        if imports_fail
            && !matches!(
                module.get_export("memory"),
                Some(wasmi::ExternType::Memory(_))
            )
        {
            return Err(PluginLoadError::MissingMemoryExport);
        }

        for (name, signature) in &functions {
            let export = module.get_export(name.as_str()).ok_or_else(|| {
                PluginLoadError::MissingFunctionExport {
                    function: name.clone(),
                }
            })?;
            let arity = signature.arity();
            let matches_abi = matches!(
                &export,
                wasmi::ExternType::Func(ty)
                    if ty.params().len() == arity
                        && ty.params().iter().all(|param| *param == wasmi::ValType::F64)
                        && ty.results() == [wasmi::ValType::F64]
            );
            if !matches_abi {
                return Err(PluginLoadError::FunctionTypeMismatch {
                    function: name.clone(),
                    expected: format!("(f64 x {arity}) -> f64"),
                    found: describe_extern_type(&export),
                });
            }
        }

        Ok(Self {
            engine: engine.clone(),
            module,
            manifest,
            functions,
            sha256: sha2::Sha256::digest(bytes).into(),
            limits,
            instance: Mutex::new(None),
        })
    }

    /// The decoded manifest embedded in the module.
    #[must_use]
    pub const fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    /// The typed signatures the module provides, in manifest order.
    #[must_use]
    pub fn functions(&self) -> &[(FnName, FunctionSignature)] {
        &self.functions
    }

    /// The typed signature of one provided function.
    #[must_use]
    pub fn signature(&self, function: &FnName) -> Option<&FunctionSignature> {
        self.functions
            .iter()
            .find(|(name, _)| name == function)
            .map(|(_, signature)| signature)
    }

    /// SHA-256 of the module bytes.
    #[must_use]
    pub const fn sha256(&self) -> &[u8; 32] {
        &self.sha256
    }

    /// SHA-256 of the module bytes as lowercase hex, the form pinned in
    /// `graphcal.lock`.
    #[must_use]
    pub fn sha256_hex(&self) -> String {
        use std::fmt::Write as _;

        self.sha256
            .iter()
            .fold(String::with_capacity(64), |mut out, byte| {
                let _ = write!(out, "{byte:02x}");
                out
            })
    }

    /// Call one plugin function with SI-normalized scalar arguments.
    ///
    /// The call runs under the module's fuel and memory limits. A non-finite
    /// result is returned as-is — policing non-finite values is the
    /// evaluator's job, shared with every other arithmetic path.
    ///
    /// # Errors
    ///
    /// Returns [`PluginCallError`] when the plugin reports a failure through
    /// `graphcal::fail`, traps, exhausts its fuel, or violates the scalar
    /// ABI.
    pub fn call(&self, function: &FnName, args: &[f64]) -> Result<f64, PluginCallError> {
        let mut slot = self.instance.lock().unwrap_or_else(PoisonError::into_inner);
        let mut live = match slot.take() {
            Some(live) => live,
            None => self.instantiate()?,
        };
        // A failed call may leave the instance arbitrarily damaged
        // (poisoned memory, mid-unwind state); it is dropped and the next
        // call starts from a fresh instantiation.
        let value = self.call_in(&mut live, function, args)?;
        *slot = Some(live);
        drop(slot);
        Ok(value)
    }

    fn instantiate(&self) -> Result<LiveInstance, PluginCallError> {
        let limiter = wasmi::StoreLimitsBuilder::new()
            .memory_size(self.limits.max_memory_bytes)
            .memories(1)
            .tables(1)
            .instances(1)
            .build();
        let mut store = wasmi::Store::new(
            &self.engine,
            CallState {
                limits: limiter,
                fail_message: None,
            },
        );
        store.limiter(|state| &mut state.limits);
        // The start function (if any) is plugin code: meter it like a call.
        set_fuel(&mut store, self.limits.fuel_per_call)?;

        let mut linker = wasmi::Linker::new(&self.engine);
        linker
            .func_wrap(FAIL_IMPORT_MODULE, FAIL_IMPORT_NAME, host_fail)
            .map_err(|err| PluginCallError::Internal {
                message: format!("failed to install the graphcal::fail import: {err}"),
            })?;

        let instance = linker
            .instantiate_and_start(&mut store, &self.module)
            .map_err(|err| error_from_wasm(&mut store, &err, self.limits.fuel_per_call))?;
        Ok(LiveInstance { store, instance })
    }

    fn call_in(
        &self,
        live: &mut LiveInstance,
        function: &FnName,
        args: &[f64],
    ) -> Result<f64, PluginCallError> {
        let func = live
            .instance
            .get_export(&live.store, function.as_str())
            .and_then(wasmi::Extern::into_func)
            .ok_or_else(|| PluginCallError::UnknownFunction {
                function: function.clone(),
            })?;

        set_fuel(&mut live.store, self.limits.fuel_per_call)?;
        live.store.data_mut().fail_message = None;

        let params: Vec<wasmi::Val> = args
            .iter()
            .map(|value| wasmi::Val::F64((*value).into()))
            .collect();
        let mut results = [wasmi::Val::F64(0.0.into())];
        func.call(&mut live.store, &params, &mut results)
            .map_err(|err| error_from_wasm(&mut live.store, &err, self.limits.fuel_per_call))?;

        match results[0] {
            wasmi::Val::F64(value) => Ok(value.into()),
            ref other => Err(PluginCallError::Internal {
                message: format!(
                    "function `{function}` returned a non-f64 value ({other:?}) despite load-time type checks"
                ),
            }),
        }
    }
}

fn set_fuel(store: &mut wasmi::Store<CallState>, fuel: u64) -> Result<(), PluginCallError> {
    store
        .set_fuel(fuel)
        .map_err(|err| PluginCallError::Internal {
            message: format!("failed to set fuel: {err}"),
        })
}

/// The host implementation of the `graphcal::fail` import: record the
/// message, then trap the current call.
fn host_fail(
    mut caller: wasmi::Caller<'_, CallState>,
    ptr: u32,
    len: u32,
) -> Result<(), wasmi::Error> {
    let message = read_fail_message(&caller, ptr, len);
    caller.data_mut().fail_message = Some(message);
    Err(wasmi::Error::new("graphcal plugin reported a failure"))
}

fn read_fail_message(caller: &wasmi::Caller<'_, CallState>, ptr: u32, len: u32) -> String {
    // Memory presence is validated at load for modules importing fail; the
    // fallbacks below are defense in depth, not reachable paths.
    let Some(memory) = caller
        .get_export("memory")
        .and_then(wasmi::Extern::into_memory)
    else {
        return "<plugin reported a failure but exports no memory>".to_string();
    };
    let len = (len as usize).min(MAX_FAIL_MESSAGE_BYTES);
    let mut buffer = vec![0_u8; len];
    if memory.read(caller, ptr as usize, &mut buffer).is_err() {
        return "<plugin reported a failure with an out-of-bounds message>".to_string();
    }
    String::from_utf8_lossy(&buffer).into_owned()
}

/// Map a wasmi execution error to the typed call error, consuming any
/// failure message the `graphcal::fail` import recorded.
fn error_from_wasm(
    store: &mut wasmi::Store<CallState>,
    err: &wasmi::Error,
    fuel: u64,
) -> PluginCallError {
    if let Some(message) = store.data_mut().fail_message.take() {
        return PluginCallError::Failed { message };
    }
    if matches!(err.as_trap_code(), Some(wasmi::TrapCode::OutOfFuel)) {
        return PluginCallError::OutOfFuel { fuel };
    }
    PluginCallError::Trap {
        message: err.to_string(),
    }
}

fn describe_extern_type(ty: &wasmi::ExternType) -> String {
    match ty {
        wasmi::ExternType::Func(func) => describe_func_type(func),
        wasmi::ExternType::Global(_) => "a global".to_string(),
        wasmi::ExternType::Memory(_) => "a memory".to_string(),
        wasmi::ExternType::Table(_) => "a table".to_string(),
    }
}

fn describe_func_type(ty: &wasmi::FuncType) -> String {
    let list = |types: &[wasmi::ValType]| {
        types
            .iter()
            .map(|ty| format!("{ty:?}").to_lowercase())
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!("({}) -> ({})", list(ty.params()), list(ty.results()))
}

/// Error validating and compiling a plugin module.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PluginLoadError {
    /// The manifest custom section is missing, duplicated, or malformed.
    #[error(transparent)]
    Manifest(#[from] ManifestFromWasmError),
    /// A manifest signature failed to convert to the typed IR.
    #[error(transparent)]
    InvalidSignature(#[from] ManifestConvertError),
    /// The bytes are not a valid WebAssembly module.
    #[error("invalid WebAssembly module: {message}")]
    InvalidModule {
        /// The wasm engine's error message.
        message: String,
    },
    /// The module imports something other than `graphcal::fail`.
    ///
    /// The import ban is what guarantees plugins cannot perform I/O; a
    /// module tripping it is not a graphcal plugin (a WASI build is the
    /// usual culprit).
    #[error(
        "plugin imports `{module}::{name}`; graphcal plugins may import nothing except \
         `{fail_module}::{fail_name}`",
        fail_module = FAIL_IMPORT_MODULE,
        fail_name = FAIL_IMPORT_NAME
    )]
    ForbiddenImport {
        /// Wasm module name of the forbidden import.
        module: String,
        /// Wasm field name of the forbidden import.
        name: String,
    },
    /// The module imports `graphcal::fail` with the wrong type.
    #[error(
        "plugin imports `{fail_module}::{fail_name}` as {found}, expected (i32, i32) -> ()",
        fail_module = FAIL_IMPORT_MODULE,
        fail_name = FAIL_IMPORT_NAME
    )]
    FailImportTypeMismatch {
        /// What the module actually imported.
        found: String,
    },
    /// The module imports `graphcal::fail` but does not export its memory.
    #[error(
        "plugin imports `{fail_module}::{fail_name}` but does not export its memory as \
         \"memory\", so failure messages cannot be read",
        fail_module = FAIL_IMPORT_MODULE,
        fail_name = FAIL_IMPORT_NAME
    )]
    MissingMemoryExport,
    /// A manifest function has no corresponding wasm export.
    #[error("plugin manifest declares `{function}`, but the module does not export it")]
    MissingFunctionExport {
        /// The undeclared function.
        function: FnName,
    },
    /// A manifest function's wasm export has the wrong type.
    #[error(
        "plugin function `{function}` is exported as {found}, but its manifest signature \
         requires {expected}"
    )]
    FunctionTypeMismatch {
        /// The mismatched function.
        function: FnName,
        /// The wasm type the manifest arity requires.
        expected: String,
        /// The wasm type (or non-function export) actually found.
        found: String,
    },
}

/// Error from calling a plugin function.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PluginCallError {
    /// The plugin reported a failure through `graphcal::fail`.
    #[error("{message}")]
    Failed {
        /// The plugin's failure message.
        message: String,
    },
    /// The plugin trapped (unreachable, out-of-bounds access, stack
    /// overflow, denied memory growth, …).
    #[error("plugin trapped: {message}")]
    Trap {
        /// The trap description from the wasm engine.
        message: String,
    },
    /// The call exceeded its fuel budget.
    #[error("plugin exceeded its execution budget ({fuel} fuel units)")]
    OutOfFuel {
        /// The configured per-call fuel budget.
        fuel: u64,
    },
    /// The function is not provided by this module (a host wiring bug —
    /// load-time validation covers every declared function).
    #[error("plugin does not provide function `{function}`")]
    UnknownFunction {
        /// The unknown function.
        function: FnName,
    },
    /// An internal invariant of the host itself failed.
    #[error("plugin host internal error: {message}")]
    Internal {
        /// Description of the violated invariant.
        message: String,
    },
}
