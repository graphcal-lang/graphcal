//! The Graphcal prelude catalog: its base and derived dimensions and its
//! units, as plain semantic values.
//!
//! The catalog declares no identities. The resolver's prelude scope
//! ([`crate::resolve::prelude::prelude_type_scope`]) names these entries, and
//! [`crate::ir::prelude_definitions`] turns them into owner-qualified static
//! definitions.

use crate::dag_id::DagId;
use crate::dimension::{BaseDimId, Dimension, PreludeBaseDimension};
use crate::ratio::RatioError;
use crate::semantic::dimension_table::BaseDimensionInfo;
use crate::semantic::unit_scale::{PositiveFiniteScale, UnitInfo, UnitScale};
use crate::syntax::dimension::UnitName;

/// Canonical synthetic owner for Graphcal prelude type-system symbols.
///
/// Prelude names are implicitly in scope, so they do not have a source module
/// alias. HIR still needs a canonical owner for resolved names; this synthetic
/// [`DagId`] is that owner at the compiler boundary.
const PRELUDE_DAG_ID_SEGMENT: &str = "__graphcal_prelude__";

/// Look up a prelude *base* dimension by name.
///
/// Returns the single-factor [`Dimension`] for one of
/// [`PreludeBaseDimension::ALL_NAMES`], without needing a loaded registry, and
/// `None` for every other name (including prelude-derived dimensions, which
/// exist only through registry resolution).
///
/// The base dimensions are the dimension alphabet of boundaries that carry
/// dimensions structurally as base-dimension exponent vectors — in particular
/// WASM plugin manifests (ABI v1, #25), which may reference these names and
/// nothing else. Every other dimension (prelude-derived or user-defined
/// derived) reduces to exponents over base dimensions; user-defined *base*
/// dimensions are scoped to their defining module and never cross such
/// boundaries.
#[must_use]
pub fn prelude_base_dimension(name: &str) -> Option<Dimension> {
    PreludeBaseDimension::parse(name).map(|base| Dimension::base(BaseDimId::Prelude(base)))
}

/// Canonical synthetic owner for Graphcal prelude symbols.
#[must_use]
pub fn prelude_dag_id() -> DagId {
    DagId::root_in_package(PRELUDE_DAG_ID_SEGMENT, PRELUDE_DAG_ID_SEGMENT)
}

// Declaration table. Registration and the public name lists are generated
// from these declarations, so they cannot drift apart.

/// A prelude dimension as integer exponents over prelude base dimensions.
type BaseFactors = &'static [(PreludeBaseDimension, i16)];

const LENGTH: BaseFactors = &[(PreludeBaseDimension::Length, 1)];
const TIME: BaseFactors = &[(PreludeBaseDimension::Time, 1)];
const MASS: BaseFactors = &[(PreludeBaseDimension::Mass, 1)];
const ANGLE: BaseFactors = &[(PreludeBaseDimension::Angle, 1)];
const FORCE: BaseFactors = &[
    (PreludeBaseDimension::Mass, 1),
    (PreludeBaseDimension::Length, 1),
    (PreludeBaseDimension::Time, -2),
];
const ENERGY: BaseFactors = &[
    (PreludeBaseDimension::Mass, 1),
    (PreludeBaseDimension::Length, 2),
    (PreludeBaseDimension::Time, -2),
];
const POWER: BaseFactors = &[
    (PreludeBaseDimension::Mass, 1),
    (PreludeBaseDimension::Length, 2),
    (PreludeBaseDimension::Time, -3),
];
const PRESSURE: BaseFactors = &[
    (PreludeBaseDimension::Mass, 1),
    (PreludeBaseDimension::Length, -1),
    (PreludeBaseDimension::Time, -2),
];
const FREQUENCY: BaseFactors = &[(PreludeBaseDimension::Time, -1)];

/// The display symbol of a base dimension, which is also the spelling of its
/// coherent base unit (scale 1).
const fn base_symbol(base: PreludeBaseDimension) -> &'static str {
    match base {
        PreludeBaseDimension::Length => "m",
        PreludeBaseDimension::Time => "s",
        PreludeBaseDimension::Mass => "kg",
        PreludeBaseDimension::Temperature => "K",
        PreludeBaseDimension::ElectricCurrent => "A",
        PreludeBaseDimension::Amount => "mol",
        PreludeBaseDimension::LuminousIntensity => "cd",
        PreludeBaseDimension::Angle => "rad",
    }
}

/// Whether real-world units of this base dimension are affine scales.
///
/// Temperature units (°C, °F) are affine, so user unit definitions on bare
/// Temperature are rejected: a linear definition would silently display wrong
/// values (#648 U4).
const fn is_affine_prone(base: PreludeBaseDimension) -> bool {
    matches!(base, PreludeBaseDimension::Temperature)
}

/// A named prelude dimension derived from the base dimensions.
struct DerivedDimensionDecl {
    name: &'static str,
    factors: BaseFactors,
}

const fn dimension(name: &'static str, factors: BaseFactors) -> DerivedDimensionDecl {
    DerivedDimensionDecl { name, factors }
}

/// Derived prelude dimensions, in registration order.
const DERIVED_DIMENSIONS: &[DerivedDimensionDecl] = &[
    dimension(
        "Velocity",
        &[
            (PreludeBaseDimension::Length, 1),
            (PreludeBaseDimension::Time, -1),
        ],
    ),
    dimension(
        "Acceleration",
        &[
            (PreludeBaseDimension::Length, 1),
            (PreludeBaseDimension::Time, -2),
        ],
    ),
    dimension("Force", FORCE),
    dimension("Energy", ENERGY),
    dimension("Power", POWER),
    dimension("Frequency", FREQUENCY),
    dimension("Pressure", PRESSURE),
    dimension("Area", &[(PreludeBaseDimension::Length, 2)]),
    dimension("Volume", &[(PreludeBaseDimension::Length, 3)]),
];

/// A prelude unit other than a coherent base unit.
struct DerivedUnitDecl {
    name: &'static str,
    factors: BaseFactors,
    scale: PositiveFiniteScale,
}

/// Evaluated only in the `DERIVED_UNITS` const initializer, so an invalid
/// built-in scale fails the build.
const fn unit(name: &'static str, factors: BaseFactors, scale: f64) -> DerivedUnitDecl {
    DerivedUnitDecl {
        name,
        factors,
        scale: PositiveFiniteScale::from_const(scale),
    }
}

/// Prelude units other than the coherent base units, in registration order.
const DERIVED_UNITS: &[DerivedUnitDecl] = &[
    unit("km", LENGTH, 1000.0),
    unit("cm", LENGTH, 0.01),
    unit("mm", LENGTH, 0.001),
    unit("h", TIME, 3600.0),
    unit("min", TIME, 60.0),
    unit("deg", ANGLE, std::f64::consts::PI / 180.0),
    unit("g", MASS, 0.001),
    unit("N", FORCE, 1.0),
    unit("kN", FORCE, 1000.0),
    unit("J", ENERGY, 1.0),
    unit("kJ", ENERGY, 1000.0),
    unit("W", POWER, 1.0),
    unit("kW", POWER, 1000.0),
    unit("Pa", PRESSURE, 1.0),
    unit("kPa", PRESSURE, 1000.0),
    unit("MPa", PRESSURE, 1_000_000.0),
    unit("Hz", FREQUENCY, 1.0),
];

/// Dimension names provided by the Graphcal prelude: base dimensions first,
/// then derived dimensions, in registration order.
pub(crate) fn prelude_dimension_names() -> impl Iterator<Item = &'static str> + Clone {
    PreludeBaseDimension::ALL_NAMES
        .into_iter()
        .chain(DERIVED_DIMENSIONS.iter().map(|decl| decl.name))
}

/// Unit names provided by the Graphcal prelude: coherent base units first,
/// then the other units, in registration order.
pub fn prelude_unit_names() -> impl Iterator<Item = &'static str> + Clone {
    PreludeBaseDimension::ALL
        .into_iter()
        .map(base_symbol)
        .chain(DERIVED_UNITS.iter().map(|decl| decl.name))
}

fn dimension_of(factors: BaseFactors) -> Result<Dimension, RatioError> {
    factors
        .iter()
        .try_fold(Dimension::dimensionless(), |product, &(base, exponent)| {
            product * Dimension::base(BaseDimId::Prelude(base)).pow(exponent)?
        })
}

/// Metadata of every prelude base dimension: its coherent unit and whether
/// its real-world units are affine.
pub fn prelude_base_dimension_infos()
-> impl Iterator<Item = (PreludeBaseDimension, BaseDimensionInfo)> {
    PreludeBaseDimension::ALL.into_iter().map(|base| {
        (
            base,
            BaseDimensionInfo::new(
                Some(UnitName::expect_valid(base_symbol(base))),
                is_affine_prone(base),
            ),
        )
    })
}

/// Every named prelude dimension with its value: base dimensions first, then
/// derived dimensions, in registration order.
///
/// # Errors
///
/// Returns an error only if a built-in derived dimension overflows, which
/// would be a compiler bug.
pub fn prelude_dimensions() -> Result<Vec<(&'static str, Dimension)>, RatioError> {
    PreludeBaseDimension::ALL
        .into_iter()
        .map(|base| Ok((base.as_str(), Dimension::base(BaseDimId::Prelude(base)))))
        .chain(
            DERIVED_DIMENSIONS
                .iter()
                .map(|decl| Ok((decl.name, dimension_of(decl.factors)?))),
        )
        .collect()
}

/// Every prelude unit with its dimension and scale: coherent base units
/// first, then the other units, in registration order.
///
/// # Errors
///
/// Returns an error only if a built-in unit dimension overflows, which would
/// be a compiler bug.
pub fn prelude_units() -> Result<Vec<(&'static str, UnitInfo)>, RatioError> {
    PreludeBaseDimension::ALL
        .into_iter()
        .map(|base| {
            Ok((
                base_symbol(base),
                UnitInfo {
                    dimension: Dimension::base(BaseDimId::Prelude(base)),
                    scale: UnitScale::Const(PositiveFiniteScale::ONE),
                },
            ))
        })
        .chain(DERIVED_UNITS.iter().map(|decl| {
            Ok((
                decl.name,
                UnitInfo {
                    dimension: dimension_of(decl.factors)?,
                    scale: UnitScale::Const(decl.scale),
                },
            ))
        }))
        .collect()
}
