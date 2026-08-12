//! Emitting the generated items: the manifest static, the
//! one-`plugin!`-per-module guard symbol, the `extern "C"` wrappers, and —
//! when arrays are involved — the allocator exports of the buffer protocol.
//!
//! Everything the wasm module needs is plain Rust with `wasm32`-gated
//! attributes, so the same expansion compiles natively — plugin crates
//! unit-test their kernels with ordinary `cargo test`, and the workspace
//! integration tests read `GRAPHCAL_PLUGIN_MANIFEST` without a wasm
//! toolchain.
//!
//! Single-value ABI functions (quantities, `Bool`, and `Int`) are emitted as a
//! single `extern "C-unwind"` item whose raw `f64` parameters double as the
//! natural test surface. Functions that move arrays split in two: a natural
//! `pub fn` taking [`graphcal_plugin::ArrayView`] values (what `cargo test`
//! calls) and a `wasm32`-only export wrapper that decodes the pointer plus one
//! extent per axis, calls the natural function, and writes an owned
//! [`graphcal_plugin::Array`] through the host-allocated out-pointer.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;

use crate::lower::{FieldIr, FieldKindIr, FunctionIr, ParamKindIr, PluginIr, ResultKindIr};

const MANIFEST_STATIC: &str = "GRAPHCAL_PLUGIN_MANIFEST";
const MANIFEST_UNIQUENESS_GUARD: &str = "GRAPHCAL_PLUGIN_MANIFEST_SECTION_IS_UNIQUE";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum RustNamespace {
    Type,
    Value,
}

#[derive(Debug, Clone, Copy)]
enum SymbolDomain {
    RustItem,
    WasmExport,
}

impl std::fmt::Display for SymbolDomain {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::RustItem => "Rust item",
            Self::WasmExport => "WebAssembly export",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RustSymbolKey {
    namespace: RustNamespace,
    spelling: String,
}

#[derive(Debug, Clone)]
enum SymbolRole {
    ManifestStatic,
    ManifestUniquenessGuard,
    AuthoredFunction { name: String },
    OutputStruct { function: String },
    InternalWrapper { function: String },
    AllocatorItem,
    DeallocatorItem,
    AllocatorExport,
    DeallocatorExport,
    LinearMemoryExport,
}

impl SymbolRole {
    fn description(&self) -> String {
        match self {
            Self::ManifestStatic => "the generated manifest static".to_string(),
            Self::ManifestUniquenessGuard => {
                "the generated one-plugin-per-module guard".to_string()
            }
            Self::AuthoredFunction { name } => format!("authored function `{name}`"),
            Self::OutputStruct { function } => {
                format!("the generated output type for function `{function}`")
            }
            Self::InternalWrapper { function } => {
                format!("the internal ABI wrapper for function `{function}`")
            }
            Self::AllocatorItem => "the internal allocator item".to_string(),
            Self::DeallocatorItem => "the internal deallocator item".to_string(),
            Self::AllocatorExport => "the generated allocator export".to_string(),
            Self::DeallocatorExport => "the generated deallocator export".to_string(),
            Self::LinearMemoryExport => "the buffer protocol's linear-memory export".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
struct SymbolOrigin {
    role: SymbolRole,
    span: Span,
}

#[derive(Default)]
struct GeneratedSymbolTable {
    rust_items: HashMap<RustSymbolKey, SymbolOrigin>,
    wasm_exports: HashMap<String, SymbolOrigin>,
}

impl GeneratedSymbolTable {
    fn register_rust_item(
        &mut self,
        namespace: RustNamespace,
        spelling: impl Into<String>,
        origin: SymbolOrigin,
    ) -> syn::Result<()> {
        let key = RustSymbolKey {
            namespace,
            spelling: spelling.into(),
        };
        let Some(existing) = self.rust_items.get(&key) else {
            self.rust_items.insert(key, origin);
            return Ok(());
        };
        Err(symbol_collision(
            SymbolDomain::RustItem,
            &key.spelling,
            existing,
            &origin,
        ))
    }

    fn allocate_internal_value(
        &mut self,
        preferred: &str,
        origin: SymbolOrigin,
    ) -> syn::Result<Ident> {
        let mut suffix = 0_usize;
        loop {
            let spelling = if suffix == 0 {
                preferred.to_string()
            } else {
                format!("{preferred}_{suffix}")
            };
            let key = RustSymbolKey {
                namespace: RustNamespace::Value,
                spelling: spelling.clone(),
            };
            match self.rust_items.entry(key) {
                Entry::Vacant(entry) => {
                    entry.insert(origin);
                    return Ok(internal_ident(&spelling));
                }
                Entry::Occupied(_) => {}
            }
            suffix = suffix.checked_add(1).ok_or_else(|| {
                syn::Error::new(
                    origin.span,
                    "plugin code generation exhausted internal Rust item names",
                )
            })?;
        }
    }

    fn register_wasm_export(
        &mut self,
        spelling: impl Into<String>,
        origin: SymbolOrigin,
    ) -> syn::Result<()> {
        let spelling = spelling.into();
        let Some(existing) = self.wasm_exports.get(&spelling) else {
            self.wasm_exports.insert(spelling, origin);
            return Ok(());
        };
        Err(symbol_collision(
            SymbolDomain::WasmExport,
            &spelling,
            existing,
            &origin,
        ))
    }
}

fn symbol_collision(
    domain: SymbolDomain,
    spelling: &str,
    existing: &SymbolOrigin,
    incoming: &SymbolOrigin,
) -> syn::Error {
    let existing_description = existing.role.description();
    let incoming_description = incoming.role.description();
    let mut error = syn::Error::new(
        incoming.span,
        format!(
            "{incoming_description} uses {domain} name `{spelling}`, which collides with \
             {existing_description}"
        ),
    );
    error.combine(syn::Error::new(
        existing.span,
        format!("{existing_description} first uses `{spelling}` here"),
    ));
    error
}

struct GeneratedSymbols {
    allocator: Option<AllocatorSymbols>,
    functions: Vec<FunctionSymbols>,
}

struct AllocatorSymbols {
    alloc: Ident,
    free: Ident,
    size: Ident,
    ptr: Ident,
}

struct FunctionSymbols {
    wrapper: Option<Ident>,
    output_struct: Option<Ident>,
    params: Vec<ParamSymbols>,
    result: Ident,
    out_ptr: Ident,
    expected_shape: Ident,
    slots: Ident,
}

enum ParamSymbols {
    Scalar,
    Array {
        ptr: Ident,
        len: Ident,
        shape: Ident,
        extents: Vec<Ident>,
    },
}

impl GeneratedSymbols {
    fn build(ir: &PluginIr) -> syn::Result<Self> {
        let generated_span = Span::call_site();
        let mut table = base_symbol_table(ir, generated_span)?;
        let output_structs = register_output_structs(ir, &mut table)?;
        let allocator = if ir.uses_buffers() {
            Some(allocate_allocator_symbols(&mut table, generated_span)?)
        } else {
            None
        };
        let functions = allocate_function_symbols(ir, output_structs, &mut table)?;
        Ok(Self {
            allocator,
            functions,
        })
    }
}

fn base_symbol_table(ir: &PluginIr, generated_span: Span) -> syn::Result<GeneratedSymbolTable> {
    let mut table = GeneratedSymbolTable::default();
    for (spelling, role) in [
        (MANIFEST_STATIC, SymbolRole::ManifestStatic),
        (
            MANIFEST_UNIQUENESS_GUARD,
            SymbolRole::ManifestUniquenessGuard,
        ),
    ] {
        table.register_rust_item(
            RustNamespace::Value,
            spelling,
            SymbolOrigin {
                role,
                span: generated_span,
            },
        )?;
    }
    if ir.uses_buffers() {
        for (export, role) in [
            (
                graphcal_plugin_abi::ALLOC_EXPORT,
                SymbolRole::AllocatorExport,
            ),
            (
                graphcal_plugin_abi::FREE_EXPORT,
                SymbolRole::DeallocatorExport,
            ),
            (
                graphcal_plugin_abi::MEMORY_EXPORT,
                SymbolRole::LinearMemoryExport,
            ),
        ] {
            table.register_wasm_export(
                export,
                SymbolOrigin {
                    role,
                    span: generated_span,
                },
            )?;
        }
    }
    for function in &ir.functions {
        let spelling = function.name.to_string();
        let origin = SymbolOrigin {
            role: SymbolRole::AuthoredFunction {
                name: spelling.clone(),
            },
            span: function.name.span(),
        };
        table.register_rust_item(RustNamespace::Value, spelling.clone(), origin.clone())?;
        table.register_wasm_export(spelling, origin)?;
    }
    Ok(table)
}

fn register_output_structs(
    ir: &PluginIr,
    table: &mut GeneratedSymbolTable,
) -> syn::Result<Vec<Option<Ident>>> {
    ir.functions
        .iter()
        .map(|function| match &function.result {
            ResultKindIr::Struct(_) => {
                let ident = output_struct_ident(&function.name)?;
                let origin = SymbolOrigin {
                    role: SymbolRole::OutputStruct {
                        function: function.name.to_string(),
                    },
                    span: function.name.span(),
                };
                for namespace in [RustNamespace::Type, RustNamespace::Value] {
                    table.register_rust_item(namespace, ident.to_string(), origin.clone())?;
                }
                Ok(Some(ident))
            }
            ResultKindIr::Bool
            | ResultKindIr::Int
            | ResultKindIr::Quantity(_)
            | ResultKindIr::Array { .. } => Ok(None),
        })
        .collect()
}

fn allocate_allocator_symbols(
    table: &mut GeneratedSymbolTable,
    generated_span: Span,
) -> syn::Result<AllocatorSymbols> {
    Ok(AllocatorSymbols {
        alloc: table.allocate_internal_value(
            "__graphcal_allocator",
            SymbolOrigin {
                role: SymbolRole::AllocatorItem,
                span: generated_span,
            },
        )?,
        free: table.allocate_internal_value(
            "__graphcal_deallocator",
            SymbolOrigin {
                role: SymbolRole::DeallocatorItem,
                span: generated_span,
            },
        )?,
        size: internal_ident("__graphcal_allocation_size"),
        ptr: internal_ident("__graphcal_allocation_ptr"),
    })
}

fn allocate_function_symbols(
    ir: &PluginIr,
    output_structs: Vec<Option<Ident>>,
    table: &mut GeneratedSymbolTable,
) -> syn::Result<Vec<FunctionSymbols>> {
    ir.functions
        .iter()
        .zip(output_structs)
        .enumerate()
        .map(|(function_index, (function, output_struct))| {
            let wrapper = if function.uses_buffers() {
                Some(table.allocate_internal_value(
                    &format!("__graphcal_export_{function_index}"),
                    SymbolOrigin {
                        role: SymbolRole::InternalWrapper {
                            function: function.name.to_string(),
                        },
                        span: function.name.span(),
                    },
                )?)
            } else {
                None
            };
            Ok(FunctionSymbols::build(
                function_index,
                function,
                wrapper,
                output_struct,
            ))
        })
        .collect()
}

impl FunctionSymbols {
    fn build(
        function_index: usize,
        function: &FunctionIr,
        wrapper: Option<Ident>,
        output_struct: Option<Ident>,
    ) -> Self {
        let params = function
            .params
            .iter()
            .enumerate()
            .map(|(param_index, param)| match &param.kind {
                ParamKindIr::Array { indexes, .. } => ParamSymbols::Array {
                    ptr: internal_ident(&format!(
                        "__graphcal_function_{function_index}_param_{param_index}_ptr"
                    )),
                    len: internal_ident(&format!(
                        "__graphcal_function_{function_index}_param_{param_index}_len"
                    )),
                    shape: internal_ident(&format!(
                        "__graphcal_function_{function_index}_param_{param_index}_shape"
                    )),
                    extents: indexes
                        .iter()
                        .enumerate()
                        .map(|(axis, _)| {
                            internal_ident(&format!(
                                "__graphcal_function_{function_index}_param_{param_index}_extent_{axis}"
                            ))
                        })
                        .collect(),
                },
                ParamKindIr::Bool | ParamKindIr::Int | ParamKindIr::Quantity(_) => {
                    ParamSymbols::Scalar
                }
            })
            .collect();
        Self {
            wrapper,
            output_struct,
            params,
            result: internal_ident(&format!("__graphcal_function_{function_index}_result")),
            out_ptr: internal_ident(&format!("__graphcal_function_{function_index}_out")),
            expected_shape: internal_ident(&format!(
                "__graphcal_function_{function_index}_expected_shape"
            )),
            slots: internal_ident(&format!("__graphcal_function_{function_index}_slots")),
        }
    }
}

fn internal_ident(spelling: &str) -> Ident {
    Ident::new(spelling, Span::mixed_site())
}

/// Generate the full expansion from the validated IR and its manifest
/// payload.
pub fn generate(ir: &PluginIr, manifest_json: &str) -> syn::Result<TokenStream> {
    let symbols = GeneratedSymbols::build(ir)?;
    let bytes = manifest_json.as_bytes();
    let len = bytes.len();
    let payload = proc_macro2::Literal::byte_string(bytes);
    let section = graphcal_plugin_abi::MANIFEST_SECTION;
    let functions = ir
        .functions
        .iter()
        .zip(&symbols.functions)
        .map(|(function, symbols)| generate_function(function, symbols))
        .collect::<syn::Result<Vec<_>>>()?;
    let allocator = match (ir.uses_buffers(), &symbols.allocator) {
        (true, Some(symbols)) => Some(generate_allocator_exports(symbols)),
        (false, None) => None,
        (true, None) | (false, Some(_)) => {
            return Err(syn::Error::new(
                Span::call_site(),
                "internal plugin code-generation invariant failed: allocator symbols do not \
                 match the lowered plugin",
            ));
        }
    };

    Ok(quote! {
        /// The plugin manifest bytes (JSON) this module embeds in the
        /// `graphcal-manifest` custom section on wasm targets.
        #[used]
        #[cfg_attr(target_arch = "wasm32", unsafe(link_section = #section))]
        pub static GRAPHCAL_PLUGIN_MANIFEST: [u8; #len] = *#payload;

        // Two `plugin!` blocks linked into one wasm module would produce a
        // concatenated (i.e. corrupt) manifest section; the unmangled
        // symbol turns that into a duplicate-symbol link error instead.
        #[doc(hidden)]
        #[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]
        pub static GRAPHCAL_PLUGIN_MANIFEST_SECTION_IS_UNIQUE: u8 = 0;

        #allocator

        #(#functions)*
    })
}

/// The buffer-protocol allocator pair the host places array buffers with.
fn generate_allocator_exports(symbols: &AllocatorSymbols) -> TokenStream {
    let alloc = &symbols.alloc;
    let free = &symbols.free;
    let size = &symbols.size;
    let ptr = &symbols.ptr;
    let alloc_export = graphcal_plugin_abi::ALLOC_EXPORT;
    let free_export = graphcal_plugin_abi::FREE_EXPORT;
    quote! {
        #[cfg(target_arch = "wasm32")]
        #[unsafe(export_name = #alloc_export)]
        pub extern "C-unwind" fn #alloc(#size: u32) -> *mut u8 {
            ::graphcal_plugin::__rt::buffer_alloc(#size)
        }

        #[cfg(target_arch = "wasm32")]
        #[unsafe(export_name = #free_export)]
        pub extern "C-unwind" fn #free(#ptr: *mut u8, #size: u32) {
            // SAFETY: the host passes back exactly the pairs it allocated.
            unsafe { ::graphcal_plugin::__rt::buffer_free(#ptr, #size) }
        }
    }
}

fn generate_function(function: &FunctionIr, symbols: &FunctionSymbols) -> syn::Result<TokenStream> {
    if function.uses_buffers() {
        generate_buffer_function(function, symbols)
    } else {
        Ok(generate_f64_abi_function(function, symbols))
    }
}

fn generate_f64_abi_function(function: &FunctionIr, symbols: &FunctionSymbols) -> TokenStream {
    let docs = &function.docs;
    let name = &function.name;
    let raw_params = function.params.iter().map(|param| {
        let name = &param.name;
        quote! { #name: f64 }
    });
    let conversions = function.params.iter().filter_map(|param| {
        let name = &param.name;
        let name_str = param.name.to_string();
        match param.kind {
            ParamKindIr::Quantity(_) | ParamKindIr::Array { .. } => None,
            ParamKindIr::Bool => Some(quote! {
                let #name: bool = ::graphcal_plugin::__rt::bool_from_abi(#name, #name_str);
            }),
            ParamKindIr::Int => Some(quote! {
                let #name: i64 = ::graphcal_plugin::__rt::int_from_abi(#name, #name_str);
            }),
        }
    });
    let result = &symbols.result;
    let (result_ty, to_abi) = match function.result {
        ResultKindIr::Quantity(_) | ResultKindIr::Array { .. } | ResultKindIr::Struct(_) => {
            (quote! { f64 }, quote! { #result })
        }
        ResultKindIr::Bool => (
            quote! { bool },
            quote! { ::graphcal_plugin::__rt::bool_to_abi(#result) },
        ),
        ResultKindIr::Int => (
            quote! { i64 },
            quote! { ::graphcal_plugin::__rt::int_to_abi(#result) },
        ),
    };
    let body = &function.body;

    quote! {
        #(#docs)*
        #[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]
        // "C-unwind", not "C": on wasm the two lower identically (panics
        // abort there), while natively it lets `fail()`/panics unwind into
        // `cargo test` instead of aborting the test process.
        pub extern "C-unwind" fn #name(#(#raw_params),*) -> f64 {
            ::graphcal_plugin::__rt::install_failure_hook();
            #(#conversions)*
            let #result: #result_ty = { #body };
            #to_abi
        }
    }
}

/// The per-parameter parts of a buffer-protocol wrapper: its raw ABI
/// parameters, the decode statements, and the natural-call arguments.
struct WrapperPieces {
    raw_params: Vec<TokenStream>,
    decodes: Vec<TokenStream>,
    natural_args: Vec<TokenStream>,
}

fn wrapper_pieces(function: &FunctionIr, symbols: &FunctionSymbols) -> syn::Result<WrapperPieces> {
    if function.params.len() != symbols.params.len() {
        return Err(internal_invariant_error(
            function.name.span(),
            &function.name,
            "parameter symbol count does not match the lowered signature",
        ));
    }

    let mut raw_params: Vec<TokenStream> = Vec::new();
    let mut decodes: Vec<TokenStream> = Vec::new();
    let mut natural_args: Vec<TokenStream> = Vec::new();
    for (param, param_symbols) in function.params.iter().zip(&symbols.params) {
        let pname = &param.name;
        let pname_str = pname.to_string();
        match (&param.kind, param_symbols) {
            (ParamKindIr::Quantity(_), ParamSymbols::Scalar) => {
                raw_params.push(quote! { #pname: f64 });
                natural_args.push(quote! { #pname });
            }
            (ParamKindIr::Bool, ParamSymbols::Scalar) => {
                raw_params.push(quote! { #pname: f64 });
                decodes.push(quote! {
                    let #pname: bool = ::graphcal_plugin::__rt::bool_from_abi(#pname, #pname_str);
                });
                natural_args.push(quote! { #pname });
            }
            (ParamKindIr::Int, ParamSymbols::Scalar) => {
                raw_params.push(quote! { #pname: f64 });
                decodes.push(quote! {
                    let #pname: i64 = ::graphcal_plugin::__rt::int_from_abi(#pname, #pname_str);
                });
                natural_args.push(quote! { #pname });
            }
            (
                ParamKindIr::Array { indexes, .. },
                ParamSymbols::Array {
                    ptr,
                    len,
                    shape,
                    extents,
                },
            ) => {
                if indexes.len() != extents.len() {
                    return Err(internal_invariant_error(
                        pname.span(),
                        &function.name,
                        "array-axis symbol count does not match the lowered parameter",
                    ));
                }
                raw_params.push(quote! { #ptr: *const f64, #(#extents: u32),* });
                decodes.push(quote! {
                    let #shape = [#(#extents as usize),*];
                    let #len = ::graphcal_plugin::__rt::shape_len_u32(&#shape, #pname_str);
                    // SAFETY: the host wrote the shape product at `ptr` inside
                    // this instance's memory and keeps it alive for the call.
                    let #pname = unsafe {
                        ::graphcal_plugin::__rt::array_view_from_abi(
                            #ptr,
                            #len,
                            &#shape,
                            #pname_str,
                        )
                    };
                });
                natural_args.push(quote! { #pname });
            }
            (
                ParamKindIr::Bool | ParamKindIr::Int | ParamKindIr::Quantity(_),
                ParamSymbols::Array { .. },
            )
            | (ParamKindIr::Array { .. }, ParamSymbols::Scalar) => {
                return Err(internal_invariant_error(
                    pname.span(),
                    &function.name,
                    "parameter symbol kind does not match the lowered signature",
                ));
            }
        }
    }
    Ok(WrapperPieces {
        raw_params,
        decodes,
        natural_args,
    })
}

/// Emit an array-moving function: the natural `pub fn` (slices in, `Vec`
/// out) plus the `wasm32`-only export wrapper speaking the buffer protocol.
fn generate_buffer_function(
    function: &FunctionIr,
    symbols: &FunctionSymbols,
) -> syn::Result<TokenStream> {
    let docs = &function.docs;
    let name = &function.name;
    let body = &function.body;

    let natural_params = function.params.iter().map(|param| {
        let pname = &param.name;
        match &param.kind {
            ParamKindIr::Quantity(_) => quote! { #pname: f64 },
            ParamKindIr::Bool => quote! { #pname: bool },
            ParamKindIr::Int => quote! { #pname: i64 },
            ParamKindIr::Array { .. } => quote! { #pname: ::graphcal_plugin::ArrayView<'_> },
        }
    });
    let output_ident = match (&function.result, &symbols.output_struct) {
        (ResultKindIr::Struct(_), Some(ident)) => Some(ident),
        (
            ResultKindIr::Bool
            | ResultKindIr::Int
            | ResultKindIr::Quantity(_)
            | ResultKindIr::Array { .. },
            None,
        ) => None,
        (ResultKindIr::Struct(_), None)
        | (
            ResultKindIr::Bool
            | ResultKindIr::Int
            | ResultKindIr::Quantity(_)
            | ResultKindIr::Array { .. },
            Some(_),
        ) => {
            return Err(internal_invariant_error(
                name.span(),
                name,
                "output-type symbols do not match the lowered result kind",
            ));
        }
    };
    let natural_result_ty = match &function.result {
        ResultKindIr::Quantity(_) => quote! { f64 },
        ResultKindIr::Bool => quote! { bool },
        ResultKindIr::Int => quote! { i64 },
        ResultKindIr::Array { .. } => quote! { ::graphcal_plugin::Array },
        ResultKindIr::Struct(_) => {
            let Some(output_ident) = output_ident else {
                return Err(internal_invariant_error(
                    name.span(),
                    name,
                    "a struct result has no generated output type",
                ));
            };
            quote! { #output_ident }
        }
    };
    // A struct-shaped result gets a named output type: positional tuples
    // would let two same-kind fields swap silently.
    let output_struct = match &function.result {
        ResultKindIr::Struct(fields) => {
            let Some(output_ident) = output_ident else {
                return Err(internal_invariant_error(
                    name.span(),
                    name,
                    "a struct result has no generated output type",
                ));
            };
            let field_defs = fields.iter().map(|field| {
                let fname = &field.name;
                let ty = match &field.kind {
                    FieldKindIr::Quantity(_) => quote! { f64 },
                    FieldKindIr::Bool => quote! { bool },
                    FieldKindIr::Int => quote! { i64 },
                };
                quote! { pub #fname: #ty }
            });
            let doc = format!(
                "The result of [`{name}`], one field per declared struct field (quantities in SI                  base units)."
            );
            Some(quote! {
                #[doc = #doc]
                #[derive(Debug, Clone, Copy, PartialEq)]
                pub struct #output_ident {
                    #(#field_defs),*
                }
            })
        }
        _ => None,
    };

    let WrapperPieces {
        raw_params,
        decodes,
        natural_args,
    } = wrapper_pieces(function, symbols)?;

    let wrapper = generate_buffer_wrapper(function, symbols, &raw_params, &decodes, &natural_args)?;

    Ok(quote! {
        #(#docs)*
        #output_struct

        pub fn #name(#(#natural_params),*) -> #natural_result_ty {
            ::graphcal_plugin::__rt::install_failure_hook();
            #body
        }

        #wrapper
    })
}

/// The `wasm32`-only export wrapper for one buffer-moving function.
fn generate_buffer_wrapper(
    function: &FunctionIr,
    symbols: &FunctionSymbols,
    raw_params: &[TokenStream],
    decodes: &[TokenStream],
    natural_args: &[TokenStream],
) -> syn::Result<TokenStream> {
    let Some(wrapper_ident) = &symbols.wrapper else {
        return Err(internal_invariant_error(
            function.name.span(),
            &function.name,
            "a buffer function has no generated wrapper symbol",
        ));
    };
    BufferWrapperCodegen {
        function,
        symbols,
        wrapper_ident,
        raw_params,
        decodes,
        natural_args,
    }
    .generate()
}

struct BufferWrapperCodegen<'a> {
    function: &'a FunctionIr,
    symbols: &'a FunctionSymbols,
    wrapper_ident: &'a Ident,
    raw_params: &'a [TokenStream],
    decodes: &'a [TokenStream],
    natural_args: &'a [TokenStream],
}

impl BufferWrapperCodegen<'_> {
    fn generate(&self) -> syn::Result<TokenStream> {
        let name = &self.function.name;
        let name_str = name.to_string();
        let wrapper_ident = self.wrapper_ident;
        let raw_params = self.raw_params;
        let decodes = self.decodes;
        let natural_args = self.natural_args;
        Ok(match &self.function.result {
            ResultKindIr::Array { indexes, .. } => self.array_result(indexes)?,
            ResultKindIr::Quantity(_) => quote! {
                #[cfg(target_arch = "wasm32")]
                #[unsafe(export_name = #name_str)]
                extern "C-unwind" fn #wrapper_ident(#(#raw_params),*) -> f64 {
                    ::graphcal_plugin::__rt::install_failure_hook();
                    #(#decodes)*
                    #name(#(#natural_args),*)
                }
            },
            ResultKindIr::Bool => quote! {
                #[cfg(target_arch = "wasm32")]
                #[unsafe(export_name = #name_str)]
                extern "C-unwind" fn #wrapper_ident(#(#raw_params),*) -> f64 {
                    ::graphcal_plugin::__rt::install_failure_hook();
                    #(#decodes)*
                    ::graphcal_plugin::__rt::bool_to_abi(#name(#(#natural_args),*))
                }
            },
            ResultKindIr::Int => quote! {
                #[cfg(target_arch = "wasm32")]
                #[unsafe(export_name = #name_str)]
                extern "C-unwind" fn #wrapper_ident(#(#raw_params),*) -> f64 {
                    ::graphcal_plugin::__rt::install_failure_hook();
                    #(#decodes)*
                    ::graphcal_plugin::__rt::int_to_abi(#name(#(#natural_args),*))
                }
            },
            ResultKindIr::Struct(fields) => self.struct_result(fields)?,
        })
    }

    fn array_result(&self, indexes: &[syn::Ident]) -> syn::Result<TokenStream> {
        // Every result extent comes from an input occurrence of the same
        // index variable; lowering guarantees each binding exists.
        let expected_extents = indexes
            .iter()
            .map(|index| binding_extent_ident(self.function, self.symbols, index))
            .collect::<syn::Result<Vec<_>>>()?;
        let name = &self.function.name;
        let name_str = name.to_string();
        let wrapper_ident = self.wrapper_ident;
        let raw_params = self.raw_params;
        let decodes = self.decodes;
        let natural_args = self.natural_args;
        let result = &self.symbols.result;
        let out_ptr = &self.symbols.out_ptr;
        let expected_shape = &self.symbols.expected_shape;
        Ok(quote! {
            #[cfg(target_arch = "wasm32")]
            #[unsafe(export_name = #name_str)]
            extern "C-unwind" fn #wrapper_ident(
                #(#raw_params,)*
                #out_ptr: *mut f64,
            ) {
                ::graphcal_plugin::__rt::install_failure_hook();
                #(#decodes)*
                let #result = #name(#(#natural_args),*);
                let #expected_shape = [#(#expected_extents as usize),*];
                // SAFETY: the host allocated the product of the
                // signature-bound result extents at this pointer.
                unsafe {
                    ::graphcal_plugin::__rt::write_array_result(
                        &#result,
                        #out_ptr,
                        &#expected_shape,
                        #name_str,
                    );
                }
            }
        })
    }

    fn struct_result(&self, fields: &[FieldIr]) -> syn::Result<TokenStream> {
        let name = &self.function.name;
        let slot_count = u32::try_from(fields.len()).map_err(|_| {
            internal_invariant_error(
                name.span(),
                name,
                "struct result field count does not fit the ABI slot-count type",
            )
        })?;
        let result = &self.symbols.result;
        let slots = fields.iter().map(|field| {
            let fname = &field.name;
            match &field.kind {
                FieldKindIr::Quantity(_) => quote! { #result.#fname },
                FieldKindIr::Bool => {
                    quote! { ::graphcal_plugin::__rt::bool_to_abi(#result.#fname) }
                }
                FieldKindIr::Int => {
                    quote! { ::graphcal_plugin::__rt::int_to_abi(#result.#fname) }
                }
            }
        });
        let name_str = name.to_string();
        let wrapper_ident = self.wrapper_ident;
        let raw_params = self.raw_params;
        let decodes = self.decodes;
        let natural_args = self.natural_args;
        let out_ptr = &self.symbols.out_ptr;
        let result_slots = &self.symbols.slots;
        Ok(quote! {
            #[cfg(target_arch = "wasm32")]
            #[unsafe(export_name = #name_str)]
            extern "C-unwind" fn #wrapper_ident(
                #(#raw_params,)*
                #out_ptr: *mut f64,
            ) {
                ::graphcal_plugin::__rt::install_failure_hook();
                #(#decodes)*
                let #result = #name(#(#natural_args),*);
                let #result_slots: [f64; #slot_count as usize] = [#(#slots),*];
                // SAFETY: the host allocated one slot per declared field.
                unsafe {
                    ::graphcal_plugin::__rt::write_slots(
                        &#result_slots,
                        #out_ptr,
                        #slot_count,
                        #name_str,
                    );
                }
            }
        })
    }
}

fn binding_extent_ident(
    function: &FunctionIr,
    symbols: &FunctionSymbols,
    index: &syn::Ident,
) -> syn::Result<Ident> {
    if function.params.len() != symbols.params.len() {
        return Err(internal_invariant_error(
            function.name.span(),
            &function.name,
            "parameter symbol count does not match the lowered signature",
        ));
    }
    for (param, param_symbols) in function.params.iter().zip(&symbols.params) {
        match (&param.kind, param_symbols) {
            (ParamKindIr::Array { indexes, .. }, ParamSymbols::Array { extents, .. }) => {
                if let Some(axis) = indexes.iter().position(|candidate| candidate == index) {
                    return extents.get(axis).cloned().ok_or_else(|| {
                        internal_invariant_error(
                            param.name.span(),
                            &function.name,
                            "array-axis symbol count does not match the lowered parameter",
                        )
                    });
                }
            }
            (
                ParamKindIr::Bool | ParamKindIr::Int | ParamKindIr::Quantity(_),
                ParamSymbols::Scalar,
            ) => {}
            (
                ParamKindIr::Bool | ParamKindIr::Int | ParamKindIr::Quantity(_),
                ParamSymbols::Array { .. },
            )
            | (ParamKindIr::Array { .. }, ParamSymbols::Scalar) => {
                return Err(internal_invariant_error(
                    param.name.span(),
                    &function.name,
                    "parameter symbol kind does not match the lowered signature",
                ));
            }
        }
    }
    Err(internal_invariant_error(
        index.span(),
        &function.name,
        &format!("result axis `{index}` has no generated extent binding from an array parameter"),
    ))
}

fn internal_invariant_error(span: Span, function: &syn::Ident, detail: &str) -> syn::Error {
    syn::Error::new(
        span,
        format!(
            "internal plugin code-generation invariant failed for function `{function}`: {detail}"
        ),
    )
}

/// `solve_orbit` → `SolveOrbitOutput`.
fn output_struct_ident(name: &syn::Ident) -> syn::Result<Ident> {
    let authored = name.to_string();
    let authored = authored.strip_prefix("r#").unwrap_or(&authored);
    let mut camel = String::new();
    for part in authored.split('_').filter(|part| !part.is_empty()) {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            camel.extend(first.to_uppercase());
            camel.push_str(chars.as_str());
        }
    }
    let generated = format!("{camel}Output");
    let mut ident = syn::parse_str::<Ident>(&generated).map_err(|_| {
        syn::Error::new(
            name.span(),
            format!(
                "cannot derive a valid Rust output type name from function `{name}`; rename the \
                 function"
            ),
        )
    })?;
    ident.set_span(name.span());
    Ok(ident)
}

#[cfg(test)]
mod tests {
    use quote::quote;

    use super::*;

    #[test]
    fn missing_result_extent_is_a_spanned_codegen_error() {
        let ast: crate::parse::PluginInput = syn::parse2(quote! {
            fn broken<I: Index>(xs: Dimensionless[I]) -> Dimensionless[I] {
                unreachable!()
            }
        })
        .expect("test signature parses");
        let mut ir = crate::lower::lower(&ast).expect("test signature lowers");
        ir.functions[0].params.clear();

        let message = generate(&ir, "{}")
            .expect_err("broken IR must fail code generation")
            .to_string();
        assert!(message.contains("function `broken`"), "got: {message}");
        assert!(message.contains("result axis `I`"), "got: {message}");
        assert!(
            message.contains("no generated extent binding"),
            "got: {message}"
        );
    }
}
