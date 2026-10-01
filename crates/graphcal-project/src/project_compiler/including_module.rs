//! The module whose include declaration is being processed.

use graphcal_compiler::ir::module_interface::ModuleInterface;
use graphcal_compiler::ir::static_dependencies::StaticScope;
use graphcal_compiler::source_id::SourceId;

/// The importer side of one include: the including module's declared
/// interface, the source its include declaration is written in, and the
/// static scope its names resolve in.
#[derive(Clone, Copy)]
pub(super) struct IncludingModule<'a> {
    pub(super) interface: &'a ModuleInterface,
    pub(super) source: SourceId,
    pub(super) scope: StaticScope<'a>,
}
