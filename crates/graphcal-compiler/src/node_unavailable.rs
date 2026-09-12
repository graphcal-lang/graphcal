//! Typed reasons a graph declaration has no evaluated value.
//!
//! Incompleteness is separate from failure. Both can occur in the same
//! dependency set; allowing incomplete evaluation must never hide a failure.

use std::collections::BTreeSet;

use crate::syntax::decl_name::ResolvedDeclName;
use crate::syntax::non_empty::NonEmpty;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NodeUnavailable {
    EvalFailed {
        message: String,
    },
    DependencyFailed {
        failed_deps: Vec<ResolvedDeclName>,
    },
    Todo {
        declaration: ResolvedDeclName,
    },
    Blocked {
        unfinished: NonEmpty<ResolvedDeclName>,
        failed_deps: Vec<ResolvedDeclName>,
    },
}

impl NodeUnavailable {
    #[must_use]
    pub const fn has_failure(&self) -> bool {
        match self {
            Self::EvalFailed { .. } | Self::DependencyFailed { .. } => true,
            Self::Todo { .. } => false,
            Self::Blocked { failed_deps, .. } => !failed_deps.is_empty(),
        }
    }

    #[must_use]
    pub fn unfinished(&self) -> &[ResolvedDeclName] {
        match self {
            Self::Todo { declaration } => std::slice::from_ref(declaration),
            Self::Blocked { unfinished, .. } => unfinished.as_slice(),
            Self::EvalFailed { .. } | Self::DependencyFailed { .. } => &[],
        }
    }

    #[must_use]
    pub fn is_incomplete(&self) -> bool {
        !self.unfinished().is_empty()
    }

    /// Derive a dependent's outcome, preserving every unfinished origin and
    /// any independently failed input. No unavailable input is a valid case.
    #[must_use]
    pub fn blocked_by<'a>(
        dependencies: impl IntoIterator<Item = (&'a ResolvedDeclName, &'a Self)>,
    ) -> Option<Self> {
        let dependencies = dependencies.into_iter().collect::<Vec<_>>();
        let unfinished = dependencies
            .iter()
            .flat_map(|(_, reason)| reason.unfinished().iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let failed_deps = dependencies
            .iter()
            .filter(|(_, reason)| reason.has_failure())
            .map(|(identity, _)| (*identity).clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        match NonEmpty::try_from_vec(unfinished) {
            Ok(unfinished) => Some(Self::Blocked {
                unfinished,
                failed_deps,
            }),
            Err(_) if failed_deps.is_empty() => None,
            Err(_) => Some(Self::DependencyFailed { failed_deps }),
        }
    }
}

fn names(declarations: &[ResolvedDeclName]) -> String {
    declarations
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

impl std::fmt::Display for NodeUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EvalFailed { message } => formatter.write_str(message),
            Self::DependencyFailed { failed_deps } => {
                write!(formatter, "dependency failed: {}", names(failed_deps))
            }
            Self::Todo { .. } => formatter.write_str("TODO — formula unfinished"),
            Self::Blocked {
                unfinished,
                failed_deps,
            } => {
                write!(
                    formatter,
                    "BLOCKED — unfinished dependencies: {}",
                    names(unfinished.as_slice())
                )?;
                if !failed_deps.is_empty() {
                    write!(formatter, "; dependency failed: {}", names(failed_deps))?;
                }
                Ok(())
            }
        }
    }
}
