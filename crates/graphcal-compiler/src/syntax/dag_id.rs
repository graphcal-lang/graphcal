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
/// The compiler never interprets these segments as filesystem paths.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DagId {
    segments: Arc<[Arc<str>]>,
}

impl DagId {
    /// Create a `DagId` from an iterator of segments.
    pub fn new(segments: impl IntoIterator<Item = impl Into<Arc<str>>>) -> Self {
        Self {
            segments: segments.into_iter().map(Into::into).collect(),
        }
    }

    /// Create a child `DagId` by appending a segment (e.g., for a nested `dag` block).
    #[must_use]
    pub fn child(&self, name: impl Into<Arc<str>>) -> Self {
        let mut segs: Vec<Arc<str>> = self.segments.to_vec();
        segs.push(name.into());
        Self {
            segments: segs.into(),
        }
    }

    /// Return the parent `DagId` (all segments except the last), or `None` if
    /// this is a root (single-segment) identifier.
    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        if self.segments.len() <= 1 {
            return None;
        }
        Some(Self {
            segments: self.segments[..self.segments.len() - 1].into(),
        })
    }

    /// The segments of this identifier.
    #[must_use]
    pub fn segments(&self) -> &[Arc<str>] {
        &self.segments
    }

    /// The last segment (leaf name).
    ///
    /// # Panics
    ///
    /// Panics if the `DagId` has no segments (which should never happen
    /// as constructors enforce at least one segment).
    #[must_use]
    #[expect(
        clippy::expect_used,
        reason = "DagId invariant: always has >= 1 segment"
    )]
    pub fn name(&self) -> &str {
        self.segments
            .last()
            .expect("DagId must have at least one segment")
    }

    /// Create a `DagId` from a relative file path, stripping the `.gcl` extension.
    ///
    /// This is the only place where filesystem paths are converted into `DagId`s.
    /// It belongs at the loader (imperative shell) boundary.
    ///
    /// # Panics
    ///
    /// Panics if the path contains non-UTF-8 components.
    #[must_use]
    #[expect(clippy::expect_used, reason = "documented panic for non-UTF-8 paths")]
    pub fn from_relative_path(path: &std::path::Path) -> Self {
        let segments: Vec<Arc<str>> = path
            .components()
            .map(|c| {
                let s = c.as_os_str().to_str().expect("non-UTF-8 path component");
                // Strip .gcl extension from the last component
                let s = s.strip_suffix(".gcl").unwrap_or(s);
                Arc::from(s)
            })
            .collect();
        assert!(
            !segments.is_empty(),
            "path must have at least one component"
        );
        Self {
            segments: segments.into(),
        }
    }
}

impl fmt::Display for DagId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for seg in self.segments.iter() {
            if !first {
                f.write_str("/")?;
            }
            first = false;
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
        let id = DagId::from_relative_path(std::path::Path::new("helpers/math.gcl"));
        assert_eq!(id.segments().len(), 2);
        assert_eq!(&*id.segments()[0], "helpers");
        assert_eq!(&*id.segments()[1], "math");
        assert_eq!(id.to_string(), "helpers/math");
    }

    #[test]
    fn child_appends_segment() {
        let parent = DagId::new(["helpers", "math"]);
        let child = parent.child("double_speed");
        assert_eq!(child.to_string(), "helpers/math/double_speed");
    }

    #[test]
    fn parent_drops_last_segment() {
        let id = DagId::new(["helpers", "math", "double_speed"]);
        let parent = id.parent().unwrap();
        assert_eq!(parent.to_string(), "helpers/math");
    }

    #[test]
    fn parent_of_root_is_none() {
        let id = DagId::new(["main"]);
        assert!(id.parent().is_none());
    }

    #[test]
    fn name_returns_last_segment() {
        let id = DagId::new(["helpers", "math", "double_speed"]);
        assert_eq!(id.name(), "double_speed");
    }

    #[test]
    fn display_joins_with_slash() {
        let id = DagId::new(["a", "b", "c"]);
        assert_eq!(id.to_string(), "a/b/c");
    }
}
