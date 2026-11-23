//! Static SVG rendering of Vega-Lite figures (`--plot FILE.svg`).
//!
//! The same vendored Vega and Vega-Lite bundles that the HTML outputs embed
//! run here inside an embedded `QuickJS` engine, headless: Vega-Lite compiles
//! the spec and Vega's string renderer produces the SVG, so the image matches
//! the browser outputs without a browser, Node.js, or a network connection.
//! Rendering is pure computation; writing the file is the caller's job.

use graphcal_report::vega_assets::{VEGA_JS, VEGA_LITE_JS};
use rquickjs::{CatchResultExt as _, Context, Function, Object, Promise, Runtime};
use serde_json::Value as JsonValue;
use thiserror::Error;

/// Host shims the browser bundles expect, and the render entry point.
///
/// `QuickJS` provides the ECMAScript core only. The bundles look up
/// `structuredClone` while loading; vega-lite applies it to JSON specs, so a
/// JSON round-trip is equivalent. Console output is collected so that
/// Vega-Lite warnings reach the caller instead of being dropped.
const PRELUDE: &str = r"
globalThis.self = globalThis;
globalThis.structuredClone = (value) => JSON.parse(JSON.stringify(value));
globalThis.__graphcalWarnings = [];
globalThis.console = {
  log() {},
  info() {},
  debug() {},
  warn(...args) { globalThis.__graphcalWarnings.push(args.join(' ')); },
  error(...args) { globalThis.__graphcalWarnings.push(args.join(' ')); },
};
";

const RENDER: &str = r"
globalThis.__graphcalRender = (specJson) => {
  globalThis.__graphcalWarnings = [];
  const vegaSpec = vegaLite.compile(JSON.parse(specJson)).spec;
  const view = new vega.View(vega.parse(vegaSpec), { renderer: 'none' });
  return view.toSVG().finally(() => view.finalize());
};
";

/// A rendered SVG document and the warnings Vega reported while rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedSvg {
    /// The standalone SVG document.
    pub svg: String,
    /// Warnings from Vega-Lite compilation or Vega rendering.
    pub warnings: Vec<String>,
}

/// Rendering failed inside the embedded JavaScript engine.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SvgRenderError {
    /// The engine or the vendored bundles could not be set up.
    #[error("could not start the SVG renderer: {0}")]
    Setup(String),
    /// Vega-Lite or Vega rejected the figure.
    #[error("could not render the figure as SVG: {0}")]
    Render(String),
}

/// A headless Vega renderer with the bundles loaded once.
pub struct SvgRenderer {
    // The context keeps the runtime alive; the field order drops the context
    // first, as rquickjs requires.
    context: Context,
    _runtime: Runtime,
}

impl SvgRenderer {
    /// Start the engine and load the vendored Vega and Vega-Lite bundles.
    ///
    /// # Errors
    ///
    /// Returns [`SvgRenderError::Setup`] when the engine cannot start or a
    /// bundle fails to load.
    pub fn new() -> Result<Self, SvgRenderError> {
        let setup = |error: &dyn std::fmt::Display| SvgRenderError::Setup(error.to_string());
        let runtime = Runtime::new().map_err(|e| setup(&e))?;
        let context = Context::full(&runtime).map_err(|e| setup(&e))?;
        context.with(|ctx| {
            [PRELUDE, VEGA_JS, VEGA_LITE_JS, RENDER]
                .into_iter()
                .try_for_each(|script| ctx.eval::<(), _>(script).catch(&ctx).map_err(|e| setup(&e)))
        })?;
        Ok(Self {
            context,
            _runtime: runtime,
        })
    }

    /// Render one Vega-Lite spec as a standalone SVG document.
    ///
    /// # Errors
    ///
    /// Returns [`SvgRenderError::Render`] when Vega-Lite or Vega rejects the
    /// spec.
    pub fn render(&self, spec: &JsonValue) -> Result<RenderedSvg, SvgRenderError> {
        let render_error =
            |error: &dyn std::fmt::Display| SvgRenderError::Render(error.to_string());
        self.context.with(|ctx| {
            let render: Function = ctx
                .globals()
                .get("__graphcalRender")
                .map_err(|e| render_error(&e))?;
            let svg = render
                .call::<_, Promise>((spec.to_string(),))
                .and_then(|promise| promise.finish::<String>())
                .catch(&ctx)
                .map_err(|e| render_error(&e))?;
            let warnings = ctx
                .globals()
                .get::<_, Object>("__graphcalWarnings")
                .and_then(|warnings| {
                    warnings
                        .into_array()
                        .ok_or_else(|| rquickjs::Error::new_from_js("object", "array"))
                })
                .and_then(|warnings| warnings.iter::<String>().collect())
                .map_err(|e| render_error(&e))?;
            Ok(RenderedSvg { svg, warnings })
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn renderer() -> SvgRenderer {
        SvgRenderer::new().unwrap()
    }

    #[test]
    fn renders_a_standalone_svg_with_title_and_axis_titles() {
        let rendered = renderer()
            .render(&json!({
                "title": "Thrust curve",
                "data": { "values": [{ "t": 0.0, "f": 1.0 }, { "t": 1.0, "f": 2.0 }] },
                "mark": "line",
                "encoding": {
                    "x": { "field": "t", "type": "quantitative", "title": "Time [s]" },
                    "y": { "field": "f", "type": "quantitative", "title": "Force [kN]" },
                },
            }))
            .unwrap();
        assert!(
            rendered
                .svg
                .starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\"")
        );
        assert!(rendered.svg.trim_end().ends_with("</svg>"));
        for text in ["Thrust curve", "Time [s]", "Force [kN]"] {
            assert!(
                rendered.svg.contains(text),
                "missing `{text}` in {}",
                rendered.svg
            );
        }
        assert!(!rendered.svg.contains("<script"));
        assert_eq!(rendered.warnings, Vec::<String>::new());
    }

    #[test]
    fn one_renderer_renders_several_figures() {
        let shared = renderer();
        for title in ["first", "second"] {
            let rendered = shared
                .render(&json!({
                    "title": title,
                    "data": { "values": [{ "x": 1.0 }] },
                    "mark": "point",
                    "encoding": { "x": { "field": "x", "type": "quantitative" } },
                }))
                .unwrap();
            assert!(rendered.svg.contains(title));
        }
    }

    #[test]
    fn an_invalid_spec_is_a_render_error() {
        let error = renderer().render(&json!({ "mark": 42 })).unwrap_err();
        assert!(matches!(error, SvgRenderError::Render(_)), "{error:?}");
    }

    #[test]
    fn vega_lite_warnings_are_reported() {
        let rendered = renderer()
            .render(&json!({
                "data": { "values": [{ "x": 1.0 }] },
                "mark": "bar",
                "encoding": {
                    "x": { "field": "x", "type": "quantitative" },
                    "shape": { "field": "x", "type": "nominal" },
                },
            }))
            .unwrap();
        assert!(!rendered.warnings.is_empty(), "{rendered:?}");
    }
}
