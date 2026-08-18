//! Vendored Vega browser bundles for self-contained HTML output.
//!
//! The bundles live in `assets/` (see `assets/README.md` for provenance,
//! licenses, and the update procedure) and are embedded at compile time so
//! generated pages work offline and from `file://` paths.

/// Pinned version of [`VEGA_JS`].
pub const VEGA_VERSION: &str = "5.33.1";
/// Pinned version of [`VEGA_LITE_JS`].
pub const VEGA_LITE_VERSION: &str = "5.23.0";
/// Pinned version of [`VEGA_EMBED_JS`].
pub const VEGA_EMBED_VERSION: &str = "6.29.0";

/// Minified `vega` browser bundle (BSD-3-Clause).
pub const VEGA_JS: &str = include_str!("../assets/vega.min.js");
/// Minified `vega-lite` browser bundle (BSD-3-Clause).
pub const VEGA_LITE_JS: &str = include_str!("../assets/vega-lite.min.js");
/// Minified `vega-embed` browser bundle (BSD-3-Clause).
pub const VEGA_EMBED_JS: &str = include_str!("../assets/vega-embed.min.js");

#[cfg(test)]
mod tests {
    use super::*;

    /// Inlining a bundle inside a `<script>` element is only sound when the
    /// bundle itself never contains a script-closing or comment-opening
    /// sequence.
    #[test]
    fn bundles_are_safe_to_inline_in_script_elements() {
        for (name, bundle) in [
            ("vega", VEGA_JS),
            ("vega-lite", VEGA_LITE_JS),
            ("vega-embed", VEGA_EMBED_JS),
        ] {
            assert!(
                !bundle.contains("</script") && !bundle.contains("<!--"),
                "{name} bundle contains a sequence unsafe for inline <script> embedding"
            );
        }
    }
}
