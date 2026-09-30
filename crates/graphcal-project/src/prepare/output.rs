//! Presentation-only assembly for evaluated project outputs.

use graphcal_compiler::syntax::module_name::{ScopeSegment, ScopedName};

use crate::project_compiler::IncludeDebugNameMap;
use graphcal_eval::eval::types::EvalResult;

pub(super) fn remap_include_debug_name(
    name: &ScopedName,
    aliases: &IncludeDebugNameMap,
) -> ScopedName {
    let Some(owner) = name.owner() else {
        return name.clone();
    };
    let ScopeSegment::IncludeInstance(first) = owner.first() else {
        return name.clone();
    };
    let Some(display) = aliases.get(first) else {
        return name.clone();
    };
    ScopedName::qualified(
        graphcal_compiler::syntax::non_empty::NonEmpty::new(
            ScopeSegment::Named(display.clone()),
            owner.as_slice()[1..].to_vec(),
        ),
        name.leaf().clone(),
    )
}

/// Replace private synthetic include scopes with unambiguous human-readable
/// target leaves at the presentation boundary.
pub(super) fn apply_include_debug_names(result: &mut EvalResult, aliases: &IncludeDebugNameMap) {
    if aliases.is_empty() {
        return;
    }

    result.rename_scoped_names(|name| remap_include_debug_name(name, aliases));
}
