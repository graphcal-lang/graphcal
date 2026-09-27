//! Whether an evaluated plot renders as its own figure.

/// Output visibility of a plot declaration or include-requested plot (#847).
///
/// `#[hidden]` on a `plot` declaration or on a plot include item makes the
/// plot [`CompositionOnly`](Self::CompositionOnly): it is still evaluated and
/// can be composed by figures/layers, but produces no standalone figure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlotVisibility {
    /// The plot renders as its own figure.
    Standalone,
    /// The plot is only usable through figure/layer composition.
    CompositionOnly,
}
