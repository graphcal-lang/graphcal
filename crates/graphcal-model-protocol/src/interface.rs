//! The static interface of a model: its parameters and public outputs.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::Domain;
use crate::label::PortName;
use crate::wire_type::{PortWire, WireType};

/// One model parameter, in declaration order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "ParameterRepr", into = "ParameterRepr")]
pub struct ParameterDescription {
    name: PortName,
    graphcal_type: String,
    wire: PortWire,
    domain: Option<Domain>,
    has_default: bool,
}

/// Why a parameter description is inconsistent.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ParameterError {
    #[error("parameter `{name}` declares a domain its wire type cannot have")]
    DomainKindMismatch { name: PortName },
}

impl ParameterDescription {
    /// Describe one parameter.
    ///
    /// `graphcal_type` is the human-readable type spelling, such as `Force`.
    ///
    /// # Errors
    ///
    /// Returns [`ParameterError::DomainKindMismatch`] when `domain` does not
    /// match `wire`'s innermost element: quantity bounds need a quantity,
    /// integer bounds an integer, datetime bounds a datetime, and an
    /// unsupported port has no domain.
    pub fn new(
        name: PortName,
        graphcal_type: impl Into<String>,
        wire: PortWire,
        domain: Option<Domain>,
        has_default: bool,
    ) -> Result<Self, ParameterError> {
        let consistent = match (wire.supported().map(WireType::leaf), &domain) {
            (_, None)
            | (Some(WireType::Quantity(_)), Some(Domain::Quantity(_)))
            | (Some(WireType::Int), Some(Domain::Int(_)))
            | (Some(WireType::Datetime(_)), Some(Domain::Datetime(_))) => true,
            (_, Some(_)) => false,
        };
        if !consistent {
            return Err(ParameterError::DomainKindMismatch { name });
        }
        Ok(Self {
            name,
            graphcal_type: graphcal_type.into(),
            wire,
            domain,
            has_default,
        })
    }

    /// The parameter name.
    #[must_use]
    pub const fn name(&self) -> &PortName {
        &self.name
    }

    /// The human-readable type spelling.
    #[must_use]
    pub fn graphcal_type(&self) -> &str {
        &self.graphcal_type
    }

    /// How the parameter travels on the wire.
    #[must_use]
    pub const fn wire(&self) -> &PortWire {
        &self.wire
    }

    /// The declared domain, in the wire unit.
    #[must_use]
    pub const fn domain(&self) -> Option<&Domain> {
        self.domain.as_ref()
    }

    /// Whether the model supplies a value when a session does not vary it.
    #[must_use]
    pub const fn has_default(&self) -> bool {
        self.has_default
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParameterRepr {
    name: PortName,
    graphcal_type: String,
    wire: PortWire,
    domain: Option<Domain>,
    has_default: bool,
}

impl TryFrom<ParameterRepr> for ParameterDescription {
    type Error = ParameterError;

    fn try_from(repr: ParameterRepr) -> Result<Self, Self::Error> {
        Self::new(
            repr.name,
            repr.graphcal_type,
            repr.wire,
            repr.domain,
            repr.has_default,
        )
    }
}

impl From<ParameterDescription> for ParameterRepr {
    fn from(parameter: ParameterDescription) -> Self {
        Self {
            name: parameter.name,
            graphcal_type: parameter.graphcal_type,
            wire: parameter.wire,
            domain: parameter.domain,
            has_default: parameter.has_default,
        }
    }
}

/// One public model output, in declaration order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputDescription {
    name: PortName,
    graphcal_type: String,
    wire: PortWire,
}

impl OutputDescription {
    /// Describe one output; `graphcal_type` is its human-readable type
    /// spelling.
    #[must_use]
    pub fn new(name: PortName, graphcal_type: impl Into<String>, wire: PortWire) -> Self {
        Self {
            name,
            graphcal_type: graphcal_type.into(),
            wire,
        }
    }

    /// The output name.
    #[must_use]
    pub const fn name(&self) -> &PortName {
        &self.name
    }

    /// The human-readable type spelling.
    #[must_use]
    pub fn graphcal_type(&self) -> &str {
        &self.graphcal_type
    }

    /// How the output travels on the wire.
    #[must_use]
    pub const fn wire(&self) -> &PortWire {
        &self.wire
    }
}

/// The parameters and public outputs of a model, each in declaration order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "InterfaceRepr", into = "InterfaceRepr")]
pub struct ModelInterface {
    parameters: Vec<ParameterDescription>,
    outputs: Vec<OutputDescription>,
}

/// Why a model interface is inconsistent.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InterfaceError {
    #[error("parameter `{name}` is listed more than once")]
    DuplicateParameter { name: PortName },
    #[error("output `{name}` is listed more than once")]
    DuplicateOutput { name: PortName },
}

impl ModelInterface {
    /// An interface over `parameters` and `outputs`, each in declaration
    /// order.
    ///
    /// # Errors
    ///
    /// Returns [`InterfaceError`] when a parameter or output name repeats.
    pub fn new(
        parameters: Vec<ParameterDescription>,
        outputs: Vec<OutputDescription>,
    ) -> Result<Self, InterfaceError> {
        let mut seen = HashSet::with_capacity(parameters.len());
        if let Some(parameter) = parameters
            .iter()
            .find(|parameter| !seen.insert(&parameter.name))
        {
            return Err(InterfaceError::DuplicateParameter {
                name: parameter.name.clone(),
            });
        }
        let mut seen = HashSet::with_capacity(outputs.len());
        if let Some(output) = outputs.iter().find(|output| !seen.insert(&output.name)) {
            return Err(InterfaceError::DuplicateOutput {
                name: output.name.clone(),
            });
        }
        Ok(Self {
            parameters,
            outputs,
        })
    }

    /// Every parameter in declaration order.
    #[must_use]
    pub fn parameters(&self) -> &[ParameterDescription] {
        &self.parameters
    }

    /// Every public output in declaration order.
    #[must_use]
    pub fn outputs(&self) -> &[OutputDescription] {
        &self.outputs
    }

    /// The parameter named `name`.
    #[must_use]
    pub fn parameter(&self, name: &str) -> Option<&ParameterDescription> {
        self.parameters
            .iter()
            .find(|parameter| parameter.name.as_str() == name)
    }

    /// The output named `name`.
    #[must_use]
    pub fn output(&self, name: &str) -> Option<&OutputDescription> {
        self.outputs
            .iter()
            .find(|output| output.name.as_str() == name)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InterfaceRepr {
    parameters: Vec<ParameterDescription>,
    outputs: Vec<OutputDescription>,
}

impl TryFrom<InterfaceRepr> for ModelInterface {
    type Error = InterfaceError;

    fn try_from(repr: InterfaceRepr) -> Result<Self, Self::Error> {
        Self::new(repr.parameters, repr.outputs)
    }
}

impl From<ModelInterface> for InterfaceRepr {
    fn from(interface: ModelInterface) -> Self {
        Self {
            parameters: interface.parameters,
            outputs: interface.outputs,
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::dimension::{Dimension, Exponent};
    use crate::domain::Bounds;
    use crate::finite::FiniteF64;
    use crate::label::{BaseDimensionName, IndexName, UnitSpelling, VariantName};
    use crate::wire_type::{KeyType, QuantityType};

    pub fn name(name: &str) -> PortName {
        PortName::new(name).unwrap()
    }

    pub fn length() -> WireType {
        WireType::Quantity(
            QuantityType::new(
                Dimension::from_iter([(
                    BaseDimensionName::new("Length").unwrap(),
                    Exponent::new(1, 1).unwrap(),
                )]),
                Some(UnitSpelling::new("m").unwrap()),
            )
            .unwrap(),
        )
    }

    pub fn mode() -> WireType {
        WireType::Key(
            KeyType::new(
                IndexName::new("Mode").unwrap(),
                vec![
                    VariantName::new("A").unwrap(),
                    VariantName::new("B").unwrap(),
                ],
            )
            .unwrap(),
        )
    }

    /// `load: Length(0..10 m)`, `count: Int = 1`, `mode: Key<Mode>`,
    /// `history: <unsupported> = …`; outputs `failure: Bool`,
    /// `margin: Length`, `series: <unsupported>`.
    pub fn sample_interface() -> ModelInterface {
        let finite = |value| FiniteF64::new(value).unwrap();
        ModelInterface::new(
            vec![
                ParameterDescription::new(
                    name("load"),
                    "Length",
                    PortWire::Supported(length()),
                    Some(Domain::Quantity(
                        Bounds::new(Some(finite(0.0)), Some(finite(10.0))).unwrap(),
                    )),
                    false,
                )
                .unwrap(),
                ParameterDescription::new(
                    name("count"),
                    "Int",
                    PortWire::Supported(WireType::Int),
                    Some(Domain::Int(Bounds::new(Some(0), None).unwrap())),
                    true,
                )
                .unwrap(),
                ParameterDescription::new(
                    name("mode"),
                    "Key<Mode>",
                    PortWire::Supported(mode()),
                    None,
                    false,
                )
                .unwrap(),
                ParameterDescription::new(
                    name("history"),
                    "Length[Step]",
                    PortWire::Unsupported {
                        reason: "indexed values are not supported".to_owned(),
                    },
                    None,
                    true,
                )
                .unwrap(),
            ],
            vec![
                OutputDescription::new(
                    name("failure"),
                    "Bool",
                    PortWire::Supported(WireType::Bool),
                ),
                OutputDescription::new(name("margin"), "Length", PortWire::Supported(length())),
                OutputDescription::new(
                    name("series"),
                    "Length[Step]",
                    PortWire::Unsupported {
                        reason: "indexed values are not supported".to_owned(),
                    },
                ),
            ],
        )
        .unwrap()
    }

    #[test]
    fn domains_must_match_the_wire_type() {
        let error = ParameterDescription::new(
            name("flag"),
            "Bool",
            PortWire::Supported(WireType::Bool),
            Some(Domain::Int(Bounds::new(Some(0), Some(1)).unwrap())),
            false,
        )
        .unwrap_err();
        assert_eq!(
            error,
            ParameterError::DomainKindMismatch { name: name("flag") }
        );
    }

    #[test]
    fn interface_names_are_unique_per_list() {
        let output =
            || OutputDescription::new(name("x"), "Bool", PortWire::Supported(WireType::Bool));
        assert_eq!(
            ModelInterface::new(Vec::new(), vec![output(), output()]),
            Err(InterfaceError::DuplicateOutput { name: name("x") })
        );
    }

    #[test]
    fn interface_round_trips_through_json() {
        let interface = sample_interface();
        let json = serde_json::to_string(&interface).unwrap();
        assert_eq!(
            serde_json::from_str::<ModelInterface>(&json).unwrap(),
            interface
        );
    }
}
