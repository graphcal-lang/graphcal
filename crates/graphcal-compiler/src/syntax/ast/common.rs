use crate::syntax::names::{GenericParamName, ModuleAliasName, NameAtom};
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::span::{Span, Spanned};

/// An attribute annotation on a declaration: `#[name]` or `#[name(arg1, arg2)]`.
#[derive(Debug, Clone)]
pub struct Attribute {
    pub name: Ident,
    pub args: Vec<AttributeArg>,
    pub span: Span,
}

/// An argument inside an attribute's parenthesized list.
///
/// Supports plain identifiers (`pressure_safe`), qualified paths
/// (`Index.Variant`), Nat range steps (`#2`), and parenthesized groups
/// (`(Mode.Boost, Phase.Launch)`, `(Mode.Boost, #2)`).
#[derive(Debug, Clone)]
pub enum AttributeArg {
    /// A path of one or more `.`-separated segments: `foo`, `Index.Variant`.
    Path {
        segments: NonEmpty<Ident>,
        span: Span,
    },
    /// A Nat range step key: `#N` — matches the `#N` slice-label syntax of
    /// `table` expressions for `range(N)` axes.
    RangeStep { step: u64, span: Span },
    /// A parenthesized group of args: `(Index.A, Index.B).`
    Group { elements: Vec<Self>, span: Span },
}

impl AttributeArg {
    /// Returns the span of this argument.
    #[must_use]
    pub const fn span(&self) -> Span {
        match self {
            Self::Path { span, .. } | Self::RangeStep { span, .. } | Self::Group { span, .. } => {
                *span
            }
        }
    }
}

/// Visibility annotation for declaration kinds that can be public but cannot be bindable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Private,
    Public,
}

impl Visibility {
    /// Returns `true` for `Public`.
    #[must_use]
    pub const fn is_public(self) -> bool {
        matches!(self, Self::Public)
    }
}

/// Visibility and bindability annotation for declaration kinds that support `pub(bind)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindableVisibility {
    Private,
    Public,
    PublicBind,
}

impl BindableVisibility {
    /// Returns `true` for `Public` and `PublicBind`.
    #[must_use]
    pub const fn is_public(self) -> bool {
        matches!(self, Self::Public | Self::PublicBind)
    }

    /// Returns `true` for `PublicBind`.
    #[must_use]
    pub const fn is_bindable(self) -> bool {
        matches!(self, Self::PublicBind)
    }
}

impl From<Visibility> for BindableVisibility {
    fn from(visibility: Visibility) -> Self {
        match visibility {
            Visibility::Private => Self::Private,
            Visibility::Public => Self::Public,
        }
    }
}
/// The kind of an `import` or `include` declaration.
///
/// For `import`:
///   - `Selective(items)`: brace-list form `import path.{X, Y};` — brings only
///     the listed names. Does NOT also bring the leaf module.
///   - `Module { alias: None }`: bare form `import path;` — brings the leaf
///     module under its own name.
///   - `Module { alias: Some(a) }`: aliased form `import path as a;`.
///
/// For `include`:
///   - `Selective(items)`: brace-list form `include path(args).{y};` — exposes
///     the listed outputs as nodes.
///   - `Module { alias: None }`: bare form `include path(args);` — sugar for
///     `as <leaf>`.
///   - `Module { alias: Some(a) }`: aliased form `include path(args) as a;`.
#[derive(Debug, Clone)]
pub enum ImportKind {
    /// Brace-list selector: `path.{ X, Y as Z, ... }`.
    Selective(Vec<ImportItem>),
    /// Bare or aliased form.
    Module {
        alias: Option<Spanned<ModuleAliasName>>,
    },
}

/// A dot-separated module path: `nasa.rocket.dynamics`.
///
/// Always absolute from a package root. The first segment is the package name
/// (real or virtual); subsequent segments walk the package's module tree
/// (directories under `source_dir`, files inside the package, and inline `dag`
/// declarations). There are no file-path strings, no `..` parent navigation,
/// and no `/` separators in the source language — only `.`.
#[derive(Debug, Clone)]
pub struct ModulePath {
    pub segments: NonEmpty<Ident>,
    pub span: Span,
}

impl ModulePath {
    #[must_use]
    pub const fn span(&self) -> Span {
        self.span
    }

    /// Borrow all path segments in source order.
    #[must_use]
    pub fn segments(&self) -> &[Ident] {
        self.segments.as_slice()
    }

    /// Number of path segments. Always at least 1.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.segments.len()
    }

    /// Returns `false`; provided for API compatibility with sequence-like code.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }

    /// Returns whether this is a one-segment module path.
    #[must_use]
    pub const fn is_bare(&self) -> bool {
        self.segments.len() == 1
    }

    /// Human-readable path string for diagnostics: `"nasa.rocket.dynamics"`.
    #[must_use]
    pub fn display_path(&self) -> String {
        self.segments
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>()
            .join(".")
    }

    /// Returns the leaf segment of the path.
    #[must_use]
    pub fn leaf(&self) -> &Ident {
        self.segments.last()
    }

    /// Split the path into qualifier segments and the leaf segment.
    ///
    /// The qualifier slice is empty for one-segment paths.
    #[must_use]
    pub fn split_last(&self) -> (&[Ident], &Ident) {
        let (leaf, qualifier) = self.segments.split_last();
        (qualifier, leaf)
    }

    /// Returns the qualifier segments before the leaf. Empty for bare paths.
    #[must_use]
    pub fn qualifier_segments(&self) -> &[Ident] {
        self.split_last().0
    }

    /// Returns qualifier segments and leaf only when this path is qualified.
    #[must_use]
    pub fn qualifier_and_leaf(&self) -> Option<(&[Ident], &Ident)> {
        let (qualifier, leaf) = self.split_last();
        (!qualifier.is_empty()).then_some((qualifier, leaf))
    }
}
/// A single item in an `import` declaration, optionally aliased.
///
/// Example: `name1 as local_name` → `ImportItem { name: "name1", alias: Some("local_name") }`
/// Example: `name1` → `ImportItem { name: "name1", alias: None }`
/// Example: `type name1` → imports from the type namespace.
/// Example: `pub name1` → re-exported at the importer (selective form).
#[derive(Debug, Clone)]
pub struct ImportItem {
    /// Attributes on this import item (e.g., `#[expected_fail(...)]`).
    pub attributes: Vec<Attribute>,
    /// Whether this item is re-exported (`pub` prefix) from the importer.
    pub is_pub: bool,
    /// Which namespace this selective import targets.
    pub namespace: ImportItemNamespace,
    /// The name requested from the imported module.
    ///
    /// Its span is the identifier's use-site span in this `import`/`include`
    /// statement, not the definition-site span in the imported module. The AST
    /// is produced before external module resolution.
    pub name: Ident,
    /// Optional local alias (introduced by `as`).
    pub alias: Option<Ident>,
}

/// Namespace targeted by a single selective import item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportItemNamespace {
    /// Default compile-time namespace: consts, dimensions, units, indexes,
    /// DAGs, assertions, and other non-type importable items.
    Default,
    /// Type namespace, written with the `type` marker.
    Type,
}

impl ImportItem {
    /// The name that this import introduces into the local scope.
    /// Returns the alias if present, otherwise the original name.
    #[must_use]
    pub fn local_name(&self) -> &str {
        self.alias
            .as_ref()
            .map_or(self.name.name.as_str(), |a| a.name.as_str())
    }

    /// The span of the local name (alias span if aliased, otherwise original name span).
    #[must_use]
    pub fn local_span(&self) -> Span {
        self.alias.as_ref().map_or(self.name.span, |a| a.span)
    }
}
/// An identifier with its source span.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Ident {
    pub name: NameAtom,
    pub span: Span,
}

impl Ident {
    /// Convert this identifier into a `Spanned<T>`, consuming the name and span.
    #[must_use]
    pub fn into_spanned<T: From<NameAtom>>(self) -> Spanned<T> {
        Spanned::new(T::from(self.name), self.span)
    }

    /// Interpret this identifier as a generic parameter name.
    #[must_use]
    pub fn as_generic_param_name(&self) -> GenericParamName {
        GenericParamName::new(&self.name)
    }
}
