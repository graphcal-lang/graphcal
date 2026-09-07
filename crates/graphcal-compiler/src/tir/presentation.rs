//! Checked plot projection shapes. Display selections belong to evaluated values,
//! not a second program containing selector HIR or invocation environments.

use crate::plot_shape::PlotChannelShape;
use crate::syntax::ast::EncodingChannel;
use crate::syntax::decl_name::ResolvedDeclName;
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct DagPresentationFacts {
    pub plot_channels: HashMap<ResolvedDeclName, HashMap<EncodingChannel, PlotChannelShape>>,
}
