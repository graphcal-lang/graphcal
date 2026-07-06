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

use graphcal_compiler::function_signature::{FunctionSignature, ValueKind};
use graphcal_compiler::syntax::function_name::FnName;
use graphcal_eval::host_fns::HostFnValue;
use graphcal_plugin_abi::{
    ALLOC_EXPORT, FAIL_IMPORT_MODULE, FAIL_IMPORT_NAME, FREE_EXPORT, MAX_FAIL_MESSAGE_BYTES,
    ManifestFromWasmError, PluginManifest,
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
            let expected = expected_wasm_type(signature);
            let matches_abi = matches!(
                &export,
                wasmi::ExternType::Func(ty)
                    if ty.params() == expected.params.as_slice()
                        && ty.results() == expected.results.as_slice()
            );
            if !matches_abi {
                return Err(PluginLoadError::FunctionTypeMismatch {
                    function: name.clone(),
                    expected: expected.describe(),
                    found: describe_extern_type(&export),
                });
            }
        }

        // Modules whose signatures move arrays need the buffer protocol:
        // an exported memory the host can read/write plus the allocator
        // pair it places buffers with.
        if functions
            .iter()
            .any(|(_, signature)| signature_uses_buffers(signature))
        {
            if !matches!(
                module.get_export("memory"),
                Some(wasmi::ExternType::Memory(_))
            ) {
                return Err(PluginLoadError::MissingBufferProtocolExport {
                    export: "memory".to_string(),
                    expected: "an exported linear memory".to_string(),
                });
            }
            check_buffer_protocol_func(
                &module,
                ALLOC_EXPORT,
                &[wasmi::ValType::I32],
                &[wasmi::ValType::I32],
            )?;
            check_buffer_protocol_func(&module, FREE_EXPORT, &[wasmi::ValType::I32; 2], &[])?;
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

    /// Call one plugin function with SI-normalized values: scalars cross as
    /// raw `f64`s, arrays as dense buffers the host places in (and reads
    /// back from) the plugin's memory through the allocator exports.
    ///
    /// The call — including the allocator round-trips it needs — runs under
    /// the module's fuel and memory limits. A non-finite result is returned
    /// as-is; policing non-finite values is the evaluator's job, shared with
    /// every other arithmetic path.
    ///
    /// # Errors
    ///
    /// Returns [`PluginCallError`] when the plugin reports a failure through
    /// `graphcal::fail`, traps, exhausts its fuel, or violates the ABI.
    pub fn call(
        &self,
        function: &FnName,
        args: &[HostFnValue],
    ) -> Result<HostFnValue, PluginCallError> {
        let signature =
            self.signature(function)
                .ok_or_else(|| PluginCallError::UnknownFunction {
                    function: function.clone(),
                })?;
        let mut slot = self.instance.lock().unwrap_or_else(PoisonError::into_inner);
        let mut live = match slot.take() {
            Some(live) => live,
            None => self.instantiate()?,
        };
        // A failed call may leave the instance arbitrarily damaged
        // (poisoned memory, mid-unwind state, leaked buffers); it is dropped
        // and the next call starts from a fresh instantiation.
        let value = self.call_in(&mut live, function, signature, args)?;
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
        signature: &FunctionSignature,
        args: &[HostFnValue],
    ) -> Result<HostFnValue, PluginCallError> {
        if args.len() != signature.arity() {
            return Err(PluginCallError::Internal {
                message: format!(
                    "function `{function}` called with {} argument(s), signature takes {}",
                    args.len(),
                    signature.arity()
                ),
            });
        }
        let func = live
            .instance
            .get_export(&live.store, function.as_str())
            .and_then(wasmi::Extern::into_func)
            .ok_or_else(|| PluginCallError::UnknownFunction {
                function: function.clone(),
            })?;

        // One fuel budget covers the whole logical call: the allocator
        // round-trips below and the function body itself.
        set_fuel(&mut live.store, self.limits.fuel_per_call)?;
        live.store.data_mut().fail_message = None;

        let mut buffers = if signature_uses_buffers(signature) {
            Some(BufferProtocol::resolve(live, function)?)
        } else {
            None
        };

        let (params, out_buffer) =
            self.marshal_params(live, function, signature, args, &mut buffers)?;

        let mut results = if out_buffer.is_some() {
            Vec::new()
        } else {
            vec![wasmi::Val::F64(0.0.into())]
        };
        func.call(&mut live.store, &params, &mut results)
            .map_err(|err| error_from_wasm(&mut live.store, &err, self.limits.fuel_per_call))?;

        let value = match (out_buffer, buffers.as_ref()) {
            (Some(out), Some(buffers)) => {
                HostFnValue::Buffer(buffers.read_buffer(live, out.ptr, out.len)?)
            }
            (Some(_), None) => {
                return Err(PluginCallError::Internal {
                    message: format!(
                        "function `{function}` produced an out-buffer without the buffer protocol"
                    ),
                });
            }
            (None, _) => match results.first() {
                Some(wasmi::Val::F64(value)) => HostFnValue::Scalar(f64::from(*value)),
                other => {
                    return Err(PluginCallError::Internal {
                        message: format!(
                            "function `{function}` returned {other:?} despite load-time type checks"
                        ),
                    });
                }
            },
        };

        // Return every buffer to the plugin's allocator so the pooled
        // instance does not leak across calls. A failing free damages the
        // instance like any other trap; the caller discards it.
        if let Some(buffers) = buffers {
            buffers.free_all(live, self.limits.fuel_per_call)?;
        }
        Ok(value)
    }

    /// Build the wasm parameter list for one call: scalars as `f64`s, arrays
    /// written into plugin memory as `(ptr, len)` pairs, plus the trailing
    /// out-pointer (returned with its element count) when the result is an
    /// array — its length is the input array bound to the result's index
    /// variable.
    fn marshal_params(
        &self,
        live: &mut LiveInstance,
        function: &FnName,
        signature: &FunctionSignature,
        args: &[HostFnValue],
        buffers: &mut Option<BufferProtocol>,
    ) -> Result<(Vec<wasmi::Val>, Option<OutBuffer>), PluginCallError> {
        let protocol_missing = || PluginCallError::Internal {
            message: format!(
                "function `{function}` moves buffers without the buffer protocol resolved"
            ),
        };

        let mut params: Vec<wasmi::Val> = Vec::with_capacity(args.len() + 2);
        for (param, arg) in signature.params().iter().zip(args) {
            match (&param.kind, arg) {
                (
                    ValueKind::Scalar(_) | ValueKind::Bool | ValueKind::Int,
                    HostFnValue::Scalar(value),
                ) => params.push(wasmi::Val::F64((*value).into())),
                (ValueKind::Indexed { .. }, HostFnValue::Buffer(values)) => {
                    let buffers = buffers.as_mut().ok_or_else(protocol_missing)?;
                    let ptr = buffers.write_buffer(live, self.limits.fuel_per_call, values)?;
                    params.push(wasmi::Val::I32(ptr));
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_possible_wrap,
                        reason = "write_buffer bounds the length to the 32-bit plugin address space"
                    )]
                    params.push(wasmi::Val::I32(values.len() as i32));
                }
                (
                    ValueKind::Scalar(_) | ValueKind::Bool | ValueKind::Int | ValueKind::Struct(_),
                    _,
                )
                | (ValueKind::Indexed { .. }, HostFnValue::Scalar(_)) => {
                    return Err(PluginCallError::Internal {
                        message: format!(
                            "function `{function}` parameter `{}` received a value of the wrong shape",
                            param.name
                        ),
                    });
                }
            }
        }

        let out_buffer = match signature.result() {
            ValueKind::Indexed { index, .. } => {
                let len = signature
                    .params()
                    .iter()
                    .zip(args)
                    .find_map(|(param, arg)| match (&param.kind, arg) {
                        (
                            ValueKind::Indexed {
                                index: param_index, ..
                            },
                            HostFnValue::Buffer(values),
                        ) if param_index == index => Some(values.len()),
                        _ => None,
                    })
                    .ok_or_else(|| PluginCallError::Internal {
                        message: format!(
                            "function `{function}` result index `{index}` is not bound by any argument"
                        ),
                    })?;
                let buffers = buffers.as_mut().ok_or_else(protocol_missing)?;
                let ptr = buffers.alloc(live, self.limits.fuel_per_call, len)?;
                params.push(wasmi::Val::I32(ptr));
                Some(OutBuffer { ptr, len })
            }
            // A struct result is a fixed-size out-buffer: one f64 slot per
            // flattened field, in declaration order.
            ValueKind::Struct(shape) => {
                let len = shape.fields().len();
                let buffers = buffers.as_mut().ok_or_else(protocol_missing)?;
                let ptr = buffers.alloc(live, self.limits.fuel_per_call, len)?;
                params.push(wasmi::Val::I32(ptr));
                Some(OutBuffer { ptr, len })
            }
            ValueKind::Scalar(_) | ValueKind::Bool | ValueKind::Int => None,
        };
        Ok((params, out_buffer))
    }
}

/// The host-allocated out-pointer an array-returning call hands the plugin.
struct OutBuffer {
    ptr: i32,
    len: usize,
}

/// The per-call handles of the array buffer protocol: the plugin's memory
/// and allocator exports, plus the allocations to release after the call.
struct BufferProtocol {
    memory: wasmi::Memory,
    alloc: wasmi::Func,
    free: wasmi::Func,
    /// `(ptr, size_bytes)` of every host-requested allocation, freed after
    /// the call completes.
    allocations: Vec<(i32, i32)>,
}

impl BufferProtocol {
    /// Resolve the memory/allocator exports (validated present at load).
    fn resolve(live: &LiveInstance, function: &FnName) -> Result<Self, PluginCallError> {
        let missing = |export: &str| PluginCallError::Internal {
            message: format!(
                "function `{function}` needs buffer export `{export}` despite load-time checks"
            ),
        };
        let memory = live
            .instance
            .get_export(&live.store, "memory")
            .and_then(wasmi::Extern::into_memory)
            .ok_or_else(|| missing("memory"))?;
        let alloc = live
            .instance
            .get_export(&live.store, ALLOC_EXPORT)
            .and_then(wasmi::Extern::into_func)
            .ok_or_else(|| missing(ALLOC_EXPORT))?;
        let free = live
            .instance
            .get_export(&live.store, FREE_EXPORT)
            .and_then(wasmi::Extern::into_func)
            .ok_or_else(|| missing(FREE_EXPORT))?;
        Ok(Self {
            memory,
            alloc,
            free,
            allocations: Vec::new(),
        })
    }

    /// Allocate space for `len` `f64` elements inside the plugin's memory.
    fn alloc(
        &mut self,
        live: &mut LiveInstance,
        fuel: u64,
        len: usize,
    ) -> Result<i32, PluginCallError> {
        let size = i32::try_from(len)
            .ok()
            .and_then(|len| len.checked_mul(8))
            .ok_or(PluginCallError::BufferTooLarge { elements: len })?;
        let mut results = [wasmi::Val::I32(0)];
        self.alloc
            .call(&mut live.store, &[wasmi::Val::I32(size)], &mut results)
            .map_err(|err| error_from_wasm(&mut live.store, &err, fuel))?;
        let ptr = match results[0] {
            wasmi::Val::I32(ptr) => ptr,
            ref other => {
                return Err(PluginCallError::Internal {
                    message: format!("allocator returned {other:?} despite load-time type checks"),
                });
            }
        };
        self.allocations.push((ptr, size));
        Ok(ptr)
    }

    /// Allocate and fill one input buffer; returns its plugin-memory pointer.
    fn write_buffer(
        &mut self,
        live: &mut LiveInstance,
        fuel: u64,
        values: &[f64],
    ) -> Result<i32, PluginCallError> {
        let ptr = self.alloc(live, fuel, values.len())?;
        let mut bytes = Vec::with_capacity(values.len() * 8);
        for value in values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        #[expect(
            clippy::cast_sign_loss,
            reason = "a negative allocator pointer is out of bounds and rejected by the write below"
        )]
        self.memory
            .write(&mut live.store, ptr as usize, &bytes)
            .map_err(|_| PluginCallError::Trap {
                message: format!(
                    "plugin allocator returned an out-of-bounds buffer (ptr {ptr}, {} bytes)",
                    bytes.len()
                ),
            })?;
        Ok(ptr)
    }

    /// Read the out-buffer the plugin filled.
    fn read_buffer(
        &self,
        live: &LiveInstance,
        ptr: i32,
        len: usize,
    ) -> Result<Vec<f64>, PluginCallError> {
        let mut bytes = vec![0_u8; len * 8];
        #[expect(
            clippy::cast_sign_loss,
            reason = "a negative allocator pointer is out of bounds and rejected by the read below"
        )]
        self.memory
            .read(&live.store, ptr as usize, &mut bytes)
            .map_err(|_| PluginCallError::Trap {
                message: format!(
                    "plugin result buffer is out of bounds (ptr {ptr}, {} bytes)",
                    bytes.len()
                ),
            })?;
        Ok(bytes
            .chunks_exact(8)
            .map(|chunk| {
                let mut raw = [0_u8; 8];
                raw.copy_from_slice(chunk);
                f64::from_le_bytes(raw)
            })
            .collect())
    }

    /// Release every allocation made for this call.
    fn free_all(self, live: &mut LiveInstance, fuel: u64) -> Result<(), PluginCallError> {
        for (ptr, size) in self.allocations {
            self.free
                .call(
                    &mut live.store,
                    &[wasmi::Val::I32(ptr), wasmi::Val::I32(size)],
                    &mut [],
                )
                .map_err(|err| error_from_wasm(&mut live.store, &err, fuel))?;
        }
        Ok(())
    }
}

/// Whether any parameter or the result of `signature` crosses as a buffer.
fn signature_uses_buffers(signature: &FunctionSignature) -> bool {
    signature
        .params()
        .iter()
        .map(|param| &param.kind)
        .chain(std::iter::once(signature.result()))
        .any(|kind| matches!(kind, ValueKind::Indexed { .. } | ValueKind::Struct(_)))
}

/// The wasm function type the ABI requires for one signature.
struct ExpectedWasmType {
    params: Vec<wasmi::ValType>,
    results: Vec<wasmi::ValType>,
}

impl ExpectedWasmType {
    fn describe(&self) -> String {
        let list = |types: &[wasmi::ValType]| {
            types
                .iter()
                .map(|ty| format!("{ty:?}").to_lowercase())
                .collect::<Vec<_>>()
                .join(", ")
        };
        format!("({}) -> ({})", list(&self.params), list(&self.results))
    }
}

fn expected_wasm_type(signature: &FunctionSignature) -> ExpectedWasmType {
    let mut params = Vec::new();
    for param in signature.params() {
        match &param.kind {
            ValueKind::Scalar(_) | ValueKind::Bool | ValueKind::Int => {
                params.push(wasmi::ValType::F64);
            }
            // Struct parameters never pass signature validation; folding
            // them into the buffer arm keeps this total without a panic path.
            ValueKind::Indexed { .. } | ValueKind::Struct(_) => {
                params.push(wasmi::ValType::I32);
                params.push(wasmi::ValType::I32);
            }
        }
    }
    let results = match signature.result() {
        ValueKind::Scalar(_) | ValueKind::Bool | ValueKind::Int => vec![wasmi::ValType::F64],
        ValueKind::Indexed { .. } | ValueKind::Struct(_) => {
            params.push(wasmi::ValType::I32);
            Vec::new()
        }
    };
    ExpectedWasmType { params, results }
}

/// Require a buffer-protocol function export with the exact wasm type.
fn check_buffer_protocol_func(
    module: &wasmi::Module,
    export: &str,
    params: &[wasmi::ValType],
    results: &[wasmi::ValType],
) -> Result<(), PluginLoadError> {
    let expected = ExpectedWasmType {
        params: params.to_vec(),
        results: results.to_vec(),
    };
    match module.get_export(export) {
        Some(wasmi::ExternType::Func(ty))
            if ty.params() == expected.params.as_slice()
                && ty.results() == expected.results.as_slice() =>
        {
            Ok(())
        }
        Some(other) => Err(PluginLoadError::BufferProtocolExportTypeMismatch {
            export: export.to_string(),
            expected: expected.describe(),
            found: describe_extern_type(&other),
        }),
        None => Err(PluginLoadError::MissingBufferProtocolExport {
            export: export.to_string(),
            expected: expected.describe(),
        }),
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
    /// The manifest declares arrays but a buffer-protocol export is missing.
    #[error(
        "plugin declares array parameters or results but does not export `{export}` \
         ({expected})"
    )]
    MissingBufferProtocolExport {
        /// The missing export.
        export: String,
        /// What the ABI requires the export to be.
        expected: String,
    },
    /// A buffer-protocol export exists with the wrong type.
    #[error("plugin exports `{export}` as {found}, but the buffer protocol requires {expected}")]
    BufferProtocolExportTypeMismatch {
        /// The mistyped export.
        export: String,
        /// The wasm type the ABI requires.
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
    /// An array argument exceeds the plugin's 32-bit address space.
    #[error("an array of {elements} element(s) cannot fit in plugin memory")]
    BufferTooLarge {
        /// The element count of the oversized array.
        elements: usize,
    },
    /// An internal invariant of the host itself failed.
    #[error("plugin host internal error: {message}")]
    Internal {
        /// Description of the violated invariant.
        message: String,
    },
}
