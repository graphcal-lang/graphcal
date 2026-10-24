//! Session configuration: which parameters a client varies, which outputs
//! it reads, and the unit each quantity port travels in.

use std::collections::{HashMap, HashSet};

use thiserror::Error;

use crate::domain::{Bounds, Domain};
use crate::fingerprint::InterfaceFingerprint;
use crate::finite::FiniteF64;
use crate::interface::ModelInterface;
use crate::label::{PortName, UnitSpelling};
use crate::unit_scale::SiScale;
use crate::wire_type::{PortWire, WireType};

/// A unit a session chooses for one selected quantity port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitChoice {
    port: PortName,
    unit: UnitSpelling,
}

impl UnitChoice {
    /// The selected port.
    #[must_use]
    pub const fn port(&self) -> &PortName {
        &self.port
    }

    /// The unit its values travel in.
    #[must_use]
    pub const fn unit(&self) -> &UnitSpelling {
        &self.unit
    }
}

/// The parameters a session varies, the outputs it reads, and the units it
/// chooses for some of them.
///
/// Order is irrelevant: a configured session lists both in the interface's
/// declaration order. A quantity port without a unit choice travels in its
/// canonical SI unit.
///
/// A parameter the session does not vary keeps its default, unless the
/// session fixes it: the model server then binds it to a value it was given
/// when it started, for every row. Fixed parameters do not travel in
/// requests, so they are not part of the [`SessionSchema`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionSpec {
    inputs: Vec<PortName>,
    outputs: Vec<PortName>,
    units: Vec<UnitChoice>,
    fixed: Vec<PortName>,
}

impl SessionSpec {
    /// A session varying `inputs` and reading `outputs` in their canonical
    /// units.
    #[must_use]
    pub const fn new(inputs: Vec<PortName>, outputs: Vec<PortName>) -> Self {
        Self {
            inputs,
            outputs,
            units: Vec::new(),
            fixed: Vec::new(),
        }
    }

    /// Fix the unvaried parameter `port` to a value the model server is
    /// given outside requests, instead of its default.
    #[must_use]
    pub fn with_fixed(mut self, port: PortName) -> Self {
        self.fixed.push(port);
        self
    }

    /// Carry the values of the selected quantity port `port` in `unit`.
    #[must_use]
    pub fn with_unit(mut self, port: PortName, unit: UnitSpelling) -> Self {
        self.units.push(UnitChoice { port, unit });
        self
    }

    /// The parameters to vary.
    #[must_use]
    pub fn inputs(&self) -> &[PortName] {
        &self.inputs
    }

    /// The outputs to read.
    #[must_use]
    pub fn outputs(&self) -> &[PortName] {
        &self.outputs
    }

    /// The chosen units.
    #[must_use]
    pub fn units(&self) -> &[UnitChoice] {
        &self.units
    }

    /// The parameters fixed to values given to the model server.
    #[must_use]
    pub fn fixed(&self) -> &[PortName] {
        &self.fixed
    }
}

/// Resolves a chosen unit spelling of one port to its [`SiScale`].
///
/// Only the model implementation knows its units, so a model server supplies
/// the resolver; a client reuses the scales the server published (see
/// [`crate::arrow::si_scales`]). The resolver also checks that the unit
/// measures the port's dimension.
pub trait UnitResolver {
    /// The scale of `unit` for `port`.
    ///
    /// # Errors
    ///
    /// Returns why `unit` cannot carry `port`'s values.
    fn si_scale(&mut self, port: &PortName, unit: &UnitSpelling) -> Result<SiScale, String>;
}

impl<F: FnMut(&PortName, &UnitSpelling) -> Result<SiScale, String>> UnitResolver for F {
    fn si_scale(&mut self, port: &PortName, unit: &UnitSpelling) -> Result<SiScale, String> {
        self(port, unit)
    }
}

/// A resolver that rejects every unit choice, for sessions in canonical
/// units only.
#[derive(Debug, Clone, Copy, Default)]
pub struct CanonicalUnitsOnly;

impl UnitResolver for CanonicalUnitsOnly {
    fn si_scale(&mut self, _port: &PortName, unit: &UnitSpelling) -> Result<SiScale, String> {
        Err(format!("no unit resolver is available for `{unit}`"))
    }
}

/// Why a session cannot be configured over an interface.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SessionError {
    #[error("a session must vary at least one parameter")]
    NoInputs,
    #[error("a session must read at least one output")]
    NoOutputs,
    #[error("parameter `{name}` is selected more than once")]
    DuplicateInput { name: PortName },
    #[error("output `{name}` is selected more than once")]
    DuplicateOutput { name: PortName },
    #[error("the model has no parameter named `{name}`")]
    UnknownInput { name: PortName },
    #[error("the model has no public output named `{name}`")]
    UnknownOutput { name: PortName },
    #[error("parameter `{name}` cannot be varied: {reason}")]
    UnsupportedInput { name: PortName, reason: String },
    #[error("output `{name}` cannot be read: {reason}")]
    UnsupportedOutput { name: PortName, reason: String },
    #[error("parameter `{name}` has no default, so the session must vary or fix it")]
    MissingDefault { name: PortName },
    #[error("parameter `{name}` is fixed more than once")]
    DuplicateFixed { name: PortName },
    #[error("the model has no parameter named `{name}` to fix")]
    UnknownFixed { name: PortName },
    #[error("parameter `{name}` is both varied and fixed")]
    VariedAndFixed { name: PortName },
    #[error("a unit is chosen more than once for port `{name}`")]
    DuplicateUnit { name: PortName },
    #[error("a unit is chosen for port `{name}`, which the session does not select")]
    UnitForUnselectedPort { name: PortName },
    #[error("a unit is chosen for port `{name}`, which carries no quantities")]
    UnitForNonQuantity { name: PortName },
    #[error("unit `{unit}` cannot carry port `{name}`: {reason}")]
    UnresolvedUnit {
        name: PortName,
        unit: UnitSpelling,
        reason: String,
    },
    #[error("the domain of parameter `{name}` is not representable in its chosen unit")]
    UnrepresentableDomain { name: PortName },
}

/// The position of a varied parameter within a [`SessionSchema`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InputSlot(usize);

impl InputSlot {
    /// The zero-based position among the session's inputs.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }
}

/// The position of a read output within a [`SessionSchema`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OutputSlot(usize);

impl OutputSlot {
    /// The zero-based position among the session's outputs.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }
}

/// One varied parameter of a session.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionInput {
    name: PortName,
    wire: WireType,
    domain: Option<Domain>,
    si_scale: Option<SiScale>,
}

impl SessionInput {
    /// A varied parameter restored from a stored description, whose domain
    /// and scale have already been checked against `wire`.
    #[cfg(feature = "arrow")]
    pub(crate) const fn restored(
        name: PortName,
        wire: WireType,
        domain: Option<Domain>,
        si_scale: Option<SiScale>,
    ) -> Self {
        Self {
            name,
            wire,
            domain,
            si_scale,
        }
    }

    /// The parameter name.
    #[must_use]
    pub const fn name(&self) -> &PortName {
        &self.name
    }

    /// The wire type of its values.
    #[must_use]
    pub const fn wire(&self) -> &WireType {
        &self.wire
    }

    /// The declared domain in the wire unit. Values outside it are row
    /// failures, not protocol errors.
    #[must_use]
    pub const fn domain(&self) -> Option<&Domain> {
        self.domain.as_ref()
    }

    /// The scale of the chosen unit, or `None` for the canonical SI unit.
    #[must_use]
    pub const fn si_scale(&self) -> Option<SiScale> {
        self.si_scale
    }
}

/// One read output of a session.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionOutput {
    name: PortName,
    wire: WireType,
    si_scale: Option<SiScale>,
}

impl SessionOutput {
    /// A read output restored from a stored description, whose scale has
    /// already been checked against `wire`.
    #[cfg(feature = "arrow")]
    pub(crate) const fn restored(
        name: PortName,
        wire: WireType,
        si_scale: Option<SiScale>,
    ) -> Self {
        Self {
            name,
            wire,
            si_scale,
        }
    }

    /// The output name.
    #[must_use]
    pub const fn name(&self) -> &PortName {
        &self.name
    }

    /// The wire type of its values.
    #[must_use]
    pub const fn wire(&self) -> &WireType {
        &self.wire
    }

    /// The scale of the chosen unit, or `None` for the canonical SI unit.
    #[must_use]
    pub const fn si_scale(&self) -> Option<SiScale> {
        self.si_scale
    }
}

/// The request and result shape that both sides of one session share.
///
/// [`ModelInterface::configure`] builds it for a live session, so its inputs
/// and outputs are supported, unique, and in declaration order. A schema
/// restored from a stored session record (see
/// `arrow::RecordSchema::decode`, with the `arrow` feature) has unique,
/// supported inputs and outputs in the record's order, and carries the
/// recorded interface fingerprint without the interface to check it against.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSchema {
    interface_fingerprint: InterfaceFingerprint,
    inputs: Vec<SessionInput>,
    outputs: Vec<SessionOutput>,
}

impl SessionSchema {
    /// A schema restored from a stored description of its ports.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] when there is no input or no output, or when
    /// two inputs or two outputs share a name.
    #[cfg(feature = "arrow")]
    pub(crate) fn restore(
        interface_fingerprint: InterfaceFingerprint,
        inputs: Vec<SessionInput>,
        outputs: Vec<SessionOutput>,
    ) -> Result<Self, SessionError> {
        if inputs.is_empty() {
            return Err(SessionError::NoInputs);
        }
        if outputs.is_empty() {
            return Err(SessionError::NoOutputs);
        }
        let mut seen = HashSet::with_capacity(inputs.len());
        if let Some(input) = inputs.iter().find(|input| !seen.insert(&input.name)) {
            return Err(SessionError::DuplicateInput {
                name: input.name.clone(),
            });
        }
        let mut seen = HashSet::with_capacity(outputs.len());
        if let Some(output) = outputs.iter().find(|output| !seen.insert(&output.name)) {
            return Err(SessionError::DuplicateOutput {
                name: output.name.clone(),
            });
        }
        Ok(Self {
            interface_fingerprint,
            inputs,
            outputs,
        })
    }

    /// The fingerprint of the interface the session was configured over.
    #[must_use]
    pub const fn interface_fingerprint(&self) -> InterfaceFingerprint {
        self.interface_fingerprint
    }

    /// The varied parameters in declaration order.
    #[must_use]
    pub fn inputs(&self) -> &[SessionInput] {
        &self.inputs
    }

    /// The read outputs in declaration order.
    #[must_use]
    pub fn outputs(&self) -> &[SessionOutput] {
        &self.outputs
    }

    /// The slot of the varied parameter named `name`.
    #[must_use]
    pub fn input_slot(&self, name: &str) -> Option<InputSlot> {
        self.inputs
            .iter()
            .position(|input| input.name.as_str() == name)
            .map(InputSlot)
    }

    /// The slot of the read output named `name`.
    #[must_use]
    pub fn output_slot(&self, name: &str) -> Option<OutputSlot> {
        self.outputs
            .iter()
            .position(|output| output.name.as_str() == name)
            .map(OutputSlot)
    }

    /// The varied parameter at `slot`.
    #[must_use]
    pub fn input(&self, slot: InputSlot) -> Option<&SessionInput> {
        self.inputs.get(slot.0)
    }

    /// The read output at `slot`.
    #[must_use]
    pub fn output(&self, slot: OutputSlot) -> Option<&SessionOutput> {
        self.outputs.get(slot.0)
    }

    /// The selection this schema was configured from, without the
    /// parameters the session fixed, which the schema does not record.
    #[must_use]
    pub fn spec(&self) -> SessionSpec {
        let chosen = |name: &PortName, wire: &WireType, scale: Option<SiScale>| {
            scale?;
            wire.quantity()?.unit().cloned().map(|unit| UnitChoice {
                port: name.clone(),
                unit,
            })
        };
        SessionSpec {
            inputs: self.inputs.iter().map(|input| input.name.clone()).collect(),
            outputs: self
                .outputs
                .iter()
                .map(|output| output.name.clone())
                .collect(),
            units: self
                .inputs
                .iter()
                .filter_map(|input| chosen(&input.name, &input.wire, input.si_scale))
                .chain(
                    self.outputs
                        .iter()
                        .filter_map(|output| chosen(&output.name, &output.wire, output.si_scale)),
                )
                .collect(),
            fixed: Vec::new(),
        }
    }
}

/// The wire type and scale of a port carried in the unit `choice`, or in its
/// canonical unit when there is no choice.
fn chosen_wire(
    name: &PortName,
    wire: &WireType,
    choice: Option<&UnitSpelling>,
    resolver: &mut impl UnitResolver,
) -> Result<(WireType, Option<SiScale>), SessionError> {
    let Some(unit) = choice else {
        return Ok((wire.clone(), None));
    };
    let Some(chosen) = wire.with_unit(unit) else {
        return Err(SessionError::UnitForNonQuantity { name: name.clone() });
    };
    let unresolved = |reason: String| SessionError::UnresolvedUnit {
        name: name.clone(),
        unit: unit.clone(),
        reason,
    };
    let scale = resolver.si_scale(name, unit).map_err(unresolved)?;
    let chosen = chosen.map_err(|error| unresolved(error.to_string()))?;
    Ok((chosen, Some(scale)))
}

/// The unit chosen for each selected port, rejecting repeated choices and
/// choices for ports outside the selection.
fn unit_choices<'s>(
    spec: &'s SessionSpec,
    varied: &HashSet<&PortName>,
    read: &HashSet<&PortName>,
) -> Result<HashMap<&'s PortName, &'s UnitSpelling>, SessionError> {
    let mut choices = HashMap::with_capacity(spec.units.len());
    for choice in &spec.units {
        if !varied.contains(&choice.port) && !read.contains(&choice.port) {
            return Err(SessionError::UnitForUnselectedPort {
                name: choice.port.clone(),
            });
        }
        if choices.insert(&choice.port, &choice.unit).is_some() {
            return Err(SessionError::DuplicateUnit {
                name: choice.port.clone(),
            });
        }
    }
    Ok(choices)
}

/// `domain` (in the canonical unit) in the unit of `scale`.
fn converted_domain(domain: Domain, scale: Option<SiScale>) -> Option<Domain> {
    let (Domain::Quantity(bounds), Some(scale)) = (domain, scale) else {
        return Some(domain);
    };
    let convert =
        |bound: Option<&FiniteF64>| bound.map(|bound| scale.from_si(*bound)).transpose().ok();
    let (min, max) = (convert(bounds.min())?, convert(bounds.max())?);
    Bounds::new(min, max).ok().map(Domain::Quantity)
}

impl ModelInterface {
    /// The parameters `spec` fixes, rejecting repeated, unknown, and varied
    /// ones.
    fn fixed_parameters<'s>(
        &self,
        spec: &'s SessionSpec,
        varied: &HashSet<&PortName>,
    ) -> Result<HashSet<&'s PortName>, SessionError> {
        let mut fixed = HashSet::with_capacity(spec.fixed.len());
        for name in &spec.fixed {
            if !fixed.insert(name) {
                return Err(SessionError::DuplicateFixed { name: name.clone() });
            }
            if self.parameter(name.as_str()).is_none() {
                return Err(SessionError::UnknownFixed { name: name.clone() });
            }
            if varied.contains(name) {
                return Err(SessionError::VariedAndFixed { name: name.clone() });
            }
        }
        Ok(fixed)
    }

    /// Configure a session that varies and reads the ports `spec` selects,
    /// resolving its unit choices with `units`.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] when the selection is empty, repeats or
    /// names an unknown port, selects a port the protocol cannot carry, or
    /// leaves a parameter without a default neither varied nor fixed; when a
    /// fixed parameter repeats, is unknown, or is also varied; or when a unit choice
    /// repeats, names an unselected or non-quantity port, does not resolve,
    /// or cannot express the parameter's domain.
    pub fn configure(
        &self,
        spec: &SessionSpec,
        mut units: impl UnitResolver,
    ) -> Result<SessionSchema, SessionError> {
        if spec.inputs.is_empty() {
            return Err(SessionError::NoInputs);
        }
        if spec.outputs.is_empty() {
            return Err(SessionError::NoOutputs);
        }
        let mut varied = HashSet::with_capacity(spec.inputs.len());
        for name in &spec.inputs {
            if !varied.insert(name) {
                return Err(SessionError::DuplicateInput { name: name.clone() });
            }
            if self.parameter(name.as_str()).is_none() {
                return Err(SessionError::UnknownInput { name: name.clone() });
            }
        }
        let mut read = HashSet::with_capacity(spec.outputs.len());
        for name in &spec.outputs {
            if !read.insert(name) {
                return Err(SessionError::DuplicateOutput { name: name.clone() });
            }
            if self.output(name.as_str()).is_none() {
                return Err(SessionError::UnknownOutput { name: name.clone() });
            }
        }
        let choices = unit_choices(spec, &varied, &read)?;
        let fixed = self.fixed_parameters(spec, &varied)?;

        let mut inputs = Vec::with_capacity(spec.inputs.len());
        for parameter in self.parameters() {
            if !varied.contains(parameter.name()) {
                if !parameter.has_default() && !fixed.contains(parameter.name()) {
                    return Err(SessionError::MissingDefault {
                        name: parameter.name().clone(),
                    });
                }
                continue;
            }
            match parameter.wire() {
                PortWire::Supported(wire) => {
                    let choice = choices.get(parameter.name()).copied();
                    let (wire, si_scale) = chosen_wire(parameter.name(), wire, choice, &mut units)?;
                    let domain = parameter
                        .domain()
                        .map(|domain| {
                            converted_domain(*domain, si_scale).ok_or_else(|| {
                                SessionError::UnrepresentableDomain {
                                    name: parameter.name().clone(),
                                }
                            })
                        })
                        .transpose()?;
                    inputs.push(SessionInput {
                        name: parameter.name().clone(),
                        wire,
                        domain,
                        si_scale,
                    });
                }
                PortWire::Unsupported { reason } => {
                    return Err(SessionError::UnsupportedInput {
                        name: parameter.name().clone(),
                        reason: reason.clone(),
                    });
                }
            }
        }
        let mut outputs = Vec::with_capacity(spec.outputs.len());
        for output in self.outputs() {
            if !read.contains(output.name()) {
                continue;
            }
            match output.wire() {
                PortWire::Supported(wire) => {
                    let choice = choices.get(output.name()).copied();
                    let (wire, si_scale) = chosen_wire(output.name(), wire, choice, &mut units)?;
                    outputs.push(SessionOutput {
                        name: output.name().clone(),
                        wire,
                        si_scale,
                    });
                }
                PortWire::Unsupported { reason } => {
                    return Err(SessionError::UnsupportedOutput {
                        name: output.name().clone(),
                        reason: reason.clone(),
                    });
                }
            }
        }
        Ok(SessionSchema {
            interface_fingerprint: InterfaceFingerprint::of(self),
            inputs,
            outputs,
        })
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::interface::tests::{name, sample_interface};

    fn spec(inputs: &[&str], outputs: &[&str]) -> SessionSpec {
        SessionSpec::new(
            inputs.iter().map(|input| name(input)).collect(),
            outputs.iter().map(|output| name(output)).collect(),
        )
    }

    /// Varies `load` and `mode`; reads `failure` and `margin`.
    pub fn sample_session() -> SessionSchema {
        sample_interface()
            .configure(
                &spec(&["mode", "load"], &["margin", "failure"]),
                CanonicalUnitsOnly,
            )
            .unwrap()
    }

    #[test]
    fn configured_ports_follow_declaration_order() {
        let session = sample_session();
        let inputs = session
            .inputs()
            .iter()
            .map(|input| input.name().as_str())
            .collect::<Vec<_>>();
        let outputs = session
            .outputs()
            .iter()
            .map(|output| output.name().as_str())
            .collect::<Vec<_>>();
        assert_eq!(inputs, ["load", "mode"]);
        assert_eq!(outputs, ["failure", "margin"]);
        assert_eq!(session.input_slot("mode").map(InputSlot::get), Some(1));
        assert_eq!(
            session.interface_fingerprint(),
            InterfaceFingerprint::of(&sample_interface())
        );
    }

    #[test]
    fn a_fixed_parameter_needs_no_default_and_is_not_in_the_schema() {
        let interface = sample_interface();
        let fixed = spec(&["load"], &["failure"]).with_fixed(name("mode"));
        let session = interface.configure(&fixed, CanonicalUnitsOnly).unwrap();
        assert_eq!(
            session
                .inputs()
                .iter()
                .map(|input| input.name().as_str())
                .collect::<Vec<_>>(),
            ["load"]
        );
        assert_eq!(session.spec().fixed(), []);

        let cases = [
            (
                spec(&["load"], &["failure"])
                    .with_fixed(name("mode"))
                    .with_fixed(name("mode")),
                SessionError::DuplicateFixed { name: name("mode") },
            ),
            (
                spec(&["load", "mode"], &["failure"]).with_fixed(name("nope")),
                SessionError::UnknownFixed { name: name("nope") },
            ),
            (
                spec(&["load", "mode"], &["failure"]).with_fixed(name("load")),
                SessionError::VariedAndFixed { name: name("load") },
            ),
        ];
        for (spec, expected) in cases {
            assert_eq!(
                interface.configure(&spec, CanonicalUnitsOnly),
                Err(expected)
            );
        }
    }

    #[test]
    fn rejects_invalid_selections() {
        let interface = sample_interface();
        let cases = [
            (spec(&[], &["failure"]), SessionError::NoInputs),
            (spec(&["load", "mode"], &[]), SessionError::NoOutputs),
            (
                spec(&["load", "load", "mode"], &["failure"]),
                SessionError::DuplicateInput { name: name("load") },
            ),
            (
                spec(&["load", "mode", "nope"], &["failure"]),
                SessionError::UnknownInput { name: name("nope") },
            ),
            (
                spec(&["load", "mode"], &["nope"]),
                SessionError::UnknownOutput { name: name("nope") },
            ),
            (
                spec(&["load"], &["failure"]),
                SessionError::MissingDefault { name: name("mode") },
            ),
        ];
        for (spec, expected) in cases {
            assert_eq!(
                interface.configure(&spec, CanonicalUnitsOnly),
                Err(expected)
            );
        }
        assert!(matches!(
            interface.configure(
                &spec(&["load", "mode", "history"], &["failure"]),
                CanonicalUnitsOnly
            ),
            Err(SessionError::UnsupportedInput { .. })
        ));
        assert!(matches!(
            interface.configure(&spec(&["load", "mode"], &["series"]), CanonicalUnitsOnly),
            Err(SessionError::UnsupportedOutput { .. })
        ));
    }

    fn unit(spelling: &str) -> UnitSpelling {
        UnitSpelling::new(spelling).unwrap()
    }

    fn kilometres(_: &PortName, unit: &UnitSpelling) -> Result<SiScale, String> {
        match unit.as_str() {
            "km" => Ok(SiScale::new(1000.0).unwrap()),
            "tiny" => Ok(SiScale::new(f64::MIN_POSITIVE).unwrap()),
            other => Err(format!("`{other}` is not a length unit")),
        }
    }

    #[test]
    fn chosen_units_rescale_wires_and_domains() {
        let spec = spec(&["load", "mode"], &["margin", "failure"])
            .with_unit(name("load"), unit("km"))
            .with_unit(name("margin"), unit("km"));
        let session = sample_interface().configure(&spec, kilometres).unwrap();
        let load = &session.inputs()[0];
        let WireType::Quantity(quantity) = load.wire() else {
            panic!("load is a quantity");
        };
        assert_eq!(quantity.unit().map(UnitSpelling::as_str), Some("km"));
        assert_eq!(load.si_scale().map(SiScale::get), Some(1000.0));
        let Some(Domain::Quantity(bounds)) = load.domain() else {
            panic!("load has a quantity domain");
        };
        assert_eq!(bounds.max().map(|max| max.get()), Some(0.01));
        assert_eq!(
            session.outputs()[1].si_scale().map(SiScale::get),
            Some(1000.0)
        );
        assert_eq!(session.outputs()[0].si_scale(), None);
        assert_eq!(session.spec().units(), spec.units());
        assert_eq!(
            sample_interface().configure(&session.spec(), kilometres),
            Ok(session)
        );
    }

    #[test]
    fn rejects_invalid_unit_choices() {
        let interface = sample_interface();
        let base = || spec(&["load", "mode"], &["failure", "margin"]);
        let cases = [
            (
                base().with_unit(name("count"), unit("km")),
                SessionError::UnitForUnselectedPort {
                    name: name("count"),
                },
            ),
            (
                base()
                    .with_unit(name("load"), unit("km"))
                    .with_unit(name("load"), unit("km")),
                SessionError::DuplicateUnit { name: name("load") },
            ),
            (
                base().with_unit(name("mode"), unit("km")),
                SessionError::UnitForNonQuantity { name: name("mode") },
            ),
            (
                base().with_unit(name("load"), unit("tiny")),
                SessionError::UnrepresentableDomain { name: name("load") },
            ),
        ];
        for (spec, expected) in cases {
            assert_eq!(interface.configure(&spec, kilometres), Err(expected));
        }
        assert!(matches!(
            interface.configure(&base().with_unit(name("load"), unit("s")), kilometres),
            Err(SessionError::UnresolvedUnit { ref reason, .. }) if reason.contains("not a length")
        ));
        assert!(matches!(
            interface.configure(
                &base().with_unit(name("load"), unit("km")),
                CanonicalUnitsOnly
            ),
            Err(SessionError::UnresolvedUnit { .. })
        ));
    }
}
