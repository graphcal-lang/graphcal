//! Node bodies after references have acquired canonical declaration identities.

use crate::syntax::decl_name::ResolvedDeclName;

pub type NodeDefinition =
    crate::node_definition::NodeDefinition<super::expr::CheckedExpr, ResolvedDeclName>;
