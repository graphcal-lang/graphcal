//! Namespace-specific policy for source-visible reserved names.
//!
//! A spelling is never globally reserved. Callers must state the semantic
//! namespace they are introducing so graph values, units, and type-system
//! symbols cannot accidentally inherit one another's policy.

use thiserror::Error;

use crate::builtin::BuiltinConst;
use crate::registry::prelude::{
    PRELUDE_BUILTIN_TYPE_NAMES, PRELUDE_DIMENSION_NAMES, PRELUDE_UNIT_NAMES,
};
use crate::syntax::names::NameAtom;

/// Semantic namespace into which a source-visible local name is introduced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReservedNameNamespace {
    /// Dimensions, built-in types, nominal types, and indexes.
    TypeSystem,
    /// Unit declarations and aliases.
    Unit,
    /// Params, nodes, and const nodes referenced with `@`.
    GraphValue,
}

impl std::fmt::Display for ReservedNameNamespace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::TypeSystem => "type-system",
            Self::Unit => "unit",
            Self::GraphValue => "graph-value",
        })
    }
}

/// Closed semantic category that reserves a spelling in one namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReservedName {
    PreludeDimension,
    BuiltinType,
    PreludeUnit,
    BuiltinConstant(BuiltinConst),
}

impl std::fmt::Display for ReservedName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PreludeDimension => f.write_str("prelude dimension"),
            Self::BuiltinType => f.write_str("built-in type"),
            Self::PreludeUnit => f.write_str("prelude unit"),
            Self::BuiltinConstant(constant) => write!(f, "built-in constant `{constant}`"),
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
/// Time-scale and built-in-function spellings are intentionally absent from
/// the graph-value policy. Syntax disambiguates `@UTC` from `Datetime<UTC>`,
/// and function calls from graph references. Built-in numeric constants remain
/// reserved because omitting `@` could otherwise select a valid numeric value.
pub fn validate_reserved_name(
    namespace: ReservedNameNamespace,
    name: &NameAtom,
) -> Result<(), ReservedNameError<'_>> {
    let reserved = match namespace {
        ReservedNameNamespace::TypeSystem => {
            if PRELUDE_DIMENSION_NAMES.contains(&name.as_str()) {
                Some(ReservedName::PreludeDimension)
            } else if PRELUDE_BUILTIN_TYPE_NAMES.contains(&name.as_str()) {
                Some(ReservedName::BuiltinType)
            } else {
                None
            }
        }
        ReservedNameNamespace::Unit => PRELUDE_UNIT_NAMES
            .contains(&name.as_str())
            .then_some(ReservedName::PreludeUnit),
        ReservedNameNamespace::GraphValue => {
            BuiltinConst::parse(name.as_str()).map(ReservedName::BuiltinConstant)
        }
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
    use crate::builtin::BuiltinFnName;
    use crate::registry::time_scale::TimeScale;

    use super::*;

    #[test]
    fn each_namespace_rejects_its_complete_reserved_vocabulary() {
        for name in PRELUDE_DIMENSION_NAMES
            .iter()
            .chain(PRELUDE_BUILTIN_TYPE_NAMES)
        {
            let atom = NameAtom::parse(*name).unwrap();
            assert!(validate_reserved_name(ReservedNameNamespace::TypeSystem, &atom).is_err());
        }
        for name in PRELUDE_UNIT_NAMES {
            let atom = NameAtom::parse(*name).unwrap();
            assert!(validate_reserved_name(ReservedNameNamespace::Unit, &atom).is_err());
        }
        for constant in BuiltinConst::ALL {
            let atom = NameAtom::parse(constant.as_str()).unwrap();
            assert!(validate_reserved_name(ReservedNameNamespace::GraphValue, &atom).is_err());
        }
    }

    #[test]
    fn graph_values_allow_time_scales_and_function_names() {
        for name in TimeScale::ALL
            .map(|scale| scale.to_string())
            .into_iter()
            .chain(
                BuiltinFnName::ALL
                    .iter()
                    .map(std::string::ToString::to_string),
            )
        {
            let atom = NameAtom::parse(name).unwrap();
            assert!(validate_reserved_name(ReservedNameNamespace::GraphValue, &atom).is_ok());
        }
    }

    #[test]
    fn reservations_do_not_leak_between_namespaces() {
        let metre = NameAtom::parse("m").unwrap();
        assert!(validate_reserved_name(ReservedNameNamespace::GraphValue, &metre).is_ok());
        let e = NameAtom::parse("E").unwrap();
        assert!(validate_reserved_name(ReservedNameNamespace::Unit, &e).is_ok());
    }
}
