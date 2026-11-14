//! Standalone HTML/JSON output for evaluated plot figures.

use serde_json::{Value as JsonValue, json};

use crate::escape::{escape_json_for_script, html_escape};
use crate::vega::RenderedFigure;
use crate::vega_assets;

/// The `<head>` script elements embedding the vendored Vega stack, so the
/// page is self-contained and works offline and from `file://` paths.
pub(crate) fn vega_script_tags() -> String {
    format!(
        "<!-- vega {} / vega-lite {} / vega-embed {} — BSD-3-Clause, (c) University of \
         Washington Interactive Data Lab and contributors -->\n  \
         <script>{}</script>\n  <script>{}</script>\n  <script>{}</script>",
        vega_assets::VEGA_VERSION,
        vega_assets::VEGA_LITE_VERSION,
        vega_assets::VEGA_EMBED_VERSION,
        vega_assets::VEGA_JS,
        vega_assets::VEGA_LITE_JS,
        vega_assets::VEGA_EMBED_JS,
    )
}

/// Render all figures as a single HTML page using Vega-Embed.
///
/// # Errors
///
/// Returns an error if any figure's spec cannot be serialized to JSON.
pub fn render_html(figures: &[RenderedFigure]) -> Result<String, serde_json::Error> {
    use std::fmt::Write;
    let mut divs = String::new();
    for (i, fig) in figures.iter().enumerate() {
        let div_id = format!("graphcal-plot-{i}");
        let spec_json = escape_json_for_script(&serde_json::to_string(&fig.spec)?);
        let escaped_name = html_escape(&fig.name);
        let _ = write!(
            divs,
            r#"<div style="margin-bottom: 2em;">
<h3>{escaped_name}</h3>
<div id="{div_id}"></div>
<script>vegaEmbed('#{div_id}', {spec_json}).catch(console.error);</script>
</div>
"#,
        );
    }
    let script_tags = vega_script_tags();
    Ok(format!(
        r#"<!DOCTYPE html>
<html>
<head>
  <meta charset="utf-8">
  <title>Graphcal Plots</title>
  {script_tags}
  <style>
    body {{ font-family: system-ui, -apple-system, sans-serif; max-width: 1200px; margin: 0 auto; padding: 20px; }}
    h3 {{ color: #333; }}
  </style>
</head>
<body>
{divs}
</body>
</html>"#
    ))
}

/// Render all figures as a JSON array of `{{ "name": "...", "spec": {{...}} }}`.
///
/// # Errors
///
/// Returns an error if JSON serialization fails. Previously this was masked by
/// `unwrap_or_else(|_| "[]".to_string())`, which produced a confusing empty
/// result with no signal to the user.
pub fn render_json(figures: &[RenderedFigure]) -> Result<String, serde_json::Error> {
    let entries: Vec<JsonValue> = figures
        .iter()
        .map(|fig| {
            json!({
                "name": fig.name,
                "spec": fig.spec,
            })
        })
        .collect();
    serde_json::to_string_pretty(&entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_close_sequence_in_title_is_escaped() {
        let rendered = vec![RenderedFigure {
            doc: None,
            name: "legitimate title".to_string(),
            spec: json!({"title": "</script><script>alert(1)</script>"}),
        }];
        let html = render_html(&rendered).unwrap();
        // The raw `</script>` from the JSON payload must not appear verbatim
        // inside the emitted `<script>` block; the `<` must be escaped to `\u003c`.
        // The vendored Vega bundles also mention `vegaEmbed`; the figure's
        // own call is the one with a `#graphcal-plot-` target.
        let script_block = html
            .split("vegaEmbed('#graphcal-plot-")
            .nth(1)
            .expect("expected a vegaEmbed script block");
        assert!(
            !script_block.contains("</script><script>alert(1)"),
            "unescaped </script> sequence leaked into the script block: {script_block}"
        );
        assert!(
            script_block.contains(r"\u003c/script>\u003cscript>alert(1)"),
            "expected `<` to be escaped as `\\u003c` in emitted script block: {script_block}"
        );
    }

    #[test]
    fn inline_page_embeds_vega_and_references_no_cdn() {
        let rendered = vec![RenderedFigure {
            doc: None,
            name: "p".to_string(),
            spec: json!({"mark": "line"}),
        }];
        let html = render_html(&rendered).unwrap();
        assert!(!html.contains("cdn.jsdelivr.net"));
        assert!(html.contains(vega_assets::VEGA_EMBED_JS));
    }
}
