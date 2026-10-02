//! The diagnostic spelling of a declared type.

/// How a diagnostic spells a declared (resolved, possibly still generic)
/// type.
///
/// It is constructed only inside `tir::typed`, by
/// [`ResolvedDeclType::spelling`](super::ResolvedDeclType::spelling), so a
/// payload holding a `DeclaredTypeSpelling` always names a declared type,
/// never free text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredTypeSpelling(String);

impl DeclaredTypeSpelling {
    pub(in crate::tir::typed) const fn new(spelling: String) -> Self {
        Self(spelling)
    }
}

impl std::fmt::Display for DeclaredTypeSpelling {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
