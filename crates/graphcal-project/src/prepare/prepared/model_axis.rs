//! The protocol axes and dimensions of model index and quantity schemas.
//!
//! A declared index or base dimension travels by its [`DeclaredName`]: the
//! package, module path, and name the user writes, taken from the module
//! identity of its declaration and never from file paths. An axis or
//! dimension the protocol cannot carry yields an [`Unsupported`] reason
//! instead.

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::dimension::{BaseDimId, Dimension as GraphcalDimension};
use graphcal_model_protocol::{
    Axis, AxisKind, BaseDimension, BaseDimensionName, DeclaredName, Dimension, Exponent, FiniteF64,
    IndexName, LabelError, ModuleName, PackageName, UnsupportedReason, VariantName,
};

use crate::prepare::model_schema::{ModelIndexKind, ModelIndexSchema};

/// Why the protocol cannot carry a port's type: a stable reason and a
/// human-readable message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Unsupported {
    pub(super) reason: UnsupportedReason,
    pub(super) message: String,
}

impl Unsupported {
    pub(super) fn new(reason: UnsupportedReason, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: message.into(),
        }
    }
}

/// The wire identity of the declaration `name` of the module `owner`, or
/// `None` when no module path names `owner` (a concrete include instance).
fn declared_name<L>(owner: &DagId, name: L) -> Result<Option<DeclaredName<L>>, LabelError> {
    let Some(path) = owner.module_path_key() else {
        return Ok(None);
    };
    let (package, module) = path.segments().split_first();
    Ok(Some(DeclaredName::new(
        PackageName::new(package.as_str())?,
        module
            .iter()
            .map(|segment| ModuleName::new(segment.as_str()))
            .collect::<Result<_, _>>()?,
        name,
    )))
}

/// The protocol base dimension of `base`, or why it has none.
fn base_dimension(base: &BaseDimId) -> Result<Result<BaseDimension, Unsupported>, LabelError> {
    Ok(Ok(match base {
        BaseDimId::Prelude(prelude) => {
            BaseDimension::prelude(BaseDimensionName::new(prelude.as_str())?)
        }
        BaseDimId::UserDefined(resolved) => {
            let Some(name) =
                declared_name(resolved.owner(), BaseDimensionName::new(resolved.as_str())?)?
            else {
                return Ok(Err(Unsupported::new(
                    UnsupportedReason::UnnamedDeclaration,
                    format!(
                        "base dimension `{}` is declared where no module path names it",
                        resolved.as_str()
                    ),
                )));
            };
            BaseDimension::Declared(name)
        }
    }))
}

/// The protocol dimension of `dimension`, or why it has none.
pub(super) fn protocol_dimension(
    dimension: &GraphcalDimension,
) -> Result<Result<Dimension, Unsupported>, LabelError> {
    let mut exponents = Vec::new();
    for (base, exponent) in dimension.iter() {
        let base = match base_dimension(base)? {
            Ok(base) => base,
            Err(unsupported) => return Ok(Err(unsupported)),
        };
        let Some(exponent) = u32::try_from(exponent.den())
            .ok()
            .and_then(|denominator| Exponent::new(exponent.num(), denominator).ok())
        else {
            return Ok(Err(Unsupported::new(
                UnsupportedReason::UnrepresentableExponent,
                format!("dimension exponent {exponent} is not representable"),
            )));
        };
        exponents.push((base, exponent));
    }
    Ok(Dimension::new(exponents).map_err(|error| {
        Unsupported::new(UnsupportedReason::AmbiguousBaseDimension, error.to_string())
    }))
}

/// The protocol axis of `index`, or why it has none.
pub(super) fn axis_wire(index: &ModelIndexSchema) -> Result<Result<Axis, Unsupported>, LabelError> {
    let declared = || -> Result<Result<DeclaredName<IndexName>, Unsupported>, LabelError> {
        let unnamed = || {
            Unsupported::new(
                UnsupportedReason::UnnamedDeclaration,
                format!(
                    "index `{}` is declared where no module path names it",
                    index.identity().display_name()
                ),
            )
        };
        let Some(resolved) = index.identity().declared_resolved() else {
            return Ok(Err(unnamed()));
        };
        Ok(
            declared_name(resolved.owner(), IndexName::new(resolved.as_str())?)?
                .ok_or_else(unnamed),
        )
    };
    let kind = match index.kind() {
        ModelIndexKind::Named { variants } => AxisKind::Named {
            index: match declared()? {
                Ok(index) => index,
                Err(unsupported) => return Ok(Err(unsupported)),
            },
            variants: variants
                .as_slice()
                .iter()
                .map(|variant| VariantName::new(variant.as_str()))
                .collect::<Result<_, _>>()?,
        },
        ModelIndexKind::Finite { index } => AxisKind::Finite {
            size: index.cardinality().get(),
        },
        ModelIndexKind::Coordinate {
            coordinates_si,
            dimension,
            ..
        } => {
            let declared = match declared()? {
                Ok(index) => index,
                Err(unsupported) => return Ok(Err(unsupported)),
            };
            let dimension = match protocol_dimension(dimension)? {
                Ok(dimension) => dimension,
                Err(unsupported) => return Ok(Err(unsupported)),
            };
            let Ok(coordinates) = coordinates_si
                .iter()
                .map(|coordinate| FiniteF64::new(*coordinate))
                .collect::<Result<Vec<_>, _>>()
            else {
                return Ok(Err(Unsupported::new(
                    UnsupportedReason::NonFiniteCoordinate,
                    format!("index `{declared}` has a non-finite coordinate"),
                )));
            };
            AxisKind::Coordinate {
                index: declared,
                dimension,
                coordinates,
            }
        }
    };
    Ok(Axis::new(kind)
        .map_err(|error| Unsupported::new(UnsupportedReason::InvalidAxis, error.to_string())))
}
