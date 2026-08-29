//! Namespace-specific policy for source-visible reserved names.
//!
//! A spelling is never globally reserved. Callers must state the semantic
//! namespace they are introducing so graph values, units, and type-system
//! symbols cannot accidentally inherit one another's policy.

use thiserror::Error;

use crate::builtin::{BuiltinConst, BuiltinFnName};
use crate::registry::prelude::{
    PRELUDE_BUILTIN_TYPE_NAMES, PRELUDE_DIMENSION_NAMES, PRELUDE_UNIT_NAMES,
};
use crate::registry::time_scale::TimeScale;
use crate::syntax::names::NameAtom;

/// Semantic namespace into which a source-visible local name is introduced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReservedNameNamespace {
    Static,
    Term,
    Unit,
}

impl std::fmt::Display for ReservedNameNamespace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Static => "Static",
            Self::Term => "Term",
            Self::Unit => "Unit",
        })
    }
}

/// Closed semantic category that reserves a spelling in one namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReservedName {
    PreludeDimension,
    BuiltinType,
    PreludeUnit,
    TimeScale(TimeScale),
    StaticFormer,
    BuiltinConstant(BuiltinConst),
    BuiltinFunction(BuiltinFnName),
    ContextualCallable,
}

impl std::fmt::Display for ReservedName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PreludeDimension => f.write_str("prelude dimension"),
            Self::BuiltinType => f.write_str("built-in type"),
            Self::PreludeUnit => f.write_str("prelude unit"),
            Self::TimeScale(scale) => write!(f, "time scale `{scale}`"),
            Self::StaticFormer => f.write_str("built-in Static former"),
            Self::BuiltinConstant(constant) => write!(f, "built-in constant `{constant}`"),
            Self::BuiltinFunction(function) => write!(f, "built-in function `{function}`"),
            Self::ContextualCallable => f.write_str("contextual callable"),
        }
    }
}

/// A local name collides with the reservation for its semantic namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("{name} is reserved by the {reserved} in the {namespace} namespace")]
pub struct ReservedNameError<'a> {
    pub namespace: ReservedNameNamespace,
    pub name: &'a NameAtom,
    pub reserved: ReservedName,
}

/// Validate one local name against exactly one semantic namespace.
///
/// Every built-in occupies exactly one of Static, Term, or Unit. Cross-
/// namespace reuse is accepted; same-namespace reuse is rejected uniformly.
pub fn validate_reserved_name(
    namespace: ReservedNameNamespace,
    name: &NameAtom,
) -> Result<(), ReservedNameError<'_>> {
    let reserved = match namespace {
        ReservedNameNamespace::Static => {
            if PRELUDE_DIMENSION_NAMES.contains(&name.as_str()) {
                Some(ReservedName::PreludeDimension)
            } else if PRELUDE_BUILTIN_TYPE_NAMES.contains(&name.as_str()) {
                Some(ReservedName::BuiltinType)
            } else if let Ok(scale) = name.as_str().parse::<TimeScale>() {
                Some(ReservedName::TimeScale(scale))
            } else if matches!(name.as_str(), "Fin" | "range" | "linspace") {
                Some(ReservedName::StaticFormer)
            } else {
                None
            }
        }
        ReservedNameNamespace::Term => BuiltinConst::parse(name.as_str())
            .map(ReservedName::BuiltinConstant)
            .or_else(|| BuiltinFnName::parse(name.as_str()).map(ReservedName::BuiltinFunction))
            .or_else(|| {
                matches!(
                    name.as_str(),
                    "scan"
                        | "unfold"
                        | "key"
                        | "fin_key"
                        | "floor_key"
                        | "ceil_key"
                        | "nearest_key"
                )
                .then_some(ReservedName::ContextualCallable)
            }),
        ReservedNameNamespace::Unit => PRELUDE_UNIT_NAMES
            .contains(&name.as_str())
            .then_some(ReservedName::PreludeUnit),
    };

    reserved.map_or(Ok(()), |reserved| {
        Err(ReservedNameError {
            namespace,
            name,
            reserved,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_namespace_rejects_its_complete_reserved_vocabulary() {
        for name in PRELUDE_DIMENSION_NAMES
            .iter()
            .chain(PRELUDE_BUILTIN_TYPE_NAMES)
        {
            let atom = NameAtom::parse(*name).unwrap();
            assert!(validate_reserved_name(ReservedNameNamespace::Static, &atom).is_err());
        }
        for scale in TimeScale::ALL {
            let atom = NameAtom::parse(scale.to_string()).unwrap();
            assert!(validate_reserved_name(ReservedNameNamespace::Static, &atom).is_err());
        }
        for name in ["Fin", "range", "linspace"] {
            let atom = NameAtom::parse(name).unwrap();
            assert!(validate_reserved_name(ReservedNameNamespace::Static, &atom).is_err());
        }
        for name in PRELUDE_UNIT_NAMES {
            let atom = NameAtom::parse(*name).unwrap();
            assert!(validate_reserved_name(ReservedNameNamespace::Unit, &atom).is_err());
        }
        for constant in BuiltinConst::ALL {
            let atom = NameAtom::parse(constant.as_str()).unwrap();
            assert!(validate_reserved_name(ReservedNameNamespace::Term, &atom).is_err());
        }
        for function in BuiltinFnName::ALL {
            let atom = NameAtom::parse(function.to_string()).unwrap();
            assert!(validate_reserved_name(ReservedNameNamespace::Term, &atom).is_err());
        }
        for name in [
            "scan",
            "unfold",
            "key",
            "fin_key",
            "floor_key",
            "ceil_key",
            "nearest_key",
        ] {
            let atom = NameAtom::parse(name).unwrap();
            assert!(validate_reserved_name(ReservedNameNamespace::Term, &atom).is_err());
        }
    }

    #[test]
    fn reservations_do_not_leak_between_namespaces() {
        let metre = NameAtom::parse("m").unwrap();
        assert!(validate_reserved_name(ReservedNameNamespace::Term, &metre).is_ok());
        let utc = NameAtom::parse("UTC").unwrap();
        assert!(validate_reserved_name(ReservedNameNamespace::Term, &utc).is_ok());
        let sqrt = NameAtom::parse("sqrt").unwrap();
        assert!(validate_reserved_name(ReservedNameNamespace::Static, &sqrt).is_ok());
        let e = NameAtom::parse("E").unwrap();
        assert!(validate_reserved_name(ReservedNameNamespace::Unit, &e).is_ok());
    }
}
