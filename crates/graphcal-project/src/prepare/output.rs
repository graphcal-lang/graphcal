//! Presentation-only assembly for evaluated project outputs.

use graphcal_compiler::display::include_scope_names::{IncludeScopeNames, name_include_scopes};
use graphcal_eval::eval::types::EvalResult;

/// Replace private synthetic include scopes with unambiguous human-readable
/// target leaves at the presentation boundary.
pub(super) fn apply_include_debug_names(result: &mut EvalResult, aliases: &IncludeScopeNames) {
    if aliases.is_empty() {
        return;
    }

    result.rename_scoped_names(|name| name_include_scopes(name, aliases));
}
