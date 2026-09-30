//! Canonical document identities of on-disk files and editor URIs.

use tower_lsp::lsp_types::Url;

use crate::workspace_revision::DocumentIdentity;

/// Identity of an on-disk file: its canonical path, or the canonical parent
/// joined with the file name when the file itself does not exist yet.
pub fn file_identity(path: &std::path::Path) -> DocumentIdentity {
    let canonical = path.canonicalize().or_else(|_| {
        let parent = path
            .parent()
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))?;
        let canonical_parent = parent.canonicalize()?;
        path.file_name().map_or_else(
            || Err(std::io::Error::from(std::io::ErrorKind::NotFound)),
            |name| Ok(canonical_parent.join(name)),
        )
    });
    DocumentIdentity::file(canonical.unwrap_or_else(|_| path.to_path_buf()))
}

/// Identity of an editor document: a file identity for `file:` URIs, the URI
/// itself otherwise.
pub fn document_identity(uri: &Url) -> DocumentIdentity {
    uri.to_file_path().map_or_else(
        |()| DocumentIdentity::virtual_uri(uri.clone()),
        |path| file_identity(&path),
    )
}
