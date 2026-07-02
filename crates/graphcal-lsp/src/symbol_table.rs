//! Symbol table for LSP features: maps source locations to definitions and references.

use std::collections::HashMap;
use std::sync::Arc;

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::desugar::desugared_ast::{
    AssertDecl, AttributeArg, BaseDimDecl, BindableVisibility, DagDecl, DeclKind, DimDecl, DimExpr,
    DomainBound, ExprKind, FigureDecl, ImportDecl, IndexDecl, IndexDeclKind, LayerDecl, NodeDecl,
    ParamDecl, PlotDecl, TypeDecl, TypeDeclBody, TypeExpr, TypeExprKind, UnitDecl, UnitExpr,
};
use graphcal_compiler::hir;
use graphcal_compiler::syntax::attribute::AttributeName;
use graphcal_compiler::syntax::module_resolve::{ModuleResolveError, ModuleResolver};
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

/// Build a symbol table for a standalone buffer with no project context.
///
/// Constructs a file-local resolver (including inline dag child modules);
/// references into unloaded imports surface through the tolerant-lowering
/// fallback, keyed by their written spelling.
pub fn build_for_buffer(
    ast: &graphcal_compiler::desugar::desugared_ast::File,
    source: &str,
) -> SymbolTable {
    fn add_modules(
        resolver: &mut ModuleResolver,
        owner: &DagId,
        declarations: &[graphcal_compiler::desugar::desugared_ast::Declaration],
    ) {
        // A duplicate-symbol failure leaves the module unregistered; the
        // walk then records its references via the spelling fallback.
        let _ = resolver.add_module(owner.clone(), declarations);
        for decl in declarations {
            if let DeclKind::Dag(dag) = &decl.kind {
                add_modules(resolver, &owner.child(dag.name.value.as_str()), &dag.body);
            }
        }
    }

    let dag_id = DagId::root_in_package("test", "buffer");
    let mut resolver = ModuleResolver::default();
    add_modules(&mut resolver, &dag_id, &ast.declarations);
    build_from_ast(ast, source, &dag_id, &resolver)
}

/// Collects expression references and lexical-local definitions from
/// tolerantly-lowered HIR bodies.
///
/// This is the HIR side of the symbol table: declaration shells are walked
/// syntactically (they carry the definition spans), while every expression
/// body is lowered through the compiler's single resolution stage and its
/// references are keyed from the canonical identities HIR carries, mapped
/// back to the file's spelling domain (module aliases, dag-body qualifiers)
/// for the editor-facing symbol keys. A body that fails to resolve still
/// contributes references for the failing names via the lowering
/// diagnostics, so IDE features keep working on incomplete code.
struct HirRefCollector<'a> {
    dag_id: &'a DagId,
    resolver: &'a ModuleResolver,
    /// Canonical module owner → the alias this file imports it under.
    alias_of: HashMap<DagId, String>,
    /// Lexical local definitions of the current body, keyed by HIR identity.
    locals: HashMap<hir::LocalId, SymbolKey>,
}

impl<'a> HirRefCollector<'a> {
    fn new(dag_id: &'a DagId, resolver: &'a ModuleResolver) -> Self {
        let mut alias_of: HashMap<DagId, String> = HashMap::new();
        if let Some(scope) = resolver.scope(dag_id) {
            for (alias, target) in scope.module_aliases() {
                let alias = alias.to_string();
                // One module can be imported under several names (bare +
                // `as` alias). HIR references carry only the canonical
                // identity, so pick the smallest alias deterministically
                // for the spelling-domain key; the imported-definition map
                // registers every alias, so lookups succeed either way.
                alias_of
                    .entry(target.target().clone())
                    .and_modify(|existing| {
                        if alias < *existing {
                            existing.clone_from(&alias);
                        }
                    })
                    .or_insert(alias);
            }
        }
        Self {
            dag_id,
            resolver,
            alias_of,
            locals: HashMap::new(),
        }
    }

    /// Spelling-domain qualifier for a canonical owner: empty for this file,
    /// the dag name for the file's own dag bodies, the import alias (plus
    /// dag name) for imported modules. `None` when the owner reaches this
    /// file without a module qualifier (selective imports).
    fn module_segments(&self, owner: &DagId) -> Option<Vec<String>> {
        if owner == self.dag_id {
            return Some(Vec::new());
        }
        if owner.parent().as_ref() == Some(self.dag_id) {
            return Some(vec![owner.name().to_string()]);
        }
        if let Some(alias) = self.alias_of.get(owner) {
            return Some(vec![alias.clone()]);
        }
        if let Some(parent) = owner.parent()
            && let Some(alias) = self.alias_of.get(&parent)
        {
            return Some(vec![alias.clone(), owner.name().to_string()]);
        }
        None
    }

    fn name_key(&self, owner: &DagId, leaf: &str) -> SymbolKey {
        match self.module_segments(owner) {
            Some(module) if module.is_empty() => SymbolKey::TopLevel(leaf.to_string()),
            Some(module) => SymbolKey::Qualified {
                module,
                name: leaf.to_string(),
            },
            None => SymbolKey::TopLevel(leaf.to_string()),
        }
    }

    fn path_for(&self, owner: &DagId, leaf: &str) -> SymbolPath {
        match self.module_segments(owner) {
            Some(module) if module.is_empty() => SymbolPath::local(leaf.to_string()),
            Some(module) => SymbolPath::qualified(module, leaf.to_string()),
            None => SymbolPath::local(leaf.to_string()),
        }
    }

    fn variant_key(
        &self,
        variant: &graphcal_compiler::syntax::index_name::ResolvedIndexVariant,
    ) -> SymbolKey {
        SymbolKey::Variant {
            parent: self.path_for(variant.index().owner(), variant.index().as_str()),
            variant: variant.variant().as_str().to_string(),
        }
    }

    /// Lower one declaration body and record its references.
    fn collect_body(
        &mut self,
        expr: &graphcal_compiler::desugar::desugared_ast::Expr,
        table: &mut SymbolTable,
    ) {
        let generic_scope = hir::GenericScope::new();
        let prelude = hir::PreludeTypeScope::graphcal();
        let ctx = hir::ExprLoweringContext::new(self.dag_id, self.resolver, &generic_scope)
            .with_prelude(&prelude);
        let (lowered, diagnostics) = hir::lower_expr_tolerant(expr, ctx);
        self.locals.clear();
        self.walk(&lowered, table);
        for diagnostic in &diagnostics {
            Self::record_unresolved(diagnostic, table);
        }
    }

    /// Record a reference for an unresolved name from a lowering diagnostic,
    /// keyed by its written spelling so IDE features answer on broken code.
    fn record_unresolved(err: &hir::ExprLowerError, table: &mut SymbolTable) {
        use graphcal_compiler::syntax::names::NameAtom;
        let (target, span) = match err {
            hir::ExprLowerError::UnknownGraphRef { name, span } => {
                let target = if name.is_qualified() {
                    SymbolKey::Qualified {
                        module: name.qualifier().iter().map(ToString::to_string).collect(),
                        name: name.member().to_string(),
                    }
                } else {
                    SymbolKey::TopLevel(name.member().to_string())
                };
                (target, *span)
            }
            hir::ExprLowerError::UnknownLocalRef { name, span } => {
                (SymbolKey::TopLevel(name.to_string()), *span)
            }
            hir::ExprLowerError::UnknownFunction { path, span }
                if NameAtom::parse(path).is_ok() =>
            {
                (SymbolKey::TopLevel(path.clone()), *span)
            }
            hir::ExprLowerError::ModuleResolve {
                source: ModuleResolveError::UnknownName { name, .. },
                span,
            } if NameAtom::parse(name).is_ok() => (SymbolKey::TopLevel(name.clone()), *span),
            _ => return,
        };
        table.references.push(ReferenceInfo { span, target });
    }

    fn define_local(
        &mut self,
        local: &hir::LocalDef,
        kind: ExprScopeKind,
        offset: usize,
        detail: String,
        table: &mut SymbolTable,
    ) {
        let key = SymbolKey::ExprScoped {
            kind,
            offset,
            local: local.name.to_string(),
        };
        table.insert_definition(
            key.clone(),
            DefinitionInfo {
                name: local.name.to_string(),
                category: SymbolCategory::LocalVar,
                name_span: local.span,
                decl_span: local.span,
                type_description: None,
                detail: Some(detail),
                visibility: None,
            },
        );
        self.locals.insert(local.id, key);
    }

    fn reference(table: &mut SymbolTable, span: Span, target: SymbolKey) {
        table.references.push(ReferenceInfo { span, target });
    }

    /// Record references for an index-variant reference: the variant segment
    /// targets the variant symbol, and the index segment (when one is written
    /// — the `Maneuver` in `Maneuver.Departure`, or the axis token inside
    /// `table[...]` for desugared table rows) targets the index declaration.
    /// Keeping the two segments separate makes rename edits and goto-def
    /// segment-precise instead of splicing whole qualified paths.
    fn variant_reference(&self, variant: &hir::expr::IndexVariantRef, table: &mut SymbolTable) {
        let target = self.variant_key(&variant.variant);
        Self::reference(table, variant.variant_span, target);
        if let Some(index_span) = variant.index_span {
            let index = variant.variant.index();
            let key = self.name_key(index.owner(), index.as_str());
            Self::reference(table, index_span, key);
        }
    }

    fn walk(&mut self, expr: &hir::Expr, table: &mut SymbolTable) {
        graphcal_compiler::stack::with_stack_growth(|| self.walk_inner(expr, table));
    }

    #[expect(
        clippy::too_many_lines,
        reason = "reference extraction handles every HIR ExprKind variant"
    )]
    fn walk_inner(&mut self, expr: &hir::Expr, table: &mut SymbolTable) {
        match &expr.kind {
            hir::ExprKind::Error
            | hir::ExprKind::Number(_)
            | hir::ExprKind::Integer(_)
            | hir::ExprKind::Bool(_)
            | hir::ExprKind::StringLiteral(_) => {}
            hir::ExprKind::GraphRef(target) => {
                let key = self.name_key(target.value.owner(), target.value.as_str());
                Self::reference(table, target.span, key);
            }
            hir::ExprKind::ConstRef(const_ref) => {
                let target = match &const_ref.value {
                    hir::ConstRef::Decl(name) => self.name_key(name.owner(), name.as_str()),
                    hir::ConstRef::Constructor(name) => {
                        SymbolKey::Constructor(self.path_for(name.owner(), name.as_str()))
                    }
                    hir::ConstRef::Builtin(builtin) => {
                        SymbolKey::TopLevel(builtin.as_str().to_string())
                    }
                    hir::ConstRef::TimeScale(scale) => SymbolKey::TopLevel(scale.to_string()),
                    hir::ConstRef::GenericNatParam(_) => return,
                };
                Self::reference(table, const_ref.span, target);
            }
            hir::ExprKind::LocalRef(local) => {
                if let Some(key) = self.locals.get(&local.value) {
                    Self::reference(table, local.span, key.clone());
                }
            }
            hir::ExprKind::TypeSystemRef(type_ref) => {
                let target = match &type_ref.value {
                    hir::expr::TypeSystemRef::Type(name) => {
                        self.name_key(name.owner(), name.as_str())
                    }
                    hir::expr::TypeSystemRef::Dimension(name) => {
                        self.name_key(name.owner(), name.as_str())
                    }
                    hir::expr::TypeSystemRef::Index(name) => {
                        self.name_key(name.owner(), name.as_str())
                    }
                    hir::expr::TypeSystemRef::IndexVariant(variant) => self.variant_key(variant),
                };
                Self::reference(table, type_ref.span, target);
            }
            hir::ExprKind::VariantLiteral(variant) => {
                self.variant_reference(variant, table);
            }
            hir::ExprKind::BinOp { lhs, rhs, .. } => {
                self.walk(lhs, table);
                self.walk(rhs, table);
            }
            hir::ExprKind::UnaryOp { operand, .. }
            | hir::ExprKind::DisplayTimezone { expr: operand, .. }
            | hir::ExprKind::FieldAccess { expr: operand, .. } => {
                // Bare-field references would need TIR-level type info to
                // know the owning struct; pattern bindings still record
                // precisely-keyed field references.
                self.walk(operand, table);
            }
            hir::ExprKind::FnCall {
                callee,
                type_args,
                args,
            } => {
                let hir::FunctionRef::Builtin(builtin) = callee.value;
                Self::reference(
                    table,
                    callee.span,
                    SymbolKey::TopLevel(builtin.as_str().to_string()),
                );
                for type_arg in type_args {
                    if let hir::expr::GenericArg::Type(type_expr) = type_arg {
                        self.walk_type(type_expr, table);
                    }
                }
                for arg in args {
                    self.walk(arg, table);
                }
            }
            hir::ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.walk(condition, table);
                self.walk(then_branch, table);
                self.walk(else_branch, table);
            }
            hir::ExprKind::UnitLiteral { unit, .. } => collect_unit_expr_refs(unit, table),
            hir::ExprKind::Convert {
                expr: inner,
                target,
            } => {
                self.walk(inner, table);
                collect_unit_expr_refs(target, table);
            }
            hir::ExprKind::ConstructorCall {
                callee,
                generic_args,
                fields,
            } => {
                let constructor_path = self.path_for(callee.value.owner(), callee.value.as_str());
                Self::reference(
                    table,
                    callee.span,
                    SymbolKey::Constructor(constructor_path.clone()),
                );
                for generic_arg in generic_args {
                    if let hir::expr::GenericArg::Type(type_expr) = generic_arg {
                        self.walk_type(type_expr, table);
                    }
                }
                for field in fields {
                    Self::reference(
                        table,
                        field.name.span,
                        SymbolKey::Field {
                            owner: constructor_path.clone(),
                            field_name: field.name.value.to_string(),
                        },
                    );
                    self.walk(&field.value, table);
                }
            }
            hir::ExprKind::MapLiteral { entries } => {
                for entry in entries {
                    for key in &entry.keys {
                        if let hir::expr::MapEntryKey::IndexVariant(variant) = key {
                            self.variant_reference(variant, table);
                        }
                    }
                    self.walk(&entry.value, table);
                }
            }
            hir::ExprKind::ForComp { bindings, body } => {
                for binding in bindings {
                    let detail = match &binding.index {
                        hir::expr::ForBindingIndex::Named(index) => {
                            let key = self.name_key(index.value.owner(), index.value.as_str());
                            Self::reference(table, index.span, key);
                            format!("loop variable over {}", index.value.as_str())
                        }
                        hir::expr::ForBindingIndex::Range { arg, .. } => {
                            format!("loop variable over range({})", nat_expr_label(arg))
                        }
                    };
                    self.define_local(
                        &binding.local,
                        ExprScopeKind::For,
                        binding.local.span.offset(),
                        detail,
                        table,
                    );
                }
                self.walk(body, table);
            }
            hir::ExprKind::IndexAccess { expr: inner, args } => {
                self.walk(inner, table);
                for arg in args {
                    match arg {
                        hir::expr::IndexArg::Variant(variant) => {
                            self.variant_reference(variant, table);
                        }
                        hir::expr::IndexArg::Var(local) => {
                            if let Some(key) = self.locals.get(&local.value) {
                                Self::reference(table, local.span, key.clone());
                            }
                        }
                        hir::expr::IndexArg::Expr(arg_expr) => self.walk(arg_expr, table),
                    }
                }
            }
            hir::ExprKind::Scan {
                source,
                init,
                acc,
                val,
                body,
            } => {
                self.walk(source, table);
                self.walk(init, table);
                let offset = expr.span.offset();
                self.define_local(
                    acc,
                    ExprScopeKind::Scan,
                    offset,
                    "scan accumulator".into(),
                    table,
                );
                self.define_local(val, ExprScopeKind::Scan, offset, "scan value".into(), table);
                self.walk(body, table);
            }
            hir::ExprKind::Unfold {
                init,
                prev,
                curr,
                body,
            } => {
                self.walk(init, table);
                let offset = expr.span.offset();
                self.define_local(
                    prev,
                    ExprScopeKind::Unfold,
                    offset,
                    "unfold previous step".into(),
                    table,
                );
                self.define_local(
                    curr,
                    ExprScopeKind::Unfold,
                    offset,
                    "unfold current step".into(),
                    table,
                );
                self.walk(body, table);
            }
            hir::ExprKind::Match { scrutinee, arms } => {
                self.walk(scrutinee, table);
                for arm in arms {
                    match &arm.pattern {
                        hir::expr::MatchPattern::Constructor {
                            constructor,
                            bindings,
                            ..
                        } => {
                            let constructor_path = self
                                .path_for(constructor.value.owner(), constructor.value.as_str());
                            Self::reference(
                                table,
                                constructor.span,
                                SymbolKey::Constructor(constructor_path.clone()),
                            );
                            for binding in bindings {
                                match binding {
                                    hir::expr::PatternBinding::Bind { field, local } => {
                                        Self::reference(
                                            table,
                                            field.span,
                                            SymbolKey::Field {
                                                owner: constructor_path.clone(),
                                                field_name: field.value.to_string(),
                                            },
                                        );
                                        self.define_local(
                                            local,
                                            ExprScopeKind::Match,
                                            arm.span.offset(),
                                            format!("bound from {constructor_path}"),
                                            table,
                                        );
                                    }
                                    hir::expr::PatternBinding::Wildcard { field, .. } => {
                                        Self::reference(
                                            table,
                                            field.span,
                                            SymbolKey::Field {
                                                owner: constructor_path.clone(),
                                                field_name: field.value.to_string(),
                                            },
                                        );
                                    }
                                }
                            }
                        }
                        hir::expr::MatchPattern::IndexLabel { variant, .. } => {
                            self.variant_reference(variant, table);
                        }
                    }
                    self.walk(&arm.body, table);
                }
            }
            hir::ExprKind::InlineDagRef {
                target,
                args,
                output,
            } => {
                let dag_key = target.value.parent().map_or_else(
                    || SymbolKey::TopLevel(target.value.name().to_string()),
                    |parent| self.name_key(&parent, target.value.name()),
                );
                Self::reference(table, target.span, dag_key);
                Self::reference(
                    table,
                    output.span,
                    self.name_key(output.value.owner(), output.value.as_str()),
                );
                for arg in args {
                    self.walk(&arg.value, table);
                }
            }
        }
    }

    fn walk_type(&self, type_expr: &hir::TypeExpr, table: &mut SymbolTable) {
        match &type_expr.kind {
            hir::TypeExprKind::Builtin(_) | hir::TypeExprKind::GenericTypeParam(_) => {}
            hir::TypeExprKind::DimExpr(dim_expr) => {
                for item in &dim_expr.terms {
                    if let hir::DimTermTarget::Dimension(name) = &item.term.target {
                        let key = self.name_key(name.value.owner(), name.value.as_str());
                        Self::reference(table, name.span, key);
                    }
                }
            }
            hir::TypeExprKind::Index(index) => {
                if let hir::IndexRef::Concrete(name) = index {
                    let key = self.name_key(name.value.owner(), name.value.as_str());
                    Self::reference(table, name.span, key);
                }
            }
            hir::TypeExprKind::Struct(name) => {
                let key = self.name_key(name.value.owner(), name.value.as_str());
                Self::reference(table, name.span, key);
            }
            hir::TypeExprKind::Indexed { base, indexes } => {
                self.walk_type(base, table);
                for index in indexes {
                    if let hir::IndexRef::Concrete(name) = index {
                        let key = self.name_key(name.value.owner(), name.value.as_str());
                        Self::reference(table, name.span, key);
                    }
                }
            }
            hir::TypeExprKind::TypeApplication { name, type_args } => {
                let key = self.name_key(name.value.owner(), name.value.as_str());
                Self::reference(table, name.span, key);
                for arg in type_args {
                    self.walk_type(arg, table);
                }
            }
        }
    }
}

/// Short display label for a HIR type-level natural-number expression,
/// used in local-variable hover details.
fn nat_expr_label(nat_expr: &hir::NatExpr) -> String {
    match nat_expr {
        hir::NatExpr::Literal(value, _) => value.to_string(),
        hir::NatExpr::Param(param) => param.value.name.to_string(),
        hir::NatExpr::Add(..) | hir::NatExpr::Mul(..) => "..".to_string(),
    }
}

fn symbol_key_for_name_path(path: &NamePath) -> SymbolKey {
    match SymbolPath::from_name_path(path) {
        SymbolPath::Local(name) => SymbolKey::TopLevel(name),
        SymbolPath::Qualified { module, name } => SymbolKey::Qualified { module, name },
    }
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

    /// The name of the innermost `dag` declaration whose body contains
    /// `offset`, if any. Used by completion to respect lexical scope: inside
    /// a dag body, top-level declarations are not referenceable.
    pub fn enclosing_dag_at(&self, offset: usize) -> Option<&str> {
        self.definitions
            .values()
            .filter(|def| {
                def.category == SymbolCategory::Dag
                    && offset >= def.decl_span.offset()
                    && offset < def.decl_span.offset() + def.decl_span.len()
            })
            .min_by_key(|def| def.decl_span.len())
            .map(|def| def.name.as_str())
    }
}

/// Build a symbol table from a parsed AST file.
///
/// `source` is the text the `ast` was parsed from — it is used to precompute
/// LSP `Position`s for inlay-hint candidates so the request path avoids
/// O(source) scans.
pub fn build_from_ast(
    ast: &graphcal_compiler::desugar::desugared_ast::File,
    source: &str,
    dag_id: &DagId,
    resolver: &ModuleResolver,
) -> SymbolTable {
    let mut table = SymbolTable::default();
    let mut refs = HirRefCollector::new(dag_id, resolver);

    register_builtins(&mut table);

    for decl in &ast.declarations {
        collect_attribute_refs(&decl.attributes, &mut table);

        match &decl.kind {
            DeclKind::Param(p) => collect_param_decl(
                p,
                decl.span,
                BindableVisibility::PublicBind,
                &mut table,
                &mut refs,
            ),
            DeclKind::Node(n) => {
                collect_node_decl(n, decl.span, n.visibility.into(), &mut table, &mut refs);
            }
            DeclKind::ConstNode(c) => {
                collect_const_node_decl(c, decl.span, c.visibility.into(), &mut table, &mut refs);
            }
            DeclKind::BaseDimension(d) => {
                collect_base_dim_decl(d, decl.span, d.visibility.into(), &mut table);
            }
            DeclKind::Dimension(d) => collect_dim_decl(d, decl.span, d.visibility, &mut table),
            DeclKind::Unit(u) => {
                collect_unit_decl(u, decl.span, u.visibility.into(), &mut table, &mut refs);
            }
            DeclKind::Type(t) => collect_type_decl(t, decl.span, t.visibility, &mut table),
            DeclKind::Index(idx) => collect_index_decl(idx, decl.span, idx.visibility, &mut table),
            DeclKind::Assert(a) => {
                collect_assert_decl(a, decl.span, a.visibility.into(), &mut table, &mut refs);
            }
            DeclKind::Plot(p) => {
                collect_plot_decl(p, decl.span, p.visibility.into(), &mut table, &mut refs);
            }
            DeclKind::Figure(f) => {
                collect_figure_decl(f, decl.span, f.visibility.into(), &mut table, &mut refs);
            }
            DeclKind::Layer(l) => {
                collect_layer_decl(l, decl.span, l.visibility.into(), &mut table, &mut refs);
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

    // Sort references by offset for binary search, and drop duplicates:
    // every row of a desugared `table[Axis] { ... }` records an index
    // reference at the same axis-token span, but each occurrence should
    // appear once in find-references/rename results.
    table
        .references
        .sort_by_key(|r| (r.span.offset(), r.span.len()));
    let mut seen = std::collections::HashSet::new();
    table
        .references
        .retain(|r| seen.insert((r.span.offset(), r.span.len(), r.target.clone())));
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
    attributes: &[graphcal_compiler::desugar::desugared_ast::Attribute],
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
                    AttributeArg::Path { .. }
                    | AttributeArg::RangeStep { .. }
                    | AttributeArg::Group { .. } => {}
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
    refs: &mut HirRefCollector<'_>,
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
        refs.collect_body(value, table);
    }
}

fn collect_node_decl(
    n: &NodeDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    refs: &mut HirRefCollector<'_>,
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
    refs.collect_body(&n.value, table);
}

fn collect_const_node_decl(
    c: &graphcal_compiler::desugar::desugared_ast::ConstNodeDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    refs: &mut HirRefCollector<'_>,
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
    refs.collect_body(&c.value, table);
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
    refs: &mut HirRefCollector<'_>,
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
        refs.collect_body(&unit_def.scale_expr, table);
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
    refs: &mut HirRefCollector<'_>,
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
        graphcal_compiler::desugar::desugared_ast::AssertBody::Expr(expr) => {
            refs.collect_body(expr, table);
        }
        graphcal_compiler::desugar::desugared_ast::AssertBody::Tolerance {
            actual,
            expected,
            tolerance,
            ..
        } => {
            refs.collect_body(actual, table);
            refs.collect_body(expected, table);
            refs.collect_body(tolerance, table);
        }
    }
}

fn collect_plot_decl(
    p: &PlotDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    refs: &mut HirRefCollector<'_>,
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
        refs.collect_body(&encoding.value, table);
    }
    for prop in &p.mark.properties {
        refs.collect_body(&prop.value, table);
    }
    for prop in &p.properties {
        refs.collect_body(&prop.value, table);
    }
}

fn collect_figure_decl(
    f: &FigureDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    refs: &mut HirRefCollector<'_>,
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
        refs.collect_body(&field.value, table);
    }
}

fn collect_layer_decl(
    l: &LayerDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    refs: &mut HirRefCollector<'_>,
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
        refs.collect_body(&field.value, table);
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
            graphcal_compiler::desugar::desugared_ast::DeclKind::Param(p) => {
                (p.name.value.to_string(), p.name.span, SymbolCategory::Param)
            }
            graphcal_compiler::desugar::desugared_ast::DeclKind::Node(n) => {
                (n.name.value.to_string(), n.name.span, SymbolCategory::Node)
            }
            graphcal_compiler::desugar::desugared_ast::DeclKind::ConstNode(c) => {
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
                    graphcal_compiler::desugar::desugared_ast::DeclKind::Node(n) => {
                        n.visibility.into()
                    }
                    graphcal_compiler::desugar::desugared_ast::DeclKind::ConstNode(c) => {
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
    u: &graphcal_compiler::desugar::desugared_ast::IncludeDecl,
    table: &mut SymbolTable,
) {
    collect_import_or_include_names(&u.kind, table);
}

fn collect_import_or_include_names(
    kind: &graphcal_compiler::desugar::desugared_ast::ImportKind,
    table: &mut SymbolTable,
) {
    if let graphcal_compiler::desugar::desugared_ast::ImportKind::Selective(names) = kind {
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
/// Collect references from a type expression.
fn collect_type_expr_refs(
    type_expr: &graphcal_compiler::desugar::desugared_ast::TypeExpr,
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
                    graphcal_compiler::desugar::desugared_ast::IndexExpr::Name(path) => {
                        table.references.push(ReferenceInfo {
                            span: path.span,
                            target: symbol_key_for_name_path(&path.value),
                        });
                    }
                    graphcal_compiler::desugar::desugared_ast::IndexExpr::NatExpr(_) => {
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
    expr: &graphcal_compiler::desugar::desugared_ast::Expr,
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
fn format_bound_expr(expr: &graphcal_compiler::desugar::desugared_ast::Expr) -> String {
    match &expr.kind {
        ExprKind::Number(v) => format_number(*v),
        ExprKind::Integer(v) => v.to_string(),
        ExprKind::UnitLiteral { value, unit } => {
            let num = format_number(*value);
            let unit_str = format_unit_expr_with_config(unit, true);
            format!("{num} {unit_str}")
        }
        ExprKind::UnaryOp {
            op: graphcal_compiler::desugar::desugared_ast::UnaryOp::Neg,
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
///
/// `dag_id` selects which DAG's resolved declaration types to use — the
/// analyzed file's own id for the root table, the dependency's id for an
/// imported file's table. Using the root unconditionally left every
/// imported declaration without a type (`(unknown type)` hover, #831):
/// the type information lives on the dep's merged [`DagTIR`], not the
/// root's.
#[expect(
    clippy::too_many_lines,
    reason = "linear match over all symbol categories"
)]
pub fn enrich_from_tir(table: &mut SymbolTable, tir: &TIR, dag_id: &DagId) {
    let registry = &tir.registry;

    if let Some(dag) = tir.dags.get(dag_id) {
        // Build a map from declaration name to its AST TypeExpr constraints.
        let mut decl_constraints: HashMap<String, &[DomainBound]> = HashMap::new();
        for e in &dag.params {
            let constraints = extract_constraints(&e.type_ann);
            if !constraints.is_empty() {
                decl_constraints.insert(e.name.to_string(), constraints);
            }
        }
        for e in &dag.nodes {
            let constraints = extract_constraints(&e.type_ann);
            if !constraints.is_empty() {
                decl_constraints.insert(e.name.to_string(), constraints);
            }
        }
        for e in &dag.consts {
            let constraints = extract_constraints(&e.type_ann);
            if !constraints.is_empty() {
                decl_constraints.insert(e.name.to_string(), constraints);
            }
        }

        // Enrich param/node/const declarations with resolved types + constraints.
        for (name, resolved_type) in &dag.resolved_decl_types {
            let name_str = name.to_string();
            let key = SymbolKey::TopLevel(name_str.clone());
            if let Some(def) = table.definitions.get_mut(&key) {
                let type_desc = decl_constraints.get(&name_str).map_or_else(
                    || resolved_type.format(registry),
                    |constraints| {
                        format_type_with_constraints(resolved_type, constraints, registry)
                    },
                );
                def.type_description = Some(type_desc);
            }
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
                // Definition symbols are file-local declarations, so the
                // registry key is always the bare reference.
                if let Some(unit_info) =
                    registry
                        .units
                        .get_unit(&graphcal_compiler::syntax::dimension::UnitRef::local(
                            graphcal_compiler::syntax::dimension::UnitName::expect_valid(name),
                        ))
                    && let Some(def_mut) = table.definitions.get_mut(key)
                {
                    let scale_str = match &unit_info.scale {
                        UnitScale::Static(s) => format!("{s}"),
                        UnitScale::Dynamic { .. } => "dynamic".to_string(),
                    };
                    let timing = if unit_info.constness.is_const() {
                        "const"
                    } else {
                        "runtime"
                    };
                    def_mut.type_description = Some(format!(
                        "{}, {timing}, scale = {scale_str}",
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
                                .map(
                                    graphcal_compiler::syntax::index_name::IndexVariantName::as_str,
                                )
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
                    let desc = match type_def.union_members() {
                        None => format!("{} (required)", type_def.name),
                        Some(members) => match type_def.record_fields() {
                            Some(fields) if fields.is_empty() => type_def.name.to_string(),
                            Some(fields) => {
                                let field_descs: Vec<String> =
                                    fields.iter().map(|f| f.name.to_string()).collect();
                                format!("{} {{ {} }}", type_def.name, field_descs.join(", "))
                            }
                            None => {
                                // Union type: show members separated by |
                                let member_descs: Vec<String> =
                                    members.iter().map(|m| m.name.to_string()).collect();
                                member_descs.join(" | ")
                            }
                        },
                    };
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
        let Some(members) = type_def.union_members() else {
            continue;
        };
        for member in members {
            for field in &member.fields {
                let field_key = SymbolKey::Field {
                    owner: SymbolPath::local(member.name.to_string()),
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
                            detail: Some(format!("field of {}.{}", type_def.name, member.name)),
                            visibility: None,
                        },
                    );
                }
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
        let file = desugared;
        let table = build_for_buffer(&file, source);

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
        let file = desugared;
        let table = build_for_buffer(&file, source);

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
        let file = desugared;
        let table = build_for_buffer(&file, source);

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

    /// Issues #827/#828 repro: a table literal over a named index plus a
    /// qualified `Index.Variant` access.
    const TABLE_SOURCE: &str = "\
pub index Maneuver = { Departure, Correction };
param dv: Velocity[Maneuver] = table[Maneuver] {
    Departure: 2.0 km/s;
    Correction: 0.1 km/s;
};
node total: Velocity = @dv[Maneuver.Departure];
";

    fn table_for(source: &str) -> SymbolTable {
        let raw_file = graphcal_compiler::syntax::parser::Parser::with_name(source, "test.gcl")
            .parse_file()
            .unwrap();
        let desugared = graphcal_compiler::syntax::desugar::desugar_multi_decls_in_file(raw_file);
        build_for_buffer(&desugared, source)
    }

    fn slice(source: &str, span: Span) -> &str {
        &source[span.offset()..span.offset() + span.len()]
    }

    /// Issue #827: row-key reference spans must cover exactly the variant
    /// label, not a multi-line merge starting at the table-axis token.
    #[test]
    fn table_row_key_references_are_label_precise() {
        let source = TABLE_SOURCE;
        let table = table_for(source);
        let departure_key = SymbolKey::Variant {
            parent: SymbolPath::local("Maneuver"),
            variant: "Departure".to_string(),
        };
        let refs = table.find_all_references(&departure_key);
        assert!(!refs.is_empty(), "expected references to `Departure`");
        for r in refs {
            assert_eq!(
                slice(source, r.span),
                "Departure",
                "reference span must cover exactly the variant identifier"
            );
        }
    }

    /// Issue #827: the cursor on a row key must resolve to *that* row's
    /// variant — overlapping merged spans used to send `Departure` to
    /// `Correction` and `Correction` to nothing.
    #[test]
    fn table_row_key_cursor_resolves_to_own_variant() {
        let source = TABLE_SOURCE;
        let table = table_for(source);
        for variant in ["Departure", "Correction"] {
            let row_offset = source.find(&format!("    {variant}:")).unwrap() + 4;
            let reference = table
                .find_reference_at(row_offset)
                .unwrap_or_else(|| panic!("expected reference at `{variant}` row key"));
            assert_eq!(
                reference.target,
                SymbolKey::Variant {
                    parent: SymbolPath::local("Maneuver"),
                    variant: variant.to_string(),
                },
                "row key must resolve to its own variant"
            );
        }
    }

    /// The table-axis token inside `table[...]` and the index segment of a
    /// qualified `Index.Variant` path resolve to the index declaration.
    #[test]
    fn index_segments_resolve_to_index_declaration() {
        let source = TABLE_SOURCE;
        let table = table_for(source);
        let axis_offset = source.find("table[Maneuver]").unwrap() + "table[".len();
        let qualifier_offset = source.find("@dv[Maneuver.Departure]").unwrap() + "@dv[".len();
        for offset in [axis_offset, qualifier_offset] {
            let reference = table
                .find_reference_at(offset)
                .expect("expected index reference at index segment");
            assert_eq!(
                reference.target,
                SymbolKey::TopLevel("Maneuver".to_string())
            );
        }
    }

    /// Issue #828: the reference recorded for `Maneuver.Departure` in an
    /// index access must address only the `Departure` segment so rename
    /// rewrites `@dv[Maneuver.Departure]` into `@dv[Maneuver.Begin]`.
    #[test]
    fn qualified_variant_reference_is_segment_precise() {
        let source = TABLE_SOURCE;
        let table = table_for(source);
        let departure_offset = source.find("Maneuver.Departure").unwrap() + "Maneuver.".len();
        let reference = table
            .find_reference_at(departure_offset)
            .expect("expected variant reference at `Departure` segment");
        assert_eq!(
            reference.target,
            SymbolKey::Variant {
                parent: SymbolPath::local("Maneuver"),
                variant: "Departure".to_string(),
            }
        );
        assert_eq!(slice(source, reference.span), "Departure");
    }

    /// Variant literals (`Season.Winter` in expression position) and match
    /// patterns get the same segment-precise spans.
    #[test]
    fn variant_literal_and_match_pattern_references_are_segment_precise() {
        let source = "\
index Season = { Winter, Summer };
node pick: Season = Season.Winter;
node out: Dimensionless = match @pick { Season.Winter => 1.0, Season.Summer => 2.0 };
";
        let table = table_for(source);
        let winter_key = SymbolKey::Variant {
            parent: SymbolPath::local("Season"),
            variant: "Winter".to_string(),
        };
        let refs = table.find_all_references(&winter_key);
        assert_eq!(
            refs.len(),
            2,
            "expected literal + match-pattern references to `Winter`"
        );
        for r in refs {
            assert_eq!(slice(source, r.span), "Winter");
        }
    }
}
