//! Protocol constants shared by plugin hosts and guests.

/// The plugin ABI version this crate speaks.
///
/// Stored in [`PluginManifest::abi_version`](crate::PluginManifest::abi_version); a manifest with any other
/// version is rejected at decode time so hosts can report "plugin requires a
/// newer/older graphcal" instead of a shape error. Version 5 gives array
/// elements an explicit quantity, `Bool`, or `Int` kind. Version 4 added
/// multi-axis array shapes; version 3 renamed
/// the quantity manifest kind from `scalar` to `quantity`; version 2 introduced
/// arrays and allocator exports. Older modules are not accepted; rebuild
/// against the current SDK.
pub const ABI_VERSION: u32 = 5;

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

/// Maximum encoded byte length of the JSON manifest custom-section payload.
///
/// This fixed protocol limit is enforced before decoding, while encoding, and
/// by the raw custom-section embedding helper.
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;

/// Maximum number of functions declared by one plugin manifest.
pub const MAX_MANIFEST_FUNCTIONS: usize = 256;

/// Maximum UTF-8 byte length of any name in a plugin manifest.
pub const MAX_MANIFEST_NAME_BYTES: usize = 256;

/// Maximum number of dimension variables or index variables declared by one
/// manifest function.
pub const MAX_MANIFEST_VARIABLES: usize = 32;

/// Maximum number of Graphcal parameters declared by one manifest function.
pub const MAX_MANIFEST_PARAMETERS: usize = MAX_ABI_FUNCTION_PARAMS;

/// Maximum number of axes in one manifest array kind.
///
/// An array parameter also consumes one raw pointer parameter, so this is one
/// less than [`MAX_ABI_FUNCTION_PARAMS`]. Aggregate signature width is checked
/// separately.
pub const MAX_MANIFEST_ARRAY_AXES: usize = MAX_ABI_FUNCTION_PARAMS - 1;

/// Maximum number of flattened fields in one struct result.
pub const MAX_MANIFEST_STRUCT_FIELDS: usize = 256;

/// Maximum combined variable and fixed-dimension factors in one monomial.
pub const MAX_MANIFEST_MONOMIAL_FACTORS: usize = 64;

/// Maximum raw WebAssembly parameters in one exported plugin function.
///
/// A single-value Graphcal parameter occupies one slot. An array occupies one
/// pointer plus one extent per axis, and an array or struct result adds one
/// trailing out-pointer. This mirrors the malicious-module compilation policy
/// used by the host, so accepted manifests remain compilable.
pub const MAX_ABI_FUNCTION_PARAMS: usize = 32;

/// Export name of the plugin's linear memory.
///
/// Required whenever the manifest declares any array parameter/result or a
/// struct result, and whenever the plugin imports [`FAIL_IMPORT_NAME`].
pub const MEMORY_EXPORT: &str = "memory";

/// Export name of the plugin allocator the host places array buffers with.
///
/// Wasm type `(i32 size) -> i32`, returning a nonzero,
/// [`BUFFER_ALIGN`]-aligned pointer whose complete requested range is in the
/// exported memory. A return value of zero is reserved to report allocation
/// failure. Required (with [`FREE_EXPORT`] and an exported memory) whenever
/// the manifest declares any array parameter or result.
pub const ALLOC_EXPORT: &str = "graphcal_alloc";

/// Export name of the matching deallocator: wasm type
/// `(i32 ptr, i32 size) -> ()`. The host frees every buffer it allocated
/// once the call completes.
pub const FREE_EXPORT: &str = "graphcal_free";

/// Alignment in bytes of every host-requested buffer allocation ([`ALLOC_EXPORT`]).
pub const BUFFER_ALIGN: usize = 8;
