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
//! Scalar-only functions are emitted as a single `extern "C-unwind"` item
//! whose raw `f64` parameters double as the natural test surface. Functions
//! that move arrays split in two: a natural `pub fn` taking `&[f64]` slices
//! (what `cargo test` calls) and a `wasm32`-only export wrapper that decodes
//! the `(ptr, len)` pairs, calls the natural function, and writes the
//! result through the host-allocated out-pointer.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::lower::{FunctionIr, KindIr, PluginIr};

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
        generate_scalar_function(function)
    }
}

fn generate_scalar_function(function: &FunctionIr) -> TokenStream {
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
            KindIr::Scalar(_) | KindIr::Array { .. } => None,
            KindIr::Bool => Some(quote! {
                let #name: bool = ::graphcal_plugin::__rt::bool_from_abi(#name, #name_str);
            }),
            KindIr::Int => Some(quote! {
                let #name: i64 = ::graphcal_plugin::__rt::int_from_abi(#name, #name_str);
            }),
        }
    });
    let (result_ty, to_abi) = match function.result {
        KindIr::Scalar(_) | KindIr::Array { .. } => (quote! { f64 }, quote! { __graphcal_result }),
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
            KindIr::Scalar(_) => {
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
            KindIr::Array { .. } => {
                let ptr = format_ident!("{pname}_ptr");
                let len = format_ident!("{pname}_len");
                raw_params.push(quote! { #ptr: *const f64, #len: u32 });
                decodes.push(quote! {
                    // SAFETY: the host wrote `len` elements at `ptr` inside
                    // this instance's memory and keeps them alive for the
                    // duration of the call.
                    let #pname: &[f64] =
                        unsafe { ::graphcal_plugin::__rt::slice_from_abi(#ptr, #len) };
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
    let name_str = name.to_string();
    let body = &function.body;

    let natural_params = function.params.iter().map(|param| {
        let pname = &param.name;
        match &param.kind {
            KindIr::Scalar(_) => quote! { #pname: f64 },
            KindIr::Bool => quote! { #pname: bool },
            KindIr::Int => quote! { #pname: i64 },
            KindIr::Array { .. } => quote! { #pname: &[f64] },
        }
    });
    let natural_result_ty = match &function.result {
        KindIr::Scalar(_) => quote! { f64 },
        KindIr::Bool => quote! { bool },
        KindIr::Int => quote! { i64 },
        KindIr::Array { .. } => quote! { ::std::vec::Vec<f64> },
    };

    let WrapperPieces {
        raw_params,
        decodes,
        natural_args,
    } = wrapper_pieces(function);

    let wrapper_ident = format_ident!("__graphcal_export_{name}");
    let wrapper = match &function.result {
        KindIr::Array { index, .. } => {
            // The out-buffer length is the input array bound to the result's
            // index variable; lowering guarantees one exists.
            let binding_len = function
                .params
                .iter()
                .find_map(|param| match &param.kind {
                    KindIr::Array {
                        index: param_index, ..
                    } if param_index == index => Some(format_ident!("{}_len", param.name)),
                    _ => None,
                })
                .unwrap_or_else(|| format_ident!("__graphcal_unreachable"));
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
                    // SAFETY: the host allocated the out-buffer with the
                    // binding input's length, which is what is checked here.
                    unsafe {
                        ::graphcal_plugin::__rt::write_array_result(
                            &__graphcal_result,
                            __graphcal_out,
                            #binding_len,
                            #name_str,
                        );
                    }
                }
            }
        }
        KindIr::Scalar(_) => quote! {
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
    };

    quote! {
        #(#docs)*
        pub fn #name(#(#natural_params),*) -> #natural_result_ty {
            ::graphcal_plugin::__rt::install_failure_hook();
            #body
        }

        #wrapper
    }
}
