//! Targets of a view: what a reader asks a view of, within one root.
//!
//! A root module is projected alone ([`ViewTarget::Module`]), and each
//! include instance its evaluation materializes is projected as that
//! instance ([`ViewTarget::Instance`]): the template's declarations with the
//! values the include's arguments give them. The two are distinct targets
//! even when the include binds nothing, so a host keys its models by target,
//! never by module.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::model::ViewModel;
use super::wire_key::ViewInstanceId;

/// What to project of a root: the root module itself, or one include
/// instance of its evaluation. Hosts read it from the wire
/// (`{"kind": "module"}`, `{"kind": "instance", "instance": "<id>"}`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ViewTarget {
    Module,
    Instance { instance: ViewInstanceId },
}

/// The models of every target of one root.
#[derive(Debug, Clone)]
pub struct ViewModels {
    module: ViewModel,
    instances: BTreeMap<ViewInstanceId, ViewModel>,
}

impl ViewModels {
    pub(super) const fn new(
        module: ViewModel,
        instances: BTreeMap<ViewInstanceId, ViewModel>,
    ) -> Self {
        Self { module, instances }
    }

    /// The model of `target`; `None` for an instance the root's evaluation
    /// does not materialize.
    #[must_use]
    pub fn get(&self, target: &ViewTarget) -> Option<&ViewModel> {
        match target {
            ViewTarget::Module => Some(&self.module),
            ViewTarget::Instance { instance } => self.instances.get(instance),
        }
    }

    /// The root module's model.
    #[must_use]
    pub const fn module(&self) -> &ViewModel {
        &self.module
    }
}
