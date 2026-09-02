//! Parametric closure rule for reusable DAG template bodies.
//!
//! A `pub(bind)` Static declaration with a local definition is an optional
//! input, not a constant. Template bodies are checked as if every Static input
//! were rigid; only parameter defaults may observe an optional input's default,
//! because V005 requires consumers that override the input to reconcile those
//! values.

use crate::ir::static_interface::{StaticInputKind, StaticRole};

/// Semantic context in which a Static dependency occurs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum StaticUseContext {
    /// A parameter default, protected by V005 reconciliation.
    ParameterDefault,
    /// A node, const, sink, or runtime-unit body.
    TemplateBody,
}

/// Whether an operation is parametric or observes a concrete Static default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum StaticDependency {
    /// The operation remains valid for every admissible binding.
    Abstract,
    /// The operation needs the optional port's concrete default definition.
    DefaultDefinition,
}

/// One complete input to the pure template-closure rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TemplateClosureCheck {
    pub(crate) kind: StaticInputKind,
    pub(crate) role: StaticRole,
    pub(crate) context: StaticUseContext,
    pub(crate) dependency: StaticDependency,
}

/// Typed semantic reason for rejecting a template body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TemplateClosureViolation {
    pub(crate) kind: StaticInputKind,
}

/// Validate one dependency without consulting syntax or diagnostic prose.
pub(crate) const fn validate(check: TemplateClosureCheck) -> Result<(), TemplateClosureViolation> {
    if matches!(check.role, StaticRole::OptionalInput)
        && matches!(check.context, StaticUseContext::TemplateBody)
        && matches!(check.dependency, StaticDependency::DefaultDefinition)
    {
        Err(TemplateClosureViolation { kind: check.kind })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod formal_conformance;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_optional_default_dependencies_in_template_bodies_are_rejected() {
        for kind in [
            StaticInputKind::Type,
            StaticInputKind::Dimension,
            StaticInputKind::Index,
        ] {
            for role in [
                StaticRole::Fixed,
                StaticRole::OptionalInput,
                StaticRole::RequiredInput,
            ] {
                for context in [
                    StaticUseContext::ParameterDefault,
                    StaticUseContext::TemplateBody,
                ] {
                    for dependency in [
                        StaticDependency::Abstract,
                        StaticDependency::DefaultDefinition,
                    ] {
                        let rejected = role == StaticRole::OptionalInput
                            && context == StaticUseContext::TemplateBody
                            && dependency == StaticDependency::DefaultDefinition;
                        assert_eq!(
                            validate(TemplateClosureCheck {
                                kind,
                                role,
                                context,
                                dependency,
                            })
                            .is_err(),
                            rejected
                        );
                    }
                }
            }
        }
    }
}
