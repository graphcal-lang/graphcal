//! Semantic registry of plot, mark, and figure/layer properties.
//!
//! This is the single source of truth for which property names exist on each
//! plot-family declaration and what value type each expects. Validation,
//! evaluation, and rendering all dispatch on these enums — never on raw
//! property-name strings; `from_name` is the only place the source spelling
//! crosses into the typed core.

/// Expected value type of a plot-family property.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlotPropertyType {
    /// A string literal (e.g. `title: "Thrust"`).
    String,
    /// A dimensionless number.
    Number,
    /// A dimensionless number that must be strictly positive
    /// (e.g. `width`, `height`); positivity is value-dependent and
    /// checked at evaluation time.
    PositiveNumber,
    /// A boolean (e.g. `filled: true`).
    Bool,
}

impl PlotPropertyType {
    /// Human-readable expectation for diagnostics.
    #[must_use]
    pub(crate) const fn describe(self) -> &'static str {
        match self {
            Self::String => "a string literal",
            Self::Number => "a dimensionless number",
            Self::PositiveNumber => "a positive dimensionless number",
            Self::Bool => "a boolean",
        }
    }
}

/// A mark-level property (style applied to the mark in a plot).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MarkProperty {
    StrokeWidth,
    Opacity,
    Size,
    Color,
    Filled,
    Interpolate,
}

impl MarkProperty {
    /// Every mark property, for diagnostics listing the valid set.
    pub(crate) const ALL: [Self; 6] = [
        Self::StrokeWidth,
        Self::Opacity,
        Self::Size,
        Self::Color,
        Self::Filled,
        Self::Interpolate,
    ];

    /// Parse a mark property from its source-level name.
    #[must_use]
    pub fn from_name(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.name() == s)
    }

    /// The source-level property name.
    #[must_use]
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::StrokeWidth => "stroke_width",
            Self::Opacity => "opacity",
            Self::Size => "size",
            Self::Color => "color",
            Self::Filled => "filled",
            Self::Interpolate => "interpolate",
        }
    }

    /// The Vega-Lite camelCase property name.
    #[must_use]
    pub const fn vega_name(self) -> &'static str {
        match self {
            Self::StrokeWidth => "strokeWidth",
            Self::Opacity => "opacity",
            Self::Size => "size",
            Self::Color => "color",
            Self::Filled => "filled",
            Self::Interpolate => "interpolate",
        }
    }

    /// The value type this property expects.
    #[must_use]
    pub(crate) const fn value_type(self) -> PlotPropertyType {
        match self {
            Self::StrokeWidth | Self::Opacity | Self::Size => PlotPropertyType::Number,
            Self::Color | Self::Interpolate => PlotPropertyType::String,
            Self::Filled => PlotPropertyType::Bool,
        }
    }
}

/// A plot-level property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlotProperty {
    Title,
    Width,
    Height,
    XLabel,
    YLabel,
}

impl PlotProperty {
    /// Every plot property, for diagnostics listing the valid set.
    pub(crate) const ALL: [Self; 5] = [
        Self::Title,
        Self::Width,
        Self::Height,
        Self::XLabel,
        Self::YLabel,
    ];

    /// Parse a plot property from its source-level name.
    #[must_use]
    pub fn from_name(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.name() == s)
    }

    /// The source-level property name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Width => "width",
            Self::Height => "height",
            Self::XLabel => "x_label",
            Self::YLabel => "y_label",
        }
    }

    /// The value type this property expects.
    #[must_use]
    pub const fn value_type(self) -> PlotPropertyType {
        match self {
            Self::Title | Self::XLabel | Self::YLabel => PlotPropertyType::String,
            Self::Width | Self::Height => PlotPropertyType::PositiveNumber,
        }
    }
}

/// A figure/layer-level property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompositionProperty {
    Title,
    Width,
    Height,
}

impl CompositionProperty {
    /// Every composition property, for diagnostics listing the valid set.
    pub(crate) const ALL: [Self; 3] = [Self::Title, Self::Width, Self::Height];

    /// Parse a composition property from its source-level name.
    #[must_use]
    pub fn from_name(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.name() == s)
    }

    /// The source-level property name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Width => "width",
            Self::Height => "height",
        }
    }

    /// The value type this property expects.
    #[must_use]
    pub const fn value_type(self) -> PlotPropertyType {
        match self {
            Self::Title => PlotPropertyType::String,
            Self::Width | Self::Height => PlotPropertyType::PositiveNumber,
        }
    }

    /// Whether the property is honored on `figure` declarations.
    ///
    /// Figures render as Vega-Lite `hconcat`, which has no top-level
    /// width/height — only `title` applies; sizes belong on the
    /// constituent plots (or on layers).
    #[must_use]
    pub(crate) const fn applies_to_figure(self) -> bool {
        matches!(self, Self::Title)
    }
}
