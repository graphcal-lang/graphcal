//! The module whose include declaration is being processed.

use std::sync::Arc;

use graphcal_compiler::ir::module_interface::ModuleInterface;
use graphcal_compiler::ir::static_dependencies::StaticScope;
use miette::NamedSource;

/// The importer side of one include: the including module's declared
/// interface, the source its include declaration is written in, and the
/// static scope its names resolve in.
#[derive(Clone, Copy)]
pub(super) struct IncludingModule<'a> {
    pub(super) interface: &'a ModuleInterface,
    pub(super) source: &'a NamedSource<Arc<String>>,
    pub(super) scope: StaticScope<'a>,
}
