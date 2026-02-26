use graphcal_eval::eval::{FigureSpec, PlotFieldValue, PlotSpec};
use graphcal_syntax::ast::ChartType;
use plotly::common::{Font, Mode};
use plotly::layout::{Axis, LayoutTemplate, Template};
use plotly::{Bar, HeatMap, Layout, Plot, Scatter, Trace};

/// A rendered figure ready for output.
pub struct RenderedFigure {
    /// The figure name (used for JSON output and HTML div IDs).
    pub name: String,
    /// The Plotly plot.
    pub plot: Plot,
}

/// Build figures from evaluated plot and figure specs.
///
/// - Each non-hidden `PlotSpec` produces one standalone figure.
/// - Each `FigureSpec` produces one combined figure with subplots.
pub fn build_figures(plots: &[PlotSpec], figures: &[FigureSpec]) -> Vec<RenderedFigure> {
    let mut result = Vec::new();

    // Standalone figures from non-hidden plots
    for spec in plots {
        if spec.hidden {
            continue;
        }
        result.push(RenderedFigure {
            name: spec.name.as_str().to_string(),
            plot: build_single_plot(spec),
        });
    }

    // Combined figures from figure specs
    for fig in figures {
        result.push(RenderedFigure {
            name: fig.name.as_str().to_string(),
            plot: build_subplot_figure(fig, plots),
        });
    }

    result
}

/// Build a single Plotly `Plot` from one `PlotSpec`.
fn build_single_plot(spec: &PlotSpec) -> Plot {
    let mut plot = Plot::new();

    let title = get_string_field_from_plot(spec, "title");
    let x_label = get_string_field_from_plot(spec, "x_label");
    let y_label = get_string_field_from_plot(spec, "y_label");
    let series_name =
        get_string_field_from_plot(spec, "name").unwrap_or_else(|| spec.name.as_str().to_string());

    let trace: Box<dyn Trace> = match spec.chart_type {
        ChartType::Line => build_scatter_trace(spec, Mode::Lines, &series_name),
        ChartType::Scatter => build_scatter_trace(spec, Mode::Markers, &series_name),
        ChartType::Bar => build_bar_trace(spec, &series_name),
        ChartType::Heatmap => build_heatmap_trace(spec, &series_name),
    };
    plot.add_trace(trace);

    let mut layout = graphcal_layout();
    if let Some(t) = title {
        layout = layout.title(t.as_str());
    }
    let mut x_axis = Axis::new()
        .show_grid(true)
        .grid_color("#E5E5E5")
        .zero_line(false);
    if let Some(xl) = x_label {
        x_axis = x_axis.title(xl.as_str());
    }
    let mut y_axis = Axis::new()
        .show_grid(true)
        .grid_color("#E5E5E5")
        .zero_line(false);
    if let Some(yl) = y_label {
        y_axis = y_axis.title(yl.as_str());
    }
    layout = layout.x_axis(x_axis).y_axis(y_axis);
    plot.set_layout(layout);

    plot
}

/// Build a combined subplot figure from a `FigureSpec`.
///
/// Each referenced plot gets its own axis pair, tiled in a grid.
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "subplot grid positions use small non-negative integers"
)]
fn build_subplot_figure(fig: &FigureSpec, all_plots: &[PlotSpec]) -> Plot {
    let mut plot = Plot::new();

    // Collect the referenced plot specs
    let referenced: Vec<&PlotSpec> = fig
        .plot_names
        .iter()
        .filter_map(|name| all_plots.iter().find(|p| p.name == *name))
        .collect();

    let n = referenced.len();
    if n == 0 {
        return plot;
    }

    // Compute grid dimensions: ceil(sqrt(n)) columns
    let cols = (n as f64).sqrt().ceil().max(1.0) as usize;
    let rows = n.div_ceil(cols);

    let mut layout = graphcal_layout();

    // Extract figure-level title
    let fig_title = get_string_field_from_fields(&fig.fields, "title");
    if let Some(t) = fig_title {
        layout = layout.title(t.as_str());
    }

    let gap = 0.08; // gap between subplots

    for (i, spec) in referenced.iter().enumerate() {
        let row = i / cols;
        let col = i % cols;

        // Compute domain for this subplot
        let cell_width = (1.0 - gap * (cols as f64 - 1.0)) / cols as f64;
        let cell_height = (1.0 - gap * (rows as f64 - 1.0)) / rows as f64;

        let x_start = col as f64 * (cell_width + gap);
        let x_end = x_start + cell_width;
        // Y is inverted: row 0 at top
        let y_start = (row as f64 + 1.0).mul_add(-(cell_height + gap), 1.0) + gap;
        let y_end = y_start + cell_height;

        let series_name = get_string_field_from_plot(spec, "name")
            .unwrap_or_else(|| spec.name.as_str().to_string());
        let subplot_title = get_string_field_from_plot(spec, "title");
        let x_label = get_string_field_from_plot(spec, "x_label");
        let y_label = get_string_field_from_plot(spec, "y_label");

        // Build axis pair
        let mut x_axis = Axis::new()
            .domain(&[x_start, x_end])
            .show_grid(true)
            .grid_color("#E5E5E5")
            .zero_line(false);
        if let Some(xl) = x_label {
            x_axis = x_axis.title(xl.as_str());
        }

        let x_anchor = if i == 0 {
            "y".to_string()
        } else {
            format!("y{}", i + 1)
        };
        let y_anchor = if i == 0 {
            "x".to_string()
        } else {
            format!("x{}", i + 1)
        };
        x_axis = x_axis.anchor(x_anchor.as_str());

        let mut y_axis = Axis::new()
            .domain(&[y_start, y_end])
            .show_grid(true)
            .grid_color("#E5E5E5")
            .zero_line(false);
        if let Some(yl) = y_label {
            y_axis = y_axis.title(yl.as_str());
        }
        if let Some(t) = subplot_title {
            // Use the subplot title as the y-axis title prefix or annotation
            y_axis = y_axis.title(t.as_str());
        }
        y_axis = y_axis.anchor(y_anchor.as_str());

        // Set the axis on the layout using the numbered methods
        layout = set_x_axis(layout, i, x_axis);
        layout = set_y_axis(layout, i, y_axis);

        // Build trace bound to the correct axis pair
        let x_axis_ref = if i == 0 {
            "x".to_string()
        } else {
            format!("x{}", i + 1)
        };
        let y_axis_ref = if i == 0 {
            "y".to_string()
        } else {
            format!("y{}", i + 1)
        };

        let trace: Box<dyn Trace> = match spec.chart_type {
            ChartType::Line => build_scatter_trace_with_axis(
                spec,
                Mode::Lines,
                &series_name,
                &x_axis_ref,
                &y_axis_ref,
            ),
            ChartType::Scatter => build_scatter_trace_with_axis(
                spec,
                Mode::Markers,
                &series_name,
                &x_axis_ref,
                &y_axis_ref,
            ),
            ChartType::Bar => {
                build_bar_trace_with_axis(spec, &series_name, &x_axis_ref, &y_axis_ref)
            }
            ChartType::Heatmap => build_heatmap_trace(spec, &series_name),
        };
        plot.add_trace(trace);
    }

    plot.set_layout(layout);
    plot
}

/// Render all figures as a single HTML page.
pub fn render_html(figures: &[RenderedFigure]) -> String {
    use std::fmt::Write;
    let mut divs = String::new();
    for (i, fig) in figures.iter().enumerate() {
        let div_id = format!("graphcal-plot-{i}");
        let inline = fig.plot.to_inline_html(Some(&div_id));
        let _ = write!(
            divs,
            "<div style=\"margin-bottom: 2em;\">\n<h3>{}</h3>\n{inline}\n</div>\n",
            fig.name
        );
    }
    format!(
        r#"<!DOCTYPE html>
<html>
<head>
  <meta charset="utf-8">
  <title>Graphcal Plots</title>
  <script src="https://cdn.plot.ly/plotly-3.0.1.min.js"></script>
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
    let entries: Vec<String> = figures
        .iter()
        .map(|fig| {
            format!(
                r#"  {{ "name": {}, "spec": {} }}"#,
                serde_json::json!(fig.name),
                fig.plot.to_json()
            )
        })
        .collect();
    format!("[\n{}\n]", entries.join(",\n"))
}

// ---------------------------------------------------------------------------
// Axis data and trace builders
// ---------------------------------------------------------------------------

/// Extract x-axis data from a plot spec.
enum AxisData {
    Numbers(Vec<f64>),
    Labels(Vec<String>),
}

fn get_axis_data(spec: &PlotSpec, field_name: &str) -> AxisData {
    for (name, value) in &spec.fields {
        if name == field_name {
            return match value {
                PlotFieldValue::Numbers(nums) => AxisData::Numbers(nums.clone()),
                PlotFieldValue::Labels(labels) => AxisData::Labels(labels.clone()),
                PlotFieldValue::Number(n) => AxisData::Numbers(vec![*n]),
                PlotFieldValue::String(s) => AxisData::Labels(vec![s.clone()]),
            };
        }
    }
    AxisData::Numbers(vec![])
}

/// Build a Scatter trace (used for both `line` and `scatter` chart types).
fn build_scatter_trace(spec: &PlotSpec, mode: Mode, name: &str) -> Box<dyn Trace> {
    let x = get_axis_data(spec, "x");
    let y = get_number_field(spec, "y").unwrap_or_default();

    match x {
        AxisData::Numbers(nums) => Scatter::new(nums, y).mode(mode).name(name),
        AxisData::Labels(labels) => Scatter::new(labels, y).mode(mode).name(name),
    }
}

/// Build a Scatter trace bound to specific x/y axes (for subplots).
fn build_scatter_trace_with_axis(
    spec: &PlotSpec,
    mode: Mode,
    name: &str,
    x_axis_ref: &str,
    y_axis_ref: &str,
) -> Box<dyn Trace> {
    let x = get_axis_data(spec, "x");
    let y = get_number_field(spec, "y").unwrap_or_default();

    match x {
        AxisData::Numbers(nums) => Scatter::new(nums, y)
            .mode(mode)
            .name(name)
            .x_axis(x_axis_ref)
            .y_axis(y_axis_ref),
        AxisData::Labels(labels) => Scatter::new(labels, y)
            .mode(mode)
            .name(name)
            .x_axis(x_axis_ref)
            .y_axis(y_axis_ref),
    }
}

/// Build a Bar trace.
fn build_bar_trace(spec: &PlotSpec, name: &str) -> Box<dyn Trace> {
    let x = get_axis_data(spec, "x");
    let y = get_number_field(spec, "y").unwrap_or_default();

    match x {
        AxisData::Numbers(nums) => Bar::new(nums, y).name(name),
        AxisData::Labels(labels) => Bar::new(labels, y).name(name),
    }
}

/// Build a Bar trace bound to specific x/y axes (for subplots).
fn build_bar_trace_with_axis(
    spec: &PlotSpec,
    name: &str,
    x_axis_ref: &str,
    y_axis_ref: &str,
) -> Box<dyn Trace> {
    let x = get_axis_data(spec, "x");
    let y = get_number_field(spec, "y").unwrap_or_default();

    match x {
        AxisData::Numbers(nums) => Bar::new(nums, y)
            .name(name)
            .x_axis(x_axis_ref)
            .y_axis(y_axis_ref),
        AxisData::Labels(labels) => Bar::new(labels, y)
            .name(name)
            .x_axis(x_axis_ref)
            .y_axis(y_axis_ref),
    }
}

/// Build a `HeatMap` trace.
fn build_heatmap_trace(spec: &PlotSpec, name: &str) -> Box<dyn Trace> {
    let x = get_axis_data(spec, "x");
    let y = get_axis_data(spec, "y");
    let z = get_number_field(spec, "z").unwrap_or_default();

    match (x, y) {
        (AxisData::Numbers(xn), AxisData::Numbers(yn)) => HeatMap::new(xn, yn, z).name(name),
        (AxisData::Labels(xl), AxisData::Labels(yl)) => HeatMap::new(xl, yl, z).name(name),
        (AxisData::Numbers(xn), AxisData::Labels(yl)) => HeatMap::new(xn, yl, z).name(name),
        (AxisData::Labels(xl), AxisData::Numbers(yn)) => HeatMap::new(xl, yn, z).name(name),
    }
}

/// Get a numeric field (as `Vec<f64>`) from a plot spec.
fn get_number_field(spec: &PlotSpec, field_name: &str) -> Option<Vec<f64>> {
    for (name, value) in &spec.fields {
        if name == field_name {
            return match value {
                PlotFieldValue::Numbers(nums) => Some(nums.clone()),
                PlotFieldValue::Number(n) => Some(vec![*n]),
                _ => None,
            };
        }
    }
    None
}

/// Get a string field from a plot spec.
fn get_string_field_from_plot(spec: &PlotSpec, field_name: &str) -> Option<String> {
    for (name, value) in &spec.fields {
        if name == field_name {
            return match value {
                PlotFieldValue::String(s) => Some(s.clone()),
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

// ---------------------------------------------------------------------------
// Layout axis setters (indexed by subplot position)
// ---------------------------------------------------------------------------

/// Set the x-axis on a layout by subplot index (0-based).
fn set_x_axis(layout: Layout, idx: usize, axis: Axis) -> Layout {
    match idx {
        0 => layout.x_axis(axis),
        1 => layout.x_axis2(axis),
        2 => layout.x_axis3(axis),
        3 => layout.x_axis4(axis),
        4 => layout.x_axis5(axis),
        5 => layout.x_axis6(axis),
        6 => layout.x_axis7(axis),
        7 => layout.x_axis8(axis),
        _ => layout, // plotly.rs supports up to 8 axes
    }
}

/// Set the y-axis on a layout by subplot index (0-based).
fn set_y_axis(layout: Layout, idx: usize, axis: Axis) -> Layout {
    match idx {
        0 => layout.y_axis(axis),
        1 => layout.y_axis2(axis),
        2 => layout.y_axis3(axis),
        3 => layout.y_axis4(axis),
        4 => layout.y_axis5(axis),
        5 => layout.y_axis6(axis),
        6 => layout.y_axis7(axis),
        7 => layout.y_axis8(axis),
        _ => layout, // plotly.rs supports up to 8 axes
    }
}

// ---------------------------------------------------------------------------
// Graphcal Plotly theme
// ---------------------------------------------------------------------------

/// Graphcal color palette — 8 colors for publication-quality plots.
const GRAPHCAL_COLORS: &[&str] = &[
    "#2563EB", // blue-600
    "#DC2626", // red-600
    "#059669", // emerald-600
    "#D97706", // amber-600
    "#7C3AED", // violet-600
    "#DB2777", // pink-600
    "#0891B2", // cyan-600
    "#65A30D", // lime-600
];

/// Build a Graphcal-themed Layout.
fn graphcal_layout() -> Layout {
    let font = Font::new()
        .family("system-ui, -apple-system, sans-serif")
        .size(14);

    let template = Template::new().layout(
        LayoutTemplate::new()
            .paper_background_color("#FFFFFF")
            .plot_background_color("#FFFFFF")
            .font(font.clone())
            .colorway(GRAPHCAL_COLORS.iter().map(|c| (*c).to_string()).collect()),
    );

    Layout::new()
        .template(template)
        .paper_background_color("#FFFFFF")
        .plot_background_color("#FFFFFF")
        .font(font)
}
