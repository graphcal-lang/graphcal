//! [`DagId`]: an abstract, filesystem-independent identifier for a DAG (module).
//!
//! Every file and every `dag` block gets a unique `DagId`. File-based DAGs
//! derive their segments from the relative path (e.g., `helpers/math.gcl` →
//! `["helpers", "math"]`), while inline `dag` blocks append their name as an
//! additional segment (e.g., `["helpers", "math", "double_speed"]`).
//!
//! This keeps filesystem concerns (`PathBuf`) in the loader (imperative shell)
//! and gives the compiler/evaluator (functional core) an opaque identity type.

use std::fmt;
use std::sync::Arc;

/// An abstract identifier for a DAG in the compiler pipeline.
///
/// Segments form a hierarchical name: for example, a file at `helpers/math.gcl`
/// has segments `["helpers", "math"]`, and an inline `dag double_speed` within
/// it has segments `["helpers", "math", "double_speed"]`.
///
/// Non-emptiness is encoded structurally as a `head` segment plus an
/// optional tail, so [`DagId::name`] (the leaf segment) is total — there is
/// no value of this type that has zero segments.
///
/// The compiler never interprets these segments as filesystem paths.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DagId {
    /// The first segment. Always present.
    head: Arc<str>,
    /// Remaining segments after `head`. Empty for a root (single-segment) id.
    tail: Arc<[Arc<str>]>,
}

/// Returned by [`DagId::from_relative_path`] when the path has zero components
/// or contains a non-UTF-8 component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DagIdPathError {
    /// The path produced no components (e.g., an empty `Path`).
    Empty,
    /// A path component was not valid UTF-8.
    NonUtf8Component,
}

impl fmt::Display for DagIdPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("path has no components"),
            Self::NonUtf8Component => f.write_str("path contains a non-UTF-8 component"),
        }
    }
}

impl std::error::Error for DagIdPathError {}

impl DagId {
    /// Create a `DagId` from a leading segment and any further segments.
    ///
    /// The `head` argument enforces non-emptiness at the type level: every
    /// `DagId` is guaranteed to have at least one segment.
    pub fn new(
        head: impl Into<Arc<str>>,
        tail: impl IntoIterator<Item = impl Into<Arc<str>>>,
    ) -> Self {
        Self {
            head: head.into(),
            tail: tail.into_iter().map(Into::into).collect(),
        }
    }

    /// Create a single-segment (root) `DagId`.
    pub fn root(name: impl Into<Arc<str>>) -> Self {
        Self {
            head: name.into(),
            tail: Arc::from([] as [Arc<str>; 0]),
        }
    }

    /// Create a child `DagId` by appending a segment (e.g., for a nested `dag` block).
    #[must_use]
    pub fn child(&self, name: impl Into<Arc<str>>) -> Self {
        let mut tail: Vec<Arc<str>> = self.tail.to_vec();
        tail.push(name.into());
        Self {
            head: Arc::clone(&self.head),
            tail: tail.into(),
        }
    }

    /// Return the parent `DagId` (all segments except the last), or `None` if
    /// this is a root (single-segment) identifier.
    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        if self.tail.is_empty() {
            return None;
        }
        Some(Self {
            head: Arc::clone(&self.head),
            tail: self.tail[..self.tail.len() - 1].into(),
        })
    }

    /// The segments of this identifier as an iterator (head first, then tail).
    pub fn segments(&self) -> impl Iterator<Item = &Arc<str>> {
        std::iter::once(&self.head).chain(self.tail.iter())
    }

    /// Number of segments — always at least 1.
    #[must_use]
    pub fn segment_count(&self) -> usize {
        1 + self.tail.len()
    }

    /// The last segment (leaf name). Always present.
    #[must_use]
    pub fn name(&self) -> &str {
        self.tail.last().map_or(&self.head, |s| s)
    }

    /// Create a `DagId` from a relative file path, stripping the `.gcl` extension.
    ///
    /// This is the only place where filesystem paths are converted into `DagId`s.
    /// It belongs at the loader (imperative shell) boundary.
    ///
    /// # Errors
    ///
    /// Returns [`DagIdPathError`] if `path` has no components or contains a
    /// non-UTF-8 component.
    pub fn from_relative_path(path: &std::path::Path) -> Result<Self, DagIdPathError> {
        let mut segments = path.components().map(|c| {
            c.as_os_str()
                .to_str()
                .map(|s| {
                    // Strip .gcl extension from the last component.
                    Arc::<str>::from(s.strip_suffix(".gcl").unwrap_or(s))
                })
                .ok_or(DagIdPathError::NonUtf8Component)
        });
        let head = segments.next().ok_or(DagIdPathError::Empty)??;
        let tail: Arc<[Arc<str>]> = segments.collect::<Result<Vec<_>, _>>()?.into();
        Ok(Self { head, tail })
    }
}

impl fmt::Display for DagId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.head)?;
        for seg in self.tail.iter() {
            f.write_str("/")?;
            f.write_str(seg)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]
    use super::*;

    #[test]
    fn from_relative_path_strips_gcl() {
        let id = DagId::from_relative_path(std::path::Path::new("helpers/math.gcl")).unwrap();
        let segs: Vec<&str> = id.segments().map(|s| &**s).collect();
        assert_eq!(segs, ["helpers", "math"]);
        assert_eq!(id.to_string(), "helpers/math");
    }

    #[test]
    fn from_relative_path_rejects_empty_path() {
        let err = DagId::from_relative_path(std::path::Path::new("")).unwrap_err();
        assert_eq!(err, DagIdPathError::Empty);
    }

    #[test]
    fn child_appends_segment() {
        let parent = DagId::new("helpers", ["math"]);
        let child = parent.child("double_speed");
        assert_eq!(child.to_string(), "helpers/math/double_speed");
    }

    #[test]
    fn parent_drops_last_segment() {
        let id = DagId::new("helpers", ["math", "double_speed"]);
        let parent = id.parent().unwrap();
        assert_eq!(parent.to_string(), "helpers/math");
    }

    #[test]
    fn parent_of_root_is_none() {
        let id = DagId::root("main");
        assert!(id.parent().is_none());
    }

    #[test]
    fn name_returns_last_segment() {
        let id = DagId::new("helpers", ["math", "double_speed"]);
        assert_eq!(id.name(), "double_speed");
    }

    #[test]
    fn name_of_root_returns_head() {
        let id = DagId::root("main");
        assert_eq!(id.name(), "main");
    }

    #[test]
    fn display_joins_with_slash() {
        let id = DagId::new("a", ["b", "c"]);
        assert_eq!(id.to_string(), "a/b/c");
    }
}
