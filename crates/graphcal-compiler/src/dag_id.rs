//! [`DagId`]: an abstract, filesystem-independent identifier for a DAG (module).
//!
//! Every file, inline `dag` block, and concrete include instance gets a unique
//! package-qualified `DagId`. File-based DAGs derive their segments from the
//! loader-provided module path (e.g., `helpers/math.gcl` →
//! `["helpers", "math"]`), while inline `dag` blocks append their declaration
//! name as an inline-DAG segment (e.g., `["helpers", "math", "double_speed"]`).
//! Include instances append an instance segment instead. Each segment keeps its
//! kind, so a file submodule, an inline DAG, and an instance with the same
//! displayed path remain structurally distinct.
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

use crate::syntax::decl_name::DeclName;
use crate::syntax::module_name::{ModuleAliasName, ScopeSegment};
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

/// One typed segment of a [`DagId`].
///
/// A segment records both its relationship to the parent and the child's
/// identity:
///
/// - [`Self::File`] segments are the loader-provided file-path components of a
///   file root. They form the non-empty prefix of every [`DagId`] and never
///   follow another kind of segment.
/// - [`Self::InlineDag`] is a `dag` declaration nested in its parent module.
/// - [`Self::Instance`] is a concrete include instance, carrying the scope it
///   was instantiated under (a module alias, or an anonymous selective
///   include).
///
/// A file submodule, an inline DAG, and a named instance may share a
/// spelling, but they are different semantic identities.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DagSegment {
    /// A file-path component of a file root.
    File(Arc<str>),
    /// An inline `dag` declaration.
    InlineDag(DeclName),
    /// A concrete include instance and the scope it was instantiated under.
    Instance(ScopeSegment),
}

impl DagSegment {
    /// Whether this segment is a concrete include instance.
    #[must_use]
    pub const fn is_instance(&self) -> bool {
        matches!(self, Self::Instance(_))
    }

    /// The `dag` declaration this segment names, when it is an inline DAG.
    #[must_use]
    pub const fn inline_dag(&self) -> Option<&DeclName> {
        match self {
            Self::InlineDag(name) => Some(name),
            Self::File(_) | Self::Instance(_) => None,
        }
    }

    /// The qualifier segment that names this child below its parent module.
    ///
    /// An inline DAG is qualified by its declaration name and an instance by
    /// its scope. A file-path component is not a module-scoped child, so it
    /// has none.
    #[must_use]
    pub fn scope(&self) -> Option<ScopeSegment> {
        match self {
            Self::File(_) => None,
            Self::InlineDag(name) => Some(ScopeSegment::Named(ModuleAliasName::classify(
                name.atom().clone(),
            ))),
            Self::Instance(scope) => Some(scope.clone()),
        }
    }

    /// Source spelling of a file or inline-DAG segment; instances have none
    /// on a module path.
    fn module_path_spelling(&self) -> Option<&str> {
        match self {
            Self::File(name) => Some(name),
            Self::InlineDag(name) => Some(name.as_str()),
            Self::Instance(_) => None,
        }
    }

    /// Rendered text, used only to keep [`DagId`]'s established ordering.
    fn rendered(&self) -> std::borrow::Cow<'_, str> {
        match self {
            Self::File(name) => std::borrow::Cow::Borrowed(name),
            Self::InlineDag(name) => std::borrow::Cow::Borrowed(name.as_str()),
            Self::Instance(scope) => std::borrow::Cow::Owned(scope.to_string()),
        }
    }
}

impl fmt::Display for DagSegment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File(name) => f.write_str(name),
            Self::InlineDag(name) => fmt::Display::fmt(name, f),
            Self::Instance(scope) => fmt::Display::fmt(scope, f),
        }
    }
}

/// An abstract identifier for a DAG in the compiler pipeline.
///
/// Segments form a hierarchical identity: for example, a file at
/// `helpers/math.gcl` has segments `["helpers", "math"]`, and an inline
/// `dag double_speed` within it has segments
/// `["helpers", "math", "double_speed"]`. Each [`DagSegment`] preserves
/// whether that child is a file-path component, an inline DAG, or a concrete
/// instance; display text alone is not identity. Lexical visibility is
/// maintained separately by the module resolver.
///
/// Non-emptiness is encoded structurally with [`NonEmpty`], so [`DagId::leaf`]
/// is total — there is no value of this type that has zero segments. Every
/// constructor starts from file segments; only the child constructors append
/// inline-DAG and instance segments, so file segments are always a prefix.
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
    /// (non-instances before instances).
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

/// Typed identity of one concrete include or DAG-call instance.
///
/// `owner` is the fresh runtime namespace allocated at the call/include site:
/// the `scope` child of `parent`. `template` is the canonical reusable DAG
/// definition it instantiates. Keeping both prevents a concrete instance from
/// being mistaken for its source module merely because their declaration
/// leaves have the same spelling.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InstanceId {
    owner: DagId,
    template: DagId,
    parent: DagId,
    scope: ScopeSegment,
}

impl InstanceId {
    /// Identify the instance of `template` allocated under `parent` in `scope`.
    #[must_use]
    pub fn new(parent: DagId, scope: ScopeSegment, template: DagId) -> Self {
        Self {
            owner: parent.instance_child(scope.clone()),
            template,
            parent,
            scope,
        }
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

    /// Module whose include or call allocated this instance.
    #[must_use]
    pub const fn parent(&self) -> &DagId {
        &self.parent
    }

    /// Scope this instance occupies in its parent.
    #[must_use]
    pub const fn scope(&self) -> &ScopeSegment {
        &self.scope
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
    /// Create a file-root `DagId` from an explicit package and non-empty
    /// file-path segments.
    pub fn new(package: impl Into<DagPackageId>, segments: impl Into<NonEmpty<Arc<str>>>) -> Self {
        Self {
            package: package.into(),
            segments: segments.into().map(DagSegment::File),
        }
    }

    /// Create a single-segment (root) file `DagId` in an explicit package.
    pub fn root_in_package(package: impl Into<DagPackageId>, name: impl Into<Arc<str>>) -> Self {
        Self {
            package: package.into(),
            segments: NonEmpty::singleton(DagSegment::File(name.into())),
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

    /// Create the child module of the inline `dag` declaration `name`.
    #[must_use]
    pub fn inline_dag_child(&self, name: DeclName) -> Self {
        self.with_child(DagSegment::InlineDag(name))
    }

    /// Create the concrete include-instance child allocated in `scope`.
    ///
    /// This is structurally distinct from [`Self::inline_dag_child`] even
    /// when both names render identically.
    #[must_use]
    pub fn instance_child(&self, scope: ScopeSegment) -> Self {
        self.with_child(DagSegment::Instance(scope))
    }

    /// Return the module this one is nested in, or `None` for a file root.
    ///
    /// File roots have no lexical parent: a file submodule (`lib/x.gcl`) is
    /// not a child of `lib.gcl`, even though its path extends it.
    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        if matches!(self.leaf(), DagSegment::File(_)) {
            return None;
        }
        let (_, parent_segments) = self.segments.as_slice().split_last()?;
        let (root, rest) = parent_segments.split_first()?;
        Some(Self {
            package: self.package.clone(),
            segments: NonEmpty::new(root.clone(), rest.to_vec()),
        })
    }

    /// The file root this module is declared or instantiated in.
    #[must_use]
    pub fn file_root(&self) -> Self {
        let mut root = self.clone();
        while let Some(parent) = root.parent() {
            root = parent;
        }
        root
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

    /// Qualifier segments that name `self` below `ancestor`: `Some(empty)`
    /// when they are equal, `None` when `self` is not in `ancestor`'s subtree.
    #[must_use]
    pub fn scopes_below(&self, ancestor: &Self) -> Option<Vec<ScopeSegment>> {
        if self != ancestor && !self.is_descendant_of(ancestor) {
            return None;
        }
        self.segments
            .iter()
            .skip(ancestor.segments.len())
            .map(DagSegment::scope)
            .collect()
    }

    /// True if `self` is a strict descendant of `ancestor` (an inline `dag`
    /// block or instance nested — at any depth — inside `ancestor`).
    ///
    /// A file submodule is never a descendant of another file.
    #[must_use]
    pub fn is_descendant_of(&self, ancestor: &Self) -> bool {
        self.package == ancestor.package
            && self
                .segments
                .as_slice()
                .strip_prefix(ancestor.segments.as_slice())
                .and_then(<[DagSegment]>::first)
                .is_some_and(|child| !matches!(child, DagSegment::File(_)))
    }

    /// The spelling of this module on an import path (file-path components,
    /// then inline DAG names), or `None` for a concrete instance, which no
    /// module path can name.
    ///
    /// Two distinct source modules of one package with equal spellings would
    /// make that module path ambiguous.
    #[must_use]
    pub fn module_path_spelling(&self) -> Option<Vec<&str>> {
        self.segments
            .iter()
            .map(DagSegment::module_path_spelling)
            .collect()
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
    use crate::syntax::module_name::IncludeInstanceId;

    fn dag(name: &str) -> DeclName {
        DeclName::try_new(name).unwrap()
    }

    fn named(alias: &str) -> ScopeSegment {
        ScopeSegment::Named(ModuleAliasName::try_new(alias).unwrap())
    }

    fn anonymous(offset: usize) -> ScopeSegment {
        ScopeSegment::IncludeInstance(IncludeInstanceId::at_source_offset(offset))
    }

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
    fn inline_dag_child_appends_segment() {
        let parent = DagId::new("test", NonEmpty::new("helpers", vec!["math"]));
        let child = parent.inline_dag_child(dag("double_speed"));
        assert_eq!(child.to_string(), "helpers.math.double_speed");
        assert_eq!(child.leaf().inline_dag(), Some(&dag("double_speed")));
    }

    #[test]
    fn concrete_instance_is_distinct_from_same_named_inline_dag() {
        let parent = DagId::root_in_package("test", "model");
        let source = parent.inline_dag_child(dag("defaults"));
        let instance = parent.instance_child(named("defaults"));

        assert_eq!(source.to_string(), instance.to_string());
        assert_ne!(source, instance);
        assert_eq!(source.parent(), Some(parent.clone()));
        assert_eq!(instance.parent(), Some(parent));
    }

    #[test]
    fn file_submodule_is_distinct_from_same_named_inline_dag() {
        let parent = DagId::new("test", NonEmpty::new("pkg", vec!["lib"]));
        let inline = parent.inline_dag_child(dag("x"));
        let file = DagId::new("test", NonEmpty::new("pkg", vec!["lib", "x"]));

        assert_eq!(inline.to_string(), file.to_string());
        assert_ne!(inline, file);
        assert_eq!(inline.module_path_spelling(), file.module_path_spelling());
        // A file submodule is not nested in the file its path extends.
        assert_eq!(file.parent(), None);
        assert!(!file.is_descendant_of(&parent));
        assert!(inline.is_descendant_of(&parent));
    }

    #[test]
    fn module_path_spelling_excludes_instances() {
        let root = DagId::root_in_package("test", "main");
        assert_eq!(root.module_path_spelling(), Some(vec!["main"]));
        assert_eq!(
            root.inline_dag_child(dag("inner")).module_path_spelling(),
            Some(vec!["main", "inner"])
        );
        assert_eq!(
            root.instance_child(named("inner")).module_path_spelling(),
            None
        );
    }

    #[test]
    fn parent_drops_last_nested_segment() {
        let id = DagId::new("test", NonEmpty::new("helpers", vec!["math"]))
            .inline_dag_child(dag("double_speed"));
        let parent = id.parent().unwrap();
        assert_eq!(parent.to_string(), "helpers.math");
    }

    #[test]
    fn parent_of_file_root_is_none() {
        assert!(DagId::root_in_package("test", "main").parent().is_none());
        assert!(
            DagId::new("test", NonEmpty::new("helpers", vec!["math"]))
                .parent()
                .is_none()
        );
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
        let child = root.inline_dag_child(dag("helper"));

        assert_eq!(child.package(), root.package());
        assert_eq!(child.parent(), Some(root));
    }

    #[test]
    fn is_descendant_of_matches_nested_blocks_only() {
        let file = DagId::new("test", NonEmpty::new("helpers", vec!["math"]));
        let child = file.inline_dag_child(dag("double_speed"));
        let grandchild = child.inline_dag_child(dag("inner"));
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
    fn scopes_below_names_nested_segments() {
        let root = DagId::root_in_package("test", "main");
        let nested = root
            .instance_child(named("inst"))
            .instance_child(anonymous(4))
            .inline_dag_child(dag("inner"));

        assert_eq!(root.scopes_below(&root), Some(Vec::new()));
        assert_eq!(
            nested.scopes_below(&root),
            Some(vec![named("inst"), anonymous(4), named("inner")])
        );
        assert_eq!(root.scopes_below(&nested), None);
        assert_eq!(
            DagId::new("test", NonEmpty::new("main", vec!["x"])).scopes_below(&root),
            None
        );
    }

    #[test]
    fn segment_scopes_and_inline_dags_are_typed() {
        assert_eq!(DagSegment::File("lib".into()).scope(), None);
        assert_eq!(DagSegment::File("lib".into()).inline_dag(), None);
        assert_eq!(
            DagSegment::InlineDag(dag("inner")).scope(),
            Some(named("inner"))
        );
        assert_eq!(
            DagSegment::Instance(anonymous(1)).scope(),
            Some(anonymous(1))
        );
        assert_eq!(DagSegment::Instance(named("inner")).inline_dag(), None);
    }

    #[test]
    fn display_joins_with_dot() {
        let id = DagId::new("test", NonEmpty::new("a", vec!["b", "c"]));
        assert_eq!(id.to_string(), "a.b.c");
    }

    #[test]
    fn include_instances_are_opaque_segments() {
        let owner = DagId::root_in_package("test", "main");
        let first = owner.instance_child(anonymous(10));
        let second = owner.instance_child(anonymous(20));
        let named = owner.instance_child(named("<include@10>"));

        assert_ne!(first, second);
        assert_eq!(first.to_string(), "main.<include@10>");
        assert_eq!(first.leaf().inline_dag(), None);
        assert!(first.leaf().is_instance());
        // A named instance can never be confused with an anonymous one, even
        // if a spelling renders identically.
        assert_eq!(named.to_string(), first.to_string());
        assert_ne!(named, first);
        assert_ne!(named.cmp(&first), std::cmp::Ordering::Equal);
        assert_eq!(first.parent(), Some(owner));
    }

    #[test]
    fn instance_id_records_parent_scope_and_owner() {
        let parent = DagId::root_in_package("test", "main");
        let template = DagId::root_in_package("test", "lib");
        let id = InstanceId::new(parent.clone(), named("inst"), template.clone());

        assert_eq!(id.owner(), &parent.instance_child(named("inst")));
        assert_eq!(id.parent(), &parent);
        assert_eq!(id.scope(), &named("inst"));
        assert_eq!(id.template(), &template);
    }

    #[test]
    fn ordering_compares_rendered_text_before_segment_kind() {
        let root = DagId::root_in_package("test", "main");
        let anonymous_late = root.instance_child(anonymous(100));
        let anonymous_early = root.instance_child(anonymous(63));
        let named = root.instance_child(named("alpha"));
        let module = root.inline_dag_child(dag("alpha"));

        // `<include@100>` < `<include@63>` < `alpha` as rendered text.
        assert!(anonymous_late < anonymous_early);
        assert!(anonymous_early < named);
        // Equal text: inline DAGs sort before instances.
        assert!(module < named);
        assert!(root < module);
        assert!(DagId::root_in_package("a", "z") < DagId::root_in_package("b", "a"));
    }

    #[test]
    fn instance_segments_participate_in_descendant_checks() {
        let root = DagId::root_in_package("test", "main");
        let module = root.inline_dag_child(dag("inner"));
        let instance = root.instance_child(named("inner"));
        let nested = instance.inline_dag_child(dag("helper"));

        assert!(nested.is_descendant_of(&instance));
        assert!(nested.is_descendant_of(&root));
        assert!(!nested.is_descendant_of(&module));
    }
}
