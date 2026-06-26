//! [`DagId`]: an abstract, filesystem-independent identifier for a DAG (module).
//!
//! Every file and every `dag` block gets a unique package-qualified `DagId`.
//! File-based DAGs derive their segments from the loader-provided module path
//! (e.g., `helpers/math.gcl` → `["helpers", "math"]`), while inline `dag`
//! blocks append their name as an additional segment (e.g.,
//! `["helpers", "math", "double_speed"]`).
//!
//! Package identity is intentionally opaque in the compiler core. Loaders erase
//! whether a package came from a lockfile, manifest-backed project, virtual
//! single-file project, or test harness before constructing a `DagId`.
//!
//! This keeps filesystem concerns (`PathBuf`) in the loader (imperative shell)
//! and gives the compiler/evaluator (functional core) an opaque identity type.

use std::fmt;
use std::sync::Arc;

use thiserror::Error;

use crate::syntax::non_empty::NonEmpty;

/// Opaque package component of a [`DagId`].
///
/// This is separate from module path segments so the compiler can distinguish
/// the same source spelling loaded from different package instances without
/// parsing package identity out of a joined module string. The compiler core
/// deliberately cannot inspect where the package id came from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DagPackageId(Arc<str>);

impl DagPackageId {
    /// Construct an opaque package id.
    #[must_use]
    pub fn new(value: impl Into<Arc<str>>) -> Self {
        Self(value.into())
    }

    /// Borrow the opaque package id payload.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for DagPackageId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for DagPackageId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<Arc<str>> for DagPackageId {
    fn from(value: Arc<str>) -> Self {
        Self::new(value)
    }
}

impl fmt::Display for DagPackageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An abstract identifier for a DAG in the compiler pipeline.
///
/// Segments form a hierarchical name: for example, a file at `helpers/math.gcl`
/// has segments `["helpers", "math"]`, and an inline `dag double_speed` within
/// it has segments `["helpers", "math", "double_speed"]`.
///
/// Non-emptiness is encoded structurally with [`NonEmpty`], so [`DagId::name`]
/// (the leaf segment) is total — there is no value of this type that has zero
/// segments.
///
/// The compiler never interprets these segments as filesystem paths.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DagId {
    /// Opaque package that owns this DAG. Every DAG belongs to exactly one package.
    package: DagPackageId,
    /// Hierarchical module/DAG segments. Always non-empty.
    segments: NonEmpty<Arc<str>>,
}

/// Returned by [`DagId::from_relative_path`] when the path is not a valid
/// graphcal source path.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DagIdPathError {
    /// The path produced no components (e.g., an empty `Path`).
    #[error("path has no components")]
    Empty,
    /// A path component was not valid UTF-8.
    #[error("path contains a non-UTF-8 component")]
    NonUtf8Component,
    /// The path did not end with `.gcl`.
    #[error("path must end with `.gcl`")]
    MissingGclExtension,
}

impl From<NonEmpty<String>> for NonEmpty<Arc<str>> {
    fn from(value: NonEmpty<String>) -> Self {
        value.map(Arc::<str>::from)
    }
}

impl<'a> From<NonEmpty<&'a str>> for NonEmpty<Arc<str>> {
    fn from(value: NonEmpty<&'a str>) -> Self {
        value.map(Arc::<str>::from)
    }
}

impl DagId {
    /// Create a `DagId` from an explicit package and non-empty hierarchical
    /// segments.
    pub fn new(package: impl Into<DagPackageId>, segments: impl Into<NonEmpty<Arc<str>>>) -> Self {
        Self {
            package: package.into(),
            segments: segments.into(),
        }
    }

    /// Create a single-segment (root) `DagId` in an explicit package.
    pub fn root_in_package(package: impl Into<DagPackageId>, name: impl Into<Arc<str>>) -> Self {
        Self {
            package: package.into(),
            segments: NonEmpty::singleton(name.into()),
        }
    }

    /// Attach an explicit package identity to an existing module id.
    #[cfg(test)]
    #[must_use]
    fn in_package(package: impl Into<DagPackageId>, module: Self) -> Self {
        Self {
            package: package.into(),
            segments: module.segments,
        }
    }

    /// The package that owns this DAG.
    #[must_use]
    pub const fn package(&self) -> &DagPackageId {
        &self.package
    }

    /// Create a child `DagId` by appending a segment (e.g., for a nested `dag` block).
    #[must_use]
    pub fn child(&self, name: impl Into<Arc<str>>) -> Self {
        let mut segments = self.segments.clone();
        segments.push(name.into());
        Self {
            package: self.package.clone(),
            segments,
        }
    }

    /// Return the parent `DagId` (all segments except the last), or `None` if
    /// this is a root (single-segment) identifier.
    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        if self.segments.len() == 1 {
            return None;
        }
        let parent_segments = &self.segments.as_slice()[..self.segments.len() - 1];
        Some(Self {
            package: self.package.clone(),
            segments: NonEmpty::new(
                Arc::clone(&parent_segments[0]),
                parent_segments[1..].to_vec(),
            ),
        })
    }

    /// The segments of this identifier as an iterator (head first, then tail).
    pub fn segments(&self) -> impl Iterator<Item = &Arc<str>> {
        self.segments.iter()
    }

    /// Number of segments — always at least 1.
    #[must_use]
    pub const fn segment_count(&self) -> usize {
        self.segments.len()
    }

    /// The last segment (leaf name). Always present.
    #[must_use]
    pub fn name(&self) -> &str {
        self.segments.last().as_ref()
    }

    /// True if `self` is a strict descendant of `ancestor` (an inline `dag`
    /// block nested — at any depth — inside the DAG identified by `ancestor`).
    #[must_use]
    pub fn is_descendant_of(&self, ancestor: &Self) -> bool {
        if self.segment_count() <= ancestor.segment_count() {
            return false;
        }
        if self.package != ancestor.package {
            return false;
        }
        self.segments()
            .zip(ancestor.segments())
            .all(|(a, b)| a == b)
    }

    /// Create a package-qualified `DagId` from a relative file path, stripping
    /// the `.gcl` extension and using the file stem as the package id.
    ///
    /// This is intended for single-file source paths where no project loader is
    /// available to provide a richer package identity.
    ///
    /// # Errors
    ///
    /// Returns [`DagIdPathError`] if `path` has no components, contains a
    /// non-UTF-8 component, or does not end with `.gcl`.
    pub fn from_virtual_relative_path(path: &std::path::Path) -> Result<Self, DagIdPathError> {
        let package = virtual_package_from_path(path)?;
        Self::from_relative_path(package, path)
    }

    /// Create a package-qualified `DagId` from a relative file path, stripping
    /// the `.gcl` extension.
    ///
    /// This is the only place where filesystem paths are converted into `DagId`
    /// segments. It belongs at the loader (imperative shell) boundary.
    ///
    /// # Errors
    ///
    /// Returns [`DagIdPathError`] if `path` has no components, contains a
    /// non-UTF-8 component, or does not end with `.gcl`.
    pub fn from_relative_path(
        package: impl Into<DagPackageId>,
        path: &std::path::Path,
    ) -> Result<Self, DagIdPathError> {
        let mut segments: Vec<Arc<str>> = path
            .components()
            .map(|c| {
                c.as_os_str()
                    .to_str()
                    .map(Arc::<str>::from)
                    .ok_or(DagIdPathError::NonUtf8Component)
            })
            .collect::<Result<_, _>>()?;

        let last = segments.last_mut().ok_or(DagIdPathError::Empty)?;
        *last = last
            .strip_suffix(".gcl")
            .map(Arc::<str>::from)
            .ok_or(DagIdPathError::MissingGclExtension)?;

        Ok(Self {
            package: package.into(),
            segments: NonEmpty::try_from_vec(segments).map_err(|_| DagIdPathError::Empty)?,
        })
    }
}

fn virtual_package_from_path(path: &std::path::Path) -> Result<DagPackageId, DagIdPathError> {
    let file_name = path
        .components()
        .next_back()
        .ok_or(DagIdPathError::Empty)?
        .as_os_str()
        .to_str()
        .ok_or(DagIdPathError::NonUtf8Component)?;
    let stem = file_name
        .strip_suffix(".gcl")
        .ok_or(DagIdPathError::MissingGclExtension)?;
    Ok(DagPackageId::new(stem))
}

impl fmt::Display for DagId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, seg) in self.segments.iter().enumerate() {
            if i > 0 {
                f.write_str(".")?;
            }
            f.write_str(seg.as_ref())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_relative_path_strips_gcl() {
        let id =
            DagId::from_relative_path("math", std::path::Path::new("helpers/math.gcl")).unwrap();
        let segs: Vec<&str> = id.segments().map(|s| &**s).collect();
        assert_eq!(segs, ["helpers", "math"]);
        assert_eq!(id.package(), &DagPackageId::new("math"));
        assert_eq!(id.to_string(), "helpers.math");
    }

    #[test]
    fn from_virtual_relative_path_uses_file_stem_as_package() {
        let id =
            DagId::from_virtual_relative_path(std::path::Path::new("helpers/math.gcl")).unwrap();
        assert_eq!(id.package(), &DagPackageId::new("math"));
        assert_eq!(id.to_string(), "helpers.math");
    }

    #[test]
    fn from_relative_path_rejects_empty_path() {
        let err = DagId::from_relative_path("empty", std::path::Path::new("")).unwrap_err();
        assert_eq!(err, DagIdPathError::Empty);
    }

    #[test]
    fn from_relative_path_rejects_path_without_gcl_extension() {
        let err =
            DagId::from_relative_path("math", std::path::Path::new("helpers/math")).unwrap_err();
        assert_eq!(err, DagIdPathError::MissingGclExtension);
    }

    #[test]
    fn child_appends_segment() {
        let parent = DagId::new("test", NonEmpty::new("helpers", vec!["math"]));
        let child = parent.child("double_speed");
        assert_eq!(child.to_string(), "helpers.math.double_speed");
    }

    #[test]
    fn parent_drops_last_segment() {
        let id = DagId::new(
            "test",
            NonEmpty::new("helpers", vec!["math", "double_speed"]),
        );
        let parent = id.parent().unwrap();
        assert_eq!(parent.to_string(), "helpers.math");
    }

    #[test]
    fn parent_of_root_is_none() {
        let id = DagId::root_in_package("test", "main");
        assert!(id.parent().is_none());
    }

    #[test]
    fn package_identity_is_structural() {
        let module = DagId::new("test", NonEmpty::new("src", vec!["units", "si"]));
        let rev1 = DagId::in_package("pkg-units-rev1", module.clone());
        let rev2 = DagId::in_package("pkg-units-rev2", module);

        assert_ne!(rev1, rev2);
        assert_eq!(rev1.package().as_str(), "pkg-units-rev1");
        assert_eq!(rev1.to_string(), "src.units.si");
        assert_eq!(
            rev1.segments()
                .map(std::convert::AsRef::as_ref)
                .collect::<Vec<_>>(),
            ["src", "units", "si"]
        );
    }

    #[test]
    fn child_and_parent_preserve_package_identity() {
        let root = DagId::root_in_package("pkg-lib", "lib");
        let child = root.child("helper");

        assert_eq!(child.package(), root.package());
        assert_eq!(child.parent(), Some(root));
    }

    #[test]
    fn is_descendant_of_matches_nested_blocks_only() {
        let file = DagId::new("test", NonEmpty::new("helpers", vec!["math"]));
        let child = file.child("double_speed");
        let grandchild = child.child("inner");
        assert!(child.is_descendant_of(&file));
        assert!(grandchild.is_descendant_of(&file));
        assert!(!file.is_descendant_of(&file));
        assert!(!file.is_descendant_of(&child));
        assert!(
            !DagId::new("test", NonEmpty::new("helpers", vec!["other"])).is_descendant_of(&file)
        );
        assert!(
            !DagId::in_package("pkg-a", child).is_descendant_of(&DagId::in_package("pkg-b", file))
        );
    }

    #[test]
    fn name_returns_last_segment() {
        let id = DagId::new(
            "test",
            NonEmpty::new("helpers", vec!["math", "double_speed"]),
        );
        assert_eq!(id.name(), "double_speed");
    }

    #[test]
    fn name_of_root_returns_head() {
        let id = DagId::root_in_package("test", "main");
        assert_eq!(id.name(), "main");
    }

    #[test]
    fn display_joins_with_dot() {
        let id = DagId::new("test", NonEmpty::new("a", vec!["b", "c"]));
        assert_eq!(id.to_string(), "a.b.c");
    }
}
