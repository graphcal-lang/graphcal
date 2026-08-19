//! Pure projection from evaluated plot specs to Vega-Lite JSON.

use graphcal_compiler::syntax::ast::{EncodingChannel, MarkType};
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_eval::eval::{
    AxisMeta, CompositionProperty, FigureSpec, LayerSpec, PlotFieldValue, PlotProperty, PlotSpec,
};
use serde_json::{Value as JsonValue, json};
use thiserror::Error;

/// A rendered figure ready for output.
pub struct RenderedFigure {
    /// The figure name (used for JSON output and HTML div IDs).
    pub name: String,
    /// The Vega-Lite spec as a JSON value.
    pub spec: JsonValue,
}

/// The kind of composition declaration that referenced a plot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlotOwnerKind {
    /// A `figure` declaration.
    Figure,
    /// A `layer` declaration.
    Layer,
}

impl std::fmt::Display for PlotOwnerKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Figure => formatter.write_str("figure"),
            Self::Layer => formatter.write_str("layer"),
        }
    }
}

/// A figure/layer referenced a plot name absent from the evaluated plot set.
///
/// Unknown names are rejected at resolution time (#843), so hitting this is an
/// internal-invariant failure of the compiler, not a user-facing contract.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("internal error: {owner_kind} `{owner}` references unknown plot `{plot}`")]
pub struct UnknownPlotReference {
    /// Which composition kind held the dangling reference.
    owner_kind: PlotOwnerKind,
    /// The figure/layer declaration holding the reference.
    owner: ScopedName,
    /// The referenced plot name that could not be found.
    plot: ScopedName,
}

/// Build figures from evaluated plot, figure, and layer specs.
///
/// - Each `pub` `PlotSpec` produces one standalone figure.
/// - Each `FigureSpec` produces one combined figure with `hconcat`.
/// - Each `LayerSpec` produces one combined figure with `layer`.
///
/// # Errors
///
/// Returns [`UnknownPlotReference`] when a figure/layer references a plot that
/// is not in `plots` — a compiler invariant violation (#843).
pub fn build_figures(
    plots: &[PlotSpec],
    figures: &[FigureSpec],
    layers: &[LayerSpec],
) -> Result<Vec<RenderedFigure>, UnknownPlotReference> {
    let mut result = Vec::new();

    // Standalone figures from displayed plots (#[hidden] plots are only
    // usable in figure/layer composition; #847)
    for spec in plots {
        if !spec.displayed {
            continue;
        }
        result.push(RenderedFigure {
            name: spec.name.to_string(),
            spec: build_single_spec(spec),
        });
    }

    // Combined figures from figure specs
    for fig in figures {
        result.push(RenderedFigure {
            name: fig.name.to_string(),
            spec: build_figure_spec(fig, plots)?,
        });
    }

    // Layered figures from layer specs
    for layer in layers {
        result.push(RenderedFigure {
            name: layer.name.to_string(),
            spec: build_layer_spec(layer, plots)?,
        });
    }

    Ok(result)
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

/// Resolve the plots referenced by a figure/layer.
fn referenced_plots<'a>(
    owner_kind: PlotOwnerKind,
    owner_name: &ScopedName,
    plot_names: &[ScopedName],
    all_plots: &'a [PlotSpec],
) -> Result<Vec<&'a PlotSpec>, UnknownPlotReference> {
    plot_names
        .iter()
        .map(|name| {
            all_plots
                .iter()
                .find(|p| p.name == *name)
                .ok_or_else(|| UnknownPlotReference {
                    owner_kind,
                    owner: owner_name.clone(),
                    plot: name.clone(),
                })
        })
        .collect()
}

/// Build a Vega-Lite `hconcat` spec from a `FigureSpec`.
fn build_figure_spec(
    fig: &FigureSpec,
    all_plots: &[PlotSpec],
) -> Result<JsonValue, UnknownPlotReference> {
    let referenced =
        referenced_plots(PlotOwnerKind::Figure, &fig.name, &fig.plot_names, all_plots)?;

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

    Ok(vl)
}

/// Build a Vega-Lite `layer` spec from a `LayerSpec`.
fn build_layer_spec(
    layer: &LayerSpec,
    all_plots: &[PlotSpec],
) -> Result<JsonValue, UnknownPlotReference> {
    let referenced = referenced_plots(
        PlotOwnerKind::Layer,
        &layer.name,
        &layer.plot_names,
        all_plots,
    )?;

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

    Ok(vl)
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
        PlotFieldValue::Datetimes(_) | PlotFieldValue::Datetime(_) => "temporal",
    }
}

/// Convert a `PlotFieldValue` to a JSON array for data values.
fn field_value_to_json_array(value: &PlotFieldValue) -> Vec<JsonValue> {
    match value {
        PlotFieldValue::Numbers(nums) => nums.iter().copied().map(json_number).collect(),
        PlotFieldValue::Labels(labels) | PlotFieldValue::Datetimes(labels) => {
            labels.iter().map(|s| json!(s)).collect()
        }
        PlotFieldValue::Number(n) => vec![json_number(*n)],
        PlotFieldValue::String(s) | PlotFieldValue::Datetime(s) => vec![json!(s)],
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vega_data_preserves_exact_int_boundary_and_datetime_nanoseconds() {
        let result = graphcal_eval::eval::compile_and_eval(
            r#"
node instant: Datetime = datetime("2026-01-01T00:00:00.000000001Z");
plot p = {
    mark: point,
    encode: {
        x: 9007199254740992,
        y: @instant,
    },
};
"#,
        )
        .unwrap();
        let figures = build_figures(&result.plots, &result.figures, &result.layers).unwrap();
        let row = &figures[0].spec["data"]["values"][0];
        assert_eq!(row["x"], json!(9_007_199_254_740_992.0));
        assert_eq!(row["y"], json!("2026-01-01T00:00:00.000000001Z"));
    }
}
