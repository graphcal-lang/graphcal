//! The capability to build the frame of a canonical DAG.
//!
//! A canonical frame runs bodies as written, so resolving an instance body's
//! handles with one would silently read the template's declarations. Only the
//! code that type-resolves a module or inline DAG — which is canonical by
//! construction — may build one. The witness lives here, in a module with no
//! dependencies, so that [`InstanceFrame`](crate::ir::instance::frame::InstanceFrame)
//! can require it without depending on the type resolver, while its field is
//! visible only inside [`crate::tir::typed`].

/// Witness that the caller is the canonical DAG type resolver in
/// [`crate::tir::typed`].
#[derive(Debug, Clone, Copy)]
pub struct CanonicalFrameMint(pub(in crate::tir::typed) ());

#[cfg(test)]
impl CanonicalFrameMint {
    /// A witness for unit tests of body handles outside the type resolver.
    #[must_use]
    pub(crate) const fn for_test() -> Self {
        Self(())
    }
}
