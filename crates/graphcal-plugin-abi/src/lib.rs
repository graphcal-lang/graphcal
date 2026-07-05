//! The graphcal plugin ABI: the protocol shared by the graphcal host and
//! WASM plugin modules (Phase B of the plugin plan, issue #25).
//!
//! This crate is the protocol *definition*, deliberately free of any WASM
//! runtime or compiler dependency so that both the host (the graphcal
//! toolchain) and plugin build tooling (the future authoring SDK, Phase C)
//! can share it. It owns:
//!
//! - the [manifest data model](manifest) a plugin embeds to describe its
//!   functions' dimensional signatures, and its JSON codec;
//! - the [custom-section codec](section) that reads and writes the manifest
//!   inside a `.wasm` binary without instantiating (or even parsing) the
//!   module beyond the section layout;
//! - the protocol constants below.
//!
//! # ABI v1 contract
//!
//! A graphcal plugin is a **core WebAssembly module** (not a component) that:
//!
//! - embeds a [`PluginManifest`] as JSON in a custom section named
//!   [`MANIFEST_SECTION`] (exactly one such section);
//! - exports, for every manifest function, a wasm function of type
//!   `(f64, …, f64) -> f64` with one `f64` parameter per manifest parameter.
//!   Scalar values cross the boundary as raw `f64`s in SI base units; `Int`
//!   parameters arrive as exactly-representable integers and `Bool`
//!   parameters as `1.0`/`0.0`, with the result converted back per the
//!   declared result kind;
//! - imports **nothing**, with a single optional exception: the host-provided
//!   `graphcal::fail` function ([`FAIL_IMPORT_MODULE`], [`FAIL_IMPORT_NAME`])
//!   of wasm type `(i32, i32) -> ()`. The import ban is what guarantees
//!   plugins are pure and free of I/O by construction;
//! - exports its linear memory as `"memory"` **if** it imports the fail
//!   function (the host reads the failure message out of that memory).
//!
//! To report a failure, a plugin calls `fail(ptr, len)` with a UTF-8 message
//! of at most [`MAX_FAIL_MESSAGE_BYTES`] bytes; the host records the message
//! and traps the current call, so `fail` never returns. Traps, exhausted
//! fuel, and failure messages all surface as per-node evaluation diagnostics
//! on the graphcal side; a non-finite `f64` result is not an ABI error and is
//! handled by graphcal's ordinary non-finite-value containment.
//!
//! Dimensions in the manifest are expressed structurally as exponent vectors
//! over the prelude base dimensions only; user-defined base dimensions never
//! cross the binary boundary in ABI v1.

pub mod manifest;
pub mod section;

pub use manifest::{
    ManifestDecodeError, ManifestDimPower, ManifestEmbedError, ManifestEncodeError,
    ManifestFromWasmError, ManifestFunction, ManifestMonomial, ManifestParam, ManifestRational,
    ManifestValidationError, ManifestValueKind, ManifestVarPower, NameRole, PluginManifest,
};
pub use section::{SectionError, embed_manifest, extract_manifest};

/// The plugin ABI version this crate speaks.
///
/// Stored in [`PluginManifest::abi_version`]; a manifest with any other
/// version is rejected at decode time so hosts can report "plugin requires a
/// newer/older graphcal" instead of a shape error.
pub const ABI_VERSION: u32 = 1;

/// Name of the wasm custom section holding the JSON-encoded manifest.
pub const MANIFEST_SECTION: &str = "graphcal-manifest";

/// Wasm module name of the only import a plugin may declare.
pub const FAIL_IMPORT_MODULE: &str = "graphcal";

/// Wasm field name of the only import a plugin may declare: the
/// host-provided failure reporter of type `(i32 ptr, i32 len) -> ()`.
pub const FAIL_IMPORT_NAME: &str = "fail";

/// Maximum length in bytes of a UTF-8 failure message passed to
/// [`FAIL_IMPORT_NAME`]; hosts truncate anything longer.
pub const MAX_FAIL_MESSAGE_BYTES: usize = 4096;
