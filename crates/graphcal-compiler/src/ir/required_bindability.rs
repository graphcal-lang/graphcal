//! Pure semantic core for required-interface bindability validation (V002).

use std::fmt;

use crate::syntax::ast::BindableVisibility;

/// Nominal declaration kinds that may obtain a missing definition from an
/// `include` binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NominalKind {
    Index,
    Type,
    Dimension,
}

impl fmt::Display for NominalKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Index => "index",
            Self::Type => "type",
            Self::Dimension => "dim",
        })
    }
}

/// Whether an interface declaration supplies its own definition or requires
/// one from outside the declaring module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Requirement {
    Defaulted,
    Required,
}

/// Declaration-interface states relevant to V002.
///
/// Input ports are distinct from nominal declarations because a `param` is
/// bindable by role, without carrying an implicit visibility annotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InterfaceDecl {
    InputPort {
        requirement: Requirement,
    },
    Nominal {
        kind: NominalKind,
        visibility: BindableVisibility,
        requirement: Requirement,
    },
}

/// Typed semantic reason that a required interface declaration is invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(super) enum Violation {
    #[error("required {kind} must be bindable")]
    RequiredMustBeBindable { kind: NominalKind },
}

/// Validate that every declaration requiring an external definition provides
/// a legal binding path.
///
/// A parameter always has such a path because it is an input port. A nominal
/// declaration has one only when it is explicitly `pub(bind)`.
pub(super) const fn validate(decl: InterfaceDecl) -> Result<(), Violation> {
    match decl {
        InterfaceDecl::InputPort {
            requirement: Requirement::Defaulted | Requirement::Required,
        }
        | InterfaceDecl::Nominal {
            requirement: Requirement::Defaulted,
            ..
        } => Ok(()),
        InterfaceDecl::Nominal {
            kind,
            visibility,
            requirement: Requirement::Required,
        } => {
            if visibility.is_bindable() {
                Ok(())
            } else {
                Err(Violation::RequiredMustBeBindable { kind })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOMINAL_KINDS: [NominalKind; 3] = [
        NominalKind::Index,
        NominalKind::Type,
        NominalKind::Dimension,
    ];
    const VISIBILITIES: [BindableVisibility; 3] = [
        BindableVisibility::Private,
        BindableVisibility::Public,
        BindableVisibility::PublicBind,
    ];

    #[test]
    fn every_input_port_state_is_valid() {
        assert!(
            [Requirement::Defaulted, Requirement::Required]
                .into_iter()
                .all(|requirement| validate(InterfaceDecl::InputPort { requirement }) == Ok(()))
        );
    }

    #[test]
    fn every_defaulted_nominal_state_is_valid() {
        assert!(NOMINAL_KINDS.into_iter().all(|kind| {
            VISIBILITIES.into_iter().all(|visibility| {
                validate(InterfaceDecl::Nominal {
                    kind,
                    visibility,
                    requirement: Requirement::Defaulted,
                }) == Ok(())
            })
        }));
    }

    #[test]
    fn required_nominals_are_valid_exactly_when_bindable() {
        assert!(NOMINAL_KINDS.into_iter().all(|kind| {
            VISIBILITIES.into_iter().all(|visibility| {
                let actual = validate(InterfaceDecl::Nominal {
                    kind,
                    visibility,
                    requirement: Requirement::Required,
                });
                let expected = match visibility {
                    BindableVisibility::PublicBind => Ok(()),
                    BindableVisibility::Private | BindableVisibility::Public => {
                        Err(Violation::RequiredMustBeBindable { kind })
                    }
                };
                actual == expected
            })
        }));
    }
}
