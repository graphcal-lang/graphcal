//! The capabilities to build the frame a DAG runs its bodies in.
//!
//! A frame decides which declarations the handles of a shared template body
//! denote, so building one elsewhere would let a body run against the wrong
//! declarations. Only the type resolver builds frames: the canonical frame
//! when it type-resolves a module or inline DAG (canonical by
//! construction), and an instance frame when it materializes a semantic
//! include edge from the edge's instance record. The witnesses live here, in
//! a module with no dependencies, so that
//! [`InstanceFrame`](crate::ir::instance::frame::InstanceFrame) and
//! [`InstanceRecord`](crate::ir::instance::InstanceRecord) can require them
//! without depending on the type resolver, while their fields are visible
//! only inside [`crate::tir::typed`].

/// Witness that the caller is the canonical DAG type resolver in
/// [`crate::tir::typed`].
#[derive(Debug, Clone, Copy)]
pub struct CanonicalFrameMint(pub(in crate::tir::typed) ());

/// Witness that the caller is the instance materialization in
/// [`crate::tir::typed`], which builds an instance's frame exactly once,
/// together with the instance DAG, from the instance's own record.
#[derive(Debug, Clone, Copy)]
pub struct InstanceFrameMint(pub(in crate::tir::typed) ());

#[cfg(test)]
impl CanonicalFrameMint {
    /// A witness for unit tests of body handles outside the type resolver.
    #[must_use]
    pub(crate) const fn for_test() -> Self {
        Self(())
    }
}
