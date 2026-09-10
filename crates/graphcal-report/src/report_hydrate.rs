//! Hydration payload for interactive reports.
//!
//! A hydrated report embeds the browser engine (the `graphcal-wasm` bundle
//! built with `wasm-pack --target no-modules`), the project sources, and the
//! baseline binding expressions into the static page. The runtime script
//! (`report_standalone.js`) adapts that engine to the transport-neutral UI
//! runtime (`report_runtime.js`), which synthesizes controls from typed
//! parameter ports and re-evaluates on input — the same evaluator, results,
//! and unit checking as the CLI, end to end.

use base64::Engine as _;
use graphcal_eval::project_bundle::ProjectBundle;
use serde_json::json;

use crate::escape::escape_json_for_script;

/// The report runtime script embedded into hydrated pages.
const REPORT_RUNTIME_JS: &str = include_str!("report_runtime.js");
/// The standalone embedded-Wasm transport bootstrap.
const REPORT_STANDALONE_JS: &str = include_str!("report_standalone.js");

/// The browser engine bundle produced by `wasm-pack --target no-modules`.
pub struct EngineBundle<'a> {
    /// The wasm-bindgen "no-modules" JS glue (defines the global
    /// `wasm_bindgen`), run inside the worker.
    pub glue_js: &'a str,
    /// The raw `.wasm` module bytes.
    pub wasm: &'a [u8],
}

/// Everything the hydration layer embeds into the page.
pub struct Hydration<'a> {
    pub engine: EngineBundle<'a>,
    pub project: ProjectBundle,
    /// Baseline binding expressions from build-time `--param` arguments,
    /// replayed as the initial reader-visible values.
    pub baseline_bindings: Vec<(String, String)>,
}

/// Render the payload block appended to the page body: project request,
/// baseline bindings, base64 engine, and the runtime script.
///
/// Base64 payloads contain no `<`, so they cannot terminate their script
/// elements; JSON payloads are escaped for script embedding.
#[must_use]
pub(crate) fn render_hydration_block(hydration: &Hydration<'_>) -> String {
    let project = json!(hydration.project);
    let baseline = json!(
        hydration
            .baseline_bindings
            .iter()
            .map(|(name, expr)| json!({ "name": name, "expr": expr }))
            .collect::<Vec<_>>()
    );
    let engine = base64::engine::general_purpose::STANDARD;
    format!(
        concat!(
            "<script id=\"graphcal-project\" type=\"application/json\">{project}</script>\n",
            "<script id=\"graphcal-baseline\" type=\"application/json\">{baseline}</script>\n",
            "<script id=\"graphcal-engine-glue\" type=\"text/plain\">{glue}</script>\n",
            "<script id=\"graphcal-engine-wasm\" type=\"application/wasm;base64\">{wasm}</script>\n",
            "<script>{runtime}</script>\n",
            "<script>{standalone}</script>\n",
        ),
        project = escape_json_for_script(&project.to_string()),
        baseline = escape_json_for_script(&baseline.to_string()),
        glue = engine.encode(hydration.engine.glue_js),
        wasm = engine.encode(hydration.engine.wasm),
        runtime = REPORT_RUNTIME_JS,
        standalone = REPORT_STANDALONE_JS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_escapes_script_closing_sequences_in_sources() {
        let hydration = Hydration {
            engine: EngineBundle {
                glue_js: "var wasm_bindgen;",
                wasm: b"\0asm",
            },
            project: ProjectBundle {
                entry: "main.gcl".to_string().try_into().unwrap(),
                files: vec![graphcal_eval::project_bundle::BundleArtifact {
                    path: "main.gcl".to_string().try_into().unwrap(),
                    content: graphcal_eval::project_bundle::ArtifactContent::Source(
                        "// </script><script>alert(1)</script>\nparam x: Dimensionless = 1.0;"
                            .to_string(),
                    ),
                }],
            },
            baseline_bindings: vec![("x".to_string(), "2.0".to_string())],
        };
        let block = render_hydration_block(&hydration);
        assert!(!block.contains("</script><script>alert(1)"));
        assert!(block.contains("graphcal-project"));
        assert!(block.contains("graphcal-engine-wasm"));
    }

    #[test]
    fn runtime_script_is_safe_to_inline() {
        assert!(
            !REPORT_RUNTIME_JS.contains("</script") && !REPORT_RUNTIME_JS.contains("<!--"),
            "the runtime script must not contain inline-script terminators"
        );
        assert!(
            !REPORT_STANDALONE_JS.contains("</script") && !REPORT_STANDALONE_JS.contains("<!--"),
            "the standalone bootstrap must not contain inline-script terminators"
        );
    }

    #[test]
    fn runtime_and_standalone_bootstrap_have_separate_responsibilities() {
        assert!(REPORT_RUNTIME_JS.contains("global.GraphcalReport = { mount: mount }"));
        assert!(REPORT_RUNTIME_JS.contains("options.createTransport"));
        assert!(!REPORT_RUNTIME_JS.contains("graphcal-engine-wasm"));
        assert!(!REPORT_RUNTIME_JS.contains("wasm_bindgen"));
        assert!(REPORT_STANDALONE_JS.contains("graphcal-engine-wasm"));
        assert!(REPORT_STANDALONE_JS.contains("window.GraphcalReport.mount"));
    }
}
