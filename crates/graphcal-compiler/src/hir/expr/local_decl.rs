//! Body handles: declaration references as a checked body spells them.

use crate::ir::instance::mint::FrameAccess;
use crate::resolved_name::ResolvedDeclName;

/// A declaration reference inside a checked body, relative to the DAG that
/// runs the body.
///
/// A template body is shared by every instance of the template, so the
/// declaration a reference denotes depends on which instance runs it. The
/// handle keeps the reference as the defining template spells it and offers
/// no public accessor for it: the only way to obtain the declaration identity
/// is [`InstanceFrame::resolve`](crate::ir::instance::frame::InstanceFrame::resolve)
/// with the frame of the DAG running the body. Since a handle is not a
/// [`ResolvedDeclName`], using it where an identity is required without
/// resolving it does not compile.
///
/// IDE trees ([`Tolerant`](crate::hir::Tolerant)) are never run
/// and name the source definition directly; refining one into a complete
/// tree is where handles are made.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LocalDecl(ResolvedDeclName);

impl LocalDecl {
    /// The handle for a reference to `definition` written in a body of
    /// `definition`'s module or of a template lexically enclosing it.
    #[must_use]
    pub(crate) const fn new(definition: ResolvedDeclName) -> Self {
        Self(definition)
    }

    /// The declaration's leaf name, which every frame keeps.
    #[must_use]
    pub const fn leaf(&self) -> &crate::syntax::decl_name::DeclName {
        self.0.leaf()
    }

    /// The declaration the defining template names, for an instance frame to
    /// resolve.
    #[must_use]
    pub(crate) const fn definition(&self, _: FrameAccess) -> &ResolvedDeclName {
        &self.0
    }
}

/// Renders the reference as the defining template names it, for diagnostics.
impl std::fmt::Display for LocalDecl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
