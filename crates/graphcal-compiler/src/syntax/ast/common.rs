use crate::syntax::format_equivalent::FormatEquivalent;
use crate::syntax::import_category::ImportItemNamespace;
use crate::syntax::index_name::IndexVariantName;
use crate::syntax::module_name::ModuleAliasName;
use crate::syntax::names::{NameDef, NameNamespace, NamePath};
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::span::{Span, Spanned};
use crate::syntax::token::SourceIdentifier;
use crate::syntax::type_name::GenericParamName;

/// An attribute annotation on a declaration: `#[name]` or `#[name(arg1, arg2)]`.
#[derive(Debug, Clone, FormatEquivalent)]
pub struct Attribute {
    pub name: Ident,
    pub args: Vec<AttributeArg>,
    #[fe(skip)]
    pub span: Span,
}

/// An argument inside an attribute's parenthesized list.
///
/// Supports Term names (`pressure_safe`, `checks::pressure_safe`), index labels
/// (`Mode#Boost`), finite structural positions (`#2`), and parenthesized groups.
#[derive(Debug, Clone, FormatEquivalent)]
pub enum AttributeArg {
    /// A syntax-directed Term name.
    Path { path: Spanned<NamePath> },
    /// An owner-qualified index label selected by `#`.
    IndexLabel {
        index: Spanned<NamePath>,
        label: Spanned<IndexVariantName>,
        #[fe(skip)]
        span: Span,
    },
    /// A finite structural position key: `#N` — matches the `#N` slice-label
    /// syntax of `table` expressions over `Fin(N)` axes.
    FinitePosition {
        position: u64,
        #[fe(skip)]
        span: Span,
    },
    /// A parenthesized group of args: `(Index#A, Index#B).`
    Group {
        elements: Vec<Self>,
        #[fe(skip)]
        span: Span,
    },
}

impl AttributeArg {
    /// Returns the span of this argument.
    #[must_use]
    pub(crate) const fn span(&self) -> Span {
        match self {
            Self::Path { path } => path.span,
            Self::IndexLabel { span, .. }
            | Self::FinitePosition { span, .. }
            | Self::Group { span, .. } => *span,
        }
    }
}

/// Visibility annotation for declaration kinds that can be public but cannot be bindable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, FormatEquivalent)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, FormatEquivalent)]
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
    pub(crate) const fn is_bindable(self) -> bool {
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
///   - `Selective(items)`: brace-list form `import path::{X, Y};` — brings only
///     the listed names. Does NOT also bring the leaf module.
///   - `Module { alias: None }`: bare form `import path;` — brings the leaf
///     module under its own name.
///   - `Module { alias: Some(a) }`: aliased form `import path as a;`.
///
/// For `include`:
///   - `Selective(items)`: brace-list form `include path(args)::{y};` — exposes
///     the listed outputs as nodes.
///   - `Module { alias: None }`: bare form `include path(args);` — sugar for
///     `as <leaf>`.
///   - `Module { alias: Some(a) }`: aliased form `include path(args) as a;`.
#[derive(Debug, Clone, FormatEquivalent)]
pub enum ImportKind {
    /// Brace-list selector: `path::{ X, Y as Z, ... }`.
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
#[derive(Debug, Clone, FormatEquivalent)]
pub struct ModulePath {
    pub segments: NonEmpty<Ident>,
    #[fe(skip)]
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
}

/// A single item in an `import` declaration, optionally aliased.
///
/// Example: `name1 as local_name` → `ImportItem { name: "name1", alias: Some("local_name") }`
/// Example: `name1` → `ImportItem { name: "name1", alias: None }`
/// Example: `type name1` → imports from the type namespace.
/// Example: `dim Length` → imports from the dimension namespace.
/// Example: `unit m` → imports from the unit namespace.
/// Example: `index Case` → imports from the index namespace.
/// Example: `pub name1` → re-exported at the importer (selective form).
#[derive(Debug, Clone, FormatEquivalent)]
pub struct ImportItem {
    /// Attributes on this import item (e.g., `#[expected_fail(...)]`).
    pub attributes: Vec<Attribute>,
    /// `Public` when the item is re-exported (`pub` prefix) from the importer.
    pub visibility: Visibility,
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

impl ImportItem {
    /// The validated name atom that this import introduces into local scope.
    /// Returns the alias if present, otherwise the original name.
    #[must_use]
    pub fn local_name_atom(&self) -> &crate::syntax::names::NameAtom {
        self.alias.as_ref().unwrap_or(&self.name).name.atom()
    }

    /// The spelling of the name that this import introduces into local scope.
    #[must_use]
    pub fn local_name(&self) -> &str {
        self.local_name_atom().as_str()
    }

    /// The span of the local name (alias span if aliased, otherwise original name span).
    #[must_use]
    pub fn local_span(&self) -> Span {
        self.alias.as_ref().map_or(self.name.span, |a| a.span)
    }
}

/// An identifier with its source span.
///
/// The spelling is a [`SourceIdentifier`]: every `Ident` is one lexer `IDENT`
/// token, so compiler-generated names (which may lie outside the source
/// identifier grammar) cannot masquerade as written identifiers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, FormatEquivalent)]
pub struct Ident {
    pub name: SourceIdentifier,
    #[fe(skip)]
    pub span: Span,
}

impl std::fmt::Display for Ident {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.name, f)
    }
}

impl Ident {
    /// Classify this identifier into the namespace fixed by its grammar
    /// position, consuming the name and span.
    #[must_use]
    pub(crate) fn classify<Ns: NameNamespace>(self) -> Spanned<NameDef<Ns>> {
        Spanned::new(NameDef::classify(self.name.into_atom()), self.span)
    }

    /// Interpret this identifier as a generic parameter name.
    #[must_use]
    pub(crate) fn as_generic_param_name(&self) -> GenericParamName {
        GenericParamName::classify(self.name.atom().clone())
    }
}
