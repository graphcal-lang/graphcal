//! The resolver's capability to mint canonical identities.
//!
//! A [`ResolvedName`](crate::resolved_name::ResolvedName) is the resolver's
//! proof that a name denotes a declaration. Only the resolver declares
//! identities, so only code inside [`crate::resolve`] can construct a
//! [`ResolverMint`], which
//! [`ResolvedName::from_def`](crate::resolved_name::ResolvedName::from_def)
//! requires. Every other compiler phase obtains identities from resolver
//! lookups instead.

/// Witness that the caller is the module resolver.
///
/// The field is visible only inside [`crate::resolve`], so the type system,
/// not a convention, confines identity declaration to the resolver.
#[derive(Debug, Clone, Copy)]
pub struct ResolverMint(pub(in crate::resolve) ());
