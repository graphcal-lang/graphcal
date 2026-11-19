//! Canonical document identities of on-disk files and editor URIs.

use std::collections::HashMap;

use graphcal_compiler::source_registry::SourceOrigin;
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

/// The URI the editor addresses each open document by, keyed by identity.
///
/// The loader names a source file by its canonical path, while the editor
/// may have opened it through a symlink or under another spelling. A
/// diagnostic about a source file is published under the URI of the open
/// document with the same identity, so it reaches the buffer the editor
/// shows.
#[derive(Debug, Clone)]
pub struct DocumentUris {
    root: Url,
    open: HashMap<DocumentIdentity, Url>,
}

impl DocumentUris {
    /// The URIs of the analyzed document `root` and of the other open
    /// documents `open`; `root` wins when two share an identity.
    pub fn new<'a>(root: &Url, open: impl IntoIterator<Item = &'a Url>) -> Self {
        let mut uris = open
            .into_iter()
            .map(|uri| (document_identity(uri), uri.clone()))
            .collect::<HashMap<_, _>>();
        uris.insert(document_identity(root), root.clone());
        Self {
            root: root.clone(),
            open: uris,
        }
    }

    /// The analyzed document.
    pub const fn root(&self) -> &Url {
        &self.root
    }

    /// The URI a diagnostic about a source from `origin` is published under:
    /// the open document that is that file, otherwise the file's own URI. A
    /// detached text (an untitled buffer) is the analyzed document.
    pub fn of_source(&self, origin: &SourceOrigin) -> Url {
        match origin {
            SourceOrigin::File(path) => self
                .open
                .get(&DocumentIdentity::file(path.clone()))
                .cloned()
                .or_else(|| Url::from_file_path(path).ok())
                .unwrap_or_else(|| self.root.clone()),
            SourceOrigin::Detached => self.root.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn a_file_source_is_published_under_the_uri_its_document_was_opened_by() {
        let directory = tempfile::tempdir().unwrap();
        let real = directory.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("main.gcl"), "").unwrap();
        std::fs::write(real.join("lib.gcl"), "").unwrap();
        std::fs::write(real.join("closed.gcl"), "").unwrap();
        let alias = directory.path().join("alias");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        let root = Url::from_file_path(alias.join("main.gcl")).unwrap();
        let lib = Url::from_file_path(alias.join("lib.gcl")).unwrap();
        let documents = DocumentUris::new(&root, [&lib]);
        let canonical = real.canonicalize().unwrap();

        let of = |name: &str| documents.of_source(&SourceOrigin::File(canonical.join(name)));
        assert_eq!(of("main.gcl"), root);
        assert_eq!(of("lib.gcl"), lib);
        assert_eq!(
            of("closed.gcl"),
            Url::from_file_path(canonical.join("closed.gcl")).unwrap()
        );
        assert_eq!(documents.of_source(&SourceOrigin::Detached), root);
    }

    #[test]
    fn a_detached_source_belongs_to_the_untitled_document() {
        let root = Url::parse("untitled:Untitled-1").unwrap();
        let documents = DocumentUris::new(&root, []);
        assert_eq!(documents.of_source(&SourceOrigin::Detached), root);
    }
}
