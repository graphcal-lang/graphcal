//! Symbol table for LSP features: maps source locations to definitions and references.

use std::collections::HashMap;
use std::sync::Arc;

use graphcal_compiler::desugar::resolved_ast::{
    AssertDecl, AttributeArg, BaseDimDecl, BindableVisibility, DagDecl, DeclKind, DimDecl, DimExpr,
    DomainBound, ExprKind, FigureDecl, ImportDecl, IndexDecl, IndexDeclKind, LayerDecl,
    MatchPattern, NodeDecl, ParamDecl, PatternBinding, PlotDecl, TypeDecl, TypeDeclBody, TypeExpr,
    TypeExprKind, UnitDecl, UnitExpr,
};
use graphcal_compiler::syntax::attribute::AttributeName;
use graphcal_compiler::syntax::names::NamePath;
use graphcal_compiler::syntax::span::Span;

use graphcal_compiler::registry::builtins::{builtin_constants, builtin_functions};
use graphcal_compiler::registry::format::format_unit_expr_with_config;
use graphcal_compiler::registry::types::{IndexKind, Registry, UnitScale};
use graphcal_compiler::tir::typed::{ResolvedIndex, ResolvedTypeExpr, TIR};
use graphcal_eval::eval::format_number;
use tower_lsp::lsp_types::Position;

use crate::convert::LineIndex;

/// The kind of expression scope that introduces local variables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExprScopeKind {
    For,
    Scan,
    Unfold,
    Match,
}

/// A structured source path used by symbol-table keys.
///
/// This is deliberately not a dotted string. Source qualifier segments remain
/// available for editor-boundary lookups, while the leaf remains distinct from
/// the qualifier so callers never have to split display text to recover
/// structure.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SymbolPath {
    /// A same-scope leaf name.
    Local(String),
    /// A module-qualified name (`module.name`, possibly with nested module
    /// qualifier segments).
    Qualified { module: Vec<String>, name: String },
}

impl SymbolPath {
    pub(crate) fn local(name: impl Into<String>) -> Self {
        Self::Local(name.into())
    }

    fn qualified(module: Vec<String>, name: impl Into<String>) -> Self {
        Self::Qualified {
            module,
            name: name.into(),
        }
    }

    fn from_ident_path(path: &graphcal_compiler::syntax::ast::IdentPath) -> Self {
        match path.qualifier_and_leaf() {
            None => Self::local(path.leaf().name.to_string()),
            Some((qualifier, leaf)) => Self::qualified(
                qualifier
                    .iter()
                    .map(|segment| segment.name.to_string())
                    .collect(),
                leaf.name.to_string(),
            ),
        }
    }

    fn from_name_path(path: &NamePath) -> Self {
        match path.qualifier_and_leaf() {
            None => Self::local(path.leaf().to_string()),
            Some((qualifier, leaf)) => Self::qualified(
                qualifier.iter().map(ToString::to_string).collect(),
                leaf.to_string(),
            ),
        }
    }

    pub(crate) fn prepend_module(&self, module_name: &str) -> Self {
        match self {
            Self::Local(name) => Self::qualified(vec![module_name.to_string()], name.clone()),
            Self::Qualified { module, name } => {
                Self::qualified(prepend_segment(module_name, module), name.clone())
            }
        }
    }

    pub(crate) fn rekey_first_segment(&self, original: &str, local: &str) -> Option<Self> {
        match self {
            Self::Local(name) if name == original => Some(Self::local(local.to_string())),
            Self::Qualified { module, name } if module.first().is_some_and(|m| m == original) => {
                let mut rekeyed = Vec::with_capacity(module.len());
                rekeyed.push(local.to_string());
                rekeyed.extend(module.iter().skip(1).cloned());
                Some(Self::qualified(rekeyed, name.clone()))
            }
            _ => None,
        }
    }
}

/// Prepend a module segment to a qualifier path. Shared by
/// [`SymbolPath::prepend_module`] and the key-rekeying in `server.rs`.
pub fn prepend_segment(module_name: &str, module: &[String]) -> Vec<String> {
    let mut nested = Vec::with_capacity(module.len() + 1);
    nested.push(module_name.to_string());
    nested.extend(module.iter().cloned());
    nested
}

impl std::fmt::Display for SymbolPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local(name) => f.write_str(name),
            Self::Qualified { module, name } => {
                for (i, seg) in module.iter().enumerate() {
                    if i > 0 {
                        f.write_str(".")?;
                    }
                    f.write_str(seg)?;
                }
                write!(f, ".{name}")
            }
        }
    }
}

/// A typed key for symbol table entries.
///
/// Replaces ad-hoc `String` keys like `"fn_name::param"` or `"field::name"`
/// with a structured enum that can be pattern-matched.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SymbolKey {
    /// Top-level declaration: param, node, const, dim, unit, type, index,
    /// assert, plot, figure, builtins.
    TopLevel(String),
    /// Module-qualified reference: e.g., `@params::dry_mass` or `a.b.c::PI`.
    /// `module` carries the structured path segments rather than a flat
    /// formatted string, so two callers that build the same logical path
    /// always produce equal keys without depending on a shared join format.
    Qualified { module: Vec<String>, name: String },
    /// Tagged-union constructor. Constructors are separate from type names, so
    /// a record-like `type Student { Student(...) }` needs a distinct key from
    /// the `StructType` definition that shares its spelling.
    Constructor(SymbolPath),
    /// Variant of an index: e.g., `Season.Winter` or `module.Season.Winter`.
    Variant { parent: SymbolPath, variant: String },
    /// Field of a struct or union variant. `owner` distinguishes fields with
    /// the same name across different types/constructors.
    Field {
        owner: SymbolPath,
        field_name: String,
    },
    /// Expression-scoped local variable (block, for, scan, unfold, match).
    ExprScoped {
        kind: ExprScopeKind,
        offset: usize,
        local: String,
    },
}

fn symbol_key_for_path(path: &graphcal_compiler::syntax::ast::IdentPath) -> SymbolKey {
    match SymbolPath::from_ident_path(path) {
        SymbolPath::Local(name) => SymbolKey::TopLevel(name),
        SymbolPath::Qualified { module, name } => SymbolKey::Qualified { module, name },
    }
}

fn symbol_key_for_name_path(path: &NamePath) -> SymbolKey {
    match SymbolPath::from_name_path(path) {
        SymbolPath::Local(name) => SymbolKey::TopLevel(name),
        SymbolPath::Qualified { module, name } => SymbolKey::Qualified { module, name },
    }
}

fn variant_key_for_parts(
    index: &NamePath,
    variant: &graphcal_compiler::syntax::names::IndexVariantName,
) -> SymbolKey {
    SymbolKey::Variant {
        parent: SymbolPath::from_name_path(index),
        variant: variant.to_string(),
    }
}

fn name_path_from_ident_segments(
    segments: &[graphcal_compiler::syntax::ast::Ident],
) -> Option<NamePath> {
    graphcal_compiler::syntax::non_empty::NonEmpty::try_from_vec(
        segments
            .iter()
            .map(|segment| segment.name.clone())
            .collect(),
    )
    .ok()
    .map(NamePath::new)
}

impl SymbolKey {
    /// Returns the top-level name if this is a `TopLevel` key.
    pub fn top_level_name(&self) -> Option<&str> {
        match self {
            Self::TopLevel(name) => Some(name),
            _ => None,
        }
    }
}

/// The category of a symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolCategory {
    Param,
    Node,
    Const,
    Dimension,
    Unit,
    StructType,
    Constructor,
    Index,
    IndexVariant,
    Field,
    LocalVar,
    BuiltinFn,
    BuiltinConst,
    Assert,
    Plot,
    Figure,
    Layer,
    Dag,
}

/// Information about a symbol definition.
#[derive(Debug, Clone)]
pub struct DefinitionInfo {
    pub name: String,
    pub category: SymbolCategory,
    /// Span of just the name token.
    pub name_span: Span,
    /// Span of the full declaration.
    pub decl_span: Span,
    /// Human-readable type/signature for hover (populated from TIR).
    pub type_description: Option<String>,
    /// Additional detail for hover.
    pub detail: Option<String>,
    /// Visibility / bindability of the declaration.
    ///
    /// `None` for builtins, fields, and local variables (concepts that have
    /// no surface annotation).
    pub visibility: Option<BindableVisibility>,
}

impl DefinitionInfo {
    /// Returns `true` if this is a built-in function or constant.
    pub const fn is_builtin(&self) -> bool {
        matches!(
            self.category,
            SymbolCategory::BuiltinFn | SymbolCategory::BuiltinConst
        )
    }

    /// Returns `true` if this definition has a navigable source location.
    ///
    /// Builtins and definitions with empty name spans have no source to navigate to.
    pub const fn is_navigable(&self) -> bool {
        !self.is_builtin() && !self.name_span.is_empty()
    }
}

/// A reference occurrence: a name that refers to a definition.
#[derive(Debug, Clone)]
pub struct ReferenceInfo {
    /// Byte-offset span of this reference in the current file.
    pub span: Span,
    /// Key into `definitions` that this reference points to.
    pub target: SymbolKey,
}

/// A precomputed inlay-hint candidate: a declaration that could produce a hint.
///
/// Populated at [`SymbolTable::finalize`] time so the inlay-hint handler
/// avoids walking every definition (including builtins/imports) and avoids
/// O(source-length) scans to turn each name-span into an LSP `Position` on
/// every request.
#[derive(Debug, Clone)]
pub struct InlayHintEntry {
    /// Key into [`SymbolTable::definitions`] for this declaration. Shared
    /// via `Rc` with [`SymbolTable::defs_by_name_span`] so each key is
    /// allocated once and refcounted across the two sorted indices.
    pub key: Arc<SymbolKey>,
    /// LSP position of the declaration's name start.
    pub name_start: Position,
    /// LSP position of the declaration's name end (where the hint is placed).
    pub name_end: Position,
}

/// The complete symbol table for one file.
#[derive(Debug, Default)]
pub struct SymbolTable {
    /// All symbol definitions keyed by a typed `SymbolKey`.
    pub(crate) definitions: HashMap<SymbolKey, DefinitionInfo>,
    /// All reference occurrences sorted by span offset.
    pub(crate) references: Vec<ReferenceInfo>,
    /// Secondary index: name-span byte offset → `SymbolKey`.
    ///
    /// Populated alongside `definitions` so that `find_definition_key` is O(1)
    /// instead of a linear reverse scan.
    name_span_to_key: HashMap<usize, SymbolKey>,
    /// Definition name-spans sorted by offset, each paired with the entry's
    /// `SymbolKey` (refcounted — shared with [`Self::inlay_hint_entries`] so
    /// the keys are allocated once at finalize time). Used by
    /// `find_definition_at` for O(log n) lookup instead of scanning every
    /// definition on every hover/goto/rename request.
    ///
    /// Populated by [`SymbolTable::finalize`]; definitions added after
    /// `finalize` is called will not appear here until it is called again.
    defs_by_name_span: Vec<(Span, Arc<SymbolKey>)>,
    /// Precomputed inlay-hint candidates, sorted by name-start line/column.
    ///
    /// Only `Param`, `Node`, and `Const` declarations with non-empty name
    /// spans appear here — builtins, imports, and other categories are
    /// filtered out up front so the request path does zero work for them.
    ///
    /// Populated by [`SymbolTable::finalize`].
    inlay_hint_entries: Vec<InlayHintEntry>,
}

impl SymbolTable {
    /// Find the reference at a given byte offset, if any.
    pub fn find_reference_at(&self, offset: usize) -> Option<&ReferenceInfo> {
        // Binary search for a reference whose span contains the offset.
        let idx = self
            .references
            .binary_search_by(|r| {
                if offset < r.span.offset() {
                    std::cmp::Ordering::Greater
                } else if offset >= r.span.offset() + r.span.len() {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .ok()?;
        Some(&self.references[idx])
    }

    /// Find the definition whose name span contains the given byte offset, if any.
    ///
    /// O(log n) via [`SymbolTable::defs_by_name_span`], which is populated by
    /// [`SymbolTable::finalize`]. If `finalize` has not been called, falls back
    /// to a linear scan so callers that forget to finalize still see correct
    /// results.
    pub fn find_definition_at(&self, offset: usize) -> Option<&DefinitionInfo> {
        if self.defs_by_name_span.is_empty() {
            return self.definitions.values().find(|d| {
                offset >= d.name_span.offset() && offset < d.name_span.offset() + d.name_span.len()
            });
        }
        let idx = self
            .defs_by_name_span
            .binary_search_by(|(span, _)| {
                if offset < span.offset() {
                    std::cmp::Ordering::Greater
                } else if offset >= span.offset() + span.len() {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .ok()?;
        let key = &self.defs_by_name_span[idx].1;
        self.definitions.get(key.as_ref())
    }

    /// Build the sorted `defs_by_name_span` index and precompute inlay-hint
    /// candidate positions. Called once at the end of `build_from_ast`.
    ///
    /// `source` is used to turn byte offsets into LSP `Position`s ahead of
    /// time, so the inlay-hint request path avoids O(source) scans per
    /// declaration.
    fn finalize(&mut self, source: &str) {
        // Build sorted-by-name-span and inlay-hint indices in one pass,
        // sharing each key via `Arc` so a definition's `SymbolKey` is
        // allocated once and refcounted across the two sorted views.
        // `Arc` (not `Rc`) because `AnalysisResult` lives in an
        // `Arc<RwLock<…>>` and must be `Send + Sync` for the LSP runtime.
        let lines = LineIndex::new(source);
        let mut span_index: Vec<(Span, Arc<SymbolKey>)> = Vec::new();
        let mut hint_entries: Vec<InlayHintEntry> = Vec::new();
        for (key, def) in &self.definitions {
            if def.name_span.is_empty() {
                continue;
            }
            let shared = Arc::new(key.clone());
            span_index.push((def.name_span, Arc::clone(&shared)));
            if matches!(
                def.category,
                SymbolCategory::Param | SymbolCategory::Node | SymbolCategory::Const
            ) {
                let start = def.name_span.offset();
                let end = start + def.name_span.len();
                hint_entries.push(InlayHintEntry {
                    key: shared,
                    name_start: lines.position(start),
                    name_end: lines.position(end),
                });
            }
        }
        span_index.sort_by_key(|(span, _)| span.offset());
        self.defs_by_name_span = span_index;
        self.inlay_hint_entries = hint_entries;
        self.inlay_hint_entries
            .sort_by_key(|e| (e.name_start.line, e.name_start.character));
    }

    /// Returns the precomputed inlay-hint candidates.
    ///
    /// Only `Param`/`Node`/`Const` declarations with non-empty name spans
    /// are included, sorted by name-start position. The inlay-hint request
    /// handler iterates this directly instead of filtering every definition.
    pub fn inlay_hint_entries(&self) -> &[InlayHintEntry] {
        &self.inlay_hint_entries
    }

    /// Look up the key for a definition by its name-span byte offset.
    ///
    /// Returns `None` if the definition has an empty name-span (builtins,
    /// fields) or if the definition did not come from this table's
    /// `insert_definition` path.
    ///
    /// O(1) via the `name_span_to_key` secondary index — no linear scan needed.
    pub fn find_definition_key(&self, definition: &DefinitionInfo) -> Option<SymbolKey> {
        self.name_span_to_key
            .get(&definition.name_span.offset())
            .cloned()
    }

    /// Insert a definition and update the secondary `name_span_to_key` index.
    fn insert_definition(&mut self, key: SymbolKey, definition: DefinitionInfo) {
        // Only index non-empty spans (builtins and fields have empty spans).
        if !definition.name_span.is_empty() {
            self.name_span_to_key
                .insert(definition.name_span.offset(), key.clone());
        }
        self.definitions.insert(key, definition);
    }

    /// Register a top-level definition from a named declaration.
    ///
    /// This is a convenience helper that handles the common pattern of:
    /// 1. Converting the name to a `String`
    /// 2. Creating a `SymbolKey::TopLevel`
    /// 3. Inserting a `DefinitionInfo` with the given category and spans
    #[expect(
        clippy::too_many_arguments,
        reason = "declaration fields line up with DefinitionInfo — splitting would just shuffle them across a struct"
    )]
    fn register_top_level(
        &mut self,
        name: impl AsRef<str>,
        name_span: Span,
        decl_span: Span,
        category: SymbolCategory,
        type_description: Option<String>,
        detail: Option<String>,
        visibility: BindableVisibility,
    ) {
        let name = name.as_ref().to_string();
        self.insert_definition(
            SymbolKey::TopLevel(name.clone()),
            DefinitionInfo {
                name,
                category,
                name_span,
                decl_span,
                type_description,
                detail,
                visibility: Some(visibility),
            },
        );
    }

    /// Find all references that point to the given target key.
    pub fn find_all_references(&self, target: &SymbolKey) -> Vec<&ReferenceInfo> {
        self.references
            .iter()
            .filter(|r| &r.target == target)
            .collect()
    }
}

/// Scope stack for tracking local variable bindings.
struct ScopeStack {
    /// Each scope maps local name -> definition key in the symbol table.
    scopes: Vec<HashMap<String, SymbolKey>>,
}

impl ScopeStack {
    fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
        }
    }

    fn push(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop(&mut self) {
        debug_assert!(
            self.scopes.len() > 1,
            "ScopeStack::pop called with only the root scope remaining"
        );
        self.scopes.pop();
    }

    fn insert(&mut self, name: String, key: SymbolKey) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, key);
        }
    }

    fn resolve(&self, name: &str) -> Option<&SymbolKey> {
        for scope in self.scopes.iter().rev() {
            if let Some(key) = scope.get(name) {
                return Some(key);
            }
        }
        None
    }
}

/// Build a symbol table from a parsed AST file.
///
/// `source` is the text the `ast` was parsed from — it is used to precompute
/// LSP `Position`s for inlay-hint candidates so the request path avoids
/// O(source) scans.
pub fn build_from_ast(
    ast: &graphcal_compiler::desugar::resolved_ast::File,
    source: &str,
) -> SymbolTable {
    let mut table = SymbolTable::default();
    let mut scopes = ScopeStack::new();

    register_builtins(&mut table);

    for decl in &ast.declarations {
        collect_attribute_refs(&decl.attributes, &mut table);

        match &decl.kind {
            DeclKind::Param(p) => collect_param_decl(
                p,
                decl.span,
                BindableVisibility::PublicBind,
                &mut table,
                &mut scopes,
            ),
            DeclKind::Node(n) => {
                collect_node_decl(n, decl.span, n.visibility.into(), &mut table, &mut scopes);
            }
            DeclKind::ConstNode(c) => {
                collect_const_node_decl(c, decl.span, c.visibility.into(), &mut table, &mut scopes);
            }
            DeclKind::BaseDimension(d) => {
                collect_base_dim_decl(d, decl.span, d.visibility.into(), &mut table);
            }
            DeclKind::Dimension(d) => collect_dim_decl(d, decl.span, d.visibility, &mut table),
            DeclKind::Unit(u) => {
                collect_unit_decl(u, decl.span, u.visibility.into(), &mut table, &mut scopes);
            }
            DeclKind::Type(t) => collect_type_decl(t, decl.span, t.visibility, &mut table),
            DeclKind::Index(idx) => collect_index_decl(idx, decl.span, idx.visibility, &mut table),
            DeclKind::Assert(a) => {
                collect_assert_decl(a, decl.span, a.visibility.into(), &mut table, &mut scopes);
            }
            DeclKind::Plot(p) => {
                collect_plot_decl(p, decl.span, p.visibility.into(), &mut table, &mut scopes);
            }
            DeclKind::Figure(f) => {
                collect_figure_decl(f, decl.span, f.visibility.into(), &mut table, &mut scopes);
            }
            DeclKind::Layer(l) => {
                collect_layer_decl(l, decl.span, l.visibility.into(), &mut table, &mut scopes);
            }
            DeclKind::Import(u) => collect_import_decl(u, &mut table),
            DeclKind::Include(u) => collect_include_decl(u, &mut table),
            DeclKind::Dag(d) => collect_dag_decl(d, decl.span, d.visibility.into(), &mut table),
            // `Sugar(_)` carries `Infallible` for the `Desugared` phase, so
            // this arm is statically unreachable. The deref of `&Infallible`
            // is sound (no value can be observed) — this is the canonical
            // proof of unreachability for sealed sugar variants.
            #[expect(
                clippy::uninhabited_references,
                reason = "Sugar(Infallible) — proof of unreachability"
            )]
            DeclKind::Sugar(s) => match *s {},
        }
    }

    // Sort references by offset for binary search.
    table.references.sort_by_key(|r| r.span.offset());
    // Build the sorted `defs_by_name_span` index for O(log n) lookups and
    // precompute inlay-hint candidate positions.
    table.finalize(source);
    table
}

fn register_builtins(table: &mut SymbolTable) {
    for name in builtin_constants().keys() {
        table.insert_definition(
            SymbolKey::TopLevel((*name).to_string()),
            DefinitionInfo {
                name: (*name).to_string(),
                category: SymbolCategory::BuiltinConst,
                name_span: Span::new(0, 0),
                decl_span: Span::new(0, 0),
                type_description: Some("Dimensionless".to_string()),
                detail: None,
                visibility: None,
            },
        );
    }

    for (name, f) in builtin_functions() {
        table.insert_definition(
            SymbolKey::TopLevel((*name).to_string()),
            DefinitionInfo {
                name: (*name).to_string(),
                category: SymbolCategory::BuiltinFn,
                name_span: Span::new(0, 0),
                decl_span: Span::new(0, 0),
                type_description: None,
                detail: Some(format!("builtin, arity {}", f.arity())),
                visibility: None,
            },
        );
    }
}

fn collect_attribute_refs(
    attributes: &[graphcal_compiler::desugar::resolved_ast::Attribute],
    table: &mut SymbolTable,
) {
    for attr in attributes {
        if attr.name.name.parse::<AttributeName>() == Ok(AttributeName::Assumes) {
            for arg in &attr.args {
                match arg {
                    AttributeArg::Path { segments, .. } if segments.len() == 1 => {
                        let ident = segments.first();
                        table.references.push(ReferenceInfo {
                            span: ident.span,
                            target: SymbolKey::TopLevel(ident.name.to_string()),
                        });
                    }
                    AttributeArg::Path { .. } | AttributeArg::Group { .. } => {}
                }
            }
        }
    }
}

fn collect_param_decl(
    p: &ParamDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
) {
    table.register_top_level(
        &p.name.value,
        p.name.span,
        decl_span,
        SymbolCategory::Param,
        None,
        None,
        visibility,
    );
    collect_type_expr_refs(&p.type_ann, table);
    if let Some(ref value) = p.value {
        collect_expr_refs(value, table, scopes);
    }
}

fn collect_node_decl(
    n: &NodeDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
) {
    table.register_top_level(
        &n.name.value,
        n.name.span,
        decl_span,
        SymbolCategory::Node,
        None,
        None,
        visibility,
    );
    collect_type_expr_refs(&n.type_ann, table);
    collect_expr_refs(&n.value, table, scopes);
}

fn collect_const_node_decl(
    c: &graphcal_compiler::desugar::resolved_ast::ConstNodeDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
) {
    table.register_top_level(
        &c.name.value,
        c.name.span,
        decl_span,
        SymbolCategory::Const,
        None,
        None,
        visibility,
    );
    collect_type_expr_refs(&c.type_ann, table);
    collect_expr_refs(&c.value, table, scopes);
}

fn collect_base_dim_decl(
    d: &BaseDimDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
) {
    table.register_top_level(
        &d.name.value,
        d.name.span,
        decl_span,
        SymbolCategory::Dimension,
        None,
        None,
        visibility,
    );
}

fn collect_dim_decl(
    d: &DimDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
) {
    table.register_top_level(
        &d.name.value,
        d.name.span,
        decl_span,
        SymbolCategory::Dimension,
        None,
        None,
        visibility,
    );
    if let Some(definition) = &d.definition {
        collect_dim_expr_refs(definition, table);
    }
}

fn collect_unit_decl(
    u: &UnitDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
) {
    table.register_top_level(
        &u.name.value,
        u.name.span,
        decl_span,
        SymbolCategory::Unit,
        None,
        None,
        visibility,
    );
    collect_dim_expr_refs(&u.dim_type, table);
    if let Some(unit_def) = &u.definition {
        collect_expr_refs(&unit_def.scale_expr, table, scopes);
        collect_unit_expr_refs(&unit_def.unit_expr, table);
    }
}

fn collect_type_decl(
    t: &TypeDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
) {
    table.register_top_level(
        &t.name.value,
        t.name.span,
        decl_span,
        SymbolCategory::StructType,
        None,
        None,
        visibility,
    );
    // Register constructor and payload-field definitions, then walk payload
    // type annotations (required types have no fields). Constructors are a
    // separate namespace from type names, so they must not reuse the type's
    // `TopLevel` key.
    if let TypeDeclBody::Constructors(members) = &t.body {
        for member in members {
            let constructor_name = member.name.value.to_string();
            table.insert_definition(
                SymbolKey::Constructor(SymbolPath::local(constructor_name.clone())),
                DefinitionInfo {
                    name: constructor_name.clone(),
                    category: SymbolCategory::Constructor,
                    name_span: member.name.span,
                    decl_span: member.span,
                    type_description: Some(format!("constructor of {}", t.name.value)),
                    detail: None,
                    visibility: Some(t.visibility),
                },
            );
            if let Some(fields) = &member.payload {
                for field in fields {
                    table.insert_definition(
                        SymbolKey::Field {
                            owner: SymbolPath::local(constructor_name.clone()),
                            field_name: field.name.value.to_string(),
                        },
                        DefinitionInfo {
                            name: field.name.value.to_string(),
                            category: SymbolCategory::Field,
                            name_span: field.name.span,
                            decl_span: field.name.span,
                            type_description: None,
                            detail: Some(format!("field of {constructor_name}")),
                            visibility: None,
                        },
                    );
                    collect_type_expr_refs(&field.type_ann, table);
                }
            }
        }
    }
}

fn collect_index_decl(
    idx: &IndexDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
) {
    let name = idx.name.value.to_string();
    table.register_top_level(
        &name,
        idx.name.span,
        decl_span,
        SymbolCategory::Index,
        None,
        None,
        visibility,
    );
    match &idx.kind {
        IndexDeclKind::Named { variants } => {
            for variant in variants {
                let vname = variant.value.to_string();
                let key = SymbolKey::Variant {
                    parent: SymbolPath::local(name.clone()),
                    variant: vname.clone(),
                };
                table.insert_definition(
                    key,
                    DefinitionInfo {
                        name: vname,
                        category: SymbolCategory::IndexVariant,
                        name_span: variant.span,
                        decl_span: variant.span,
                        type_description: None,
                        detail: Some(format!("label/value variant of index {name}")),
                        visibility: None,
                    },
                );
            }
        }
        IndexDeclKind::RequiredRange { dimension } => {
            collect_dim_expr_refs(dimension, table);
        }
        IndexDeclKind::Range { .. } | IndexDeclKind::RequiredNamed => {}
    }
}

fn collect_assert_decl(
    a: &AssertDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
) {
    table.register_top_level(
        &a.name.value,
        a.name.span,
        decl_span,
        SymbolCategory::Assert,
        Some("Bool".to_string()),
        Some("assert".to_string()),
        visibility,
    );
    match &a.body {
        graphcal_compiler::desugar::resolved_ast::AssertBody::Expr(expr) => {
            collect_expr_refs(expr, table, scopes);
        }
        graphcal_compiler::desugar::resolved_ast::AssertBody::Tolerance {
            actual,
            expected,
            tolerance,
            ..
        } => {
            collect_expr_refs(actual, table, scopes);
            collect_expr_refs(expected, table, scopes);
            collect_expr_refs(tolerance, table, scopes);
        }
    }
}

fn collect_plot_decl(
    p: &PlotDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
) {
    table.register_top_level(
        &p.name.value,
        p.name.span,
        decl_span,
        SymbolCategory::Plot,
        Some(format!("plot (mark: {})", p.mark.mark_type)),
        Some("plot".to_string()),
        visibility,
    );
    for encoding in &p.encodings {
        collect_expr_refs(&encoding.value, table, scopes);
    }
    for prop in &p.mark.properties {
        collect_expr_refs(&prop.value, table, scopes);
    }
    for prop in &p.properties {
        collect_expr_refs(&prop.value, table, scopes);
    }
}

fn collect_figure_decl(
    f: &FigureDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
) {
    table.register_top_level(
        &f.name.value,
        f.name.span,
        decl_span,
        SymbolCategory::Figure,
        Some("figure".to_string()),
        Some("figure".to_string()),
        visibility,
    );
    for field in &f.fields {
        collect_expr_refs(&field.value, table, scopes);
    }
}

fn collect_layer_decl(
    l: &LayerDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
) {
    table.register_top_level(
        &l.name.value,
        l.name.span,
        decl_span,
        SymbolCategory::Layer,
        Some("layer".to_string()),
        Some("layer".to_string()),
        visibility,
    );
    for field in &l.fields {
        collect_expr_refs(&field.value, table, scopes);
    }
}

fn collect_dag_decl(
    d: &DagDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
) {
    table.register_top_level(
        d.name.value.as_str(),
        d.name.span,
        decl_span,
        SymbolCategory::Dag,
        None,
        None,
        visibility,
    );
    // Register the dag's body params and nodes as `Qualified` members so that
    // inline-call projections (`@dag(...)::out`) and future goto-def on dag
    // params can resolve to their definitions inside the dag body.
    let dag_name = d.name.value.to_string();
    for body_decl in &d.body {
        let (member_name, member_span, category) = match &body_decl.kind {
            graphcal_compiler::desugar::resolved_ast::DeclKind::Param(p) => {
                (p.name.value.to_string(), p.name.span, SymbolCategory::Param)
            }
            graphcal_compiler::desugar::resolved_ast::DeclKind::Node(n) => {
                (n.name.value.to_string(), n.name.span, SymbolCategory::Node)
            }
            graphcal_compiler::desugar::resolved_ast::DeclKind::ConstNode(c) => {
                (c.name.value.to_string(), c.name.span, SymbolCategory::Const)
            }
            _ => continue,
        };
        table.insert_definition(
            SymbolKey::Qualified {
                module: vec![dag_name.clone()],
                name: member_name.clone(),
            },
            DefinitionInfo {
                name: member_name,
                category,
                name_span: member_span,
                decl_span: body_decl.span,
                type_description: None,
                detail: None,
                visibility: Some(match &body_decl.kind {
                    graphcal_compiler::desugar::resolved_ast::DeclKind::Node(n) => {
                        n.visibility.into()
                    }
                    graphcal_compiler::desugar::resolved_ast::DeclKind::ConstNode(c) => {
                        c.visibility.into()
                    }
                    _ => BindableVisibility::Private,
                }),
            },
        );
    }
}

fn collect_import_decl(u: &ImportDecl, table: &mut SymbolTable) {
    // Each imported name is a reference; target resolution for cross-file
    // go-to-definition is handled separately.
    collect_import_or_include_names(&u.kind, table);
}

fn collect_include_decl(
    u: &graphcal_compiler::desugar::resolved_ast::IncludeDecl,
    table: &mut SymbolTable,
) {
    collect_import_or_include_names(&u.kind, table);
}

fn collect_import_or_include_names(
    kind: &graphcal_compiler::desugar::resolved_ast::ImportKind,
    table: &mut SymbolTable,
) {
    if let graphcal_compiler::desugar::resolved_ast::ImportKind::Selective(names) = kind {
        for import_item in names {
            table.references.push(ReferenceInfo {
                span: import_item.name.span,
                target: SymbolKey::TopLevel(import_item.name.name.to_string()),
            });
            // If aliased, the alias also resolves to the same target.
            if let Some(alias) = &import_item.alias {
                table.references.push(ReferenceInfo {
                    span: alias.span,
                    target: SymbolKey::TopLevel(import_item.name.name.to_string()),
                });
            }
        }
    }
}

/// Register an expression-scoped local variable: build the `SymbolKey`,
/// insert a `LocalVar` definition into the table, and bind the name in the
/// current scope. Used by `Scan` and `Unfold`, which both bind two locals
/// the same way and only differ in `kind` and the cosmetic `detail` text.
fn register_local_var(
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
    kind: ExprScopeKind,
    offset: usize,
    name: &graphcal_compiler::syntax::span::Spanned<graphcal_compiler::syntax::names::LocalName>,
    detail: &str,
) {
    let key = SymbolKey::ExprScoped {
        kind,
        offset,
        local: name.value.as_str().to_owned(),
    };
    table.insert_definition(
        key.clone(),
        DefinitionInfo {
            name: name.value.as_str().to_owned(),
            category: SymbolCategory::LocalVar,
            name_span: name.span,
            decl_span: name.span,
            type_description: None,
            detail: Some(detail.to_string()),
            visibility: None,
        },
    );
    scopes.insert(name.value.as_str().to_owned(), key);
}

/// Collect references from an expression, tracking local scopes.
fn collect_expr_refs(
    expr: &graphcal_compiler::desugar::resolved_ast::Expr,
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
) {
    // Recursion choke point: recurses once per tree level (unbounded for
    // left-nested operator chains).
    graphcal_compiler::stack::with_stack_growth(|| collect_expr_refs_inner(expr, table, scopes));
}

#[expect(
    clippy::too_many_lines,
    reason = "expression walker needs to handle every ExprKind variant"
)]
fn collect_expr_refs_inner(
    expr: &graphcal_compiler::desugar::resolved_ast::Expr,
    table: &mut SymbolTable,
    scopes: &mut ScopeStack,
) {
    match &expr.kind {
        ExprKind::GraphRef(name) | ExprKind::ConstRef(name) => {
            let target = if name.value.is_qualified() {
                SymbolKey::Qualified {
                    module: name
                        .value
                        .qualifier()
                        .iter()
                        .map(ToString::to_string)
                        .collect(),
                    name: name.value.member().to_string(),
                }
            } else {
                SymbolKey::TopLevel(name.value.member().to_string())
            };
            table.references.push(ReferenceInfo {
                span: name.span,
                target,
            });
        }
        ExprKind::InlineDagRef { path, args, output } => {
            // Same-file calls (`@dag(args).out`) use a single-segment path —
            // the leaf names a `TopLevel` DAG declaration in the active
            // file's table. Cross-file qualified calls (`@module.dag(args).out`)
            // have multi-segment paths where the prefix segments are module
            // aliases; the leaf names the DAG under `Qualified { module: <prefix>, name: <leaf> }`
            // in `imported_definitions` (see `collect_imported_definitions`).
            let leaf = path.leaf();
            let leaf_target = if path.segments.len() > 1 {
                SymbolKey::Qualified {
                    module: path.segments.as_slice()[..path.segments.len() - 1]
                        .iter()
                        .map(|s| s.name.to_string())
                        .collect(),
                    name: leaf.name.to_string(),
                }
            } else {
                SymbolKey::TopLevel(leaf.name.to_string())
            };
            table.references.push(ReferenceInfo {
                span: leaf.span,
                target: leaf_target,
            });
            // The projected output resolves to `<dag>.output` as a qualified
            // member. For same-file calls the qualifier is the bare DAG name
            // (matches `collect_dag_decl`'s body-member entries). For cross-file
            // calls the qualifier is `<module-alias>.<dag>`, which matches the
            // re-keyed body member in `imported_definitions`.
            table.references.push(ReferenceInfo {
                span: output.span,
                target: SymbolKey::Qualified {
                    module: path.segments.iter().map(|s| s.name.to_string()).collect(),
                    name: output.value.to_string(),
                },
            });
            for binding in args {
                collect_expr_refs(&binding.value, table, scopes);
            }
        }
        ExprKind::FnCall { callee, args, .. } => {
            table.references.push(ReferenceInfo {
                span: callee.span(),
                target: symbol_key_for_path(callee),
            });
            for arg in args {
                collect_expr_refs(arg, table, scopes);
            }
        }
        ExprKind::LocalRef(ident) => {
            let target = scopes
                .resolve(&ident.name)
                .cloned()
                .unwrap_or_else(|| SymbolKey::TopLevel(ident.name.to_string()));
            table.references.push(ReferenceInfo {
                span: ident.span,
                target,
            });
        }
        ExprKind::TypeSystemRef(name) => {
            table.references.push(ReferenceInfo {
                span: name.span,
                target: SymbolKey::TopLevel(name.value.as_str().to_string()),
            });
        }
        ExprKind::BinOp { lhs, rhs, .. } => {
            collect_expr_refs(lhs, table, scopes);
            collect_expr_refs(rhs, table, scopes);
        }
        ExprKind::UnaryOp { operand, .. } => {
            collect_expr_refs(operand, table, scopes);
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_expr_refs(condition, table, scopes);
            collect_expr_refs(then_branch, table, scopes);
            collect_expr_refs(else_branch, table, scopes);
        }
        ExprKind::UnitLiteral { unit, .. } => {
            collect_unit_expr_refs(unit, table);
        }
        ExprKind::Convert { expr, target } => {
            collect_expr_refs(expr, table, scopes);
            collect_unit_expr_refs(target, table);
        }
        ExprKind::DisplayTimezone { expr, .. } => {
            collect_expr_refs(expr, table, scopes);
        }
        ExprKind::FieldAccess { expr, field: _ } => {
            collect_expr_refs(expr, table, scopes);
            // Field references on bare `expr.field` would need TIR-level type
            // info to know which struct the field belongs to. We deliberately
            // record nothing here rather than emit an `unresolved`-field key
            // that would resolve to the first struct registering a field with
            // the same name. Pattern bindings (which carry variant context)
            // and the registry-driven definition pass below still record
            // precisely-keyed `Field { owner, field_name }` entries.
        }
        ExprKind::ConstructorCall {
            callee,
            generic_args,
            fields,
        } => {
            let constructor_path = SymbolPath::from_ident_path(callee);
            table.references.push(ReferenceInfo {
                span: callee.span(),
                target: SymbolKey::Constructor(constructor_path.clone()),
            });
            for generic_arg in generic_args {
                if let graphcal_compiler::desugar::resolved_ast::GenericArg::Type(type_arg) =
                    generic_arg
                {
                    collect_type_expr_refs(type_arg, table);
                }
            }
            for field in fields {
                table.references.push(ReferenceInfo {
                    span: field.name.span,
                    target: SymbolKey::Field {
                        owner: constructor_path.clone(),
                        field_name: field.name.value.to_string(),
                    },
                });
                collect_expr_refs(&field.value, table, scopes);
            }
        }
        ExprKind::MapLiteral { entries } => {
            // Note: the `table[I, J]` bracket-prefix index references are lost
            // because the LSP consumes `File<Desugared>` (TableLiteral has
            // been desugared to MapLiteral). Entry-level `Index.Variant` keys
            // below still produce references. To restore bracket-prefix
            // references, the LSP would have to consume the raw AST or carry
            // a side-channel of preserved metadata.
            for entry in entries {
                for key in &entry.keys {
                    if let graphcal_compiler::syntax::ast::MapEntryIndex::Named(index_path) =
                        &key.index.value
                    {
                        table.references.push(ReferenceInfo {
                            span: key.index.span,
                            target: symbol_key_for_name_path(index_path),
                        });
                        table.references.push(ReferenceInfo {
                            span: key.variant.span,
                            target: variant_key_for_parts(index_path, &key.variant.value),
                        });
                    }
                }
                collect_expr_refs(&entry.value, table, scopes);
            }
        }
        ExprKind::ForComp { bindings, body } => {
            scopes.push();
            for binding in bindings {
                let (detail, ref_info) = match &binding.index {
                    graphcal_compiler::desugar::resolved_ast::ForBindingIndex::Named(spanned) => {
                        let detail = format!("loop variable over {}", spanned.value);
                        let ref_info = Some(ReferenceInfo {
                            span: spanned.span,
                            target: symbol_key_for_name_path(&spanned.value),
                        });
                        (detail, ref_info)
                    }
                    graphcal_compiler::desugar::resolved_ast::ForBindingIndex::Range {
                        arg,
                        ..
                    } => {
                        let detail = format!("loop variable over range({arg})");
                        (detail, None)
                    }
                };
                if let Some(ri) = ref_info {
                    table.references.push(ri);
                }
                let var_name = binding.var.value.as_str().to_owned();
                let key = SymbolKey::ExprScoped {
                    kind: ExprScopeKind::For,
                    offset: binding.var.span.offset(),
                    local: var_name.clone(),
                };
                table.insert_definition(
                    key.clone(),
                    DefinitionInfo {
                        name: var_name.clone(),
                        category: SymbolCategory::LocalVar,
                        name_span: binding.var.span,
                        decl_span: binding.var.span,
                        type_description: None,
                        detail: Some(detail),
                        visibility: None,
                    },
                );
                scopes.insert(var_name, key);
            }
            collect_expr_refs(body, table, scopes);
            scopes.pop();
        }
        ExprKind::IndexAccess { expr, args } => {
            collect_expr_refs(expr, table, scopes);
            for arg in args {
                match arg {
                    graphcal_compiler::desugar::resolved_ast::IndexArg::Variant {
                        index,
                        variant,
                    } => {
                        table.references.push(ReferenceInfo {
                            span: index.span,
                            target: symbol_key_for_name_path(&index.value),
                        });
                        table.references.push(ReferenceInfo {
                            span: variant.span,
                            target: variant_key_for_parts(&index.value, &variant.value),
                        });
                    }
                    graphcal_compiler::desugar::resolved_ast::IndexArg::Var(ident) => {
                        let target = scopes
                            .resolve(&ident.name)
                            .cloned()
                            .unwrap_or_else(|| SymbolKey::TopLevel(ident.name.to_string()));
                        table.references.push(ReferenceInfo {
                            span: ident.span,
                            target,
                        });
                    }
                    graphcal_compiler::desugar::resolved_ast::IndexArg::Expr(e) => {
                        collect_expr_refs(e, table, scopes);
                    }
                }
            }
        }
        ExprKind::Scan {
            source,
            init,
            acc_name,
            val_name,
            body,
        } => {
            collect_expr_refs(source, table, scopes);
            collect_expr_refs(init, table, scopes);
            scopes.push();
            let scan_offset = expr.span.offset();
            register_local_var(
                table,
                scopes,
                ExprScopeKind::Scan,
                scan_offset,
                acc_name,
                "scan accumulator",
            );
            register_local_var(
                table,
                scopes,
                ExprScopeKind::Scan,
                scan_offset,
                val_name,
                "scan value",
            );
            collect_expr_refs(body, table, scopes);
            scopes.pop();
        }
        ExprKind::Unfold {
            init,
            prev_name,
            curr_name,
            body,
        } => {
            collect_expr_refs(init, table, scopes);
            scopes.push();
            let unfold_offset = expr.span.offset();
            register_local_var(
                table,
                scopes,
                ExprScopeKind::Unfold,
                unfold_offset,
                prev_name,
                "unfold previous step",
            );
            register_local_var(
                table,
                scopes,
                ExprScopeKind::Unfold,
                unfold_offset,
                curr_name,
                "unfold current step",
            );
            collect_expr_refs(body, table, scopes);
            scopes.pop();
        }
        ExprKind::VariantLiteral { index, variant } => {
            // Reference to the index name
            table.references.push(ReferenceInfo {
                span: index.span,
                target: symbol_key_for_name_path(&index.value),
            });
            // Reference to the qualified variant: Index.Variant
            table.references.push(ReferenceInfo {
                span: variant.span,
                target: variant_key_for_parts(&index.value, &variant.value),
            });
        }
        ExprKind::Match { scrutinee, arms } => {
            collect_expr_refs(scrutinee, table, scopes);
            for arm in arms {
                let (variant_name, bindings) = match &arm.pattern {
                    MatchPattern::IndexLabel { index, variant, .. } => {
                        table.references.push(ReferenceInfo {
                            span: index.span,
                            target: symbol_key_for_name_path(&index.value),
                        });
                        table.references.push(ReferenceInfo {
                            span: variant.span,
                            target: variant_key_for_parts(&index.value, &variant.value),
                        });
                        (SymbolPath::local(variant.value.to_string()), &[][..])
                    }
                    MatchPattern::Constructor { name, bindings, .. } => {
                        let constructor_path = SymbolPath::local(name.value.to_string());
                        table.references.push(ReferenceInfo {
                            span: name.span,
                            target: SymbolKey::Constructor(constructor_path.clone()),
                        });
                        (constructor_path, bindings.as_slice())
                    }
                    MatchPattern::Path { path, bindings, .. } => {
                        let pattern_path = SymbolPath::from_ident_path(path);
                        if path.len() >= 3 {
                            let segments = path.segments();
                            if let Some(index_path) =
                                name_path_from_ident_segments(&segments[..segments.len() - 1])
                            {
                                let variant =
                                    graphcal_compiler::syntax::names::IndexVariantName::from_atom(
                                        path.leaf().name.clone(),
                                    );
                                table.references.push(ReferenceInfo {
                                    span: path.span(),
                                    target: variant_key_for_parts(&index_path, &variant),
                                });
                            }
                        } else {
                            table.references.push(ReferenceInfo {
                                span: path.span(),
                                target: SymbolKey::Constructor(pattern_path.clone()),
                            });
                        }
                        (pattern_path, bindings.as_slice())
                    }
                };

                scopes.push();
                for binding in bindings {
                    match binding {
                        PatternBinding::Bind { field, var } => {
                            table.references.push(ReferenceInfo {
                                span: field.span,
                                target: SymbolKey::Field {
                                    owner: variant_name.clone(),
                                    field_name: field.value.to_string(),
                                },
                            });
                            let var_key = SymbolKey::ExprScoped {
                                kind: ExprScopeKind::Match,
                                offset: arm.span.offset(),
                                local: var.name.to_string(),
                            };
                            table.insert_definition(
                                var_key.clone(),
                                DefinitionInfo {
                                    name: var.name.to_string(),
                                    category: SymbolCategory::LocalVar,
                                    name_span: var.span,
                                    decl_span: var.span,
                                    type_description: None,
                                    detail: Some(format!("bound from {variant_name}")),
                                    visibility: None,
                                },
                            );
                            scopes.insert(var.name.to_string(), var_key);
                        }
                        PatternBinding::Wildcard { field, .. } => {
                            table.references.push(ReferenceInfo {
                                span: field.span,
                                target: SymbolKey::Field {
                                    owner: variant_name.clone(),
                                    field_name: field.value.to_string(),
                                },
                            });
                        }
                    }
                }
                collect_expr_refs(&arm.body, table, scopes);
                scopes.pop();
            }
        }
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::StringLiteral(_) => {}
        // `Sugar` and `UnresolvedRef` payloads are `Infallible` in `Resolved`
        // — both arms are statically unreachable.
        #[expect(
            clippy::uninhabited_references,
            reason = "Sugar/UnresolvedRef(Infallible) — proof of unreachability"
        )]
        ExprKind::Sugar(s) | ExprKind::UnresolvedRef(s) => match *s {},
    }
}

/// Collect references from a type expression.
fn collect_type_expr_refs(
    type_expr: &graphcal_compiler::desugar::resolved_ast::TypeExpr,
    table: &mut SymbolTable,
) {
    match &type_expr.kind {
        TypeExprKind::Dimensionless
        | TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime => {}
        TypeExprKind::DimExpr(dim_expr) => {
            collect_dim_expr_refs(dim_expr, table);
        }
        TypeExprKind::Indexed { base, indexes } => {
            collect_type_expr_refs(base, table);
            for idx in indexes {
                match idx {
                    graphcal_compiler::desugar::resolved_ast::IndexExpr::Name(path) => {
                        table.references.push(ReferenceInfo {
                            span: path.span,
                            target: symbol_key_for_name_path(&path.value),
                        });
                    }
                    graphcal_compiler::desugar::resolved_ast::IndexExpr::NatExpr(_) => {
                        // No reference to resolve for nat expressions
                    }
                }
            }
        }
        TypeExprKind::TypeApplication { name, type_args } => {
            table.references.push(ReferenceInfo {
                span: name.span,
                target: symbol_key_for_name_path(&name.value),
            });
            for arg in type_args {
                collect_type_expr_refs(arg, table);
            }
        }
        TypeExprKind::DatetimeApplication { type_args } => {
            // `Datetime` is a built-in; no top-level reference to record.
            // Recurse into args so any user-defined names reachable from the
            // time-scale expression are still picked up by go-to-definition.
            for arg in type_args {
                collect_type_expr_refs(arg, table);
            }
        }
    }
    // Collect references from domain constraint bound expressions (e.g., unit names in `100 kg`).
    for bound in &type_expr.constraints {
        collect_constraint_expr_refs(&bound.value, table);
    }
}

/// Collect references from a constraint bound expression (limited walk for unit names).
fn collect_constraint_expr_refs(
    expr: &graphcal_compiler::desugar::resolved_ast::Expr,
    table: &mut SymbolTable,
) {
    match &expr.kind {
        ExprKind::UnitLiteral { unit, .. } => {
            collect_unit_expr_refs(unit, table);
        }
        ExprKind::UnaryOp { operand, .. } => {
            collect_constraint_expr_refs(operand, table);
        }
        _ => {}
    }
}

/// Collect references from a dimension expression.
fn collect_dim_expr_refs(dim_expr: &DimExpr, table: &mut SymbolTable) {
    for item in &dim_expr.terms {
        table.references.push(ReferenceInfo {
            span: item.term.span,
            target: symbol_key_for_name_path(&item.term.name.value),
        });
    }
}

/// Collect references from a unit expression.
fn collect_unit_expr_refs(unit_expr: &UnitExpr, table: &mut SymbolTable) {
    for item in &unit_expr.terms {
        table.references.push(ReferenceInfo {
            span: item.name.span,
            target: SymbolKey::TopLevel(item.name.value.to_string()),
        });
    }
}

/// Format a domain bound expression as a human-readable string.
///
/// Handles the common cases: number literals, unit-annotated literals, and negated forms.
fn format_bound_expr(expr: &graphcal_compiler::desugar::resolved_ast::Expr) -> String {
    match &expr.kind {
        ExprKind::Number(v) => format_number(*v),
        ExprKind::Integer(v) => v.to_string(),
        ExprKind::UnitLiteral { value, unit } => {
            let num = format_number(*value);
            let unit_str = format_unit_expr_with_config(unit, true);
            format!("{num} {unit_str}")
        }
        ExprKind::UnaryOp {
            op: graphcal_compiler::desugar::resolved_ast::UnaryOp::Neg,
            operand,
        } => {
            format!("-{}", format_bound_expr(operand))
        }
        // For complex expressions, fall back to "..."
        _ => "...".to_string(),
    }
}

/// Format a constraint clause from domain bounds.
///
/// Returns a string like `(min: 100 kg, max: 2000 kg)` or an empty string if no constraints.
fn format_constraints(constraints: &[DomainBound]) -> String {
    if constraints.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = constraints
        .iter()
        .map(|b| format!("{}: {}", b.kind, format_bound_expr(&b.value)))
        .collect();
    format!("({})", parts.join(", "))
}

/// Format a resolved type expression with domain constraints.
///
/// For indexed types like `Velocity[Maneuver]`, inserts the constraint clause
/// between the base type and the index suffix: `Velocity(min: 0 m/s)[Maneuver]`.
fn format_type_with_constraints(
    resolved: &ResolvedTypeExpr,
    constraints: &[DomainBound],
    registry: &Registry,
) -> String {
    let constraint_str = format_constraints(constraints);
    if let ResolvedTypeExpr::Indexed { base, indexes } = resolved {
        let base_str = base.format(registry);
        let idx_strs: Vec<String> = indexes
            .iter()
            .map(|i| match i {
                ResolvedIndex::Concrete(name, _) => name.as_str().to_string(),
                ResolvedIndex::GenericParam(name, _) => name.to_string(),
                ResolvedIndex::NatExpr(form, _) => form.format(),
            })
            .collect();
        format!("{base_str}{constraint_str}[{}]", idx_strs.join(", "))
    } else {
        let type_str = resolved.format(registry);
        format!("{type_str}{constraint_str}")
    }
}

/// Extract domain constraints from a `TypeExpr`, looking through `Indexed` wrappers.
fn extract_constraints(type_expr: &TypeExpr) -> &[DomainBound] {
    if !type_expr.constraints.is_empty() {
        return &type_expr.constraints;
    }
    // For indexed types, the constraints are on the base type
    if let TypeExprKind::Indexed { base, .. } = &type_expr.kind {
        return &base.constraints;
    }
    &[]
}

/// Enrich a symbol table with type information from a TIR.
#[expect(
    clippy::too_many_lines,
    reason = "linear match over all symbol categories"
)]
pub fn enrich_from_tir(table: &mut SymbolTable, tir: &TIR) {
    let registry = &tir.registry;
    let root = tir.root();

    // Build a map from declaration name to its AST TypeExpr constraints.
    let mut decl_constraints: HashMap<String, &[DomainBound]> = HashMap::new();
    for e in &root.params {
        let constraints = extract_constraints(&e.type_ann);
        if !constraints.is_empty() {
            decl_constraints.insert(e.name.to_string(), constraints);
        }
    }
    for e in &root.nodes {
        let constraints = extract_constraints(&e.type_ann);
        if !constraints.is_empty() {
            decl_constraints.insert(e.name.to_string(), constraints);
        }
    }
    for e in &root.consts {
        let constraints = extract_constraints(&e.type_ann);
        if !constraints.is_empty() {
            decl_constraints.insert(e.name.to_string(), constraints);
        }
    }

    // Enrich param/node/const declarations with resolved types + constraints.
    for (name, resolved_type) in &root.resolved_decl_types {
        let name_str = name.to_string();
        let key = SymbolKey::TopLevel(name_str.clone());
        if let Some(def) = table.definitions.get_mut(&key) {
            let type_desc = decl_constraints.get(&name_str).map_or_else(
                || resolved_type.format(registry),
                |constraints| format_type_with_constraints(resolved_type, constraints, registry),
            );
            def.type_description = Some(type_desc);
        }
    }

    // Enrich dimension/unit/index/struct definitions from registry.
    // Collect keys and categories first to avoid cloning the entire HashMap.
    let definition_keys: Vec<(SymbolKey, SymbolCategory)> = table
        .definitions
        .iter()
        .map(|(k, d)| (k.clone(), d.category))
        .collect();
    for (key, category) in &definition_keys {
        let Some(name) = key.top_level_name() else {
            continue;
        };
        match category {
            SymbolCategory::Dimension => {
                if let Some(dim) = registry.dimensions.get_dimension(name)
                    && let Some(def_mut) = table.definitions.get_mut(key)
                {
                    def_mut.type_description = Some(format!(
                        "dim {name} = {}",
                        registry.dimensions.format_dimension(dim)
                    ));
                }
            }
            SymbolCategory::Unit => {
                if let Some(unit_info) = registry.units.get_unit(name)
                    && let Some(def_mut) = table.definitions.get_mut(key)
                {
                    let scale_str = match &unit_info.scale {
                        UnitScale::Static(s) => format!("{s}"),
                        UnitScale::Dynamic { .. } => "dynamic".to_string(),
                    };
                    def_mut.type_description = Some(format!(
                        "{}, scale = {scale_str}",
                        registry.dimensions.format_dimension(&unit_info.dimension),
                    ));
                }
            }
            SymbolCategory::Index => {
                if let Some(idx_def) = registry.indexes.get_index(name)
                    && let Some(def_mut) = table.definitions.get_mut(key)
                {
                    match &idx_def.kind {
                        IndexKind::Named { variants } => {
                            let vs: Vec<&str> = variants
                                .iter()
                                .map(graphcal_compiler::syntax::names::IndexVariantName::as_str)
                                .collect();
                            def_mut.type_description = Some(format!("{{ {} }}", vs.join(", ")));
                        }
                        IndexKind::Range(data) => {
                            def_mut.type_description = Some(format!(
                                "range({}, {}, step: {})",
                                data.start, data.end, data.step
                            ));
                        }
                        IndexKind::RequiredNamed => {
                            def_mut.type_description = Some("(required)".to_string());
                        }
                        IndexKind::RequiredRange { dimension } => {
                            def_mut.type_description = Some(format!(
                                "(required, dim: {})",
                                registry.dimensions.format_dimension(dimension)
                            ));
                        }
                        IndexKind::NatRange { size } => {
                            def_mut.type_description = Some(format!("range({size})"));
                        }
                    }
                }
            }
            SymbolCategory::StructType => {
                if let Some(type_def) = registry.types.get_type(name)
                    && let Some(def_mut) = table.definitions.get_mut(key)
                {
                    let desc = type_def.union_members().map_or_else(
                        || {
                            // Record or unit type: show fields
                            let fields = type_def.fields();
                            if fields.is_empty() {
                                type_def.name.to_string()
                            } else {
                                let field_descs: Vec<String> =
                                    fields.iter().map(|f| f.name.to_string()).collect();
                                format!("{} {{ {} }}", type_def.name, field_descs.join(", "))
                            }
                        },
                        |members| {
                            // Union type: show members separated by |
                            let member_descs: Vec<String> =
                                members.iter().map(|m| m.name.to_string()).collect();
                            member_descs.join(" | ")
                        },
                    );
                    def_mut.type_description = Some(desc);
                }
            }
            _ => {}
        }
    }

    // Register field definitions from struct types under their struct-qualified
    // keys so that pattern bindings (`@s match { Variant(field: v) => …}`)
    // resolve to the field of the right struct/variant, even when two structs
    // share a field name.
    for type_def in registry.types.all_types() {
        for field in type_def.fields() {
            let field_key = SymbolKey::Field {
                owner: SymbolPath::local(type_def.name.to_string()),
                field_name: field.name.to_string(),
            };
            if !table.definitions.contains_key(&field_key) {
                table.insert_definition(
                    field_key,
                    DefinitionInfo {
                        name: field.name.to_string(),
                        category: SymbolCategory::Field,
                        name_span: Span::new(0, 0),
                        decl_span: Span::new(0, 0),
                        type_description: None,
                        detail: Some(format!("field of {}", type_def.name)),
                        visibility: None,
                    },
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_symbol_table_basic() {
        let source = "param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x + 1.0;";
        let raw_file = graphcal_compiler::syntax::parser::Parser::with_name(source, "test.gcl")
            .parse_file()
            .unwrap();
        let desugared = graphcal_compiler::syntax::desugar::desugar_multi_decls_in_file(raw_file);
        let file = graphcal_compiler::syntax::name_resolve::resolve_name_refs(desugared);
        let table = build_from_ast(&file, source);

        let x_key = SymbolKey::TopLevel("x".to_string());
        let y_key = SymbolKey::TopLevel("y".to_string());
        assert!(table.definitions.contains_key(&x_key));
        assert!(table.definitions.contains_key(&y_key));
        assert_eq!(table.definitions[&x_key].category, SymbolCategory::Param);
        assert_eq!(table.definitions[&y_key].category, SymbolCategory::Node);

        // @x is a reference
        assert!(
            table.references.iter().any(|r| r.target == x_key),
            "expected @x reference"
        );
    }

    #[test]
    fn multi_decl_slots_each_produce_separate_symbols() {
        // Multi-decl (issue #481) desugars to N single declarations. The
        // symbol table should register each slot with its own name span
        // so goto-def / rename / hover land on the slot header.
        let source = "\
index I = { A, B };

param p: Int[I],
param q: Int[I]
  = table[I, (_, _)] {
      : _, _;
      A: 1, 3;
      B: 2, 4;
  };
";
        let raw_file = graphcal_compiler::syntax::parser::Parser::with_name(source, "test.gcl")
            .parse_file()
            .unwrap();
        let desugared = graphcal_compiler::syntax::desugar::desugar_multi_decls_in_file(raw_file);
        let file = graphcal_compiler::syntax::name_resolve::resolve_name_refs(desugared);
        let table = build_from_ast(&file, source);

        let p_key = SymbolKey::TopLevel("p".to_string());
        let q_key = SymbolKey::TopLevel("q".to_string());
        assert!(
            table.definitions.contains_key(&p_key),
            "expected slot `p` in symbol table",
        );
        assert!(
            table.definitions.contains_key(&q_key),
            "expected slot `q` in symbol table",
        );
        assert_eq!(table.definitions[&p_key].category, SymbolCategory::Param);
        assert_eq!(table.definitions[&q_key].category, SymbolCategory::Param);

        // The slot name spans should cover the slot header's identifier in the
        // multi-decl surface, not the (synthesized) value expression.
        let p_def = &table.definitions[&p_key];
        let p_slice =
            &source[p_def.name_span.offset()..p_def.name_span.offset() + p_def.name_span.len()];
        assert_eq!(p_slice, "p");
        let q_def = &table.definitions[&q_key];
        let q_slice =
            &source[q_def.name_span.offset()..q_def.name_span.offset() + q_def.name_span.len()];
        assert_eq!(q_slice, "q");
    }

    #[test]
    fn find_reference_at_offset() {
        let source = "param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x;";
        let raw_file = graphcal_compiler::syntax::parser::Parser::with_name(source, "test.gcl")
            .parse_file()
            .unwrap();
        let desugared = graphcal_compiler::syntax::desugar::desugar_multi_decls_in_file(raw_file);
        let file = graphcal_compiler::syntax::name_resolve::resolve_name_refs(desugared);
        let table = build_from_ast(&file, source);

        // Find the @x reference -- it should be near the end of the source
        let at_x_offset = source.find("@x").unwrap() + 1; // offset of 'x' in '@x'
        let reference = table.find_reference_at(at_x_offset);
        assert!(reference.is_some(), "expected to find reference at @x");
        assert_eq!(
            reference.unwrap().target,
            SymbolKey::TopLevel("x".to_string())
        );
    }
    #[test]
    fn symbol_key_helpers() {
        assert_eq!(
            SymbolKey::TopLevel("x".to_string()).top_level_name(),
            Some("x")
        );
        assert_eq!(
            SymbolKey::Field {
                owner: SymbolPath::local("Rocket"),
                field_name: "x".to_string(),
            }
            .top_level_name(),
            None
        );
    }
    // --- Inline DAG invocation LSP coverage (issue #451) ---
}
