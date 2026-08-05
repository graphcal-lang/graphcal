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
use crate::syntax::decl_name::DeclName;
use crate::syntax::index_name::IndexName;
use crate::syntax::span::Span;
use crate::syntax::type_name::StructTypeName;

/// One include whose unrebound param default must remain independent of the
/// bindable nominal declarations replaced by that include.
#[derive(Debug, Clone)]
pub struct PendingOverrideReconciliation {
    pub(crate) orphan_decl: DeclName,
    pub(crate) targets: Vec<PendingOverrideTarget>,
    pub(crate) src: NamedSource<Arc<String>>,
    pub(crate) include_span: Span,
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
                source_owner: source_owner.clone(),
                replacement: replacement.clone(),
                replacement_owner: replacement_owner.clone(),
            })
            .chain(type_bindings.iter().map(|(overridden, replacement)| {
                PendingOverrideTarget::Type {
                    overridden: overridden.clone(),
                    source_owner: source_owner.clone(),
                    replacement: replacement.clone(),
                    replacement_owner: replacement_owner.clone(),
                }
            }))
            .collect();
        Self {
            orphan_decl,
            targets,
            src,
            include_span,
        }
    }
}

/// A nominal include override before its source and replacement names cross
/// the module-resolution boundary.
#[derive(Debug, Clone)]
pub enum PendingOverrideTarget {
    Index {
        overridden: IndexName,
        source_owner: DagId,
        replacement: IndexBindingTarget,
        replacement_owner: DagId,
    },
    Type {
        overridden: StructTypeName,
        source_owner: DagId,
        replacement: StructTypeName,
        replacement_owner: DagId,
    },
}
