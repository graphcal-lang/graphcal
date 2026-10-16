//! Opaque wire keys of the view model: node, axis and include instance ids.
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
use serde::{Deserialize, Serialize};

/// The key of a node: one declaration, as owned.
///
/// An include instance's declaration is owned by the instance, so its node
/// in the instance's own view has a key apart from the template's node in
/// the template's view.
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

    /// The key of the declaration `output` as the `ordinal`-th inline DAG
    /// call (in pre-order) of `caller` computes it. The call part sets it
    /// apart from every declaration's key.
    pub(super) fn of_call_output(
        caller: &ResolvedDeclName,
        ordinal: usize,
        output: &ResolvedDeclName,
    ) -> Self {
        let mut key = KeyWriter::default();
        key.dag(caller.owner());
        key.part('n', caller.as_str());
        key.part('c', &ordinal.to_string());
        key.dag(output.owner());
        key.part('n', output.as_str());
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

/// The key of an include instance materialized in the root's evaluation.
///
/// It is derived from the instance's own DAG, so a nested instance (`q`
/// inside `p`) has a key of its own. A reader names an instance by it to ask for the instance's view,
/// and never takes it apart.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ViewInstanceId(String);

impl ViewInstanceId {
    /// The key of the instance whose declarations `instance` owns.
    pub(super) fn of(instance: &DagId) -> Self {
        let mut key = KeyWriter::default();
        key.dag(instance);
        Self(key.finish())
    }

    /// A key as a reader sent it back. It names an instance only when it
    /// equals the key of one (see [`super::ViewProjections::get`]).
    #[must_use]
    pub const fn from_wire(key: String) -> Self {
        Self(key)
    }

    /// The key as sent.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ViewInstanceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The key of an index axis:a declared index by its identity, a structural
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

    #[test]
    fn a_call_output_key_differs_from_its_declarations_and_other_calls() {
        let root = DagId::root_in_package("p", "main");
        let template = root
            .inline_dag_child(graphcal_compiler::syntax::decl_name::DeclName::expect_valid("lib"));
        let decl = |owner: &DagId, name: &str| {
            ResolvedDeclName::for_test(
                owner.clone(),
                graphcal_compiler::syntax::decl_name::DeclName::expect_valid(name),
            )
        };
        let (caller, output) = (decl(&root, "y"), decl(&template, "out"));
        let first = ViewNodeId::of_call_output(&caller, 0, &output);
        assert_eq!(first.as_str(), "p:p/f:main/n:y/c:0/p:p/f:main/d:lib/n:out");
        assert_ne!(first, ViewNodeId::of_call_output(&caller, 1, &output));
        assert_ne!(first, ViewNodeId::of(&caller));
        assert_ne!(first, ViewNodeId::of(&output));
    }
}
