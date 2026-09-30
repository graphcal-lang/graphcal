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
//! # ABI v5 contract
//!
//! A graphcal plugin is a **core WebAssembly module** (not a component) that:
//!
//! - embeds a [`PluginManifest`] as JSON in a custom section named
//!   [`MANIFEST_SECTION`] (exactly one such section);
//! - exports, for every manifest function, a wasm function whose type is
//!   determined by the signature:
//!   - each quantity, `Int`, or `Bool` parameter is one `f64` (raw SI base
//!     units for quantities; `Int` parameters arrive as exactly-representable
//!     integers and `Bool` parameters as `1.0`/`0.0`);
//!   - each rank-`R` array parameter is `(i32 ptr, i32 extent_0, …,
//!     i32 extent_R-1)`: row-major dense little-endian `f64` elements in a
//!     host-allocated 8-byte-aligned buffer inside the plugin's own memory.
//!     Quantity elements must be finite, `Bool` elements are numeric zero or
//!     one, and `Int` elements follow the scalar lossless binary64 policy;
//!   - a quantity, `Int`, or `Bool` result is the single `f64` return value;
//!     an array or struct result turns the return into one trailing
//!     `i32 out_ptr` parameter (and no return values). An array writes exactly
//!     the product of its result-axis extents (all bound by inputs); a struct
//!     writes one slot per declared field;
//! - if any function moves an array or struct result, exports its linear memory as
//!   [`MEMORY_EXPORT`] plus the allocator pair [`ALLOC_EXPORT`] of type
//!   `(i32 size) -> i32` (returning 8-byte-aligned pointers) and
//!   [`FREE_EXPORT`] of type `(i32 ptr, i32 size) -> ()`. The host allocates
//!   every buffer before a call and frees them all after it; a plugin never
//!   retains a buffer pointer across calls;
//! - imports **nothing**, with a single optional exception: the host-provided
//!   `graphcal::fail` function ([`FAIL_IMPORT_MODULE`], [`FAIL_IMPORT_NAME`])
//!   of wasm type `(i32, i32) -> ()`. The import ban is what guarantees
//!   plugins are pure and free of I/O by construction;
//! - exports its linear memory as [`MEMORY_EXPORT`] **if** it imports the fail
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
//! cross the binary boundary in ABI v5. Array elements carry an explicit
//! quantity, `Bool`, or `Int` semantic kind; index variables are opaque names
//! bound per call — a plugin never learns an index's identity, only each
//! buffer's ordered shape.

mod constants;
pub mod manifest;
pub mod section;

pub use constants::{
    ABI_VERSION, ALLOC_EXPORT, BUFFER_ALIGN, FAIL_IMPORT_MODULE, FAIL_IMPORT_NAME, FREE_EXPORT,
    MANIFEST_SECTION, MAX_ABI_FUNCTION_PARAMS, MAX_FAIL_MESSAGE_BYTES, MAX_MANIFEST_ARRAY_AXES,
    MAX_MANIFEST_BYTES, MAX_MANIFEST_FUNCTIONS, MAX_MANIFEST_MONOMIAL_FACTORS,
    MAX_MANIFEST_NAME_BYTES, MAX_MANIFEST_PARAMETERS, MAX_MANIFEST_STRUCT_FIELDS,
    MAX_MANIFEST_VARIABLES, MEMORY_EXPORT,
};
pub use manifest::{
    ManifestArrayElementKind, ManifestDecodeError, ManifestDimPower, ManifestEmbedError,
    ManifestEncodeError, ManifestField, ManifestFieldKind, ManifestFromWasmError, ManifestFunction,
    ManifestListRole, ManifestMonomial, ManifestParam, ManifestParamKind, ManifestRational,
    ManifestResultKind, ManifestValidationError, ManifestVarPower, NameRole, PluginManifest,
};
pub use section::{SectionError, embed_manifest};
