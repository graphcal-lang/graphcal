//! Why a plot, figure, or layer was not rendered.
//!
//! A plot's own evaluation fails or stays incomplete like any declaration
//! ([`NodeUnavailable`]). A figure or layer can additionally be blocked by the
//! plots it composes; those are reported by the names the root gives them, not
//! by their declaration identities, which may belong to an include instance.
//! The unfinished declarations a reason names are `N`: runtime identities
//! during evaluation, output names ([`OutputDeclName`]) in a result.

use std::collections::BTreeSet;

use graphcal_compiler::node_unavailable::{NodeUnavailable, RuntimeUnavailable};
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::non_empty::NonEmpty;

use super::output_decl_name::OutputDeclName;

/// Why a plot, figure, or layer was not rendered.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlotUnavailable {
    /// Its own evaluation did not produce a value.
    #[error(transparent)]
    Evaluation(#[from] NodeUnavailable<OutputDeclName>),
    /// A figure or layer composes plots that were not rendered.
    #[error(transparent)]
    ComposedPlots(#[from] ComposedPlotsUnavailable<OutputDeclName>),
}

impl PlotUnavailable {
    /// Whether a failure (not only incompleteness) prevented rendering.
    #[must_use]
    pub const fn has_failure(&self) -> bool {
        match self {
            Self::Evaluation(reason) => reason.has_failure(),
            Self::ComposedPlots(reason) => reason.has_failure(),
        }
    }

    /// Whether unfinished formulas prevented rendering.
    #[must_use]
    pub fn is_incomplete(&self) -> bool {
        match self {
            Self::Evaluation(reason) => reason.is_incomplete(),
            Self::ComposedPlots(ComposedPlotsUnavailable::Blocked { .. }) => true,
            Self::ComposedPlots(ComposedPlotsUnavailable::Failed { .. }) => false,
        }
    }
}

/// The composed plots that were not rendered, by their names in the root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposedPlotsUnavailable<N = OutputDeclName> {
    /// Some composed plot depends on unfinished formulas; other composed
    /// plots may also have failed.
    Blocked {
        unfinished: NonEmpty<N>,
        failed_plots: Vec<DeclName>,
    },
    /// Composed plots failed.
    Failed { failed_plots: NonEmpty<DeclName> },
}

impl ComposedPlotsUnavailable<ResolvedDeclName> {
    /// Derive a composition's outcome from the unavailable plots it composes,
    /// preserving every unfinished origin and every failed plot. No
    /// unavailable plot is a valid case.
    #[must_use]
    pub(crate) fn blocked_by<'a>(
        plots: impl IntoIterator<Item = (&'a DeclName, &'a RuntimeUnavailable)>,
    ) -> Option<Self> {
        let plots = plots.into_iter().collect::<Vec<_>>();
        let unfinished = plots
            .iter()
            .flat_map(|(_, reason)| reason.unfinished().iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let failed_plots = plots
            .iter()
            .filter(|(_, reason)| reason.has_failure())
            .map(|(name, _)| (*name).clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        match NonEmpty::try_from_vec(unfinished) {
            Ok(unfinished) => Some(Self::Blocked {
                unfinished,
                failed_plots,
            }),
            Err(_) => NonEmpty::try_from_vec(failed_plots)
                .ok()
                .map(|failed_plots| Self::Failed { failed_plots }),
        }
    }
}

impl<N> ComposedPlotsUnavailable<N> {
    /// The same reason with every declaration it names renamed by `rename`.
    #[must_use]
    pub(crate) fn map_names<M>(&self, rename: impl FnMut(&N) -> M) -> ComposedPlotsUnavailable<M> {
        match self {
            Self::Blocked {
                unfinished,
                failed_plots,
            } => ComposedPlotsUnavailable::Blocked {
                unfinished: unfinished.map_ref(rename),
                failed_plots: failed_plots.clone(),
            },
            Self::Failed { failed_plots } => ComposedPlotsUnavailable::Failed {
                failed_plots: failed_plots.clone(),
            },
        }
    }

    /// Whether a composed plot failed.
    #[must_use]
    const fn has_failure(&self) -> bool {
        match self {
            Self::Blocked { failed_plots, .. } => !failed_plots.is_empty(),
            Self::Failed { .. } => true,
        }
    }
}

fn names<T: std::fmt::Display>(items: &[T]) -> String {
    items
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

impl<N: std::fmt::Display> std::fmt::Display for ComposedPlotsUnavailable<N> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed { failed_plots } => {
                write!(
                    formatter,
                    "dependency failed: {}",
                    names(failed_plots.as_slice())
                )
            }
            Self::Blocked {
                unfinished,
                failed_plots,
            } => {
                write!(
                    formatter,
                    "BLOCKED — unfinished dependencies: {}",
                    names(unfinished.as_slice())
                )?;
                if !failed_plots.is_empty() {
                    write!(formatter, "; dependency failed: {}", names(failed_plots))?;
                }
                Ok(())
            }
        }
    }
}

impl<N: std::fmt::Display + std::fmt::Debug> std::error::Error for ComposedPlotsUnavailable<N> {}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed() -> RuntimeUnavailable {
        NodeUnavailable::EvalFailed {
            message: "boom".to_string(),
        }
    }

    #[test]
    fn composed_plots_are_sorted_and_only_failures_are_failures() {
        assert_eq!(ComposedPlotsUnavailable::blocked_by([]), None);
        let (b, a) = (DeclName::expect_valid("b"), DeclName::expect_valid("a"));
        let (failed_b, failed_a) = (failed(), failed());
        let reason =
            ComposedPlotsUnavailable::blocked_by([(&b, &failed_b), (&a, &failed_a)]).unwrap();
        assert_eq!(reason.to_string(), "dependency failed: a, b");
        let reason = PlotUnavailable::from(reason.map_names(invoked));
        assert!(reason.has_failure());
        assert!(!reason.is_incomplete());
        let evaluation = PlotUnavailable::from(failed().map_names(invoked));
        assert_eq!(evaluation.to_string(), "boom");
        assert!(evaluation.has_failure());
        assert!(!evaluation.is_incomplete());
    }

    fn invoked(declaration: &ResolvedDeclName) -> OutputDeclName {
        OutputDeclName::Invoked(declaration.clone())
    }

    #[test]
    fn composed_plots_keep_unfinished_origins_under_their_output_names() {
        let pending = ResolvedDeclName::for_test(
            graphcal_compiler::dag_id::DagId::root_in_package("test", "main"),
            DeclName::expect_valid("pending"),
        );
        let blocked = NodeUnavailable::Todo {
            declaration: pending,
        };
        let (plot, failed_plot) = (DeclName::expect_valid("p"), DeclName::expect_valid("q"));
        let failure = failed();
        let reason =
            ComposedPlotsUnavailable::blocked_by([(&plot, &blocked), (&failed_plot, &failure)])
                .unwrap();
        let named = reason.map_names(|_| {
            OutputDeclName::Root(graphcal_compiler::syntax::module_name::ScopedName::local(
                DeclName::expect_valid("pending"),
            ))
        });
        assert_eq!(
            named.to_string(),
            "BLOCKED — unfinished dependencies: pending; dependency failed: q"
        );
        let reason = PlotUnavailable::from(named);
        assert!(reason.is_incomplete());
        assert!(reason.has_failure());
    }
}
