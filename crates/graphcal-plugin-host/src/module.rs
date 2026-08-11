//! A validated, callable WASM plugin module.
//!
//! Loading a [`PluginModule`] performs every load-time check the ABI demands
//! — manifest extraction and typed conversion, the import ban (purity by
//! construction), the memory-export rule, and per-function wasm type
//! verification — before any plugin code runs. [`PluginModule::call`] then
//! executes one function under the configured fuel and memory bounds, with
//! failure messages, traps, and fuel exhaustion mapped to
//! [`PluginCallError`].

use std::{
    num::NonZeroU32,
    sync::{Mutex, PoisonError},
};

use graphcal_compiler::function_signature::{FunctionSignature, ValueKind};
use graphcal_compiler::syntax::function_name::FnName;
use graphcal_compiler::syntax::index_name::IndexVarName;
use graphcal_eval::host_fns::{HostArray, HostFnValue};
use graphcal_plugin_abi::{
    ALLOC_EXPORT, BUFFER_ALIGN, FAIL_IMPORT_MODULE, FAIL_IMPORT_NAME, FREE_EXPORT,
    MAX_FAIL_MESSAGE_BYTES, ManifestFromWasmError, PluginManifest,
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

    /// Call one plugin function with SI-normalized values: single-value ABI
    /// slots cross as raw `f64`s, arrays as row-major buffers plus one extent
    /// per axis that the host places in (and reads back from) plugin memory.
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
            .memory_size(self.limits.max_memory_bytes())
            .table_elements(self.limits.max_table_elements())
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
        set_fuel(&mut store, self.limits.fuel_per_call())?;

        let mut linker = wasmi::Linker::new(&self.engine);
        linker
            .func_wrap(FAIL_IMPORT_MODULE, FAIL_IMPORT_NAME, host_fail)
            .map_err(|err| PluginCallError::Internal {
                message: format!("failed to install the graphcal::fail import: {err}"),
            })?;

        let instance = linker
            .instantiate_and_start(&mut store, &self.module)
            .map_err(|err| error_from_wasm(&mut store, &err, self.limits.fuel_per_call()))?;
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
        set_fuel(&mut live.store, self.limits.fuel_per_call())?;
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
            .map_err(|err| error_from_wasm(&mut live.store, &err, self.limits.fuel_per_call()))?;

        let value = match (out_buffer, buffers.as_ref()) {
            (Some(out), Some(buffers)) => {
                let values = buffers.read_buffer(live, out.allocation)?;
                match out.kind {
                    OutBufferKind::Array { shape } => {
                        HostFnValue::Array(HostArray::try_new(shape, values).map_err(|error| {
                            PluginCallError::Internal {
                                message: format!(
                                    "function `{function}` produced an invalid array: {error}"
                                ),
                            }
                        })?)
                    }
                    OutBufferKind::Record => HostFnValue::Record(values),
                }
            }
            (Some(_), None) => {
                return Err(PluginCallError::Internal {
                    message: format!(
                        "function `{function}` produced an out-buffer without the buffer protocol"
                    ),
                });
            }
            (None, _) => match results.first() {
                Some(wasmi::Val::F64(value)) => HostFnValue::F64(f64::from(*value)),
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
            buffers.free_all(live, self.limits.fuel_per_call())?;
        }
        Ok(value)
    }

    /// Build the wasm parameter list for one call: single-value ABI slots as
    /// `f64`s, arrays as a pointer plus one extent per declared axis, and a
    /// trailing out-pointer for array and record results.
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

        let mut params: Vec<wasmi::Val> = Vec::new();
        let mut bound_extents: std::collections::HashMap<IndexVarName, usize> =
            std::collections::HashMap::new();
        for (param, arg) in signature.params().iter().zip(args) {
            match (&param.kind, arg) {
                (
                    ValueKind::Quantity(_) | ValueKind::Bool | ValueKind::Int,
                    HostFnValue::F64(value),
                ) => params.push(wasmi::Val::F64((*value).into())),
                (ValueKind::Indexed { indexes, .. }, HostFnValue::Array(array)) => {
                    if array.shape().len() != indexes.len() {
                        return Err(PluginCallError::Internal {
                            message: format!(
                                "function `{function}` parameter `{}` received rank {}, expected {}",
                                param.name,
                                array.shape().len(),
                                indexes.len()
                            ),
                        });
                    }
                    for (index, extent) in indexes.iter().zip(array.shape()) {
                        match bound_extents.entry(index.clone()) {
                            std::collections::hash_map::Entry::Vacant(slot) => {
                                slot.insert(*extent);
                            }
                            std::collections::hash_map::Entry::Occupied(bound)
                                if *bound.get() != *extent =>
                            {
                                return Err(PluginCallError::Internal {
                                    message: format!(
                                        "function `{function}` received extent {extent} for index variable `{index}`, previously bound to {}",
                                        bound.get()
                                    ),
                                });
                            }
                            std::collections::hash_map::Entry::Occupied(_) => {}
                        }
                    }
                    let buffers = buffers.as_mut().ok_or_else(protocol_missing)?;
                    let pointer =
                        buffers.write_buffer(live, self.limits.fuel_per_call(), array.values())?;
                    params.push(wasmi::Val::I32(pointer.as_abi_i32()));
                    for extent in array.shape() {
                        let extent = u32::try_from(*extent)
                            .map_err(|_| PluginCallError::BufferTooLarge { elements: *extent })?;
                        params.push(wasmi::Val::I32(i32::from_ne_bytes(extent.to_ne_bytes())));
                    }
                }
                _ => {
                    return Err(PluginCallError::Internal {
                        message: format!(
                            "function `{function}` parameter `{}` received a value of the wrong shape",
                            param.name
                        ),
                    });
                }
            }
        }

        let out_buffer = self.allocate_result_buffer(
            live,
            function,
            signature.result(),
            &bound_extents,
            buffers,
        )?;
        if let Some(out) = &out_buffer {
            params.push(wasmi::Val::I32(out.allocation.pointer().as_abi_i32()));
        }
        Ok((params, out_buffer))
    }

    fn allocate_result_buffer(
        &self,
        live: &mut LiveInstance,
        function: &FnName,
        result: &ValueKind,
        bound_extents: &std::collections::HashMap<IndexVarName, usize>,
        buffers: &mut Option<BufferProtocol>,
    ) -> Result<Option<OutBuffer>, PluginCallError> {
        let protocol_missing = || PluginCallError::Internal {
            message: format!(
                "function `{function}` moves buffers without the buffer protocol resolved"
            ),
        };
        let (len, kind) = match result {
            ValueKind::Indexed { indexes, .. } => {
                let shape = indexes
                    .iter()
                    .map(|index| {
                        bound_extents.get(index).copied().ok_or_else(|| {
                            PluginCallError::Internal {
                                message: format!(
                                    "function `{function}` result index `{index}` is not bound by any argument"
                                ),
                            }
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let len = shape
                    .iter()
                    .try_fold(1_usize, |size, extent| size.checked_mul(*extent))
                    .ok_or(PluginCallError::BufferTooLarge {
                        elements: usize::MAX,
                    })?;
                (len, OutBufferKind::Array { shape })
            }
            // A struct result is a fixed-size out-buffer: one f64 slot per
            // flattened field, in declaration order.
            ValueKind::Struct(shape) => (shape.fields().len(), OutBufferKind::Record),
            ValueKind::Quantity(_) | ValueKind::Bool | ValueKind::Int => return Ok(None),
        };
        let buffers = buffers.as_mut().ok_or_else(protocol_missing)?;
        let allocation = buffers.alloc(live, self.limits.fuel_per_call(), len)?;
        Ok(Some(OutBuffer { allocation, kind }))
    }
}

/// The typed purpose of a host-allocated result buffer.
enum OutBufferKind {
    Array { shape: Vec<usize> },
    Record,
}

/// A non-null, ABI-aligned offset into a plugin's linear memory.
///
/// Construction also proves that the complete allocation range is currently
/// in bounds. WebAssembly memories cannot shrink, so the offset remains valid
/// for the allocation's lifetime.
#[derive(Clone, Copy)]
struct WasmBufferPointer {
    abi: NonZeroU32,
    offset: usize,
}

impl WasmBufferPointer {
    fn from_allocator(
        raw: i32,
        byte_len: usize,
        memory_bytes: usize,
    ) -> Result<Self, PluginCallError> {
        let address = u32::from_ne_bytes(raw.to_ne_bytes());
        let abi = NonZeroU32::new(address)
            .ok_or(PluginCallError::AllocationFailed { bytes: byte_len })?;
        let alignment = u32::try_from(BUFFER_ALIGN)
            .ok()
            .and_then(NonZeroU32::new)
            .ok_or_else(|| PluginCallError::Internal {
                message: format!("invalid ABI buffer alignment {BUFFER_ALIGN}"),
            })?;
        if !address.is_multiple_of(alignment.get()) {
            return Err(PluginCallError::MisalignedAllocatorPointer {
                pointer: address,
                required_alignment: alignment.get(),
            });
        }
        let offset =
            usize::try_from(address).map_err(|_| PluginCallError::AllocatorBufferOutOfBounds {
                pointer: address,
                bytes: byte_len,
                memory_bytes,
            })?;
        if offset
            .checked_add(byte_len)
            .is_none_or(|end| end > memory_bytes)
        {
            return Err(PluginCallError::AllocatorBufferOutOfBounds {
                pointer: address,
                bytes: byte_len,
                memory_bytes,
            });
        }
        Ok(Self { abi, offset })
    }

    const fn as_abi_i32(self) -> i32 {
        i32::from_ne_bytes(self.abi.get().to_ne_bytes())
    }

    const fn offset(self) -> usize {
        self.offset
    }
}

/// One allocation returned by the plugin and validated against the ABI.
#[derive(Clone, Copy)]
struct PluginAllocation {
    pointer: WasmBufferPointer,
    byte_len: usize,
    abi_byte_len: u32,
}

impl PluginAllocation {
    const fn pointer(self) -> WasmBufferPointer {
        self.pointer
    }

    const fn byte_len(self) -> usize {
        self.byte_len
    }

    const fn abi_byte_len(self) -> i32 {
        i32::from_ne_bytes(self.abi_byte_len.to_ne_bytes())
    }
}

/// A host-allocated result buffer handed to the plugin.
struct OutBuffer {
    allocation: PluginAllocation,
    kind: OutBufferKind,
}

/// The per-call handles of the array buffer protocol: the plugin's memory
/// and allocator exports, plus the allocations to release after the call.
struct BufferProtocol {
    memory: wasmi::Memory,
    alloc: wasmi::Func,
    free: wasmi::Func,
    allocations: Vec<PluginAllocation>,
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
    ) -> Result<PluginAllocation, PluginCallError> {
        let byte_len = len
            .checked_mul(size_of::<f64>())
            .ok_or(PluginCallError::BufferTooLarge { elements: len })?;
        let abi_byte_len = u32::try_from(byte_len)
            .map_err(|_| PluginCallError::BufferTooLarge { elements: len })?;
        let mut results = [wasmi::Val::I32(0)];
        self.alloc
            .call(
                &mut live.store,
                &[wasmi::Val::I32(i32::from_ne_bytes(
                    abi_byte_len.to_ne_bytes(),
                ))],
                &mut results,
            )
            .map_err(|err| error_from_wasm(&mut live.store, &err, fuel))?;
        let raw_pointer = match results[0] {
            wasmi::Val::I32(pointer) => pointer,
            ref other => {
                return Err(PluginCallError::Internal {
                    message: format!("allocator returned {other:?} despite load-time type checks"),
                });
            }
        };
        let pointer = WasmBufferPointer::from_allocator(
            raw_pointer,
            byte_len,
            self.memory.data_size(&live.store),
        )?;
        let allocation = PluginAllocation {
            pointer,
            byte_len,
            abi_byte_len,
        };
        self.allocations.push(allocation);
        Ok(allocation)
    }

    /// Allocate and fill one input buffer; returns its plugin-memory pointer.
    fn write_buffer(
        &mut self,
        live: &mut LiveInstance,
        fuel: u64,
        values: &[f64],
    ) -> Result<WasmBufferPointer, PluginCallError> {
        let allocation = self.alloc(live, fuel, values.len())?;
        let mut bytes = Vec::with_capacity(allocation.byte_len());
        bytes.extend(values.iter().flat_map(|value| value.to_le_bytes()));
        self.memory
            .write(&mut live.store, allocation.pointer().offset(), &bytes)
            .map_err(|error| PluginCallError::Internal {
                message: format!(
                    "validated plugin input buffer became inaccessible before use: {error}"
                ),
            })?;
        Ok(allocation.pointer())
    }

    /// Read the out-buffer the plugin filled.
    fn read_buffer(
        &self,
        live: &LiveInstance,
        allocation: PluginAllocation,
    ) -> Result<Vec<f64>, PluginCallError> {
        let mut bytes = vec![0_u8; allocation.byte_len()];
        self.memory
            .read(&live.store, allocation.pointer().offset(), &mut bytes)
            .map_err(|error| PluginCallError::Internal {
                message: format!(
                    "validated plugin result buffer became inaccessible before use: {error}"
                ),
            })?;
        Ok(bytes
            .chunks_exact(size_of::<f64>())
            .map(|chunk| {
                let mut raw = [0_u8; size_of::<f64>()];
                raw.copy_from_slice(chunk);
                f64::from_le_bytes(raw)
            })
            .collect())
    }

    /// Release every allocation made for this call.
    fn free_all(self, live: &mut LiveInstance, fuel: u64) -> Result<(), PluginCallError> {
        for allocation in self.allocations {
            self.free
                .call(
                    &mut live.store,
                    &[
                        wasmi::Val::I32(allocation.pointer().as_abi_i32()),
                        wasmi::Val::I32(allocation.abi_byte_len()),
                    ],
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
            ValueKind::Quantity(_) | ValueKind::Bool | ValueKind::Int => {
                params.push(wasmi::ValType::F64);
            }
            ValueKind::Indexed { indexes, .. } => {
                params.push(wasmi::ValType::I32);
                params.extend(std::iter::repeat_n(wasmi::ValType::I32, indexes.len()));
            }
            // Struct parameters never pass signature validation. Keep this
            // match total for defense in depth.
            ValueKind::Struct(_) => params.push(wasmi::ValType::I32),
        }
    }
    let results = match signature.result() {
        ValueKind::Quantity(_) | ValueKind::Bool | ValueKind::Int => vec![wasmi::ValType::F64],
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
    /// The plugin allocator reported that it could not satisfy a request.
    #[error("plugin allocator could not allocate {bytes} byte(s)")]
    AllocationFailed {
        /// Number of bytes requested by the host.
        bytes: usize,
    },
    /// The plugin allocator returned a pointer that violates the ABI's
    /// alignment requirement.
    #[error(
        "plugin allocator returned misaligned pointer {pointer}; required alignment is \
         {required_alignment} bytes"
    )]
    MisalignedAllocatorPointer {
        /// Unsigned WebAssembly address returned by the allocator.
        pointer: u32,
        /// Required ABI alignment in bytes.
        required_alignment: u32,
    },
    /// The plugin allocator returned a range outside its exported memory.
    #[error(
        "plugin allocator returned out-of-bounds buffer (pointer {pointer}, {bytes} byte(s), \
         memory size {memory_bytes} bytes)"
    )]
    AllocatorBufferOutOfBounds {
        /// Unsigned WebAssembly address returned by the allocator.
        pointer: u32,
        /// Number of bytes requested by the host.
        bytes: usize,
        /// Current exported memory size in bytes.
        memory_bytes: usize,
    },
    /// An internal invariant of the host itself failed.
    #[error("plugin host internal error: {message}")]
    Internal {
        /// Description of the violated invariant.
        message: String,
    },
}
