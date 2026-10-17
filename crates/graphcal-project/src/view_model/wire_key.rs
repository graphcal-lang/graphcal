//! Opaque wire keys of the view model: node and axis ids.
//!
//! A key is derived injectively from the typed identity it stands for, so two
//! distinct declarations (or axes) never share one, and the same declaration
//! keeps its key across evaluations and across the two refs of a comparison.
//! Readers treat a key as opaque: they compare it and look it up, and never
//! take it apart; the spelling a reader sees travels in its own field.
//!
//! The encoding is a `/`-joined list of `tag:text` parts in which every byte
//! of `text` outside `[A-Za-z0-9_.-]` is percent-encoded, so no part can
//! contain a separator and the list decodes uniquely.

use std::fmt::Write as _;

use graphcal_compiler::dag_id::{DagId, DagSegment};
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::syntax::module_name::ScopeSegment;
use graphcal_compiler::tir::typed::ResolvedIndex;
use serde::Serialize;

/// The key of a node: one declaration, as owned (an include instance's
/// declaration is owned by the instance, so a later per-instance node gets a
/// key of its own without changing this one's).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct ViewNodeId(String);

impl ViewNodeId {
    /// The key of `declaration`.
    pub(super) fn of(declaration: &ResolvedDeclName) -> Self {
        let mut key = KeyWriter::default();
        key.dag(declaration.owner());
        key.part('n', declaration.as_str());
        Self(key.finish())
    }

    /// The key as sent.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ViewNodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The key of an index axis: a declared index by its identity, a structural
/// `Fin(N)` by its cardinality, a generic parameter by its name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct ViewIndexId(String);

impl ViewIndexId {
    /// The key of `index`.
    pub(super) fn of(index: &ResolvedIndex) -> Self {
        let mut key = KeyWriter::default();
        match index {
            ResolvedIndex::Concrete(name, _) => {
                key.dag(name.owner());
                key.part('n', name.as_str());
            }
            ResolvedIndex::Finite(form, _) => key.part('F', &form.to_string()),
            ResolvedIndex::GenericParam(param, _) => key.part('G', &param.to_string()),
        }
        Self(key.finish())
    }

    /// The key as sent.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ViewIndexId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Default)]
struct KeyWriter(String);

impl KeyWriter {
    /// The package, then every segment tagged with its kind.
    fn dag(&mut self, dag: &DagId) {
        self.part('p', &dag.package().to_string());
        for segment in dag.segments() {
            match segment {
                DagSegment::File(name) => self.part('f', name),
                DagSegment::InlineDag(name) => self.part('d', name.as_str()),
                DagSegment::Instance(ScopeSegment::Named(alias)) => self.part('s', alias.as_str()),
                DagSegment::Instance(ScopeSegment::IncludeInstance(id)) => {
                    self.part('i', &id.to_string());
                }
            }
        }
    }

    fn part(&mut self, tag: char, text: &str) {
        if !self.0.is_empty() {
            self.0.push('/');
        }
        self.0.push(tag);
        self.0.push(':');
        for byte in text.bytes() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-') {
                self.0.push(char::from(byte));
            } else {
                // Writing to a `String` cannot fail.
                let _ = write!(self.0, "%{byte:02X}");
            }
        }
    }

    fn finish(self) -> String {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(parts: &[(char, &str)]) -> String {
        let mut key = KeyWriter::default();
        for (tag, text) in parts {
            key.part(*tag, text);
        }
        key.finish()
    }

    #[test]
    fn separators_inside_a_part_are_escaped() {
        assert_eq!(written(&[('f', "a/b"), ('n', "x")]), "f:a%2Fb/n:x");
        assert_ne!(
            written(&[('f', "a/b"), ('n', "x")]),
            written(&[('f', "a"), ('f', "b"), ('n', "x")])
        );
        assert_eq!(written(&[('i', "<include@4>")]), "i:%3Cinclude%404%3E");
        assert_eq!(written(&[('f', "%")]), "f:%25");
    }
}
