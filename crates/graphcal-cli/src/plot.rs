use graphcal_eval::eval::{PlotFieldValue, PlotSpec};
use graphcal_syntax::ast::ChartType;
use plotly::common::{Font, Mode};
use plotly::layout::{Axis, LayoutTemplate, Template};
use plotly::{Bar, HeatMap, Layout, Plot, Scatter, Trace};

/// Build a Plotly `Plot` from a list of evaluated `PlotSpec`s.
///
/// Each `PlotSpec` becomes one trace in the plot. All traces share
/// a single layout with the Graphcal theme applied.
pub fn build_plot(specs: &[PlotSpec]) -> Plot {
    let mut plot = Plot::new();

    for spec in specs {
        let title = get_string_field(spec, "title");
        let x_label = get_string_field(spec, "x_label");
        let y_label = get_string_field(spec, "y_label");
        let series_name =
            get_string_field(spec, "name").unwrap_or_else(|| spec.name.as_str().to_string());

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
    }

    plot
}

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

/// Build a Bar trace.
fn build_bar_trace(spec: &PlotSpec, name: &str) -> Box<dyn Trace> {
    let x = get_axis_data(spec, "x");
    let y = get_number_field(spec, "y").unwrap_or_default();

    match x {
        AxisData::Numbers(nums) => Bar::new(nums, y).name(name),
        AxisData::Labels(labels) => Bar::new(labels, y).name(name),
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
fn get_string_field(spec: &PlotSpec, field_name: &str) -> Option<String> {
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
