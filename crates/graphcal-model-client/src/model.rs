//! A described model and typed handles to its ports.

use std::marker::PhantomData;

use graphcal_model_protocol::{
    DescribeDocument, Domain, InterfaceFingerprint, ModelInterface, PortName, PortWire,
};

use crate::command::GraphcalCommand;
use crate::error::ClientError;
use crate::identity::{ModelId, PortIdentity};
use crate::kind::{Indexed, Key, KeyVariant, Kind};

/// A Graphcal model whose interface has been described.
#[derive(Debug, Clone)]
pub struct Model {
    id: ModelId,
    command: GraphcalCommand,
    document: DescribeDocument,
}

impl Model {
    /// Run `graphcal model describe` and keep its interface.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] when the executable cannot run, reports a
    /// failure, or prints an invalid or other-version document.
    pub fn describe(command: GraphcalCommand) -> Result<Self, ClientError> {
        let output = command.describe().output().map_err(ClientError::Spawn)?;
        if !output.status.success() {
            return Err(ClientError::Describe {
                status: output.status,
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        let document =
            serde_json::from_slice(&output.stdout).map_err(ClientError::DescribeDocument)?;
        Ok(Self::from_document(command, document))
    }

    /// A model from an already obtained describe document.
    #[must_use]
    pub fn from_document(command: GraphcalCommand, document: DescribeDocument) -> Self {
        Self {
            id: ModelId::fresh(),
            command,
            document,
        }
    }

    /// The describe document.
    #[must_use]
    pub const fn document(&self) -> &DescribeDocument {
        &self.document
    }

    /// The model interface.
    #[must_use]
    pub const fn interface(&self) -> &ModelInterface {
        self.document.interface()
    }

    /// The interface fingerprint every session of this model is checked
    /// against.
    #[must_use]
    pub const fn fingerprint(&self) -> InterfaceFingerprint {
        self.document.interface_fingerprint()
    }

    pub(crate) const fn id(&self) -> ModelId {
        self.id
    }

    pub(crate) const fn command(&self) -> &GraphcalCommand {
        &self.command
    }

    /// A typed handle to the parameter `name`.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] when there is no such parameter, the protocol
    /// cannot carry it, or it is not of kind `K`.
    pub fn input<K: Kind>(&self, name: &str) -> Result<InputHandle<K>, ClientError> {
        let parameter =
            self.interface()
                .parameter(name)
                .ok_or_else(|| ClientError::UnknownInput {
                    name: name.to_owned(),
                })?;
        let detail = port_detail::<K>(parameter.name(), parameter.wire())?;
        Ok(InputHandle {
            port: PortIdentity::new(self.id, parameter.name().clone()),
            detail,
            domain: parameter.domain().copied(),
            has_default: parameter.has_default(),
            kind: PhantomData,
        })
    }

    /// A typed handle to the public output `name`.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] when there is no such output, the protocol
    /// cannot carry it, or it is not of kind `K`.
    pub fn output<K: Kind>(&self, name: &str) -> Result<OutputHandle<K>, ClientError> {
        let output = self
            .interface()
            .output(name)
            .ok_or_else(|| ClientError::UnknownOutput {
                name: name.to_owned(),
            })?;
        let detail = port_detail::<K>(output.name(), output.wire())?;
        Ok(OutputHandle {
            port: PortIdentity::new(self.id, output.name().clone()),
            detail,
            kind: PhantomData,
        })
    }
}

fn port_detail<K: Kind>(name: &PortName, wire: &PortWire) -> Result<K::Detail, ClientError> {
    match wire {
        PortWire::Unsupported { reason } => Err(ClientError::UnsupportedPort {
            name: name.clone(),
            reason: reason.clone(),
        }),
        PortWire::Supported(wire) => K::detail(wire).ok_or_else(|| ClientError::KindMismatch {
            name: name.clone(),
            expected: K::NAME,
        }),
    }
}

/// A typed handle to one parameter of one [`Model`].
#[derive(Debug, Clone)]
pub struct InputHandle<K: Kind> {
    port: PortIdentity,
    detail: K::Detail,
    domain: Option<Domain>,
    has_default: bool,
    kind: PhantomData<K>,
}

impl<K: Kind> InputHandle<K> {
    /// The parameter name.
    #[must_use]
    pub const fn name(&self) -> &PortName {
        self.port.name()
    }

    /// The wire details: the [`QuantityType`](graphcal_model_protocol::QuantityType)
    /// of a quantity or the [`KeyType`](graphcal_model_protocol::KeyType) of a key.
    #[must_use]
    pub const fn detail(&self) -> &K::Detail {
        &self.detail
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

    pub(crate) const fn port(&self) -> &PortIdentity {
        &self.port
    }
}

impl InputHandle<Key> {
    /// The variant spelled `name` of this key parameter.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError::UnknownVariant`] when the index has no such
    /// variant.
    pub fn variant(&self, name: &str) -> Result<KeyVariant, ClientError> {
        self.detail
            .position(name)
            .map(|position| KeyVariant::new(self.port.clone(), position))
            .ok_or_else(|| ClientError::UnknownVariant {
                input: self.port.name().clone(),
                variant: name.to_owned(),
            })
    }

    /// Every variant of this key parameter, in declaration order.
    pub fn variants(&self) -> impl ExactSizeIterator<Item = KeyVariant> + '_ {
        self.detail
            .entries()
            .map(|(position, _)| KeyVariant::new(self.port.clone(), position))
    }
}

impl InputHandle<Indexed<Key>> {
    /// The variant spelled `name` of this parameter's key elements.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError::UnknownVariant`] when the index has no such
    /// variant.
    pub fn variant(&self, name: &str) -> Result<KeyVariant, ClientError> {
        self.detail
            .element()
            .position(name)
            .map(|position| KeyVariant::new(self.port.clone(), position))
            .ok_or_else(|| ClientError::UnknownVariant {
                input: self.port.name().clone(),
                variant: name.to_owned(),
            })
    }
}

/// A typed handle to one public output of one [`Model`].
#[derive(Debug, Clone)]
pub struct OutputHandle<K: Kind> {
    port: PortIdentity,
    detail: K::Detail,
    kind: PhantomData<K>,
}

impl<K: Kind> OutputHandle<K> {
    /// The output name.
    #[must_use]
    pub const fn name(&self) -> &PortName {
        self.port.name()
    }

    /// The wire details of the output.
    #[must_use]
    pub const fn detail(&self) -> &K::Detail {
        &self.detail
    }

    pub(crate) const fn port(&self) -> &PortIdentity {
        &self.port
    }
}
