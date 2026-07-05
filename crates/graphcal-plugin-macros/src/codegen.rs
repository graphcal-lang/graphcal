//! Emitting the generated items: the manifest static, the
//! one-`plugin!`-per-module guard symbol, and the `extern "C"` wrappers.
//!
//! Everything the wasm module needs is plain Rust with `wasm32`-gated
//! attributes, so the same expansion compiles natively — plugin crates
//! unit-test their kernels with ordinary `cargo test`, and the workspace
//! integration tests read `GRAPHCAL_PLUGIN_MANIFEST` without a wasm
//! toolchain.

use proc_macro2::TokenStream;
use quote::quote;

use crate::lower::{FunctionIr, KindIr, PluginIr};

/// Generate the full expansion from the validated IR and its manifest
/// payload.
pub fn generate(ir: &PluginIr, manifest_json: &str) -> TokenStream {
    let bytes = manifest_json.as_bytes();
    let len = bytes.len();
    let payload = proc_macro2::Literal::byte_string(bytes);
    let section = graphcal_plugin_abi::MANIFEST_SECTION;
    let functions = ir.functions.iter().map(generate_function);

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

        #(#functions)*
    }
}

fn generate_function(function: &FunctionIr) -> TokenStream {
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
            KindIr::Scalar(_) => None,
            KindIr::Bool => Some(quote! {
                let #name: bool = ::graphcal_plugin::__rt::bool_from_abi(#name, #name_str);
            }),
            KindIr::Int => Some(quote! {
                let #name: i64 = ::graphcal_plugin::__rt::int_from_abi(#name, #name_str);
            }),
        }
    });
    let (result_ty, to_abi) = match function.result {
        KindIr::Scalar(_) => (quote! { f64 }, quote! { __graphcal_result }),
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
