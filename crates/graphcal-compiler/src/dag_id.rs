//! [`DagId`]: an abstract, filesystem-independent identifier for a DAG (module).
//!
//! Every file, inline `dag` block, and concrete include instance gets a unique
//! package-qualified `DagId`. File-based DAGs derive their segments from the
//! loader-provided module path (e.g., `helpers/math.gcl` →
//! `["helpers", "math"]`), while inline `dag` blocks append their name as a
//! source-module segment (e.g., `["helpers", "math", "double_speed"]`).
//! Include instances append an instance segment instead, so a source module and
//! an instance with the same displayed path remain structurally distinct.
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
    #[cfg(test)]
    #[must_use]
    fn as_str(&self) -> &str {
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

/// Opaque owner-local identity of a selective include instance.
///
/// A selective include introduces declaration aliases but no source-visible
/// module alias. Compiler lowering still needs a private namespace for the
/// included implementation, so its source occurrence is represented directly
/// instead of fabricating an alias spelling.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IncludeInstanceId {
    source_offset: usize,
}

impl IncludeInstanceId {
    /// Identify the include occurrence whose module path starts at byte offset
    /// `source_offset` within its owning DAG's source.
    #[must_use]
    pub const fn at_source_offset(source_offset: usize) -> Self {
        Self { source_offset }
    }
}

impl fmt::Display for IncludeInstanceId {
    /// Render the opaque identity for diagnostics and debug output only.
    ///
    /// The angle-bracket spelling cannot collide with a source identifier and
    /// is never parsed back; [`IncludeInstanceId`] is the authoritative form.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<include@{}>", self.source_offset)
    }
}

impl fmt::Debug for IncludeInstanceId {
    /// Debug output uses the rendered form so debug views keep one spelling
    /// for an anonymous include instance.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// One typed segment of a [`DagId`].
///
/// A segment records both its relationship to the parent (lexical
/// source-module nesting versus a concrete include instance) and the child's
/// identity. A source module and a named instance may share a spelling, but
/// they are different semantic identities; an anonymous include instance has
/// no spelling at all.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DagSegment {
    /// A file-path component or an inline `dag` declaration name.
    SourceModule(Arc<str>),
    /// A concrete include instance named by its source-visible module alias.
    NamedInstance(Arc<str>),
    /// A concrete selective include instance, which has no module alias.
    IncludeInstance(IncludeInstanceId),
}

impl DagSegment {
    /// Whether this segment is a concrete include instance.
    #[must_use]
    pub const fn is_instance(&self) -> bool {
        matches!(self, Self::NamedInstance(_) | Self::IncludeInstance(_))
    }

    /// The source-visible spelling of a source-module or named-instance
    /// segment. Anonymous include instances have none.
    #[must_use]
    pub fn spelling(&self) -> Option<&str> {
        match self {
            Self::SourceModule(name) | Self::NamedInstance(name) => Some(name),
            Self::IncludeInstance(_) => None,
        }
    }

    /// The concrete-instance segment with this segment's identity.
    ///
    /// Re-instantiating a template's nested child yields a concrete instance
    /// named like that child; an anonymous include keeps its opaque identity.
    #[must_use]
    pub fn to_instance(&self) -> Self {
        match self {
            Self::SourceModule(name) | Self::NamedInstance(name) => {
                Self::NamedInstance(Arc::clone(name))
            }
            Self::IncludeInstance(id) => Self::IncludeInstance(*id),
        }
    }

    /// Rendered text, used only to keep [`DagId`]'s established ordering.
    fn rendered(&self) -> std::borrow::Cow<'_, str> {
        self.spelling().map_or_else(
            || std::borrow::Cow::Owned(self.to_string()),
            std::borrow::Cow::Borrowed,
        )
    }
}

impl fmt::Display for DagSegment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SourceModule(name) | Self::NamedInstance(name) => f.write_str(name),
            Self::IncludeInstance(id) => fmt::Display::fmt(id, f),
        }
    }
}

/// An abstract identifier for a DAG in the compiler pipeline.
///
/// Segments form a hierarchical identity: for example, a file at
/// `helpers/math.gcl` has segments `["helpers", "math"]`, and an inline
/// `dag double_speed` within it has segments
/// `["helpers", "math", "double_speed"]`. Each [`DagSegment`] preserves
/// whether that child is a source module or a concrete instance; display text
/// alone is not identity. Lexical visibility is maintained separately by the
/// module resolver.
///
/// Non-emptiness is encoded structurally with [`NonEmpty`], so [`DagId::leaf`]
/// is total — there is no value of this type that has zero segments. Every
/// constructor starts from source-module segments; only the child
/// constructors append instance segments.
///
/// The compiler never interprets these segments as filesystem paths.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DagId {
    /// Opaque package that owns this DAG. Every DAG belongs to exactly one package.
    package: DagPackageId,
    /// Hierarchical module/instance segments. Always non-empty.
    segments: NonEmpty<DagSegment>,
}

impl Ord for DagId {
    /// Order by package, then by rendered segment text, then by segment kind
    /// (source modules before instances).
    ///
    /// This is the order `DagId` had while its segments were spellings with a
    /// parallel edge-kind array, so sorted outputs stay unchanged. The final
    /// structural comparison keeps `Ord` consistent with `Eq` for distinct
    /// segments that happen to render identically.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.package
            .cmp(&other.package)
            .then_with(|| {
                self.segments
                    .iter()
                    .map(DagSegment::rendered)
                    .cmp(other.segments.iter().map(DagSegment::rendered))
            })
            .then_with(|| {
                self.segments
                    .iter()
                    .map(DagSegment::is_instance)
                    .cmp(other.segments.iter().map(DagSegment::is_instance))
            })
            .then_with(|| self.segments.iter().cmp(other.segments.iter()))
    }
}

impl PartialOrd for DagId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Result of attempting to move a [`DagId`] subtree onto a new owner.
///
/// Callers must choose explicitly whether an outside-subtree identity is a
/// legitimate external reference or a broken ownership invariant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DescendantRebase {
    /// The identity was the requested ancestor or one of its descendants.
    Rebased(DagId),
    /// The identity lies outside the requested ancestor's subtree.
    OutsideSubtree,
}

/// Typed identity of one concrete include or DAG-call instance.
///
/// `owner` is the fresh runtime namespace allocated at the call/include site;
/// `template` is the canonical reusable DAG definition it instantiates. Keeping
/// both prevents a concrete instance from being mistaken for its source module
/// merely because their declaration leaves have the same spelling.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InstanceId {
    owner: DagId,
    template: DagId,
}

impl InstanceId {
    /// Construct an explicit instance identity from its concrete owner and template.
    #[must_use]
    pub const fn new(owner: DagId, template: DagId) -> Self {
        Self { owner, template }
    }

    /// Concrete owner allocated to this instance.
    #[must_use]
    pub const fn owner(&self) -> &DagId {
        &self.owner
    }

    /// Canonical module/DAG template instantiated here.
    #[must_use]
    pub const fn template(&self) -> &DagId {
        &self.template
    }
}

/// Returned by [`DagId::from_relative_path`] when the path is not a valid
/// graphcal source path.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DagIdPathError {
    /// The path produced no components (e.g., an empty `Path`).
    #[error("path has no components")]
    Empty,
    /// The path was absolute or contained a platform root/prefix component.
    #[error("path must be relative")]
    NotRelative,
    /// The path contained a parent-directory component.
    #[error("path must not contain `..`")]
    ParentTraversal,
    /// The path contained a current-directory component.
    #[error("path must be canonical and must not contain `.`")]
    CurrentDirectory,
    /// A path component was not valid UTF-8.
    #[error("path contains a non-UTF-8 component")]
    NonUtf8Component,
    /// The path did not end with `.gcl`.
    #[error("path must end with `.gcl`")]
    MissingGclExtension,
    /// The `.gcl` path had an empty file stem.
    #[error("path must have a non-empty file stem before `.gcl`")]
    EmptyFileStem,
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
    /// source-module segments.
    pub fn new(package: impl Into<DagPackageId>, segments: impl Into<NonEmpty<Arc<str>>>) -> Self {
        Self {
            package: package.into(),
            segments: segments.into().map(DagSegment::SourceModule),
        }
    }

    /// Create a single-segment (root) `DagId` in an explicit package.
    pub fn root_in_package(package: impl Into<DagPackageId>, name: impl Into<Arc<str>>) -> Self {
        Self {
            package: package.into(),
            segments: NonEmpty::singleton(DagSegment::SourceModule(name.into())),
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

    fn with_child(&self, segment: DagSegment) -> Self {
        let mut segments = self.segments.clone();
        segments.push(segment);
        Self {
            package: self.package.clone(),
            segments,
        }
    }

    /// Create a source-module child by appending a segment (e.g., for a nested
    /// `dag` block).
    #[must_use]
    pub fn child(&self, name: impl Into<Arc<str>>) -> Self {
        self.with_child(DagSegment::SourceModule(name.into()))
    }

    /// Create a concrete include-instance child named by a module alias.
    ///
    /// This is structurally distinct from [`Self::child`] even when both names
    /// render identically.
    #[must_use]
    pub fn named_instance_child(&self, alias: impl Into<Arc<str>>) -> Self {
        self.with_child(DagSegment::NamedInstance(alias.into()))
    }

    /// Create the concrete instance child of an anonymous selective include.
    #[must_use]
    pub fn include_instance_child(&self, id: IncludeInstanceId) -> Self {
        self.with_child(DagSegment::IncludeInstance(id))
    }

    /// Create a concrete instance child with the identity of `segment`
    /// (see [`DagSegment::to_instance`]).
    #[must_use]
    pub fn instance_child_like(&self, segment: &DagSegment) -> Self {
        self.with_child(segment.to_instance())
    }

    /// Return the parent `DagId` (all segments except the last), or `None` if
    /// this is a root (single-segment) identifier.
    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        let (_, parent_segments) = self.segments.as_slice().split_last()?;
        let (root, rest) = parent_segments.split_first()?;
        Some(Self {
            package: self.package.clone(),
            segments: NonEmpty::new(root.clone(), rest.to_vec()),
        })
    }

    /// The segments of this identifier (head first, then tail).
    #[must_use]
    pub const fn segments(&self) -> &NonEmpty<DagSegment> {
        &self.segments
    }

    /// The last segment. Always present.
    #[must_use]
    pub fn leaf(&self) -> &DagSegment {
        self.segments.last()
    }

    /// True if `self` is a strict descendant of `ancestor` (an inline `dag`
    /// block or instance nested — at any depth — inside `ancestor`).
    #[must_use]
    pub fn is_descendant_of(&self, ancestor: &Self) -> bool {
        self.segments.len() > ancestor.segments.len()
            && self.package == ancestor.package
            && self
                .segments
                .as_slice()
                .starts_with(ancestor.segments.as_slice())
    }

    /// Rebase this identity from one ancestor onto another while preserving
    /// every source-module/concrete-instance segment in the descendant suffix.
    ///
    /// Returns [`DescendantRebase::OutsideSubtree`] when `self` is neither
    /// `ancestor` nor its descendant. The typed result forces each caller to
    /// decide whether that case is expected or an invariant violation.
    #[must_use]
    pub fn rebase_descendant(&self, ancestor: &Self, replacement: &Self) -> DescendantRebase {
        if self == ancestor {
            return DescendantRebase::Rebased(replacement.clone());
        }
        if !self.is_descendant_of(ancestor) {
            return DescendantRebase::OutsideSubtree;
        }

        DescendantRebase::Rebased(
            self.segments
                .iter()
                .skip(ancestor.segments.len())
                .fold(replacement.clone(), |rebased, segment| {
                    rebased.with_child(segment.clone())
                }),
        )
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
        let segments = NonEmpty::try_from_vec(relative_path_segments(path)?)
            .map_err(|_| DagIdPathError::Empty)?;
        let package = DagPackageId::new(Arc::clone(segments.last()));
        Ok(Self::new(package, segments))
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
        let segments = relative_path_segments(path)?;
        let segments = NonEmpty::try_from_vec(segments).map_err(|_| DagIdPathError::Empty)?;
        Ok(Self::new(package, segments))
    }
}

fn relative_path_segments(path: &std::path::Path) -> Result<Vec<Arc<str>>, DagIdPathError> {
    use std::path::Component;

    if path.as_os_str().is_empty() {
        return Err(DagIdPathError::Empty);
    }

    let mut segments = path
        .components()
        .map(|component| match component {
            Component::Normal(component) => component
                .to_str()
                .map(Arc::<str>::from)
                .ok_or(DagIdPathError::NonUtf8Component),
            Component::ParentDir => Err(DagIdPathError::ParentTraversal),
            Component::CurDir => Err(DagIdPathError::CurrentDirectory),
            Component::RootDir | Component::Prefix(_) => Err(DagIdPathError::NotRelative),
        })
        .collect::<Result<Vec<_>, _>>()?;

    let last = segments.last_mut().ok_or(DagIdPathError::Empty)?;
    let stem = last
        .strip_suffix(".gcl")
        .ok_or(DagIdPathError::MissingGclExtension)?;
    if stem.is_empty() {
        return Err(DagIdPathError::EmptyFileStem);
    }
    *last = Arc::<str>::from(stem);

    Ok(segments)
}

impl fmt::Display for DagId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, seg) in self.segments.iter().enumerate() {
            if i > 0 {
                f.write_str(".")?;
            }
            fmt::Display::fmt(seg, f)?;
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
        let segs: Vec<String> = id.segments().iter().map(ToString::to_string).collect();
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
    fn from_relative_path_rejects_absolute_paths() {
        let err = DagId::from_relative_path("math", std::path::Path::new("/helpers/math.gcl"))
            .unwrap_err();
        assert_eq!(err, DagIdPathError::NotRelative);
    }

    #[test]
    fn from_relative_path_rejects_parent_traversal() {
        let err = DagId::from_relative_path("math", std::path::Path::new("helpers/../math.gcl"))
            .unwrap_err();
        assert_eq!(err, DagIdPathError::ParentTraversal);
    }

    #[test]
    fn from_relative_path_rejects_current_directory_components() {
        let err =
            DagId::from_relative_path("math", std::path::Path::new("./math.gcl")).unwrap_err();
        assert_eq!(err, DagIdPathError::CurrentDirectory);
    }

    #[test]
    fn from_relative_path_rejects_empty_file_stem() {
        let err = DagId::from_relative_path("math", std::path::Path::new(".gcl")).unwrap_err();
        assert_eq!(err, DagIdPathError::EmptyFileStem);
        assert_eq!(
            DagId::from_virtual_relative_path(std::path::Path::new(".gcl")).unwrap_err(),
            DagIdPathError::EmptyFileStem
        );
    }

    #[test]
    fn child_appends_segment() {
        let parent = DagId::new("test", NonEmpty::new("helpers", vec!["math"]));
        let child = parent.child("double_speed");
        assert_eq!(child.to_string(), "helpers.math.double_speed");
    }

    #[test]
    fn concrete_instance_is_distinct_from_same_named_source_module() {
        let parent = DagId::root_in_package("test", "model");
        let source = DagId::new("test", NonEmpty::new("model", vec!["defaults"]));
        let instance = parent.named_instance_child("defaults");

        assert_eq!(source.to_string(), instance.to_string());
        assert_ne!(source, instance);
        assert_eq!(source.parent(), Some(parent.clone()));
        assert_eq!(instance.parent(), Some(parent));
    }

    #[test]
    fn rebase_descendant_preserves_instance_edges() {
        let template = DagId::root_in_package("test", "template");
        let nested = template.named_instance_child("inner").child("helper");
        let configured = DagId::root_in_package("test", "main").named_instance_child("configured");

        let DescendantRebase::Rebased(rebased) = nested.rebase_descendant(&template, &configured)
        else {
            panic!("nested identity must be inside the template subtree");
        };
        let expected = configured.named_instance_child("inner").child("helper");
        assert_eq!(rebased, expected);
        assert_eq!(rebased.to_string(), "main.configured.inner.helper");
    }

    #[test]
    fn rebase_descendant_classifies_an_external_owner() {
        let template = DagId::root_in_package("test", "template");
        let external = DagId::root_in_package("dependency", "external");
        let configured = DagId::root_in_package("test", "main").named_instance_child("configured");

        assert_eq!(
            external.rebase_descendant(&template, &configured),
            DescendantRebase::OutsideSubtree
        );
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
                .iter()
                .map(ToString::to_string)
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
    fn leaf_returns_last_segment() {
        let id = DagId::new(
            "test",
            NonEmpty::new("helpers", vec!["math", "double_speed"]),
        );
        assert_eq!(id.leaf(), &DagSegment::SourceModule("double_speed".into()));
        assert_eq!(id.leaf().spelling(), Some("double_speed"));
    }

    #[test]
    fn leaf_of_root_returns_head() {
        let id = DagId::root_in_package("test", "main");
        assert_eq!(id.leaf().spelling(), Some("main"));
    }

    #[test]
    fn display_joins_with_dot() {
        let id = DagId::new("test", NonEmpty::new("a", vec!["b", "c"]));
        assert_eq!(id.to_string(), "a.b.c");
    }

    #[test]
    fn include_instances_are_opaque_segments() {
        let owner = DagId::root_in_package("test", "main");
        let first = owner.include_instance_child(IncludeInstanceId::at_source_offset(10));
        let second = owner.include_instance_child(IncludeInstanceId::at_source_offset(20));
        let named = owner.named_instance_child("<include@10>");

        assert_ne!(first, second);
        assert_eq!(first.to_string(), "main.<include@10>");
        assert_eq!(first.leaf().spelling(), None);
        assert!(first.leaf().is_instance());
        // A named instance can never be confused with an anonymous one, even
        // if a spelling renders identically.
        assert_eq!(named.to_string(), first.to_string());
        assert_ne!(named, first);
        assert_ne!(named.cmp(&first), std::cmp::Ordering::Equal);
        assert_eq!(first.parent(), Some(owner));
    }

    #[test]
    fn to_instance_keeps_identity_and_marks_instance() {
        let module = DagSegment::SourceModule("helper".into());
        let named = DagSegment::NamedInstance("helper".into());
        let anonymous = DagSegment::IncludeInstance(IncludeInstanceId::at_source_offset(3));

        assert!(!module.is_instance());
        assert_eq!(module.to_instance(), named);
        assert_eq!(named.to_instance(), named);
        assert_eq!(anonymous.to_instance(), anonymous);

        let owner = DagId::root_in_package("test", "main");
        assert_eq!(
            owner.instance_child_like(&module),
            owner.named_instance_child("helper")
        );
        assert_eq!(
            owner.instance_child_like(&anonymous),
            owner.include_instance_child(IncludeInstanceId::at_source_offset(3))
        );
    }

    #[test]
    fn ordering_compares_rendered_text_before_segment_kind() {
        let root = DagId::root_in_package("test", "main");
        let anonymous_late = root.include_instance_child(IncludeInstanceId::at_source_offset(100));
        let anonymous_early = root.include_instance_child(IncludeInstanceId::at_source_offset(63));
        let named = root.named_instance_child("alpha");
        let module = root.child("alpha");

        // `<include@100>` < `<include@63>` < `alpha` as rendered text.
        assert!(anonymous_late < anonymous_early);
        assert!(anonymous_early < named);
        // Equal text: source modules sort before instances.
        assert!(module < named);
        assert!(root < module);
        assert!(DagId::root_in_package("a", "z") < DagId::root_in_package("b", "a"));
    }

    #[test]
    fn instance_segments_participate_in_descendant_checks() {
        let root = DagId::root_in_package("test", "main");
        let module = root.child("inner");
        let instance = root.named_instance_child("inner");
        let nested = instance.child("helper");

        assert!(nested.is_descendant_of(&instance));
        assert!(nested.is_descendant_of(&root));
        assert!(!nested.is_descendant_of(&module));
        assert_eq!(
            nested.rebase_descendant(&module, &root),
            DescendantRebase::OutsideSubtree
        );
    }
}
