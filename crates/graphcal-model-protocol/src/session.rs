//! Session configuration: which parameters a client varies and which outputs
//! it reads.

use std::collections::HashSet;

use thiserror::Error;

use crate::domain::Domain;
use crate::fingerprint::InterfaceFingerprint;
use crate::interface::ModelInterface;
use crate::label::PortName;
use crate::wire_type::{PortWire, WireType};

/// The parameters a session varies and the outputs it reads.
///
/// Order is irrelevant: a configured session lists both in the interface's
/// declaration order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionSpec {
    inputs: Vec<PortName>,
    outputs: Vec<PortName>,
}

impl SessionSpec {
    /// A session varying `inputs` and reading `outputs`.
    #[must_use]
    pub const fn new(inputs: Vec<PortName>, outputs: Vec<PortName>) -> Self {
        Self { inputs, outputs }
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
    #[error("parameter `{name}` has no default, so the session must vary it")]
    MissingDefault { name: PortName },
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
}

impl SessionInput {
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

    /// The declared domain. Values outside it are row failures, not
    /// protocol errors.
    #[must_use]
    pub const fn domain(&self) -> Option<&Domain> {
        self.domain.as_ref()
    }
}

/// One read output of a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionOutput {
    name: PortName,
    wire: WireType,
}

impl SessionOutput {
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
}

/// The request and result shape that both sides of one session share.
///
/// Only [`ModelInterface::configure`] builds it, so its inputs and outputs
/// are always supported, unique, and in declaration order.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSchema {
    interface_fingerprint: InterfaceFingerprint,
    inputs: Vec<SessionInput>,
    outputs: Vec<SessionOutput>,
}

impl SessionSchema {
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
}

impl ModelInterface {
    /// Configure a session that varies and reads the ports `spec` selects.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] when the selection is empty, repeats or
    /// names an unknown port, selects a port the protocol cannot carry, or
    /// leaves a parameter without a default unvaried.
    pub fn configure(&self, spec: &SessionSpec) -> Result<SessionSchema, SessionError> {
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

        let mut inputs = Vec::with_capacity(spec.inputs.len());
        for parameter in self.parameters() {
            if !varied.contains(parameter.name()) {
                if !parameter.has_default() {
                    return Err(SessionError::MissingDefault {
                        name: parameter.name().clone(),
                    });
                }
                continue;
            }
            match parameter.wire() {
                PortWire::Supported(wire) => inputs.push(SessionInput {
                    name: parameter.name().clone(),
                    wire: wire.clone(),
                    domain: parameter.domain().copied(),
                }),
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
                PortWire::Supported(wire) => outputs.push(SessionOutput {
                    name: output.name().clone(),
                    wire: wire.clone(),
                }),
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
            .configure(&spec(&["mode", "load"], &["margin", "failure"]))
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
            assert_eq!(interface.configure(&spec), Err(expected));
        }
        assert!(matches!(
            interface.configure(&spec(&["load", "mode", "history"], &["failure"])),
            Err(SessionError::UnsupportedInput { .. })
        ));
        assert!(matches!(
            interface.configure(&spec(&["load", "mode"], &["series"])),
            Err(SessionError::UnsupportedOutput { .. })
        ));
    }
}
