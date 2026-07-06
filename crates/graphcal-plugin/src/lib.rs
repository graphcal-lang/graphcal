//! The graphcal plugin authoring SDK (Phase C of the plugin plan, issue #25).
//!
//! A graphcal plugin is a WebAssembly module that exports pure scalar
//! kernels and embeds a manifest describing their dimensional signatures
//! (see the `graphcal-plugin-abi` crate for the protocol). Writing that
//! module by hand means keeping three things in sync: the manifest JSON,
//! the `extern "C"` exports, and the `.gcl` extern declaration. This crate
//! collapses the first two into one declaration:
//!
//! ```
//! graphcal_plugin::plugin! {
//!     /// Linear interpolation between `a` and `b`.
//!     fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D {
//!         a + (b - a) * t
//!     }
//! }
//! # fn main() {}
//! ```
//!
//! The [`plugin!`] macro parses the same signature syntax as the `.gcl`
//! import site, so the declaration above can be pasted verbatim into the
//! importing project:
//!
//! ```text
//! import plugin "plugins/my_plugin.wasm" as my_plugin {
//!     fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D;
//! }
//! ```
//!
//! From the one declaration the macro generates:
//!
//! - the plugin manifest, embedded in the `graphcal-manifest` custom
//!   section (on wasm targets) and exposed as the
//!   `GRAPHCAL_PLUGIN_MANIFEST` static (on every target, for tests and
//!   tooling);
//! - one `extern "C-unwind"` wrapper per function that converts between
//!   the raw ABI (`f64`s in SI base units) and the declared value kinds —
//!   `Bool` parameters arrive in the body as `bool`, `Int` parameters as
//!   `i64`, and scalar parameters as `f64` SI values;
//! - for functions with array parameters or results (`xs: D[I]`), the
//!   buffer-protocol plumbing: the body sees `&[f64]` slices and returns a
//!   `Vec<f64>`, while the generated wasm wrapper and the
//!   `graphcal_alloc`/`graphcal_free` exports move the dense SI buffers
//!   across the boundary;
//! - a panic hook that forwards panic messages through the host's
//!   `graphcal::fail` import, so a `panic!` in plugin code surfaces as a
//!   readable per-node diagnostic instead of an anonymous trap.
//!
//! # Values are SI
//!
//! Scalar values cross the plugin boundary as bare `f64`s **in SI base
//! units** — a `Pressure` parameter is pascals, a `Velocity` result is
//! metres per second. The declared dimensions are checked by the graphcal
//! compiler at every call site; reading a pascal as a bar inside the body
//! is the one mistake the type system cannot catch for you. Keep kernel
//! math in SI throughout.
//!
//! # Failures
//!
//! Report domain failures with [`fail()`] or the [`fail!`] format macro;
//! the message surfaces in the failing node's diagnostic. Panics are
//! forwarded the same way. On non-wasm targets both become ordinary Rust
//! panics, so `cargo test` in a plugin crate behaves as usual.
//!
//! # Building
//!
//! Compile with `cargo build --release --target wasm32-unknown-unknown`
//! (as a `cdylib`), then vendor the artifact into the graphcal project and
//! pin it with `graphcal deps lock`. `graphcal plugin new` scaffolds a
//! ready-to-build crate, and `graphcal plugin test` validates and calls
//! the built module without a graphcal project.

/// Declare the plugin's exported functions: signatures in graphcal's
/// extern-declaration syntax, bodies in Rust.
///
/// ```
/// graphcal_plugin::plugin! {
///     /// Ideal-gas density of dry air.
///     fn air_density(p: Pressure, t: Temperature) -> Mass / Volume {
///         const R_SPECIFIC: f64 = 287.052874; // J/(kg*K)
///         if t <= 0.0 {
///             graphcal_plugin::fail!("temperature must be positive, got {t} K");
///         }
///         p / (R_SPECIFIC * t)
///     }
///
///     /// Cube root, dimensionally exact.
///     fn cbrt<D: Dim>(x: D) -> D^(1/3) {
///         x.cbrt()
///     }
/// }
/// # fn main() {
/// #     assert!((cbrt(27.0) - 3.0).abs() < 1e-12);
/// # }
/// ```
///
/// # Signature syntax
///
/// Each function is `fn name<Vars>(params) -> Result { body }`, with
/// binders written `name: constraint` (`D: Dim` for dimension variables,
/// `I: Index` for index variables). Parameter and result types are `Bool`,
/// `Int`, dimension expressions, or arrays of scalars over one declared
/// index variable (`xs: D[I]`, `-> Dimensionless[I]`). Dimension
/// expressions range over:
///
/// - dimension variables declared in the `<...>` binder (`D`, `D1`, …);
/// - the prelude base dimensions `Length`, `Time`, `Mass`, `Temperature`,
///   `ElectricCurrent`, `Amount`, `LuminousIntensity`, `Angle`;
/// - the prelude derived dimensions `Velocity`, `Acceleration`, `Force`,
///   `Energy`, `Power`, `Frequency`, `Pressure`, `Area`, `Volume`
///   (expanded to base dimensions in the manifest);
/// - `Dimensionless`;
///
/// combined with `*`, `/`, parentheses, and `^` powers whose exponents are
/// integers (`^2`, `^-3`) or parenthesized rationals (`^(1/2)`, `^(-1/2)`).
/// Every dimension variable must first appear as a bare parameter type
/// (`x: D`, or a bare array element `xs: D[I]`) before it is used in a
/// compound form — the same rule the graphcal compiler enforces on the
/// `.gcl` declaration. A result array must reuse an index variable that
/// indexes some array parameter: a plugin can never invent its output
/// length.
///
/// # In the body
///
/// Parameters are in scope with their declared names: `f64` (SI) for
/// scalar types, `bool` for `Bool`, `i64` for `Int`, and `&[f64]` (SI,
/// dense, in index order) for arrays. The body is an ordinary Rust block
/// evaluating to `f64`, `bool`, `i64`, or `Vec<f64>` to match the declared
/// result; an array result must have exactly the length of the input array
/// bound to its index variable. Dimension and index variables are
/// *parametric*: the body never learns what `D` or `I` was bound to beyond
/// each slice's length, so keep the math dimension-uniform.
///
/// # Generated items
///
/// Besides one `pub extern "C-unwind"` wrapper per function (exported
/// from the wasm module under the function's name), the macro emits the
/// manifest bytes as `GRAPHCAL_PLUGIN_MANIFEST` and, on wasm targets, an
/// unmangled guard symbol so that linking two `plugin!` blocks into one
/// module fails with a duplicate-symbol error instead of a corrupt
/// manifest section. Use **one `plugin!` block per plugin**; helper
/// functions can live anywhere in the crate and be called from the bodies.
pub use graphcal_plugin_macros::plugin;

/// Report a plugin failure with `format!` syntax and abort the call.
///
/// Equivalent to [`fail()`] with a formatted message:
///
/// ```should_panic
/// let x = -1.0_f64;
/// if x < 0.0 {
///     graphcal_plugin::fail!("expected a non-negative value, got {x}");
/// }
/// ```
#[macro_export]
macro_rules! fail {
    ($($arg:tt)*) => {
        $crate::fail(&::std::format!($($arg)*))
    };
}

/// Report a plugin failure and abort the current call.
///
/// On wasm targets this forwards the message through the host's
/// `graphcal::fail` import, which records it and traps the call; graphcal
/// reports it as the failing node's diagnostic and other nodes keep
/// evaluating. On non-wasm targets (unit tests in the plugin crate) it
/// panics with the same message.
///
/// The host truncates messages to the ABI limit (4096 bytes).
pub fn fail(message: &str) -> ! {
    #[cfg(target_arch = "wasm32")]
    {
        raw_fail(message);
        // `graphcal::fail` always traps host-side; if a non-graphcal host
        // ever returned, trap locally rather than continue in a state the
        // ABI does not define.
        core::arch::wasm32::unreachable()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        native_fail(message)
    }
}

/// The native stand-in for the wasm trap: tests observe failures as
/// ordinary panics carrying the same message.
#[cfg(not(target_arch = "wasm32"))]
#[expect(
    clippy::panic,
    reason = "panicking is this function's contract off-wasm"
)]
fn native_fail(message: &str) -> ! {
    panic!("{message}")
}

/// Call the host's `graphcal::fail` import without diverging type-wise, so
/// the panic hook (which must be able to return as far as the type system
/// is concerned) can share it with [`fail()`].
#[cfg(target_arch = "wasm32")]
fn raw_fail(message: &str) {
    #[expect(unsafe_code, reason = "the ABI's failure channel is a raw wasm import")]
    #[link(wasm_import_module = "graphcal")]
    unsafe extern "C" {
        /// Host-provided failure reporter: records a UTF-8 message and
        /// traps the current call. See `graphcal-plugin-abi`.
        fn fail(ptr: *const u8, len: u32);
    }
    let len = u32::try_from(message.len()).unwrap_or(u32::MAX);
    // SAFETY: the pointer/length pair describes the live UTF-8 buffer of
    // `message`, which outlives the call; the import only reads from it.
    #[expect(unsafe_code, reason = "calling the raw wasm import")]
    unsafe {
        fail(message.as_ptr(), len);
    }
}

/// Support functions the [`plugin!`] expansion calls.
///
/// Not a public API: everything here may change with the macro in any
/// release.
#[doc(hidden)]
pub mod __rt {
    /// Install (once) the panic hook that forwards panic messages
    /// through `graphcal::fail`.
    ///
    /// With the hook, panics in plugin bodies surface as readable
    /// diagnostics. No-op on non-wasm targets, where test panics should
    /// reach the test harness untouched.
    #[expect(
        clippy::missing_const_for_fn,
        reason = "only the non-wasm body is empty; on wasm32 this installs the hook"
    )]
    pub fn install_failure_hook() {
        #[cfg(target_arch = "wasm32")]
        {
            static HOOK: std::sync::Once = std::sync::Once::new();
            HOOK.call_once(|| {
                std::panic::set_hook(Box::new(|info| {
                    // Forwarding traps the call before the abort runtime
                    // runs, so the message wins over the anonymous trap.
                    crate::raw_fail(&info.to_string());
                }));
            });
        }
    }

    /// Convert a raw ABI value into a `Bool` parameter.
    ///
    /// The host sends exactly `1.0` or `0.0`; anything else is a broken
    /// host contract and fails the call rather than being reinterpreted.
    #[must_use]
    pub fn bool_from_abi(raw: f64, param: &str) -> bool {
        if raw.to_bits() == 1.0_f64.to_bits() {
            true
        } else if raw.to_bits() == 0.0_f64.to_bits() {
            false
        } else {
            crate::fail!("parameter `{param}`: expected a Bool encoded as 1.0 or 0.0, got {raw}")
        }
    }

    /// Convert a `Bool` result onto the raw ABI.
    #[must_use]
    pub const fn bool_to_abi(value: bool) -> f64 {
        if value { 1.0 } else { 0.0 }
    }

    /// Convert a raw ABI value into an `Int` parameter.
    ///
    /// The ABI contract requires an exactly-representable integer;
    /// anything else fails the call.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the deliberate f64->i128 truncation is proven exact by the bit round-trip"
    )]
    pub fn int_from_abi(raw: f64, param: &str) -> i64 {
        // NaN and infinities saturate/zero in the cast and then fail the
        // bit round-trip, as do fractional values and out-of-range ones.
        #[expect(
            clippy::cast_precision_loss,
            reason = "round-trip comparison detects any loss"
        )]
        let exact = i64::try_from(raw as i128)
            .ok()
            .filter(|value| (*value as f64).to_bits() == raw.to_bits());
        exact.unwrap_or_else(|| {
            crate::fail!(
                "parameter `{param}`: expected an Int encoded as an exactly-representable \
                 integer, got {raw}"
            )
        })
    }

    /// Convert an `Int` result onto the raw ABI.
    ///
    /// Fails when the value is not exactly representable as an `f64`
    /// (beyond ±2^53 some integers are not, and silently rounding one
    /// would be an implicit conversion).
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "round-trip comparison through i128 detects any loss, including at i64::MAX \
                  where the i64 cast saturates and would false-positive"
    )]
    pub fn int_to_abi(value: i64) -> f64 {
        let raw = value as f64;
        if raw as i128 == i128::from(value) {
            raw
        } else {
            crate::fail!("Int result {value} is not exactly representable as an f64")
        }
    }

    // -- Buffer protocol (arrays over index variables, issue #25 Phase D) --

    /// Alignment of every host-requested buffer allocation. Mirrors the ABI
    /// crate's `BUFFER_ALIGN`; the drift test pins the two together through
    /// the real loader.
    const BUFFER_ALIGN: usize = 8;

    fn buffer_layout(size: u32) -> std::alloc::Layout {
        // `size.max(1)` sidesteps the zero-size allocation edge; the host
        // never requests zero (graphcal indexes are non-empty).
        #[expect(
            clippy::expect_used,
            reason = "the layout is invalid only for sizes near usize::MAX, which cannot \
                      arrive through the 32-bit ABI"
        )]
        std::alloc::Layout::from_size_align(size.max(1) as usize, BUFFER_ALIGN)
            .expect("buffer layout must be valid for 32-bit sizes")
    }

    /// The `graphcal_alloc` export body: allocate one host-requested buffer.
    #[must_use]
    #[expect(
        unsafe_code,
        reason = "the buffer protocol hands raw pointers to the host"
    )]
    pub fn buffer_alloc(size: u32) -> *mut u8 {
        // SAFETY: the layout has non-zero size by construction.
        unsafe { std::alloc::alloc(buffer_layout(size)) }
    }

    /// The `graphcal_free` export body: release one host-requested buffer.
    ///
    /// # Safety
    ///
    /// `ptr` must be exactly a pointer `buffer_alloc(size)` returned during
    /// the same call (the host guarantees this pairing).
    #[expect(
        unsafe_code,
        reason = "the buffer protocol hands raw pointers to the host"
    )]
    pub unsafe fn buffer_free(ptr: *mut u8, size: u32) {
        if ptr.is_null() {
            return;
        }
        // SAFETY: per the ABI, `ptr` came from `buffer_alloc(size)`.
        unsafe { std::alloc::dealloc(ptr, buffer_layout(size)) }
    }

    /// View one host-written array parameter as a slice.
    ///
    /// # Safety
    ///
    /// `ptr` must point at `len` initialized `f64`s that stay alive and
    /// unaliased for the duration of the call — the host guarantees this
    /// for every array parameter it passes.
    #[must_use]
    #[expect(
        unsafe_code,
        reason = "viewing host-written plugin memory is inherently raw"
    )]
    pub const unsafe fn slice_from_abi<'call>(ptr: *const f64, len: u32) -> &'call [f64] {
        // SAFETY: forwarded from the caller.
        unsafe { std::slice::from_raw_parts(ptr, len as usize) }
    }

    /// Write an array result through the host-allocated out-pointer.
    ///
    /// The result length is fixed by the signature (the input array bound to
    /// the result's index variable); a body returning any other length is a
    /// plugin bug reported through `fail` rather than truncated or padded.
    ///
    /// # Safety
    ///
    /// `out` must point at `expected_len` writable `f64` slots (the host
    /// allocates exactly that many for the result buffer).
    #[expect(unsafe_code, reason = "writing through the host-allocated out-pointer")]
    pub unsafe fn write_array_result(
        values: &[f64],
        out: *mut f64,
        expected_len: u32,
        function: &str,
    ) {
        if values.len() != expected_len as usize {
            crate::fail!(
                "{function}: the result array has {} element(s), expected {expected_len} (the \
                 length of the input array bound to the result's index variable)",
                values.len()
            );
        }
        // SAFETY: the host allocated `expected_len` f64 slots at `out`, and
        // the length was checked above.
        unsafe {
            std::ptr::copy_nonoverlapping(values.as_ptr(), out, values.len());
        }
    }
}
