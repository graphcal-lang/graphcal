//! Pure projection from evaluated plot specs to Vega-Lite JSON.

use graphcal_compiler::plot_visibility::PlotVisibility;
use graphcal_compiler::syntax::ast::{EncodingChannel, MarkType};
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_eval::eval::{
    AxisMeta, CompositionProperty, FigureSpec, LayerSpec, PlotFieldValue, PlotProperty, PlotSpec,
    PropertyValue,
};
use serde_json::{Value as JsonValue, json};
use thiserror::Error;

/// The narrowest band, in pixels, in which a nominal x label stays
/// horizontal.
///
/// Vega-Lite rotates nominal x labels vertically by default. When a view
/// with an explicit width gives every distinct label at least this much
/// room, the labels are kept horizontal instead.
const MIN_HORIZONTAL_LABEL_STEP_PX: f64 = 64.0;

/// A rendered figure ready for output.
pub struct RenderedFigure {
    /// Caption from the plot, figure, or layer declaration.
    pub doc: Option<String>,
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
        match spec.visibility {
            PlotVisibility::Standalone => {}
            PlotVisibility::CompositionOnly => continue,
        }
        result.push(RenderedFigure {
            doc: spec.doc.clone(),
            name: spec.name.to_string(),
            spec: build_single_spec(spec),
        });
    }

    // Combined figures from figure specs
    for fig in figures {
        result.push(RenderedFigure {
            doc: fig.doc.clone(),
            name: fig.name.to_string(),
            spec: build_figure_spec(fig, plots)?,
        });
    }

    // Layered figures from layer specs
    for layer in layers {
        result.push(RenderedFigure {
            doc: layer.doc.clone(),
            name: layer.name.to_string(),
            spec: build_layer_spec(layer, plots)?,
        });
    }

    Ok(result)
}

/// Add an interval pan/zoom binding to one single-view spec.
///
/// Applies only when the spec is a unit view (has a top-level `mark`), has no
/// params yet, and at least one positional encoding is continuous
/// (quantitative/temporal) — Vega-Lite requires selection params on child
/// views for composed specs, and scale-bound intervals need a continuous
/// scale to act on.
pub fn add_pan_zoom(spec: &mut JsonValue) {
    if spec.get("mark").is_none() || spec.get("params").is_some() {
        return;
    }
    // This is the Vega-Lite JSON boundary, not Graphcal identifier dispatch.
    // Project explicitly: an unrestricted interval also binds categorical axes.
    let encodings: Vec<_> = ["x", "y"]
        .into_iter()
        .filter(|channel| {
            spec.get("encoding")
                .and_then(|encoding| encoding.get(channel))
                .is_some_and(|encoding| {
                    encoding
                        .get("type")
                        .and_then(JsonValue::as_str)
                        .is_some_and(|kind| matches!(kind, "quantitative" | "temporal"))
                        && matches!(encoding.get("bin"), None | Some(JsonValue::Bool(false)))
                })
        })
        .collect();
    if encodings.is_empty() {
        return;
    }
    spec["params"] = json!([{
        "name": "graphcal_pan_zoom",
        "select": { "type": "interval", "encodings": encodings },
        "bind": "scales",
    }]);
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

    // Width (also decides how the encoding lays out its x labels)
    let width = get_number_property(&spec.properties, &PlotProperty::Width);

    // Encoding
    vl["encoding"] = build_encoding(spec, x_label_layout(x_labels(spec), width));

    // Title
    if let Some(title) = get_string_property(&spec.properties, &PlotProperty::Title) {
        vl["title"] = json!(title);
    }

    // Width/height
    if let Some(w) = width {
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

    // The entries share one x axis, whose labels are the union of theirs.
    let width = get_number_property(&layer.properties, &CompositionProperty::Width);
    let label_layout = x_label_layout(referenced.iter().flat_map(|spec| x_labels(spec)), width);

    // Each sub-spec is a layer entry: mark + encoding + data (no $schema).
    let sub_specs: Vec<JsonValue> = referenced
        .iter()
        .map(|spec| {
            let mut entry = json!({});
            entry["data"] = json!({ "values": build_data_values(spec) });
            entry["mark"] = build_mark(spec);
            entry["encoding"] = build_encoding(spec, label_layout);
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
    if let Some(w) = width {
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
        mark_obj.insert(prop.vega_name().to_string(), property_json(value));
    }

    JsonValue::Object(mark_obj)
}

/// The Vega-Lite JSON of an evaluated property value, keeping its type.
///
/// A boolean must stay a JSON boolean: Vega-Lite reads the string `"true"`
/// (or `"false"`) for `filled` as a transparent, unstroked point.
fn property_json(value: &PropertyValue) -> JsonValue {
    match value {
        PropertyValue::String(text) => json!(text),
        PropertyValue::Number(number) => json!(number),
        PropertyValue::Bool(flag) => json!(flag),
    }
}

/// Build the Vega-Lite `"encoding"` field, laying out the labels of a
/// nominal x axis as `label_layout` says.
fn build_encoding(spec: &PlotSpec, label_layout: XLabelLayout) -> JsonValue {
    let mut encoding = serde_json::Map::new();

    for (channel, value) in &spec.encodings {
        let ch_name = channel_vega_name(*channel);
        let vega_type = infer_vega_type(value);
        let mut ch_spec = serde_json::Map::new();
        ch_spec.insert("field".to_string(), json!(ch_name));
        ch_spec.insert("type".to_string(), json!(vega_type));

        match value {
            PlotFieldValue::Labels(_) => {
                // Labels arrive in row order, which follows the declaration
                // order of their index; Vega-Lite would sort them
                // alphabetically, so keep the data order instead.
                if channel_has_scale(*channel) {
                    ch_spec.insert("sort".to_string(), JsonValue::Null);
                }
                if *channel == EncodingChannel::X && label_layout == XLabelLayout::Horizontal {
                    ch_spec.insert("axis".to_string(), json!({ "labelAngle": 0 }));
                }
            }
            PlotFieldValue::Numbers(_) | PlotFieldValue::Datetimes(_) => {}
        }

        // Without a title, Vega-Lite would title the axis or legend with the
        // synthetic field name (`x`, `color`, ...); `null` removes it.
        ch_spec.insert("title".to_string(), json!(channel_title(spec, *channel)));

        encoding.insert(ch_name.to_string(), JsonValue::Object(ch_spec));
    }

    JsonValue::Object(encoding)
}

/// The title of an encoding channel: an explicit `x_label`/`y_label`,
/// otherwise the dimension and unit of a quantity channel, otherwise none.
fn channel_title(spec: &PlotSpec, channel: EncodingChannel) -> Option<String> {
    let explicit_property = match channel {
        EncodingChannel::X => Some(PlotProperty::XLabel),
        EncodingChannel::Y => Some(PlotProperty::YLabel),
        EncodingChannel::Color
        | EncodingChannel::Size
        | EncodingChannel::Shape
        | EncodingChannel::Opacity
        | EncodingChannel::Detail
        | EncodingChannel::Text
        | EncodingChannel::Tooltip => None,
    };
    explicit_property
        .and_then(|property| get_string_property(&spec.properties, &property))
        .or_else(|| get_encoding_meta(spec, channel).and_then(format_axis_title))
}

/// Whether Vega-Lite gives the channel a scale, whose domain order `sort`
/// controls.
const fn channel_has_scale(channel: EncodingChannel) -> bool {
    match channel {
        EncodingChannel::X
        | EncodingChannel::Y
        | EncodingChannel::Color
        | EncodingChannel::Size
        | EncodingChannel::Shape
        | EncodingChannel::Opacity => true,
        EncodingChannel::Detail | EncodingChannel::Text | EncodingChannel::Tooltip => false,
    }
}

/// How a view lays out the labels of a nominal x axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum XLabelLayout {
    /// Vega-Lite's default, which rotates nominal x labels vertically.
    Default,
    /// Horizontal labels, which every label has room for.
    Horizontal,
}

/// The labels of a plot's nominal x channel, in row order (none when its x
/// channel is numeric, temporal, or absent).
fn x_labels(spec: &PlotSpec) -> impl Iterator<Item = &str> {
    spec.encodings
        .iter()
        .filter(|(channel, _)| *channel == EncodingChannel::X)
        .flat_map(|(_, value)| match value {
            PlotFieldValue::Labels(labels) => labels.as_slice(),
            PlotFieldValue::Numbers(_) | PlotFieldValue::Datetimes(_) => &[],
        })
        .map(String::as_str)
}

/// The layout of nominal x `labels` in a view `view_width` pixels wide:
/// horizontal when every distinct label gets at least
/// [`MIN_HORIZONTAL_LABEL_STEP_PX`] of room.
///
/// A view without an explicit width keeps Vega-Lite's default layout.
fn x_label_layout<'a>(
    labels: impl IntoIterator<Item = &'a str>,
    view_width: Option<f64>,
) -> XLabelLayout {
    let Some(width) = view_width else {
        return XLabelLayout::Default;
    };
    let distinct = labels
        .into_iter()
        .collect::<std::collections::HashSet<_>>()
        .len();
    let fits = u32::try_from(distinct)
        .is_ok_and(|count| count > 0 && width / f64::from(count) >= MIN_HORIZONTAL_LABEL_STEP_PX);
    if fits {
        XLabelLayout::Horizontal
    } else {
        XLabelLayout::Default
    }
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
/// - Dimension "Velocity" alone (no canonical unit exists) -> "Velocity"
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
        PlotFieldValue::Numbers(_) => "quantitative",
        PlotFieldValue::Labels(_) => "nominal",
        PlotFieldValue::Datetimes(_) => "temporal",
    }
}

/// Convert a `PlotFieldValue` to a JSON array for data values.
fn field_value_to_json_array(value: &PlotFieldValue) -> Vec<JsonValue> {
    match value {
        PlotFieldValue::Numbers(nums) => nums.iter().copied().map(json_number).collect(),
        PlotFieldValue::Labels(labels) | PlotFieldValue::Datetimes(labels) => {
            labels.iter().map(|s| json!(s)).collect()
        }
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
    properties: &[(P, PropertyValue)],
    prop: &P,
) -> Option<String> {
    properties
        .iter()
        .find(|(p, _)| p == prop)
        .and_then(|(_, v)| match v {
            PropertyValue::String(s) => Some(s.clone()),
            PropertyValue::Number(_) | PropertyValue::Bool(_) => None,
        })
}

/// Look up a property by key and return its numeric value.
fn get_number_property<P: PartialEq>(properties: &[(P, PropertyValue)], prop: &P) -> Option<f64> {
    properties
        .iter()
        .find(|(p, _)| p == prop)
        .and_then(|(_, v)| match v {
            PropertyValue::Number(n) => Some(*n),
            PropertyValue::String(_) | PropertyValue::Bool(_) => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pan_zoom_projects_only_unbinned_continuous_axes() {
        for (x, y, expected) in [
            ("nominal", "quantitative", json!(["y"])),
            ("quantitative", "nominal", json!(["x"])),
            ("quantitative", "quantitative", json!(["x", "y"])),
            ("nominal", "ordinal", json!([])),
            ("temporal", "quantitative", json!(["x", "y"])),
            ("nominal", "temporal", json!(["y"])),
        ] {
            let mut spec = json!({"mark": "point", "encoding": {
                "x": {"field": "x", "type": x}, "y": {"field": "y", "type": y}
            }});
            add_pan_zoom(&mut spec);
            if expected == json!([]) {
                assert!(spec.get("params").is_none());
            } else {
                assert_eq!(
                    spec["params"][0],
                    json!({
                        "name": "graphcal_pan_zoom",
                        "select": {"type": "interval", "encodings": expected},
                        "bind": "scales"
                    })
                );
            }
        }
        for bin in [json!(true), json!({"maxbins": 10}), json!("binned")] {
            let mut spec = json!({"mark": "bar", "encoding": {
                "x": {"field": "x", "type": "quantitative", "bin": bin},
                "y": {"field": "y", "type": "quantitative", "bin": false}
            }});
            add_pan_zoom(&mut spec);
            assert_eq!(spec["params"][0]["select"]["encodings"], json!(["y"]));
        }
    }

    #[test]
    fn pan_zoom_preserves_existing_params_and_skips_compositions_and_missing_axes() {
        for mut spec in [
            json!({"mark": "point"}),
            json!({"layer": [], "encoding": {"x": {"type": "quantitative"}}}),
            json!({"mark": "point", "params": [], "encoding": {"x": {"type": "quantitative"}}}),
        ] {
            let original = spec.clone();
            add_pan_zoom(&mut spec);
            assert_eq!(spec, original);
        }
    }

    #[test]
    fn vega_data_preserves_exact_int_boundary_and_datetime_nanoseconds() {
        let result = graphcal_project::prepare::compile_and_eval(
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

    /// The Vega-Lite specs of every figure `source` renders, by figure name.
    fn rendered_specs(source: &str) -> Vec<(String, JsonValue)> {
        let result = graphcal_project::prepare::compile_and_eval(source).unwrap();
        assert!(!result.has_errors(), "{result:?}");
        build_figures(&result.plots, &result.figures, &result.layers)
            .unwrap()
            .into_iter()
            .map(|figure| (figure.name, figure.spec))
            .collect()
    }

    fn spec<'a>(specs: &'a [(String, JsonValue)], name: &str) -> &'a JsonValue {
        let Some((_, spec)) = specs.iter().find(|(candidate, _)| candidate == name) else {
            panic!("no figure `{name}` in {specs:?}");
        };
        spec
    }

    #[test]
    fn boolean_mark_properties_are_json_booleans() {
        let specs = rendered_specs(
            r##"
plot filled_points = {
    mark: point { filled: true, size: 30.0, color: "#0891b2" },
    encode: { x: 1.0, y: 2.0 },
};
plot hollow_points = { mark: point { filled: false }, encode: { x: 1.0, y: 2.0 } };
"##,
        );
        // Vega-Lite draws a point whose `filled` is the string "true" with a
        // transparent fill and no stroke: it must be a JSON boolean.
        assert_eq!(
            spec(&specs, "filled_points")["mark"],
            json!({ "type": "point", "filled": true, "size": 30.0, "color": "#0891b2" })
        );
        assert_eq!(
            spec(&specs, "hollow_points")["mark"],
            json!({ "type": "point", "filled": false })
        );
    }

    #[test]
    fn label_encodings_keep_declaration_order_without_placeholder_titles() {
        let specs = rendered_specs(
            r#"
pub index Maneuver = { Departure, Correction, Insertion };
param delta_v: Velocity[Maneuver] = table[Maneuver] {
    Departure: 2.46 km/s;
    Correction: 0.12 km/s;
    Insertion: 1.00 km/s;
};
param executed: Bool[Maneuver] = table[Maneuver] {
    Departure: true;
    Correction: false;
    Insertion: true;
};
plot budget = {
    mark: point,
    encode: {
        x: for m: Maneuver { m },
        y: for m: Maneuver { @delta_v[m] -> km/s },
        color: for m: Maneuver { m },
        shape: for m: Maneuver { @executed[m] },
        tooltip: for m: Maneuver { m },
        detail: "budget",
    },
};
"#,
        );
        let budget = spec(&specs, "budget");
        let x_values: Vec<_> = budget["data"]["values"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["x"].clone())
            .collect();
        assert_eq!(
            x_values,
            [json!("Departure"), json!("Correction"), json!("Insertion")]
        );
        // Labels keep their (declaration) data order instead of Vega-Lite's
        // alphabetical default, on every channel with a scale, and carry no
        // synthetic `x`/`color`/... title.
        for channel in ["x", "color", "shape"] {
            let encoding = &budget["encoding"][channel];
            assert_eq!(encoding["type"], json!("nominal"), "{channel}");
            assert_eq!(encoding.get("sort"), Some(&JsonValue::Null), "{channel}");
            assert_eq!(encoding.get("title"), Some(&JsonValue::Null), "{channel}");
        }
        // Channels without a scale take no sort.
        for channel in ["tooltip", "detail"] {
            assert_eq!(budget["encoding"][channel].get("sort"), None, "{channel}");
        }
        // A quantity channel keeps no sort and is titled by its unit.
        assert_eq!(budget["encoding"]["y"].get("sort"), None);
        assert_eq!(budget["encoding"]["y"]["title"], json!("Velocity (km/s)"));
    }

    #[test]
    fn quantity_titles_name_the_unit_of_the_plotted_numbers() {
        let specs = rendered_specs(
            r#"
index Epoch = linspace(0.0 min, 10.0 min, points: 3);
node elapsed: Time[Epoch] = for t: Epoch { coord(t) };
node speed: Velocity[Epoch] = for t: Epoch { (2.0 m/s) * coord(t) / (1.0 s) };
node ratio: Dimensionless[Epoch] = for t: Epoch { coord(t) / (1.0 s) };
plot unconverted = {
    mark: point,
    encode: { x: @elapsed, y: @speed, color: @ratio, tooltip: @speed },
};
plot converted = {
    mark: line,
    encode: { x: for t: Epoch { t }, y: for t: Epoch { @speed[t] -> km/s } },
};
plot labelled = {
    mark: line,
    encode: { x: @elapsed, y: @speed },
    x_label: "Elapsed",
    y_label: "Speed",
};
"#,
        );
        let unconverted = &spec(&specs, "unconverted")["encoding"];
        // Without a conversion, the numbers are SI: the canonical unit says so.
        assert_eq!(unconverted["x"]["title"], json!("Time (s)"));
        assert_eq!(unconverted["y"]["title"], json!("Velocity (m/s)"));
        assert_eq!(unconverted["tooltip"]["title"], json!("Velocity (m/s)"));
        // A dimensionless channel has no unit to name.
        assert_eq!(unconverted["color"].get("title"), Some(&JsonValue::Null));

        let converted = spec(&specs, "converted");
        // A coordinate key plots its SI coordinate, not the minutes its index
        // displays.
        assert_eq!(converted["encoding"]["x"]["title"], json!("Time (s)"));
        assert_eq!(converted["data"]["values"][1]["x"], json!(300));
        assert_eq!(
            converted["encoding"]["y"]["title"],
            json!("Velocity (km/s)")
        );

        // Explicit labels override generated titles.
        let labelled = &spec(&specs, "labelled")["encoding"];
        assert_eq!(labelled["x"]["title"], json!("Elapsed"));
        assert_eq!(labelled["y"]["title"], json!("Speed"));
    }

    #[test]
    fn wide_views_keep_nominal_x_labels_horizontal() {
        let specs = rendered_specs(
            r"
pub index Maneuver = { Departure, Correction, Insertion };
pub index Phase = { Early, Late };
param delta_v: Velocity[Maneuver] = table[Maneuver] {
    Departure: 2.46 km/s;
    Correction: 0.12 km/s;
    Insertion: 1.00 km/s;
};
plot wide = {
    mark: bar,
    encode: { x: for m: Maneuver { m }, y: @delta_v },
    width: 480,
};
plot narrow = {
    mark: bar,
    encode: { x: for m: Maneuver { m }, y: @delta_v },
    width: 150,
};
plot unsized = { mark: bar, encode: { x: for m: Maneuver { m }, y: @delta_v } };
plot sideways = {
    mark: bar,
    encode: { x: @delta_v, y: for m: Maneuver { m } },
    width: 480,
};
plot repeated = {
    mark: rect,
    encode: {
        x: for m: Maneuver { m },
        y: for p: Phase { p },
        color: for m: Maneuver, p: Phase { @delta_v[m] },
    },
    width: 192,
};
#[hidden]
plot bars = { mark: bar, encode: { x: for m: Maneuver { m }, y: @delta_v } };
#[hidden]
plot ticks = { mark: tick, encode: { x: for m: Maneuver { m }, y: @delta_v } };
layer overlay = { plots: [bars, ticks], width: 400 };
",
        );
        let label_angle = |name: &str, channel: &str| {
            spec(&specs, name)["encoding"][channel].get("axis").cloned()
        };
        // 480 px over 3 labels leaves room for horizontal labels.
        assert_eq!(label_angle("wide", "x"), Some(json!({ "labelAngle": 0 })));
        // 150 px over 3 labels does not; nor does a view without a width.
        assert_eq!(label_angle("narrow", "x"), None);
        assert_eq!(label_angle("unsized", "x"), None);
        // Only the x axis is affected.
        assert_eq!(label_angle("sideways", "y"), None);
        assert_eq!(label_angle("sideways", "x"), None);
        // Repeated labels count once: 192 px over 3 distinct labels is 64 px.
        assert_eq!(
            label_angle("repeated", "x"),
            Some(json!({ "labelAngle": 0 }))
        );
        // A layer's entries share its width.
        for entry in spec(&specs, "overlay")["layer"].as_array().unwrap() {
            assert_eq!(entry["encoding"]["x"]["axis"], json!({ "labelAngle": 0 }));
        }
    }

    #[test]
    fn x_label_layout_needs_an_explicit_width_with_room_per_distinct_label() {
        let labels = ["A", "B", "A", "C"];
        assert_eq!(
            x_label_layout(labels, Some(192.0)),
            XLabelLayout::Horizontal
        );
        assert_eq!(x_label_layout(labels, Some(191.9)), XLabelLayout::Default);
        assert_eq!(x_label_layout(labels, None), XLabelLayout::Default);
        assert_eq!(x_label_layout([], Some(480.0)), XLabelLayout::Default);
    }
}
