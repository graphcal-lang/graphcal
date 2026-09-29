//! Include-override reconciliation facts awaiting canonical name resolution.
//!
//! Include assembly knows which bindable type/index declarations were replaced,
//! but canonical expression ownership is only available after HIR/type
//! resolution. These records preserve the typed parts across that phase
//! boundary without inspecting source spelling or dispatching on field names.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::dag_id::DagId;
use crate::registry::types::IndexBindingTarget;
use crate::resolved_name::ResolvedDeclName;
use crate::syntax::decl_name::DeclName;
use crate::syntax::index_name::IndexName;
use crate::syntax::span::Span;
use crate::syntax::type_name::StructTypeName;

/// One include whose unrebound param default must remain independent of the
/// bindable nominal declarations replaced by that include.
///
/// Every overridden name is declared by the owner of [`Self::source_decl`]
/// (the included module), and every declared replacement by
/// [`Self::replacement_owner`] (the including module); the targets carry
/// only the names.
#[derive(Debug, Clone)]
pub struct PendingOverrideReconciliation {
    source_decl: ResolvedDeclName,
    replacement_owner: DagId,
    targets: Vec<PendingOverrideTarget>,
    src: NamedSource<Arc<String>>,
    include_span: Span,
}

impl PendingOverrideReconciliation {
    /// Build a pending check for the nominal overrides on one include.
    #[must_use]
    pub(crate) fn new(
        orphan_decl: DeclName,
        source_owner: &DagId,
        replacement_owner: &DagId,
        index_bindings: &HashMap<IndexName, IndexBindingTarget>,
        type_bindings: &HashMap<StructTypeName, StructTypeName>,
        src: NamedSource<Arc<String>>,
        include_span: Span,
    ) -> Self {
        let targets = index_bindings
            .iter()
            .map(|(overridden, replacement)| PendingOverrideTarget::Index {
                overridden: overridden.clone(),
                replacement: replacement.clone(),
            })
            .chain(type_bindings.iter().map(|(overridden, replacement)| {
                PendingOverrideTarget::Type {
                    overridden: overridden.clone(),
                    replacement: replacement.clone(),
                }
            }))
            .collect();
        Self {
            source_decl: ResolvedDeclName::from_def(source_owner.clone(), orphan_decl),
            replacement_owner: replacement_owner.clone(),
            targets,
            src,
            include_span,
        }
    }

    /// The unrebound param whose default must not mention an overridden name.
    #[must_use]
    pub(crate) const fn source_decl(&self) -> &ResolvedDeclName {
        &self.source_decl
    }

    /// The included module that declares every overridden name.
    #[must_use]
    pub(crate) const fn source_owner(&self) -> &DagId {
        self.source_decl.owner()
    }

    /// The including module that declares every declared replacement.
    #[must_use]
    pub(crate) const fn replacement_owner(&self) -> &DagId {
        &self.replacement_owner
    }

    /// The nominal overrides of this include.
    #[must_use]
    pub(crate) fn targets(&self) -> &[PendingOverrideTarget] {
        &self.targets
    }

    /// Source of the including module.
    #[must_use]
    pub(crate) const fn src(&self) -> &NamedSource<Arc<String>> {
        &self.src
    }

    /// Span of the include site.
    #[must_use]
    pub(crate) const fn include_span(&self) -> Span {
        self.include_span
    }
}

/// A nominal include override before its source and replacement names cross
/// the module-resolution boundary.
#[derive(Debug, Clone)]
pub enum PendingOverrideTarget {
    Index {
        overridden: IndexName,
        replacement: IndexBindingTarget,
    },
    Type {
        overridden: StructTypeName,
        replacement: StructTypeName,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_reconciliation_owns_names_once() {
        let source_owner = DagId::root_in_package("test", "lib");
        let replacement_owner = DagId::root_in_package("test", "main");
        let index = IndexName::expect_valid("Phase");
        let replacement_index = IndexName::expect_valid("Stage");
        let ty = StructTypeName::expect_valid("Point");
        let replacement_ty = StructTypeName::expect_valid("Vec");
        let span = Span::new(3, 9);

        let pending = PendingOverrideReconciliation::new(
            DeclName::expect_valid("orphan"),
            &source_owner,
            &replacement_owner,
            &HashMap::from([(
                index.clone(),
                IndexBindingTarget::Declared(replacement_index.clone()),
            )]),
            &HashMap::from([(ty.clone(), replacement_ty.clone())]),
            NamedSource::new("main.gcl", Arc::new(String::new())),
            span,
        );

        assert_eq!(
            pending.source_decl(),
            &ResolvedDeclName::from_def(source_owner.clone(), DeclName::expect_valid("orphan"))
        );
        assert_eq!(pending.source_owner(), &source_owner);
        assert_eq!(pending.replacement_owner(), &replacement_owner);
        assert_eq!(pending.include_span(), span);
        assert_eq!(pending.src().name(), "main.gcl");
        let [
            PendingOverrideTarget::Index {
                overridden: overridden_index,
                replacement: IndexBindingTarget::Declared(bound_index),
            },
            PendingOverrideTarget::Type {
                overridden: overridden_ty,
                replacement: bound_ty,
            },
        ] = pending.targets()
        else {
            panic!(
                "expected one index and one type target: {:?}",
                pending.targets()
            );
        };
        assert_eq!(overridden_index, &index);
        assert_eq!(bound_index, &replacement_index);
        assert_eq!(overridden_ty, &ty);
        assert_eq!(bound_ty, &replacement_ty);
    }
}
