use graphcal_compiler::syntax::ast::MarkType;
use graphcal_eval::eval::{AxisMeta, FigureSpec, LayerSpec, PlotFieldValue, PlotSpec};
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
/// - Each non-hidden `PlotSpec` produces one standalone figure.
/// - Each `FigureSpec` produces one combined figure with `hconcat`.
/// - Each `LayerSpec` produces one combined figure with `layer`.
pub fn build_figures(
    plots: &[PlotSpec],
    figures: &[FigureSpec],
    layers: &[LayerSpec],
) -> Vec<RenderedFigure> {
    let mut result = Vec::new();

    // Standalone figures from non-hidden plots
    for spec in plots {
        if spec.hidden {
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
    if let Some(title) = get_string_field(spec, "title") {
        vl["title"] = json!(title);
    }

    // Width/height
    if let Some(w) = get_number_field_single(spec, "width") {
        vl["width"] = json!(w);
    }
    if let Some(h) = get_number_field_single(spec, "height") {
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

    if let Some(title) = get_string_field_from_fields(&fig.fields, "title") {
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

    if let Some(title) = get_string_field_from_fields(&layer.fields, "title") {
        vl["title"] = json!(title);
    }

    // Width/height from layer fields
    if let Some(w) = get_number_field_from_fields(&layer.fields, "width") {
        vl["width"] = json!(w);
    }
    if let Some(h) = get_number_field_from_fields(&layer.fields, "height") {
        vl["height"] = json!(h);
    }

    vl
}

/// Build the `"data": { "values": [...] }` array from a plot spec's encoding fields.
///
/// Converts column-oriented encoding data (`x: [1,2,3], y: [4,5,6]`) into
/// row-oriented records (`[{x:1, y:4}, {x:2, y:5}, {x:3, y:6}]`).
fn build_data_values(spec: &PlotSpec) -> Vec<JsonValue> {
    // Collect encoding channel data (x, y, color, size, etc.)
    let encoding_channels = [
        "x", "y", "color", "size", "shape", "opacity", "detail", "text", "tooltip",
    ];
    let mut channel_data: Vec<(&str, Vec<JsonValue>)> = Vec::new();
    let mut max_len = 0;

    for &ch in &encoding_channels {
        if let Some(values) = get_field_as_json_array(spec, ch) {
            if values.len() > max_len {
                max_len = values.len();
            }
            channel_data.push((ch, values));
        }
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

    // Check for mark properties (e.g., stroke_width, opacity)
    let mark_props = [
        "stroke_width",
        "opacity",
        "size",
        "color",
        "filled",
        "interpolate",
    ];
    let mut has_props = false;
    let mut mark_obj = serde_json::Map::new();
    mark_obj.insert("type".to_string(), json!(mark_type_str));

    for &prop in &mark_props {
        let value = get_number_field_single(spec, prop)
            .map(|v| json!(v))
            .or_else(|| get_string_field(spec, prop).map(|v| json!(v)));
        if let Some(val) = value {
            mark_obj.insert(to_camel_case(prop), val);
            has_props = true;
        }
    }

    if has_props {
        JsonValue::Object(mark_obj)
    } else {
        json!(mark_type_str)
    }
}

/// Build the Vega-Lite `"encoding"` field.
fn build_encoding(spec: &PlotSpec) -> JsonValue {
    let mut encoding = serde_json::Map::new();

    let channels = [
        "x", "y", "color", "size", "shape", "opacity", "detail", "text", "tooltip",
    ];

    for &ch in &channels {
        if has_field(spec, ch) {
            let vega_type = infer_vega_type(spec, ch);
            let mut ch_spec = serde_json::Map::new();
            ch_spec.insert("field".to_string(), json!(ch));
            ch_spec.insert("type".to_string(), json!(vega_type));

            // Axis title: explicit x_label/y_label overrides auto-generated titles
            let explicit_label = match ch {
                "x" => get_string_field(spec, "x_label"),
                "y" => get_string_field(spec, "y_label"),
                _ => None,
            };
            let axis_title = explicit_label.or_else(|| {
                let meta = get_encoding_meta(spec, ch)?;
                format_axis_title(meta)
            });
            if let Some(title) = axis_title {
                ch_spec.insert("axis".to_string(), json!({ "title": title }));
            }

            encoding.insert(ch.to_string(), JsonValue::Object(ch_spec));
        }
    }

    JsonValue::Object(encoding)
}

/// Look up axis metadata for an encoding channel.
fn get_encoding_meta<'a>(spec: &'a PlotSpec, channel: &str) -> Option<&'a AxisMeta> {
    spec.encoding_meta
        .iter()
        .find(|(name, _)| name == channel)
        .map(|(_, meta)| meta)
}

/// Format an axis title from dimension and unit metadata.
///
/// - Dimension "Velocity" + unit "km/s" → "Velocity (km/s)"
/// - Dimension "Velocity" alone → "Velocity"
/// - Unit "km/s" alone → None (unit without dimension isn't meaningful as title)
/// - Neither → None
fn format_axis_title(meta: &AxisMeta) -> Option<String> {
    match (&meta.dimension_label, &meta.unit_label) {
        (Some(dim), Some(unit)) => Some(format!("{dim} ({unit})")),
        (Some(dim), None) => Some(dim.clone()),
        _ => None,
    }
}

/// Infer Vega-Lite data type from field values.
fn infer_vega_type(spec: &PlotSpec, field_name: &str) -> &'static str {
    for (name, value) in &spec.fields {
        if name == field_name {
            return match value {
                PlotFieldValue::Numbers(_) | PlotFieldValue::Number(_) => "quantitative",
                PlotFieldValue::Labels(_) | PlotFieldValue::String(_) => "nominal",
            };
        }
    }
    "quantitative"
}

/// Check if a field exists in the plot spec.
fn has_field(spec: &PlotSpec, field_name: &str) -> bool {
    spec.fields.iter().any(|(name, _)| name == field_name)
}

/// Get field data as a JSON array (for data values).
fn get_field_as_json_array(spec: &PlotSpec, field_name: &str) -> Option<Vec<JsonValue>> {
    for (name, value) in &spec.fields {
        if name == field_name {
            return Some(match value {
                PlotFieldValue::Numbers(nums) => nums.iter().copied().map(json_number).collect(),
                PlotFieldValue::Labels(labels) => labels.iter().map(|s| json!(s)).collect(),
                PlotFieldValue::Number(n) => vec![json_number(*n)],
                PlotFieldValue::String(s) => vec![json!(s)],
            });
        }
    }
    None
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

/// Get a string field from a plot spec.
fn get_string_field(spec: &PlotSpec, field_name: &str) -> Option<String> {
    get_string_field_from_fields(&spec.fields, field_name)
}

/// Get a single numeric value from a plot spec.
fn get_number_field_single(spec: &PlotSpec, field_name: &str) -> Option<f64> {
    for (name, value) in &spec.fields {
        if name == field_name {
            return match value {
                PlotFieldValue::Number(n) => Some(*n),
                PlotFieldValue::Numbers(nums) if nums.len() == 1 => Some(nums[0]),
                _ => None,
            };
        }
    }
    None
}

/// Get a string field from a list of (name, value) pairs.
fn get_string_field_from_fields(
    fields: &[(String, PlotFieldValue)],
    field_name: &str,
) -> Option<String> {
    for (name, value) in fields {
        if name == field_name {
            return match value {
                PlotFieldValue::String(s) => Some(s.clone()),
                _ => None,
            };
        }
    }
    None
}

/// Get a single numeric value from a list of (name, value) pairs.
fn get_number_field_from_fields(
    fields: &[(String, PlotFieldValue)],
    field_name: &str,
) -> Option<f64> {
    for (name, value) in fields {
        if name == field_name {
            return match value {
                PlotFieldValue::Number(n) => Some(*n),
                PlotFieldValue::Numbers(nums) if nums.len() == 1 => Some(nums[0]),
                _ => None,
            };
        }
    }
    None
}

/// Convert `snake_case` to `camelCase`.
fn to_camel_case(s: &str) -> String {
    let mut result = String::new();
    let mut capitalize_next = false;
    for ch in s.chars() {
        if ch == '_' {
            capitalize_next = true;
        } else if capitalize_next {
            result.extend(ch.to_uppercase());
            capitalize_next = false;
        } else {
            result.push(ch);
        }
    }
    result
}

/// Render all figures as a single HTML page using Vega-Embed.
pub fn render_html(figures: &[RenderedFigure]) -> String {
    use std::fmt::Write;
    let mut divs = String::new();
    for (i, fig) in figures.iter().enumerate() {
        let div_id = format!("graphcal-plot-{i}");
        let spec_json = serde_json::to_string(&fig.spec).unwrap_or_default();
        let _ = write!(
            divs,
            r#"<div style="margin-bottom: 2em;">
<h3>{name}</h3>
<div id="{div_id}"></div>
<script>vegaEmbed('#{div_id}', {spec_json}).catch(console.error);</script>
</div>
"#,
            name = fig.name,
        );
    }
    format!(
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
    )
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
