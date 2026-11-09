//! The one classification of the items a selective include selects.
//!
//! Every registration an include item drives — the importer's value names,
//! the include's exposed outputs, plot and assertion projections, and type
//! and unit projections — derives from what the dependency's resolved export
//! surface binds the item to. That surface also covers the items the
//! dependency itself re-exports from its own imports and includes, which its
//! declared interface does not (#2259).

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::resolve::category::DeclSymbolKind;
use graphcal_compiler::resolve::exports::{ExportedBinding, ExportedBindingTarget};
use graphcal_compiler::syntax::ast::{ImportItem, ImportItemNamespace, UnitConstness};
use graphcal_compiler::syntax::decl_name::DeclName;

/// Whether a selected graph value is a compile-time constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SelectedValueKind {
    /// A `const node`.
    Const,
    /// A `param` or a `node`.
    Runtime,
}

/// What one selective include item selects, by what the dependency exports
/// under its name in its namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SelectedItemKind {
    /// A graph value of the instance, materialized as a local alias in the
    /// importer.
    Value(SelectedValueKind),
    /// A constant the dependency re-exports from a pure import. It belongs
    /// to no instance; the resolver binds it to its canonical declaration.
    ImportedConstant,
    /// An assertion the include exposes.
    Assertion,
    /// A plot the include requests.
    Plot,
    /// A figure or layer. No include projection exists for one yet.
    Composition,
    /// A constructor. The resolver binds it; nothing is materialized.
    Constructor,
    /// A type the include may project onto an instance specialization.
    Type,
    /// A dimension or an index. The resolver binds it.
    Static,
    /// A unit; a runtime unit is aliased from the instance.
    Unit(UnitConstness),
}

impl SelectedItemKind {
    /// Classify `item` by the exported bindings `exports` of the dependency
    /// `template`, or `None` when it exports nothing selectable under it.
    ///
    /// A Term item may name both a declaration and a constructor; the
    /// declaration decides what the include materializes.
    pub(super) fn classify(
        exports: &[ExportedBinding],
        template: &DagId,
        item: &ImportItem,
    ) -> Option<Self> {
        let name = item.name.name.atom();
        let mut targets = exports
            .iter()
            .filter(|binding| {
                &binding.name == name && binding.target.kind().namespace() == item.namespace
            })
            .map(|binding| &binding.target);
        match item.namespace {
            ImportItemNamespace::Term => {
                let targets = targets.collect::<Vec<_>>();
                targets
                    .iter()
                    .find_map(|target| match target {
                        ExportedBindingTarget::Decl { identity, kind } => {
                            Self::of_decl(template, identity.owner(), *kind)
                        }
                        _ => None,
                    })
                    .or_else(|| {
                        targets
                            .iter()
                            .any(|target| matches!(target, ExportedBindingTarget::Constructor(_)))
                            .then_some(Self::Constructor)
                    })
            }
            ImportItemNamespace::Type => targets.next().map(|_| Self::Type),
            ImportItemNamespace::Dimension | ImportItemNamespace::Index => {
                targets.next().map(|_| Self::Static)
            }
            ImportItemNamespace::Unit => targets.find_map(|target| match target {
                ExportedBindingTarget::Unit { constness, .. } => Some(Self::Unit(*constness)),
                _ => None,
            }),
        }
    }

    /// The selection of a declaration of `kind` owned by `owner`; `None` for
    /// a DAG blueprint, which is not an instance member.
    ///
    /// A declaration of the template's own body, or of an instance nested in
    /// it, is a member of the include's instance. A constant owned elsewhere
    /// reached the template's surface through a pure import.
    fn of_decl(template: &DagId, owner: &DagId, kind: DeclSymbolKind) -> Option<Self> {
        let in_instance = owner.scopes_below(template).is_some();
        match kind {
            DeclSymbolKind::Const if !in_instance => Some(Self::ImportedConstant),
            DeclSymbolKind::Const => Some(Self::Value(SelectedValueKind::Const)),
            DeclSymbolKind::Param | DeclSymbolKind::Node => {
                Some(Self::Value(SelectedValueKind::Runtime))
            }
            DeclSymbolKind::Assert => Some(Self::Assertion),
            DeclSymbolKind::Plot => Some(Self::Plot),
            DeclSymbolKind::Figure | DeclSymbolKind::Layer => Some(Self::Composition),
            DeclSymbolKind::Dag => None,
        }
    }
}

/// The instance values the dependency `template` exports, by name: its
/// `param` input ports and its exported values, including the ones it
/// re-exports from its own includes.
pub(super) fn exported_values<'a>(
    exports: &'a [ExportedBinding],
    template: &'a DagId,
) -> impl Iterator<Item = (DeclName, SelectedValueKind)> + 'a {
    exports.iter().filter_map(|binding| match &binding.target {
        ExportedBindingTarget::Decl { identity, kind } => {
            match SelectedItemKind::of_decl(template, identity.owner(), *kind) {
                Some(SelectedItemKind::Value(value)) => {
                    Some((DeclName::classify(binding.name.clone()), value))
                }
                _ => None,
            }
        }
        _ => None,
    })
}

#[cfg(test)]
mod tests;
