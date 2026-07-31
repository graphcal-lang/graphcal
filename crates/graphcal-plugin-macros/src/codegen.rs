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

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::lower::{FieldKindIr, FunctionIr, KindIr, PluginIr};

/// Generate the full expansion from the validated IR and its manifest
/// payload.
pub fn generate(ir: &PluginIr, manifest_json: &str) -> TokenStream {
    let bytes = manifest_json.as_bytes();
    let len = bytes.len();
    let payload = proc_macro2::Literal::byte_string(bytes);
    let section = graphcal_plugin_abi::MANIFEST_SECTION;
    let functions = ir.functions.iter().map(generate_function);
    let allocator = ir.uses_buffers().then(generate_allocator_exports);

    quote! {
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
    }
}

/// The buffer-protocol allocator pair the host places array buffers with.
fn generate_allocator_exports() -> TokenStream {
    quote! {
        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C-unwind" fn graphcal_alloc(size: u32) -> *mut u8 {
            ::graphcal_plugin::__rt::buffer_alloc(size)
        }

        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C-unwind" fn graphcal_free(ptr: *mut u8, size: u32) {
            // SAFETY: the host passes back exactly the pairs it allocated.
            unsafe { ::graphcal_plugin::__rt::buffer_free(ptr, size) }
        }
    }
}

fn generate_function(function: &FunctionIr) -> TokenStream {
    if function.uses_buffers() {
        generate_buffer_function(function)
    } else {
        generate_f64_abi_function(function)
    }
}

fn generate_f64_abi_function(function: &FunctionIr) -> TokenStream {
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
            KindIr::Quantity(_) | KindIr::Array { .. } | KindIr::Struct(_) => None,
            KindIr::Bool => Some(quote! {
                let #name: bool = ::graphcal_plugin::__rt::bool_from_abi(#name, #name_str);
            }),
            KindIr::Int => Some(quote! {
                let #name: i64 = ::graphcal_plugin::__rt::int_from_abi(#name, #name_str);
            }),
        }
    });
    let (result_ty, to_abi) = match function.result {
        KindIr::Quantity(_) | KindIr::Array { .. } | KindIr::Struct(_) => {
            (quote! { f64 }, quote! { __graphcal_result })
        }
        KindIr::Bool => (
            quote! { bool },
            quote! { ::graphcal_plugin::__rt::bool_to_abi(__graphcal_result) },
        ),
        KindIr::Int => (
            quote! { i64 },
            quote! { ::graphcal_plugin::__rt::int_to_abi(__graphcal_result) },
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
            let __graphcal_result: #result_ty = { #body };
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

fn wrapper_pieces(function: &FunctionIr) -> WrapperPieces {
    let mut raw_params: Vec<TokenStream> = Vec::new();
    let mut decodes: Vec<TokenStream> = Vec::new();
    let mut natural_args: Vec<TokenStream> = Vec::new();
    for param in &function.params {
        let pname = &param.name;
        let pname_str = pname.to_string();
        match &param.kind {
            // Struct parameters cannot be written (the parser only accepts
            // the braced shape in result position); the arm keeps the match
            // total without a panic path.
            KindIr::Quantity(_) | KindIr::Struct(_) => {
                raw_params.push(quote! { #pname: f64 });
                natural_args.push(quote! { #pname });
            }
            KindIr::Bool => {
                raw_params.push(quote! { #pname: f64 });
                decodes.push(quote! {
                    let #pname: bool = ::graphcal_plugin::__rt::bool_from_abi(#pname, #pname_str);
                });
                natural_args.push(quote! { #pname });
            }
            KindIr::Int => {
                raw_params.push(quote! { #pname: f64 });
                decodes.push(quote! {
                    let #pname: i64 = ::graphcal_plugin::__rt::int_from_abi(#pname, #pname_str);
                });
                natural_args.push(quote! { #pname });
            }
            KindIr::Array { indexes, .. } => {
                let ptr = format_ident!("{pname}_ptr");
                let len = format_ident!("{pname}_len");
                let shape = format_ident!("{pname}_shape");
                let extents = indexes
                    .iter()
                    .enumerate()
                    .map(|(axis, _)| array_extent_ident(pname, axis))
                    .collect::<Vec<_>>();
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
        }
    }
    WrapperPieces {
        raw_params,
        decodes,
        natural_args,
    }
}

/// Emit an array-moving function: the natural `pub fn` (slices in, `Vec`
/// out) plus the `wasm32`-only export wrapper speaking the buffer protocol.
fn generate_buffer_function(function: &FunctionIr) -> TokenStream {
    let docs = &function.docs;
    let name = &function.name;
    let body = &function.body;

    let natural_params = function.params.iter().map(|param| {
        let pname = &param.name;
        match &param.kind {
            KindIr::Quantity(_) | KindIr::Struct(_) => quote! { #pname: f64 },
            KindIr::Bool => quote! { #pname: bool },
            KindIr::Int => quote! { #pname: i64 },
            KindIr::Array { .. } => quote! { #pname: ::graphcal_plugin::ArrayView<'_> },
        }
    });
    let output_ident = output_struct_ident(name);
    let natural_result_ty = match &function.result {
        KindIr::Quantity(_) => quote! { f64 },
        KindIr::Bool => quote! { bool },
        KindIr::Int => quote! { i64 },
        KindIr::Array { .. } => quote! { ::graphcal_plugin::Array },
        KindIr::Struct(_) => quote! { #output_ident },
    };
    // A struct-shaped result gets a named output type: positional tuples
    // would let two same-kind fields swap silently.
    let output_struct = match &function.result {
        KindIr::Struct(fields) => {
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
    } = wrapper_pieces(function);

    let wrapper = generate_buffer_wrapper(function, &raw_params, &decodes, &natural_args);

    quote! {
        #(#docs)*
        #output_struct

        pub fn #name(#(#natural_params),*) -> #natural_result_ty {
            ::graphcal_plugin::__rt::install_failure_hook();
            #body
        }

        #wrapper
    }
}

/// The `wasm32`-only export wrapper for one buffer-moving function.
fn generate_buffer_wrapper(
    function: &FunctionIr,
    raw_params: &[TokenStream],
    decodes: &[TokenStream],
    natural_args: &[TokenStream],
) -> TokenStream {
    let name = &function.name;
    let name_str = name.to_string();
    let wrapper_ident = format_ident!("__graphcal_export_{name}");
    match &function.result {
        KindIr::Array { indexes, .. } => {
            // Every result extent comes from an input occurrence of the same
            // index variable; lowering guarantees each binding exists.
            let expected_extents = indexes
                .iter()
                .map(|index| binding_extent_ident(function, index))
                .collect::<Vec<_>>();
            quote! {
                #[cfg(target_arch = "wasm32")]
                #[unsafe(export_name = #name_str)]
                extern "C-unwind" fn #wrapper_ident(
                    #(#raw_params,)*
                    __graphcal_out: *mut f64,
                ) {
                    ::graphcal_plugin::__rt::install_failure_hook();
                    #(#decodes)*
                    let __graphcal_result = #name(#(#natural_args),*);
                    let __graphcal_expected_shape = [#(#expected_extents as usize),*];
                    // SAFETY: the host allocated the product of the
                    // signature-bound result extents at this pointer.
                    unsafe {
                        ::graphcal_plugin::__rt::write_array_result(
                            &__graphcal_result,
                            __graphcal_out,
                            &__graphcal_expected_shape,
                            #name_str,
                        );
                    }
                }
            }
        }
        KindIr::Quantity(_) => quote! {
            #[cfg(target_arch = "wasm32")]
            #[unsafe(export_name = #name_str)]
            extern "C-unwind" fn #wrapper_ident(#(#raw_params),*) -> f64 {
                ::graphcal_plugin::__rt::install_failure_hook();
                #(#decodes)*
                #name(#(#natural_args),*)
            }
        },
        KindIr::Bool => quote! {
            #[cfg(target_arch = "wasm32")]
            #[unsafe(export_name = #name_str)]
            extern "C-unwind" fn #wrapper_ident(#(#raw_params),*) -> f64 {
                ::graphcal_plugin::__rt::install_failure_hook();
                #(#decodes)*
                ::graphcal_plugin::__rt::bool_to_abi(#name(#(#natural_args),*))
            }
        },
        KindIr::Int => quote! {
            #[cfg(target_arch = "wasm32")]
            #[unsafe(export_name = #name_str)]
            extern "C-unwind" fn #wrapper_ident(#(#raw_params),*) -> f64 {
                ::graphcal_plugin::__rt::install_failure_hook();
                #(#decodes)*
                ::graphcal_plugin::__rt::int_to_abi(#name(#(#natural_args),*))
            }
        },
        KindIr::Struct(fields) => {
            let slot_count = u32::try_from(fields.len()).unwrap_or(u32::MAX);
            let slots = fields.iter().map(|field| {
                let fname = &field.name;
                match &field.kind {
                    FieldKindIr::Quantity(_) => quote! { __graphcal_result.#fname },
                    FieldKindIr::Bool => quote! {
                        ::graphcal_plugin::__rt::bool_to_abi(__graphcal_result.#fname)
                    },
                    FieldKindIr::Int => quote! {
                        ::graphcal_plugin::__rt::int_to_abi(__graphcal_result.#fname)
                    },
                }
            });
            quote! {
                #[cfg(target_arch = "wasm32")]
                #[unsafe(export_name = #name_str)]
                extern "C-unwind" fn #wrapper_ident(
                    #(#raw_params,)*
                    __graphcal_out: *mut f64,
                ) {
                    ::graphcal_plugin::__rt::install_failure_hook();
                    #(#decodes)*
                    let __graphcal_result = #name(#(#natural_args),*);
                    let __graphcal_slots: [f64; #slot_count as usize] = [#(#slots),*];
                    // SAFETY: the host allocated one slot per declared field.
                    unsafe {
                        ::graphcal_plugin::__rt::write_slots(
                            &__graphcal_slots,
                            __graphcal_out,
                            #slot_count,
                            #name_str,
                        );
                    }
                }
            }
        }
    }
}

fn array_extent_ident(param: &syn::Ident, axis: usize) -> syn::Ident {
    format_ident!("{param}_extent_{axis}")
}

fn binding_extent_ident(function: &FunctionIr, index: &syn::Ident) -> syn::Ident {
    function
        .params
        .iter()
        .find_map(|param| match &param.kind {
            KindIr::Array { indexes, .. } => indexes
                .iter()
                .position(|candidate| candidate == index)
                .map(|axis| array_extent_ident(&param.name, axis)),
            KindIr::Bool | KindIr::Int | KindIr::Quantity(_) | KindIr::Struct(_) => None,
        })
        .unwrap_or_else(|| format_ident!("__graphcal_unreachable_extent"))
}

/// `solve_orbit` → `SolveOrbitOutput`.
fn output_struct_ident(name: &syn::Ident) -> syn::Ident {
    let mut camel = String::new();
    for part in name.to_string().split('_').filter(|part| !part.is_empty()) {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            camel.extend(first.to_uppercase());
            camel.push_str(chars.as_str());
        }
    }
    format_ident!("{camel}Output", span = name.span())
}
