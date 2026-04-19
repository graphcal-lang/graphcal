use graphcal_compiler::syntax::ast::{EncodingChannel, MarkType};
use graphcal_eval::eval::{
    AxisMeta, CompositionProperty, FigureSpec, LayerSpec, PlotFieldValue, PlotProperty, PlotSpec,
};
use serde_json::{Value as JsonValue, json};

/// A rendered figure ready for output.
pub struct RenderedFigure {
    /// The figure name (used for JSON output and HTML div IDs).
    pub name: String,
    /// The Vega-Lite spec as a JSON value.
    pub spec: JsonValue,
}

/// Build figures from evaluated plot, figure, and layer specs.
///
/// - Each `pub` `PlotSpec` produces one standalone figure.
/// - Each `FigureSpec` produces one combined figure with `hconcat`.
/// - Each `LayerSpec` produces one combined figure with `layer`.
pub fn build_figures(
    plots: &[PlotSpec],
    figures: &[FigureSpec],
    layers: &[LayerSpec],
) -> Vec<RenderedFigure> {
    let mut result = Vec::new();

    // Standalone figures from pub plots (non-pub plots are only usable in figures/layers)
    for spec in plots {
        if !spec.is_pub {
            continue;
        }
        result.push(RenderedFigure {
            name: spec.name.as_str().to_string(),
            spec: build_single_spec(spec),
        });
    }

    // Combined figures from figure specs
    for fig in figures {
        result.push(RenderedFigure {
            name: fig.name.as_str().to_string(),
            spec: build_figure_spec(fig, plots),
        });
    }

    // Layered figures from layer specs
    for layer in layers {
        result.push(RenderedFigure {
            name: layer.name.as_str().to_string(),
            spec: build_layer_spec(layer, plots),
        });
    }

    result
}

/// Build a Vega-Lite spec from one `PlotSpec`.
fn build_single_spec(spec: &PlotSpec) -> JsonValue {
    let mut vl = json!({
        "$schema": "https://vega.github.io/schema/vega-lite/v5.json",
    });

    // Data
    let data_values = build_data_values(spec);
    vl["data"] = json!({ "values": data_values });

    // Mark
    vl["mark"] = build_mark(spec);

    // Encoding
    vl["encoding"] = build_encoding(spec);

    // Title
    if let Some(title) = get_string_property(&spec.properties, &PlotProperty::Title) {
        vl["title"] = json!(title);
    }

    // Width/height
    if let Some(w) = get_number_property(&spec.properties, &PlotProperty::Width) {
        vl["width"] = json!(w);
    }
    if let Some(h) = get_number_property(&spec.properties, &PlotProperty::Height) {
        vl["height"] = json!(h);
    }

    vl
}

/// Build a Vega-Lite `hconcat` spec from a `FigureSpec`.
fn build_figure_spec(fig: &FigureSpec, all_plots: &[PlotSpec]) -> JsonValue {
    let referenced: Vec<&PlotSpec> = fig
        .plot_names
        .iter()
        .filter_map(|name| all_plots.iter().find(|p| p.name == *name))
        .collect();

    let sub_specs: Vec<JsonValue> = referenced
        .iter()
        .map(|spec| build_single_spec(spec))
        .collect();

    let mut vl = json!({
        "$schema": "https://vega.github.io/schema/vega-lite/v5.json",
        "hconcat": sub_specs,
    });

    if let Some(title) = get_string_property(&fig.properties, &CompositionProperty::Title) {
        vl["title"] = json!(title);
    }

    vl
}

/// Build a Vega-Lite `layer` spec from a `LayerSpec`.
fn build_layer_spec(layer: &LayerSpec, all_plots: &[PlotSpec]) -> JsonValue {
    let referenced: Vec<&PlotSpec> = layer
        .plot_names
        .iter()
        .filter_map(|name| all_plots.iter().find(|p| p.name == *name))
        .collect();

    // Each sub-spec is a layer entry: mark + encoding + data (no $schema).
    let sub_specs: Vec<JsonValue> = referenced
        .iter()
        .map(|spec| {
            let mut entry = json!({});
            entry["data"] = json!({ "values": build_data_values(spec) });
            entry["mark"] = build_mark(spec);
            entry["encoding"] = build_encoding(spec);
            entry
        })
        .collect();

    let mut vl = json!({
        "$schema": "https://vega.github.io/schema/vega-lite/v5.json",
        "layer": sub_specs,
    });

    if let Some(title) = get_string_property(&layer.properties, &CompositionProperty::Title) {
        vl["title"] = json!(title);
    }

    // Width/height from layer properties
    if let Some(w) = get_number_property(&layer.properties, &CompositionProperty::Width) {
        vl["width"] = json!(w);
    }
    if let Some(h) = get_number_property(&layer.properties, &CompositionProperty::Height) {
        vl["height"] = json!(h);
    }

    vl
}

/// Build the `"data": { "values": [...] }` array from a plot spec's encoding channels.
///
/// Converts column-oriented encoding data (`x: [1,2,3], y: [4,5,6]`) into
/// row-oriented records (`[{x:1, y:4}, {x:2, y:5}, {x:3, y:6}]`).
fn build_data_values(spec: &PlotSpec) -> Vec<JsonValue> {
    let mut channel_data: Vec<(&str, Vec<JsonValue>)> = Vec::new();
    let mut max_len = 0;

    for (channel, value) in &spec.encodings {
        let json_values = field_value_to_json_array(value);
        if json_values.len() > max_len {
            max_len = json_values.len();
        }
        channel_data.push((channel_vega_name(*channel), json_values));
    }

    // Build row-oriented records
    let mut rows = Vec::with_capacity(max_len);
    for i in 0..max_len {
        let mut row = serde_json::Map::new();
        for &(ch, ref values) in &channel_data {
            if let Some(v) = values.get(i) {
                row.insert(ch.to_string(), v.clone());
            }
        }
        rows.push(JsonValue::Object(row));
    }
    rows
}

/// Build the Vega-Lite `"mark"` field.
fn build_mark(spec: &PlotSpec) -> JsonValue {
    let mark_type_str = match spec.mark_type {
        MarkType::Point => "point",
        MarkType::Line => "line",
        MarkType::Bar => "bar",
        MarkType::Area => "area",
        MarkType::Rect => "rect",
        MarkType::Tick => "tick",
    };

    if spec.mark_properties.is_empty() {
        return json!(mark_type_str);
    }

    let mut mark_obj = serde_json::Map::new();
    mark_obj.insert("type".to_string(), json!(mark_type_str));

    for (prop, value) in &spec.mark_properties {
        let json_val = match value {
            PlotFieldValue::Number(n) => json!(n),
            PlotFieldValue::String(s) => json!(s),
            PlotFieldValue::Numbers(nums) if nums.len() == 1 => json!(nums[0]),
            _ => continue,
        };
        mark_obj.insert(prop.vega_name().to_string(), json_val);
    }

    JsonValue::Object(mark_obj)
}

/// Build the Vega-Lite `"encoding"` field.
fn build_encoding(spec: &PlotSpec) -> JsonValue {
    let mut encoding = serde_json::Map::new();

    for (channel, value) in &spec.encodings {
        let ch_name = channel_vega_name(*channel);
        let vega_type = infer_vega_type(value);
        let mut ch_spec = serde_json::Map::new();
        ch_spec.insert("field".to_string(), json!(ch_name));
        ch_spec.insert("type".to_string(), json!(vega_type));

        // Axis title: explicit x_label/y_label overrides auto-generated titles
        let explicit_label = match channel {
            EncodingChannel::X => get_string_property(&spec.properties, &PlotProperty::XLabel),
            EncodingChannel::Y => get_string_property(&spec.properties, &PlotProperty::YLabel),
            _ => None,
        };
        let axis_title = explicit_label.or_else(|| {
            let meta = get_encoding_meta(spec, *channel)?;
            format_axis_title(meta)
        });
        if let Some(title) = axis_title {
            ch_spec.insert("axis".to_string(), json!({ "title": title }));
        }

        encoding.insert(ch_name.to_string(), JsonValue::Object(ch_spec));
    }

    JsonValue::Object(encoding)
}

/// The Vega-Lite field name for an encoding channel.
const fn channel_vega_name(channel: EncodingChannel) -> &'static str {
    match channel {
        EncodingChannel::X => "x",
        EncodingChannel::Y => "y",
        EncodingChannel::Color => "color",
        EncodingChannel::Size => "size",
        EncodingChannel::Shape => "shape",
        EncodingChannel::Opacity => "opacity",
        EncodingChannel::Detail => "detail",
        EncodingChannel::Text => "text",
        EncodingChannel::Tooltip => "tooltip",
    }
}

/// Look up axis metadata for an encoding channel.
fn get_encoding_meta(spec: &PlotSpec, channel: EncodingChannel) -> Option<&AxisMeta> {
    spec.encoding_meta
        .iter()
        .find(|(ch, _)| *ch == channel)
        .map(|(_, meta)| meta)
}

/// Format an axis title from dimension and unit metadata.
///
/// - Dimension "Velocity" + unit "km/s" -> "Velocity (km/s)"
/// - Dimension "Velocity" alone -> "Velocity"
/// - Unit "km/s" alone -> None (unit without dimension isn't meaningful as title)
/// - Neither -> None
fn format_axis_title(meta: &AxisMeta) -> Option<String> {
    match (&meta.dimension_label, &meta.unit_label) {
        (Some(dim), Some(unit)) => Some(format!("{dim} ({unit})")),
        (Some(dim), None) => Some(dim.clone()),
        _ => None,
    }
}

/// Infer Vega-Lite data type from a field value.
const fn infer_vega_type(value: &PlotFieldValue) -> &'static str {
    match value {
        PlotFieldValue::Numbers(_) | PlotFieldValue::Number(_) => "quantitative",
        PlotFieldValue::Labels(_) | PlotFieldValue::String(_) => "nominal",
    }
}

/// Convert a `PlotFieldValue` to a JSON array for data values.
fn field_value_to_json_array(value: &PlotFieldValue) -> Vec<JsonValue> {
    match value {
        PlotFieldValue::Numbers(nums) => nums.iter().copied().map(json_number).collect(),
        PlotFieldValue::Labels(labels) => labels.iter().map(|s| json!(s)).collect(),
        PlotFieldValue::Number(n) => vec![json_number(*n)],
        PlotFieldValue::String(s) => vec![json!(s)],
    }
}

/// Convert an f64 to a JSON number, using integer representation when possible.
fn json_number(n: f64) -> JsonValue {
    #[expect(clippy::cast_possible_truncation, reason = "intentional integer check")]
    if n.fract() == 0.0 && n.abs() < f64::from(i32::MAX) {
        json!(n as i64)
    } else {
        json!(n)
    }
}

/// Look up a property by key and return the associated string value.
fn get_string_property<P: PartialEq>(
    properties: &[(P, PlotFieldValue)],
    prop: &P,
) -> Option<String> {
    properties
        .iter()
        .find(|(p, _)| p == prop)
        .and_then(|(_, v)| match v {
            PlotFieldValue::String(s) => Some(s.clone()),
            _ => None,
        })
}

/// Look up a property by key and return a single numeric value.
///
/// Accepts both `Number(n)` and a single-element `Numbers([n])`.
fn get_number_property<P: PartialEq>(properties: &[(P, PlotFieldValue)], prop: &P) -> Option<f64> {
    properties
        .iter()
        .find(|(p, _)| p == prop)
        .and_then(|(_, v)| match v {
            PlotFieldValue::Number(n) => Some(*n),
            PlotFieldValue::Numbers(nums) if nums.len() == 1 => Some(nums[0]),
            _ => None,
        })
}

/// HTML-escape a string to prevent XSS when interpolated into HTML content.
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Escape a JSON string for safe embedding inside an HTML `<script>` element.
///
/// `serde_json` does not escape `</`, so a user-controlled string containing
/// `</script>` would close the script tag. Replace `<` with `\u003c` to neutralize
/// any `</script>` or `<!--` sequences in the JSON payload.
fn escape_json_for_script(s: &str) -> String {
    s.replace('<', r"\u003c")
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
    Ok(format!(
        r#"<!DOCTYPE html>
<html>
<head>
  <meta charset="utf-8">
  <title>Graphcal Plots</title>
  <script src="https://cdn.jsdelivr.net/npm/vega@5"></script>
  <script src="https://cdn.jsdelivr.net/npm/vega-lite@5"></script>
  <script src="https://cdn.jsdelivr.net/npm/vega-embed@6"></script>
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
pub fn render_json(figures: &[RenderedFigure]) -> String {
    let entries: Vec<JsonValue> = figures
        .iter()
        .map(|fig| {
            json!({
                "name": fig.name,
                "spec": fig.spec,
            })
        })
        .collect();
    serde_json::to_string_pretty(&entries).unwrap_or_else(|_| "[]".to_string())
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "test code"
    )]

    use super::*;

    #[test]
    fn script_close_sequence_in_title_is_escaped() {
        let rendered = vec![RenderedFigure {
            name: "legitimate title".to_string(),
            spec: json!({"title": "</script><script>alert(1)</script>"}),
        }];
        let html = render_html(&rendered).unwrap();
        // The raw `</script>` from the JSON payload must not appear verbatim
        // inside the emitted `<script>` block; the `<` must be escaped to `\u003c`.
        let script_block = html
            .split("vegaEmbed")
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
    fn html_escape_handles_critical_characters() {
        assert_eq!(
            html_escape("<img src=x onerror=alert(1)>"),
            "&lt;img src=x onerror=alert(1)&gt;"
        );
        assert_eq!(html_escape("\"'&"), "&quot;&#x27;&amp;");
    }
}
