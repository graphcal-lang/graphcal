//! The nominal record type bound to a struct-returning extern function.

use crate::function_signature::{StructResult, StructShape};
use crate::resolved_name::ResolvedStructTypeName;
use crate::syntax::type_name::ConstructorName;

/// The record type a struct-returning extern function was declared with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternStructResult {
    /// Canonical identity of the record type named at the declaration site.
    pub resolved: ResolvedStructTypeName,
    /// Checked record constructor; invocation need not recover it from a type spelling.
    pub constructor: ConstructorName,
    /// The record's flattened field shape, as the plugin manifest sees it.
    pub shape: StructShape,
}

impl ExternStructResult {
    /// Whether two struct results name the same record type.
    pub(crate) fn same_record(&self, other: &Self) -> bool {
        self.resolved == other.resolved && self.constructor == other.constructor
    }
}

impl StructResult for ExternStructResult {
    fn shape(&self) -> &StructShape {
        &self.shape
    }
}
