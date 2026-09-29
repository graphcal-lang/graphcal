//! Specialization's capability to mint instance identities.
//!
//! Instantiating a DAG template copies the template's declarations into a
//! concrete instance owner that the resolver never declared. Only the
//! specialization API in [`crate::ir::instance`] may construct those copies,
//! so only it can construct a [`SpecializationMint`], which
//! [`ResolvedName::specialized`](crate::resolved_name::ResolvedName::specialized)
//! requires.

/// Witness that the caller is the specialization API in
/// [`crate::ir::instance`].
///
/// The field is visible only inside [`crate::ir::instance`], so the type
/// system, not a convention, confines instance identity construction there.
#[derive(Debug, Clone, Copy)]
pub struct SpecializationMint(pub(in crate::ir::instance) ());
