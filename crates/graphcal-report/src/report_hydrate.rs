//! Hydration payload for interactive reports.
//!
//! A hydrated report embeds the browser engine (the `graphcal-wasm` bundle
//! built with `wasm-pack --target no-modules`), the project sources, and the
//! baseline binding expressions into the static page. The runtime script
//! (`report_standalone.js`) adapts that engine to the transport-neutral UI
//! runtime (`report_runtime.js`), which synthesizes controls from typed
//! parameter ports and atomically applies complete drafts — the same evaluator,
//! results, and unit checking as the CLI, end to end.

use base64::Engine as _;
use graphcal_eval::project_bundle::ProjectBundle;
use serde_json::json;

use crate::escape::escape_json_for_script;

/// Pure recursive form-state transitions embedded into hydrated pages.
const REPORT_FORM_STATE_JS: &str = include_str!("report_form_state.js");
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
    engine: EngineBundle<'a>,
    project_json: String,
    /// Baseline binding expressions from build-time `--param` arguments,
    /// replayed as the initial reader-visible values.
    baseline_bindings: Vec<(String, String)>,
}

impl<'a> Hydration<'a> {
    /// Freeze the final HTML-escaped wire payload. Escaping can expand source
    /// bytes, so the browser's envelope limit must be checked after this step.
    ///
    /// # Errors
    /// Returns a bundle error when serialization or the encoded size limit fails.
    pub fn new(
        engine: EngineBundle<'a>,
        project: &ProjectBundle,
        baseline_bindings: Vec<(String, String)>,
    ) -> Result<Self, graphcal_eval::project_bundle::BundleError> {
        let project_json = escape_json_for_script(&project.to_json()?);
        ProjectBundle::check_json_size(project_json.len())?;
        Ok(Self {
            engine,
            project_json,
            baseline_bindings,
        })
    }
}

/// Render the payload block appended to the page body: project request,
/// baseline bindings, base64 engine, and the runtime script.
///
/// Base64 payloads contain no `<`, so they cannot terminate their script
/// elements; JSON payloads are escaped for script embedding.
#[must_use]
pub(crate) fn render_hydration_block(hydration: &Hydration<'_>) -> String {
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
            "<script>{form_state}</script>\n",
            "<script>{runtime}</script>\n",
            "<script>{standalone}</script>\n",
        ),
        project = hydration.project_json,
        baseline = escape_json_for_script(&baseline.to_string()),
        glue = engine.encode(hydration.engine.glue_js),
        wasm = engine.encode(hydration.engine.wasm),
        form_state = REPORT_FORM_STATE_JS,
        runtime = REPORT_RUNTIME_JS,
        standalone = REPORT_STANDALONE_JS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_escapes_script_closing_sequences_in_sources() {
        let hydration = Hydration::new(
            EngineBundle {
                glue_js: "var wasm_bindgen;",
                wasm: b"\0asm",
            },
            &ProjectBundle {
                dependencies: vec![],
                entry: "main.gcl".to_string().try_into().unwrap(),
                files: vec![graphcal_eval::project_bundle::BundleArtifact {
                    path: "main.gcl".to_string().try_into().unwrap(),
                    content: graphcal_eval::project_bundle::ArtifactContent::Source(
                        "// </script><script>alert(1)</script>\nparam x: Dimensionless = 1.0;"
                            .to_string(),
                    ),
                }],
            },
            vec![("x".to_string(), "2.0".to_string())],
        )
        .unwrap();
        let block = render_hydration_block(&hydration);
        assert!(!block.contains("</script><script>alert(1)"));
        assert!(block.contains("graphcal-project"));
        assert!(block.contains("graphcal-engine-wasm"));
    }

    #[test]
    fn html_escaping_cannot_exceed_the_browser_envelope_budget() {
        use graphcal_eval::project_bundle::{
            ArtifactContent, BundleArtifact, BundleError, MAX_BUNDLE_JSON_BYTES,
        };
        let project = ProjectBundle {
            entry: "main.gcl".to_string().try_into().unwrap(),
            files: vec![BundleArtifact {
                path: "main.gcl".to_string().try_into().unwrap(),
                content: ArtifactContent::Source("<".repeat(MAX_BUNDLE_JSON_BYTES / 6)),
            }],
            dependencies: vec![],
        };
        assert!(matches!(
            Hydration::new(
                EngineBundle {
                    glue_js: "",
                    wasm: &[]
                },
                &project,
                vec![]
            ),
            Err(BundleError::TotalSize)
        ));
    }

    #[test]
    fn runtime_script_is_safe_to_inline() {
        assert!(
            !REPORT_FORM_STATE_JS.contains("</script")
                && !REPORT_FORM_STATE_JS.contains("<!--")
                && !REPORT_RUNTIME_JS.contains("</script")
                && !REPORT_RUNTIME_JS.contains("<!--"),
            "the form state and runtime scripts must not contain inline-script terminators"
        );
        assert!(
            !REPORT_STANDALONE_JS.contains("</script") && !REPORT_STANDALONE_JS.contains("<!--"),
            "the standalone bootstrap must not contain inline-script terminators"
        );
    }

    #[test]
    fn runtime_and_standalone_bootstrap_have_separate_responsibilities() {
        assert!(REPORT_FORM_STATE_JS.contains("global.GraphcalReportFormState"));
        assert!(REPORT_RUNTIME_JS.contains("global.GraphcalReport = { mount: mount }"));
        assert!(REPORT_RUNTIME_JS.contains("options.createTransport"));
        assert!(!REPORT_RUNTIME_JS.contains("graphcal-engine-wasm"));
        assert!(!REPORT_RUNTIME_JS.contains("wasm_bindgen"));
        assert!(REPORT_STANDALONE_JS.contains("graphcal-engine-wasm"));
        assert!(REPORT_STANDALONE_JS.contains("window.GraphcalReport.mount"));
    }
}
