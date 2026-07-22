//! The WASM plugin host: loading, validating, and executing graphcal
//! plugins (Phase B of the plugin plan, issue #25).
//!
//! This crate is the runtime half of the plugin boundary. The protocol —
//! manifest model, custom-section codec, ABI constants — lives in
//! [`graphcal_plugin_abi`]; this crate consumes it to:
//!
//! - validate a `.wasm` binary against the ABI at load time
//!   ([`PluginModule`] via [`PluginHost::load`]): manifest present and
//!   decodable, signatures convertible to the compiler's typed
//!   [`FunctionSignature`](graphcal_compiler::function_signature::FunctionSignature)
//!   IR ([`convert`]), no imports beyond `graphcal::fail`, memory exported
//!   when failure messages need reading, and every manifest function
//!   exported with the scalar wasm type `(f64 × arity) -> f64`;
//! - execute plugin functions under mandatory resource bounds
//!   ([`PluginLimits`]: per-call fuel plus a linear-memory cap), mapping
//!   failure messages, traps, and fuel exhaustion to [`PluginCallError`];
//! - cache compiled modules by content hash ([`PluginHost`]), so the
//!   language server's keystroke-frequency re-evaluation never recompiles
//!   an unchanged plugin.
//!
//! Evaluation itself stays WASM-free: the evaluator calls plugins through
//! the `HostFunctionRegistry` interface of `graphcal-eval`, and embedders
//! (CLI, LSP) bridge the two by registering wasm-backed closures.
//!
//! The interpreter is [`wasmi`], matching Typst's plugin architecture: it
//! runs everywhere the toolchain does, including future wasm32 hosts
//! (browser playground, #43), and its arithmetic is IEEE-754 deterministic
//! so plugin results are bit-identical across platforms.

pub mod convert;
pub mod host;
pub mod module;
pub mod registry;

pub use convert::{ConvertErrorKind, ManifestConvertError, convert_manifest};
pub use host::{PluginHost, PluginLimits};
pub use module::{PluginCallError, PluginLoadError, PluginModule};
pub use registry::register_project_plugins;
