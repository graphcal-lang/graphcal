//! Symbol table for LSP features: maps source locations to definitions and references.

use std::collections::HashMap;
use std::sync::Arc;

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::desugar::desugared_ast::{
    AssertDecl, AttributeArg, BaseDimDecl, BindableVisibility, DagDecl, DeclKind, DimDecl, DimExpr,
    FigureDecl, ImportDecl, IndexDecl, IndexDeclKind, LayerDecl, NodeDecl, ParamDecl, PlotDecl,
    TypeDecl, TypeDeclBody, TypeExprKind, UnitDecl, UnitExpr,
};
use graphcal_compiler::hir;
use graphcal_compiler::syntax::attribute::AttributeName;
use graphcal_compiler::syntax::decl_name::{DeclName, ResolvedDeclName};
use graphcal_compiler::syntax::dimension::{ResolvedDimName, ResolvedUnitName};
use graphcal_compiler::syntax::index_name::ResolvedIndexName;
use graphcal_compiler::syntax::module_name::{ModuleAliasName, ScopedName};
use graphcal_compiler::syntax::module_resolve::{ModuleResolveError, ModuleResolver};
use graphcal_compiler::syntax::names::{NameAtom, NamePath};
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::syntax::type_name::{
    ConstructorName, FieldName, GenericParamName, ResolvedConstructorName, ResolvedStructTypeName,
};

use graphcal_compiler::builtin::{BuiltinConst, BuiltinFnName};
use graphcal_compiler::registry::builtins::builtin_functions;
use graphcal_compiler::registry::format::format_unit_terms_with_config;
use graphcal_compiler::registry::time_zone::TimeZoneRegistry;
use graphcal_compiler::registry::types::{IndexKind, SemanticRegistry, UnitScale};
use graphcal_compiler::tir::typed::{ResolvedDomainBound, ResolvedIndex, ResolvedTypeExpr, TIR};
use graphcal_eval::eval::format_number;
use tower_lsp::lsp_types::Position;

use crate::convert::LineIndex;
use crate::nominal_type_index::NominalTypeIndex;
use crate::symbol_identity::{
    ExternFunctionId, FieldId, GenericParamId, IndexVariantId, LocalSymbolId, ReferenceTarget,
    SourceSymbolPath, UnresolvedSymbol,
};

/// The kind of expression scope that introduces local variables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExprScopeKind {
    For,
    Scan,
    Unfold,
    Match,
}

/// Canonical typed key for symbol-table definitions.
///
/// Re-exported under the historical name while callers migrate; unlike the
/// previous spelling-shaped enum, every value is a resolved semantic identity.
pub use crate::symbol_identity::SymbolId as SymbolKey;

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
    nominal_types: &'a NominalTypeIndex,
    time_zones: TimeZoneRegistry,
    /// Lexical local definitions of the current body, keyed by HIR identity.
    locals: HashMap<hir::LocalId, SymbolKey>,
    /// Span of the expression currently being lowered. Together with the DAG
    /// owner it scopes HIR-local IDs, which are only body-local identities.
    body_span: Span,
}

impl<'a> HirRefCollector<'a> {
    fn new(
        dag_id: &'a DagId,
        resolver: &'a ModuleResolver,
        nominal_types: &'a NominalTypeIndex,
    ) -> Self {
        Self {
            dag_id,
            resolver,
            nominal_types,
            time_zones: TimeZoneRegistry::bundled(),
            locals: HashMap::new(),
            body_span: Span::new(0, 0),
        }
    }

    fn declaration_target(&self, path: &NamePath) -> ReferenceTarget {
        self.resolver
            .resolve_decl_path(self.dag_id, path)
            .map_or_else(
                |_| {
                    ReferenceTarget::Unresolved(UnresolvedSymbol::Declaration(
                        SourceSymbolPath::from_name_path(path),
                    ))
                },
                |name| SymbolKey::Declaration(name).into(),
            )
    }

    fn collect_resolved_unit_expr_refs(unit: &hir::ResolvedUnitExpr, table: &mut SymbolTable) {
        for item in &unit.terms {
            table.references.push(ReferenceInfo {
                span: item.name.span,
                target: SymbolKey::Unit(item.name.value.resolved().clone()).into(),
            });
        }
    }

    fn variant_key(
        variant: &graphcal_compiler::syntax::index_name::ResolvedIndexVariant,
    ) -> SymbolKey {
        SymbolKey::IndexVariant(IndexVariantId::from_resolved(variant))
    }

    /// Lower one declaration body and record its references.
    fn collect_body(
        &mut self,
        expr: &graphcal_compiler::desugar::desugared_ast::Expr,
        table: &mut SymbolTable,
    ) {
        let generic_scope = hir::GenericScope::new();
        let prelude = hir::PreludeTypeScope::graphcal();
        let ctx = hir::ExprLoweringContext::new(
            self.dag_id,
            self.resolver,
            &generic_scope,
            &self.time_zones,
        )
        .with_prelude(&prelude);
        let (lowered, diagnostics) = hir::lower_expr_tolerant(expr, ctx);
        self.locals.clear();
        self.body_span = expr.span;
        self.walk(&lowered, table);
        for diagnostic in &diagnostics {
            Self::record_unresolved(diagnostic, table);
        }
    }

    /// Record a reference for an unresolved name from a lowering diagnostic,
    /// keyed by its written spelling so IDE features answer on broken code.
    fn record_unresolved(err: &hir::ExprLowerError, table: &mut SymbolTable) {
        let (target, span) = match err {
            hir::ExprLowerError::UnknownGraphRef { name, span } => {
                let path = SourceSymbolPath::module_member(
                    name.qualifier()
                        .iter()
                        .map(|segment| segment.atom().clone())
                        .collect(),
                    name.member().atom().clone(),
                );
                (UnresolvedSymbol::Declaration(path), *span)
            }
            hir::ExprLowerError::UnknownLocalRef { name, span } => (
                UnresolvedSymbol::Declaration(SourceSymbolPath::local(name.atom().clone())),
                *span,
            ),
            hir::ExprLowerError::UnknownUnit { name, span } => {
                let qualifier = name
                    .qualifier()
                    .into_iter()
                    .flat_map(|owner| owner.segments().iter().cloned())
                    .collect();
                let path = SourceSymbolPath::module_member(qualifier, name.name().atom().clone());
                (UnresolvedSymbol::Unit(path), *span)
            }
            hir::ExprLowerError::UnknownFunction { path, span } => {
                let Ok(name) = NameAtom::parse(path) else {
                    return;
                };
                (
                    UnresolvedSymbol::Function(SourceSymbolPath::local(name)),
                    *span,
                )
            }
            hir::ExprLowerError::ModuleResolve {
                source: ModuleResolveError::UnknownName { name, .. },
                span,
            } => {
                let Ok(name) = NameAtom::parse(name) else {
                    return;
                };
                (
                    UnresolvedSymbol::Declaration(SourceSymbolPath::local(name)),
                    *span,
                )
            }
            _ => return,
        };
        table.references.push(ReferenceInfo {
            span,
            target: ReferenceTarget::Unresolved(target),
        });
    }

    fn define_local(
        &mut self,
        local: &hir::LocalDef,
        _kind: ExprScopeKind,
        _offset: usize,
        detail: String,
        table: &mut SymbolTable,
    ) {
        let key = SymbolKey::Local(LocalSymbolId::new(
            self.dag_id.clone(),
            self.body_span,
            local.id,
        ));
        table.insert_definition(
            key.clone(),
            DefinitionInfo {
                doc: None,
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
        table.references.push(ReferenceInfo {
            span,
            target: target.into(),
        });
    }

    /// Record references for an index-variant reference: the variant segment
    /// targets the variant symbol, and the index segment (when one is written
    /// — the `Maneuver` in `Maneuver#Departure`, or the axis token inside
    /// `table[...]` for desugared table rows) targets the index declaration.
    /// Keeping the two segments separate makes rename edits and goto-def
    /// segment-precise instead of splicing whole qualified paths.
    fn variant_reference(variant: &hir::expr::IndexVariantRef, table: &mut SymbolTable) {
        let target = Self::variant_key(&variant.variant);
        Self::reference(table, variant.variant_span, target);
        let index = variant.variant.index();
        let key = SymbolKey::Index(index.clone());
        for index_span in variant.index_spans() {
            Self::reference(table, index_span, key.clone());
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
            hir::ExprKind::Error { children } => {
                for child in children {
                    self.walk(child, table);
                }
            }
            hir::ExprKind::Number(_)
            | hir::ExprKind::Integer(_)
            | hir::ExprKind::Bool(_)
            | hir::ExprKind::StringLiteral(_)
            | hir::ExprKind::OffsetDateTimeLiteral(_)
            | hir::ExprKind::CivilDateTimeLiteral(_)
            | hir::ExprKind::ZonedDateTimeLiteral(_)
            | hir::ExprKind::IanaTimeZoneLiteral(_) => {}
            hir::ExprKind::GraphRef(target) => {
                Self::reference(
                    table,
                    target.span,
                    SymbolKey::Declaration(target.value.clone()),
                );
            }
            hir::ExprKind::ConstRef(const_ref) => {
                let target = match &const_ref.value {
                    hir::ConstRef::Decl(name) => SymbolKey::Declaration(name.clone()),
                    hir::ConstRef::Constructor(name) => SymbolKey::Constructor(name.clone()),
                    hir::ConstRef::Builtin(builtin) => SymbolKey::BuiltinConstant(*builtin),
                    hir::ConstRef::TimeScale(scale) => SymbolKey::TimeScale(*scale),
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
                    hir::expr::TypeSystemRef::Type(name) => SymbolKey::StructType(name.clone()),
                    hir::expr::TypeSystemRef::Dimension(name) => SymbolKey::Dimension(name.clone()),
                    hir::expr::TypeSystemRef::Index(name) => SymbolKey::Index(name.clone()),
                    hir::expr::TypeSystemRef::IndexVariant(variant) => Self::variant_key(variant),
                };
                Self::reference(table, type_ref.span, target);
            }
            hir::ExprKind::VariantLiteral(variant) => {
                Self::variant_reference(variant, table);
            }
            hir::ExprKind::BinOp { lhs, rhs, .. } => {
                self.walk(lhs, table);
                self.walk(rhs, table);
            }
            hir::ExprKind::UnaryOp { operand, .. }
            | hir::ExprKind::DisplayTimezone { expr: operand, .. } => {
                self.walk(operand, table);
            }
            hir::ExprKind::FieldAccess { expr, field } => {
                self.walk(expr, table);
                let target = self.nominal_types.expression_constructor(expr).map_or_else(
                    || ReferenceTarget::Unresolved(UnresolvedSymbol::Field(field.value.clone())),
                    |constructor| {
                        SymbolKey::Field(FieldId::new(constructor, field.value.clone())).into()
                    },
                );
                table.references.push(ReferenceInfo {
                    span: field.span,
                    target,
                });
            }
            hir::ExprKind::FnCall { callee, args } => {
                match &callee.value {
                    hir::FunctionRef::Builtin(builtin) => {
                        Self::reference(table, callee.span, SymbolKey::BuiltinFunction(*builtin));
                    }
                    hir::FunctionRef::Epoch { scale } => {
                        Self::reference(
                            table,
                            callee.span,
                            SymbolKey::BuiltinFunction(BuiltinFnName::Epoch),
                        );
                        Self::reference(table, scale.span, SymbolKey::TimeScale(scale.value));
                    }
                    hir::FunctionRef::External(ext) => Self::reference(
                        table,
                        callee.span,
                        SymbolKey::ExternFunction(ExternFunctionId::new(
                            self.dag_id.clone(),
                            ext.alias.clone(),
                            ext.name.clone(),
                        )),
                    ),
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
            hir::ExprKind::QuantityLiteral { unit, .. } => {
                Self::collect_resolved_unit_expr_refs(unit, table);
            }
            hir::ExprKind::Convert {
                expr: inner,
                target,
            } => {
                self.walk(inner, table);
                Self::collect_resolved_unit_expr_refs(target, table);
            }
            hir::ExprKind::ConstructorCall {
                callee,
                generic_args,
                fields,
            } => {
                let constructor = callee.value.clone();
                Self::reference(
                    table,
                    callee.span,
                    SymbolKey::Constructor(constructor.clone()),
                );
                for generic_arg in generic_args {
                    self.walk_generic_arg(generic_arg, table);
                }
                for field in fields {
                    Self::reference(
                        table,
                        field.name.span,
                        SymbolKey::Field(FieldId::new(
                            constructor.clone(),
                            field.name.value.clone(),
                        )),
                    );
                    self.walk(&field.value, table);
                }
            }
            hir::ExprKind::MapLiteral { entries } => {
                for entry in entries {
                    for key in &entry.keys {
                        if let hir::expr::MapEntryKey::IndexVariant(variant) = key {
                            Self::variant_reference(variant, table);
                        }
                    }
                    self.walk(&entry.value, table);
                }
            }
            hir::ExprKind::ForComp { bindings, body } => {
                for binding in bindings {
                    let detail = match &binding.index {
                        hir::expr::ForBindingIndex::Named(index) => {
                            Self::reference(
                                table,
                                index.span,
                                SymbolKey::Index(index.value.clone()),
                            );
                            format!("loop variable over {}", index.value.as_str())
                        }
                        hir::expr::ForBindingIndex::Finite { cardinality, .. } => {
                            format!("loop variable over Fin({})", nat_expr_label(cardinality))
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
                            Self::variant_reference(variant, table);
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
                recurrence,
                init,
                body,
            } => {
                let axis = &recurrence.axis;
                Self::reference(table, axis.span, SymbolKey::Index(axis.value.clone()));
                self.walk(init, table);
                let offset = expr.span.offset();
                self.define_local(
                    &recurrence.previous_state,
                    ExprScopeKind::Unfold,
                    offset,
                    "unfold previous state".into(),
                    table,
                );
                self.define_local(
                    &recurrence.previous_index,
                    ExprScopeKind::Unfold,
                    offset,
                    "unfold previous index".into(),
                    table,
                );
                self.define_local(
                    &recurrence.current_index,
                    ExprScopeKind::Unfold,
                    offset,
                    "unfold current index".into(),
                    table,
                );
                self.walk(body, table);
            }
            hir::ExprKind::KeyForm { axis, arg, .. } => {
                if let hir::expr::ForBindingIndex::Named(axis) = axis {
                    Self::reference(table, axis.span, SymbolKey::Index(axis.value.clone()));
                }
                self.walk(arg, table);
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
                            let constructor_id = constructor.value.clone();
                            Self::reference(
                                table,
                                constructor.span,
                                SymbolKey::Constructor(constructor_id.clone()),
                            );
                            for binding in bindings {
                                match binding {
                                    hir::expr::PatternBinding::Bind { field, local } => {
                                        Self::reference(
                                            table,
                                            field.span,
                                            SymbolKey::Field(FieldId::new(
                                                constructor_id.clone(),
                                                field.value.clone(),
                                            )),
                                        );
                                        self.define_local(
                                            local,
                                            ExprScopeKind::Match,
                                            arm.span.offset(),
                                            format!("bound from {}", constructor.value),
                                            table,
                                        );
                                    }
                                    hir::expr::PatternBinding::Wildcard { field, .. } => {
                                        Self::reference(
                                            table,
                                            field.span,
                                            SymbolKey::Field(FieldId::new(
                                                constructor_id.clone(),
                                                field.value.clone(),
                                            )),
                                        );
                                    }
                                }
                            }
                        }
                        hir::expr::MatchPattern::IndexLabel { variant, .. } => {
                            Self::variant_reference(variant, table);
                        }
                    }
                    self.walk(&arm.body, table);
                }
            }
            hir::ExprKind::DagCall {
                target,
                args,
                output,
                ..
            } => {
                if let Some(parent) = target.value.parent() {
                    Self::reference(
                        table,
                        target.span,
                        SymbolKey::Declaration(ResolvedDeclName::from_def(
                            parent,
                            DeclName::expect_valid(target.value.name()),
                        )),
                    );
                }
                Self::reference(
                    table,
                    output.span,
                    SymbolKey::Declaration(output.value.clone()),
                );
                for arg in args {
                    Self::reference(
                        table,
                        arg.target.span,
                        SymbolKey::Declaration(arg.target.value.clone()),
                    );
                    self.walk(&arg.value, table);
                }
            }
        }
    }

    fn walk_type(&self, type_expr: &hir::TypeExpr, table: &mut SymbolTable) {
        match &type_expr.kind {
            hir::TypeExprKind::Builtin(_) | hir::TypeExprKind::GenericTypeParam(_) => {}
            hir::TypeExprKind::Complex(dimension) => {
                if let hir::DimArg::Expr(dim_expr) = dimension {
                    for item in &dim_expr.terms {
                        if let hir::DimTermTarget::Dimension(name) = &item.term.target {
                            Self::reference(
                                table,
                                name.span,
                                SymbolKey::Dimension(name.value.clone()),
                            );
                        }
                    }
                }
            }
            hir::TypeExprKind::DimExpr(dim_expr) => {
                for item in &dim_expr.terms {
                    if let hir::DimTermTarget::Dimension(name) = &item.term.target {
                        Self::reference(table, name.span, SymbolKey::Dimension(name.value.clone()));
                    }
                }
            }
            hir::TypeExprKind::Index(index) | hir::TypeExprKind::Key(index) => {
                if let hir::IndexRef::Concrete(name) = index {
                    Self::reference(table, name.span, SymbolKey::Index(name.value.clone()));
                }
            }
            hir::TypeExprKind::Struct(name) => {
                Self::reference(table, name.span, SymbolKey::StructType(name.value.clone()));
            }
            hir::TypeExprKind::Indexed { base, indexes } => {
                self.walk_type(base, table);
                for index in indexes {
                    if let hir::IndexRef::Concrete(name) = index {
                        Self::reference(table, name.span, SymbolKey::Index(name.value.clone()));
                    }
                }
            }
            hir::TypeExprKind::TypeApplication { name, generic_args } => {
                Self::reference(table, name.span, SymbolKey::StructType(name.value.clone()));
                for arg in generic_args {
                    self.walk_generic_arg(arg, table);
                }
            }
        }
    }

    fn walk_generic_arg(&self, arg: &hir::GenericArg, table: &mut SymbolTable) {
        match arg {
            hir::GenericArg::Dim(hir::DimArg::Dimensionless(_))
            | hir::GenericArg::Index(hir::IndexRef::GenericParam(_) | hir::IndexRef::Finite(_))
            | hir::GenericArg::Nat(_) => {}
            hir::GenericArg::Dim(hir::DimArg::Expr(dim_expr)) => {
                for item in &dim_expr.terms {
                    if let hir::DimTermTarget::Dimension(name) = &item.term.target {
                        Self::reference(table, name.span, SymbolKey::Dimension(name.value.clone()));
                    }
                }
            }
            hir::GenericArg::Index(hir::IndexRef::Concrete(name)) => {
                Self::reference(table, name.span, SymbolKey::Index(name.value.clone()));
            }
            hir::GenericArg::Type(type_expr) => self.walk_type(type_expr, table),
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

/// A source declaration identity whose syntactic kind and canonical namespace
/// agree by construction.
///
/// This private builder type prevents callers from pairing, for example, a
/// unit key with `SymbolCategory::Const`. The presentation category and
/// semantic ID are derived together in [`TopLevelDefinition::into_parts`].
enum TopLevelDefinition {
    Param(ResolvedDeclName),
    Node(ResolvedDeclName),
    Const(ResolvedDeclName),
    Dimension(ResolvedDimName),
    Unit(ResolvedUnitName),
    StructType(ResolvedStructTypeName),
    Index(ResolvedIndexName),
    Assert(ResolvedDeclName),
    Plot(ResolvedDeclName),
    Figure(ResolvedDeclName),
    Layer(ResolvedDeclName),
    Dag(ResolvedDeclName),
}

impl TopLevelDefinition {
    fn into_parts(self) -> (SymbolKey, SymbolCategory, String) {
        match self {
            Self::Param(name) => {
                definition_parts(SymbolKey::Declaration(name), SymbolCategory::Param)
            }
            Self::Node(name) => {
                definition_parts(SymbolKey::Declaration(name), SymbolCategory::Node)
            }
            Self::Const(name) => {
                definition_parts(SymbolKey::Declaration(name), SymbolCategory::Const)
            }
            Self::Dimension(name) => {
                definition_parts(SymbolKey::Dimension(name), SymbolCategory::Dimension)
            }
            Self::Unit(name) => definition_parts(SymbolKey::Unit(name), SymbolCategory::Unit),
            Self::StructType(name) => {
                definition_parts(SymbolKey::StructType(name), SymbolCategory::StructType)
            }
            Self::Index(name) => definition_parts(SymbolKey::Index(name), SymbolCategory::Index),
            Self::Assert(name) => {
                definition_parts(SymbolKey::Declaration(name), SymbolCategory::Assert)
            }
            Self::Plot(name) => {
                definition_parts(SymbolKey::Declaration(name), SymbolCategory::Plot)
            }
            Self::Figure(name) => {
                definition_parts(SymbolKey::Declaration(name), SymbolCategory::Figure)
            }
            Self::Layer(name) => {
                definition_parts(SymbolKey::Declaration(name), SymbolCategory::Layer)
            }
            Self::Dag(name) => definition_parts(SymbolKey::Declaration(name), SymbolCategory::Dag),
        }
    }
}

fn definition_parts(
    key: SymbolKey,
    category: SymbolCategory,
) -> (SymbolKey, SymbolCategory, String) {
    let name = key.leaf_name().to_string();
    (key, category, name)
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
    GenericParam,
    LocalVar,
    BuiltinFn,
    /// Extern function declared by an `import plugin` block.
    ExternFn,
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
    /// The `///` doc block preceding the declaration, rendered in hover.
    pub doc: Option<String>,
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
    /// Canonical target, or a namespace-aware unresolved spelling retained
    /// while the user is editing incomplete code.
    pub target: ReferenceTarget,
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

/// A duplicate canonical definition rejected by [`DefinitionIndex`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("duplicate editor definition for {key:?}")]
pub struct DuplicateDefinition {
    pub key: SymbolKey,
    pub first_span: Span,
    pub duplicate_span: Span,
}

/// Invariant-preserving definition map.
///
/// Mutation is private and fallible: a canonical ID can never silently replace
/// another definition. Consumers receive only read APIs.
#[derive(Debug, Default)]
pub struct DefinitionIndex {
    entries: HashMap<SymbolKey, DefinitionInfo>,
}

impl DefinitionIndex {
    fn insert(
        &mut self,
        key: SymbolKey,
        definition: DefinitionInfo,
    ) -> Result<(), Box<DuplicateDefinition>> {
        use std::collections::hash_map::Entry;
        match self.entries.entry(key.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(definition);
                Ok(())
            }
            Entry::Occupied(entry) => Err(Box::new(DuplicateDefinition {
                key,
                first_span: entry.get().name_span,
                duplicate_span: definition.name_span,
            })),
        }
    }

    pub fn get(&self, key: &SymbolKey) -> Option<&DefinitionInfo> {
        self.entries.get(key)
    }

    fn get_mut(&mut self, key: &SymbolKey) -> Option<&mut DefinitionInfo> {
        self.entries.get_mut(key)
    }

    pub fn contains_key(&self, key: &SymbolKey) -> bool {
        self.entries.contains_key(key)
    }

    pub fn values(&self) -> std::collections::hash_map::Values<'_, SymbolKey, DefinitionInfo> {
        self.entries.values()
    }

    pub fn iter(&self) -> std::collections::hash_map::Iter<'_, SymbolKey, DefinitionInfo> {
        self.entries.iter()
    }

    pub fn keys(&self) -> std::collections::hash_map::Keys<'_, SymbolKey, DefinitionInfo> {
        self.entries.keys()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

impl<'a> IntoIterator for &'a DefinitionIndex {
    type Item = (&'a SymbolKey, &'a DefinitionInfo);
    type IntoIter = std::collections::hash_map::Iter<'a, SymbolKey, DefinitionInfo>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}

impl std::ops::Index<&SymbolKey> for DefinitionIndex {
    type Output = DefinitionInfo;

    fn index(&self, index: &SymbolKey) -> &Self::Output {
        &self.entries[index]
    }
}

/// The complete symbol table for one file.
#[derive(Debug)]
pub struct SymbolTable {
    /// Canonical module owning source definitions in this table.
    owner: DagId,
    /// All symbol definitions keyed by a typed `SymbolKey`.
    pub(crate) definitions: DefinitionIndex,
    /// Duplicate IDs rejected while tolerantly indexing invalid source.
    definition_conflicts: Vec<DuplicateDefinition>,
    /// All reference occurrences sorted by span offset.
    references: Vec<ReferenceInfo>,
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

impl Default for SymbolTable {
    fn default() -> Self {
        Self::new(DagId::root_in_package("__graphcal_lsp__", "empty"))
    }
}

impl SymbolTable {
    fn new(owner: DagId) -> Self {
        Self {
            owner,
            definitions: DefinitionIndex::default(),
            definition_conflicts: Vec::new(),
            references: Vec::new(),
            name_span_to_key: HashMap::new(),
            defs_by_name_span: Vec::new(),
            inlay_hint_entries: Vec::new(),
        }
    }

    pub(crate) const fn owner(&self) -> &DagId {
        &self.owner
    }

    /// Whether a source-defined identity can be referenced outside this file.
    /// Attached members inherit their nominal declaration's visibility, while
    /// symbols inside a private enclosing DAG remain file-local even when the
    /// port itself is bindable.
    #[must_use]
    pub fn is_externally_visible(&self, key: &SymbolKey) -> bool {
        if matches!(
            key,
            SymbolKey::Local(_)
                | SymbolKey::ExternFunction(_)
                | SymbolKey::BuiltinFunction(_)
                | SymbolKey::BuiltinConstant(_)
                | SymbolKey::TimeScale(_)
        ) {
            return false;
        }
        let Some(definition) = self.definitions.get(key) else {
            return true;
        };
        if definition.visibility == Some(BindableVisibility::Private) {
            return false;
        }

        let mut owner = key.owner().cloned();
        while let Some(current) = owner {
            if current == self.owner {
                return true;
            }
            let Some(parent) = current.parent() else {
                return true;
            };
            let dag_key = SymbolKey::Declaration(ResolvedDeclName::from_def(
                parent.clone(),
                DeclName::expect_valid(current.name()),
            ));
            match self.definitions.get(&dag_key) {
                Some(dag) if dag.visibility == Some(BindableVisibility::Private) => return false,
                Some(_) => owner = Some(parent),
                None => return true,
            }
        }
        true
    }

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
    ///
    /// The operation is atomic: duplicate semantic IDs return a typed error and
    /// leave both indices unchanged.
    fn try_insert_definition(
        &mut self,
        key: SymbolKey,
        definition: DefinitionInfo,
    ) -> Result<(), Box<DuplicateDefinition>> {
        let name_span = definition.name_span;
        self.definitions.insert(key.clone(), definition)?;
        if !name_span.is_empty() {
            self.name_span_to_key.insert(name_span.offset(), key);
        }
        Ok(())
    }

    /// Tolerant builder boundary: preserve the first valid definition and
    /// retain every typed duplicate error for diagnostics/debugging.
    fn insert_definition(&mut self, key: SymbolKey, definition: DefinitionInfo) {
        if let Err(conflict) = self.try_insert_definition(key, definition) {
            self.definition_conflicts.push(*conflict);
        }
    }

    #[cfg(test)]
    fn definition_conflicts(&self) -> &[DuplicateDefinition] {
        &self.definition_conflicts
    }

    /// Register a source definition under its canonical semantic identity.
    fn register_definition(
        &mut self,
        identity: TopLevelDefinition,
        name_span: Span,
        decl_span: Span,
        type_description: Option<String>,
        detail: Option<String>,
        visibility: BindableVisibility,
    ) {
        let (key, category, name) = identity.into_parts();
        self.insert_definition(
            key,
            DefinitionInfo {
                doc: None,
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

    /// Every indexed reference occurrence in source order.
    #[must_use]
    pub fn references(&self) -> &[ReferenceInfo] {
        &self.references
    }

    /// Resolve a reference target against definitions declared in this table.
    /// Qualified unresolved spellings belong to an import binding and are
    /// intentionally left for the workspace analysis layer.
    pub fn resolve_local_target(&self, target: &ReferenceTarget) -> Option<SymbolKey> {
        match target {
            ReferenceTarget::Resolved(target) => Some(target.clone()),
            ReferenceTarget::Unresolved(unresolved) => {
                let leaf = if let UnresolvedSymbol::Field(field) = unresolved {
                    field.as_str()
                } else {
                    let path = unresolved.path()?;
                    if !path.is_local() {
                        return None;
                    }
                    path.leaf().as_str()
                };
                let mut candidates = self.definitions.keys().filter(|candidate| {
                    candidate.leaf_name() == leaf && unresolved.accepts(candidate)
                });
                let candidate = candidates.next()?;
                candidates.next().is_none().then(|| candidate.clone())
            }
        }
    }

    /// Whether a source occurrence has the target's spelling and namespace but
    /// could not be assigned a unique canonical identity. Rename must refuse
    /// such a target rather than silently omit a possible occurrence.
    pub fn has_ambiguous_reference_to(&self, target: &SymbolKey) -> bool {
        self.references.iter().any(|reference| {
            let ReferenceTarget::Unresolved(unresolved) = &reference.target else {
                return false;
            };
            let same_local_spelling = match unresolved {
                UnresolvedSymbol::Field(field) => field.as_str() == target.leaf_name(),
                _ => unresolved.path().is_some_and(|path| {
                    path.is_local() && path.leaf().as_str() == target.leaf_name()
                }),
            };
            same_local_spelling
                && unresolved.accepts(target)
                && self.resolve_local_target(&reference.target).is_none()
        })
    }

    /// Find all references that point to the given target key.
    pub fn find_all_references(&self, target: &SymbolKey) -> Vec<&ReferenceInfo> {
        self.references
            .iter()
            .filter(|reference| {
                self.resolve_local_target(&reference.target).as_ref() == Some(target)
            })
            .collect()
    }

    pub fn unique_definition_named(&self, name: &str) -> Option<&DefinitionInfo> {
        let mut matches = self.definitions.values().filter(|definition| {
            definition.name == name
                && !definition.name_span.is_empty()
                && !matches!(
                    definition.category,
                    SymbolCategory::Constructor
                        | SymbolCategory::Field
                        | SymbolCategory::GenericParam
                        | SymbolCategory::LocalVar
                        | SymbolCategory::BuiltinFn
                        | SymbolCategory::BuiltinConst
                        | SymbolCategory::ExternFn
                )
        });
        let definition = matches.next()?;
        matches.next().is_none().then_some(definition)
    }

    pub fn scoped_name_for_decl(&self, key: &SymbolKey) -> Option<ScopedName> {
        let SymbolKey::Declaration(name) = key else {
            return None;
        };
        let member = name.to_unowned_def_name();
        if name.owner() == &self.owner {
            return Some(ScopedName::local(member));
        }
        if !name.owner().is_descendant_of(&self.owner) {
            return None;
        }
        let qualifier = name
            .owner()
            .segments()
            .iter()
            .skip(self.owner.segments().len())
            .map(|segment| ModuleAliasName::try_new(segment.to_string()))
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        Some(ScopedName::qualified_path(qualifier, member))
    }

    pub fn definition_for_scoped_decl(&self, name: &ScopedName) -> Option<&DefinitionInfo> {
        self.definitions.iter().find_map(|(key, definition)| {
            let SymbolKey::Declaration(resolved) = key else {
                return None;
            };
            let owner_matches = if name.qualifier().is_empty() {
                resolved.owner() == &self.owner
            } else {
                let qualifier = name.qualifier();
                let owner_segments = resolved.owner().segments().as_slice();
                owner_segments.len() >= qualifier.len()
                    && owner_segments[owner_segments.len() - qualifier.len()..]
                        .iter()
                        .zip(qualifier)
                        .all(|(segment, qualifier)| segment.as_ref() == qualifier.as_str())
            };
            (owner_matches && resolved.as_str() == name.member().as_str()).then_some(definition)
        })
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
    let mut table = SymbolTable::new(dag_id.clone());
    register_builtins(&mut table);
    let nominal_types = NominalTypeIndex::build(&ast.declarations, dag_id, resolver);
    collect_declarations(
        &ast.declarations,
        source,
        dag_id,
        resolver,
        &nominal_types,
        &mut table,
    );

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
    apply_doc_comments(&ast.declarations, &mut table);
    table
}

/// Copy each declaration's attached `///` doc block onto the definitions
/// registered for it, matching by full-declaration span (one span maps to
/// several definitions for expanded multi-decls).
fn apply_doc_comments(
    declarations: &[graphcal_compiler::desugar::desugared_ast::Declaration],
    table: &mut SymbolTable,
) {
    for declaration in declarations {
        if let Some(doc) = &declaration.doc {
            for definition in table.definitions.entries.values_mut() {
                if definition.decl_span == declaration.span && definition.doc.is_none() {
                    definition.doc = Some(doc.text().to_string());
                }
            }
        }
        if let DeclKind::Dag(dag) = &declaration.kind {
            apply_doc_comments(&dag.body, table);
        }
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "exhaustive declaration dispatch keeps owner recursion visibly complete"
)]
fn collect_declarations(
    declarations: &[graphcal_compiler::desugar::desugared_ast::Declaration],
    source: &str,
    owner: &DagId,
    resolver: &ModuleResolver,
    nominal_types: &NominalTypeIndex,
    table: &mut SymbolTable,
) {
    let mut refs = HirRefCollector::new(owner, resolver, nominal_types);
    for declaration in declarations {
        collect_attribute_refs(&declaration.attributes, table, &refs);
        match &declaration.kind {
            DeclKind::Param(param) => collect_param_decl(
                param,
                declaration.span,
                BindableVisibility::PublicBind,
                table,
                &mut refs,
            ),
            DeclKind::Node(node) => collect_node_decl(
                node,
                declaration.span,
                node.visibility.into(),
                table,
                &mut refs,
            ),
            DeclKind::ConstNode(constant) => collect_const_node_decl(
                constant,
                declaration.span,
                constant.visibility.into(),
                table,
                &mut refs,
            ),
            DeclKind::BaseDimension(dimension) => collect_base_dim_decl(
                dimension,
                declaration.span,
                dimension.visibility.into(),
                table,
                &refs,
            ),
            DeclKind::Dimension(dimension) => collect_dim_decl(
                dimension,
                declaration.span,
                dimension.visibility,
                table,
                &refs,
            ),
            DeclKind::Unit(unit) => collect_unit_decl(
                unit,
                declaration.span,
                unit.visibility.into(),
                table,
                &mut refs,
            ),
            DeclKind::Type(type_decl) => collect_type_decl(
                type_decl,
                declaration.span,
                type_decl.visibility,
                table,
                &mut refs,
            ),
            DeclKind::Index(index) => {
                collect_index_decl(index, declaration.span, index.visibility, table, &mut refs);
            }
            DeclKind::Assert(assertion) => collect_assert_decl(
                assertion,
                declaration.span,
                assertion.visibility.into(),
                table,
                &mut refs,
            ),
            DeclKind::Plot(plot) => collect_plot_decl(
                plot,
                declaration.span,
                plot.visibility.into(),
                table,
                &mut refs,
            ),
            DeclKind::Figure(figure) => collect_figure_decl(
                figure,
                declaration.span,
                figure.visibility.into(),
                table,
                &mut refs,
            ),
            DeclKind::Layer(layer) => collect_layer_decl(
                layer,
                declaration.span,
                layer.visibility.into(),
                table,
                &mut refs,
            ),
            DeclKind::Import(import) => collect_import_decl(import, table),
            DeclKind::PluginImport(plugin) => {
                collect_plugin_import_decl(plugin, source, table, &mut refs);
            }
            DeclKind::Include(include) => collect_include_decl(include, table, &mut refs),
            DeclKind::Dag(dag) => {
                collect_dag_decl(dag, declaration.span, dag.visibility.into(), table, &refs);
                collect_declarations(
                    &dag.body,
                    source,
                    &owner.child(dag.name.value.as_str()),
                    resolver,
                    nominal_types,
                    table,
                );
            }
            #[expect(
                clippy::uninhabited_references,
                reason = "Sugar(Infallible) — proof of unreachability"
            )]
            DeclKind::Sugar(sugar) => match *sugar {},
        }
    }
}

fn register_builtins(table: &mut SymbolTable) {
    for constant in BuiltinConst::ALL {
        let name = constant.as_str();
        table.insert_definition(
            SymbolKey::BuiltinConstant(*constant),
            DefinitionInfo {
                doc: None,
                name: name.to_string(),
                category: SymbolCategory::BuiltinConst,
                name_span: Span::new(0, 0),
                decl_span: Span::new(0, 0),
                type_description: Some("Dimensionless".to_string()),
                detail: None,
                visibility: None,
            },
        );
    }

    let registry_functions = builtin_functions();
    for name in BuiltinFnName::ALL {
        let spelling = name.as_str();
        let detail = registry_functions.get(name).map_or_else(
            || match (name.complex(), name.aggregation(), name.linear_algebra()) {
                (Some(function), None, None) => {
                    format!("builtin, arity {}", function.arity())
                }
                (None, Some(function), None) => {
                    format!("builtin, arity {}", function.arity())
                }
                (None, None, Some(function)) => {
                    format!("builtin, arity {}", function.arity())
                }
                _ => "builtin".to_string(),
            },
            |function| format!("builtin, arity {}", function.arity()),
        );
        table.insert_definition(
            SymbolKey::BuiltinFunction(*name),
            DefinitionInfo {
                doc: None,
                name: spelling.to_string(),
                category: SymbolCategory::BuiltinFn,
                name_span: Span::new(0, 0),
                decl_span: Span::new(0, 0),
                type_description: None,
                detail: Some(detail),
                visibility: None,
            },
        );
    }
}

fn collect_attribute_refs(
    attributes: &[graphcal_compiler::desugar::desugared_ast::Attribute],
    table: &mut SymbolTable,
    refs: &HirRefCollector<'_>,
) {
    fn collect_argument(
        argument: &AttributeArg,
        table: &mut SymbolTable,
        refs: &HirRefCollector<'_>,
    ) {
        match argument {
            AttributeArg::Path { path } => {
                table.references.push(ReferenceInfo {
                    span: path.span,
                    target: refs.declaration_target(&path.value),
                });
            }
            AttributeArg::Group { elements, .. } => {
                for element in elements {
                    collect_argument(element, table, refs);
                }
            }
            AttributeArg::IndexLabel { .. } | AttributeArg::FinitePosition { .. } => {}
        }
    }

    for attribute in attributes {
        if attribute.name.name.parse::<AttributeName>() == Ok(AttributeName::Assumes) {
            for argument in &attribute.args {
                collect_argument(argument, table, refs);
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
    table.register_definition(
        TopLevelDefinition::Param(ResolvedDeclName::from_def(
            refs.dag_id.clone(),
            p.name.value.clone(),
        )),
        p.name.span,
        decl_span,
        None,
        None,
        visibility,
    );
    collect_type_expr_refs(&p.type_ann, table, refs);
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
    table.register_definition(
        TopLevelDefinition::Node(ResolvedDeclName::from_def(
            refs.dag_id.clone(),
            n.name.value.clone(),
        )),
        n.name.span,
        decl_span,
        None,
        None,
        visibility,
    );
    collect_type_expr_refs(&n.type_ann, table, refs);
    refs.collect_body(&n.value, table);
}

fn collect_const_node_decl(
    c: &graphcal_compiler::desugar::desugared_ast::ConstNodeDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    refs: &mut HirRefCollector<'_>,
) {
    table.register_definition(
        TopLevelDefinition::Const(ResolvedDeclName::from_def(
            refs.dag_id.clone(),
            c.name.value.clone(),
        )),
        c.name.span,
        decl_span,
        None,
        None,
        visibility,
    );
    collect_type_expr_refs(&c.type_ann, table, refs);
    refs.collect_body(&c.value, table);
}

fn collect_base_dim_decl(
    d: &BaseDimDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    refs: &HirRefCollector<'_>,
) {
    table.register_definition(
        TopLevelDefinition::Dimension(ResolvedDimName::from_def(
            refs.dag_id.clone(),
            d.name.value.clone(),
        )),
        d.name.span,
        decl_span,
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
    refs: &HirRefCollector<'_>,
) {
    table.register_definition(
        TopLevelDefinition::Dimension(ResolvedDimName::from_def(
            refs.dag_id.clone(),
            d.name.value.clone(),
        )),
        d.name.span,
        decl_span,
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
    table.register_definition(
        TopLevelDefinition::Unit(ResolvedUnitName::from_def(
            refs.dag_id.clone(),
            u.name.value.clone(),
        )),
        u.name.span,
        decl_span,
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
    refs: &mut HirRefCollector<'_>,
) {
    let type_id = ResolvedStructTypeName::from_def(refs.dag_id.clone(), t.name.value.clone());
    table.register_definition(
        TopLevelDefinition::StructType(type_id.clone()),
        t.name.span,
        decl_span,
        None,
        None,
        visibility,
    );

    let generic_scope = t
        .generic_params
        .iter()
        .map(|param| {
            let key = SymbolKey::GenericParam(GenericParamId::new(
                type_id.clone(),
                param.name.value.clone(),
            ));
            table.insert_definition(
                key.clone(),
                DefinitionInfo {
                    doc: None,
                    name: param.name.value.to_string(),
                    category: SymbolCategory::GenericParam,
                    name_span: param.name.span,
                    decl_span: param.name.span,
                    type_description: Some(generic_constraint_name(param.constraint).to_string()),
                    detail: Some(format!("generic parameter of {}", t.name.value)),
                    visibility: Some(t.visibility),
                },
            );
            (param.name.value.clone(), key)
        })
        .collect::<GenericParamSymbolScope>();

    let mut earlier_params = GenericParamSymbolScope::new();
    for param in &t.generic_params {
        if let Some(default) = &param.default {
            collect_generic_arg_refs(default, Some(&earlier_params), table, refs);
        }
        if let Some(key) = generic_scope.get(&param.name.value) {
            earlier_params.insert(param.name.value.clone(), key.clone());
        }
    }

    // Register constructor and payload-field definitions, then walk payload
    // type annotations (required types have no fields). Constructors are a
    // separate namespace from type names, so they must not reuse the type's
    // `TopLevel` key.
    if let TypeDeclBody::Constructors(members) = &t.body {
        for member in members {
            let constructor_name = member.name.value.to_string();
            let constructor_id =
                ResolvedConstructorName::from_def(refs.dag_id.clone(), member.name.value.clone());
            table.insert_definition(
                SymbolKey::Constructor(constructor_id.clone()),
                DefinitionInfo {
                    doc: None,
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
                        SymbolKey::Field(FieldId::new(
                            constructor_id.clone(),
                            field.name.value.clone(),
                        )),
                        DefinitionInfo {
                            doc: None,
                            name: field.name.value.to_string(),
                            category: SymbolCategory::Field,
                            name_span: field.name.span,
                            decl_span: field.name.span,
                            type_description: None,
                            detail: Some(format!("field of {constructor_name}")),
                            visibility: Some(t.visibility),
                        },
                    );
                    collect_type_expr_refs_in_scope(
                        &field.type_ann,
                        Some(&generic_scope),
                        table,
                        refs,
                    );
                }
            }
        }
    }
}

const fn generic_constraint_name(
    constraint: graphcal_compiler::syntax::ast::GenericConstraint,
) -> &'static str {
    match constraint {
        graphcal_compiler::syntax::ast::GenericConstraint::Dim => "Dim",
        graphcal_compiler::syntax::ast::GenericConstraint::Index => "Index",
        graphcal_compiler::syntax::ast::GenericConstraint::Nat => "Nat",
        graphcal_compiler::syntax::ast::GenericConstraint::Type => "Type",
    }
}

fn collect_index_decl(
    idx: &IndexDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    refs: &mut HirRefCollector<'_>,
) {
    let name = idx.name.value.to_string();
    let index_id = ResolvedIndexName::from_def(refs.dag_id.clone(), idx.name.value.clone());
    table.register_definition(
        TopLevelDefinition::Index(index_id.clone()),
        idx.name.span,
        decl_span,
        None,
        None,
        visibility,
    );
    match &idx.kind {
        IndexDeclKind::Named { variants } => {
            for variant in variants {
                let vname = variant.value.to_string();
                let key = SymbolKey::IndexVariant(IndexVariantId::new(
                    index_id.clone(),
                    variant.value.clone(),
                ));
                table.insert_definition(
                    key,
                    DefinitionInfo {
                        doc: None,
                        name: vname,
                        category: SymbolCategory::IndexVariant,
                        name_span: variant.span,
                        decl_span: variant.span,
                        type_description: None,
                        detail: Some(format!("label/value variant of index {name}")),
                        visibility: Some(visibility),
                    },
                );
            }
        }
        IndexDeclKind::Range { start, end, step } => {
            refs.collect_body(start, table);
            refs.collect_body(end, table);
            refs.collect_body(step, table);
        }
        IndexDeclKind::Linspace { start, end, .. } => {
            refs.collect_body(start, table);
            refs.collect_body(end, table);
        }
        IndexDeclKind::RequiredCoordinate { dimension } => {
            collect_dim_expr_refs(dimension, table);
        }
        IndexDeclKind::RequiredNamed => {}
    }
}

fn collect_assert_decl(
    a: &AssertDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    refs: &mut HirRefCollector<'_>,
) {
    table.register_definition(
        TopLevelDefinition::Assert(ResolvedDeclName::from_def(
            refs.dag_id.clone(),
            a.name.value.clone(),
        )),
        a.name.span,
        decl_span,
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
    table.register_definition(
        TopLevelDefinition::Plot(ResolvedDeclName::from_def(
            refs.dag_id.clone(),
            p.name.value.clone(),
        )),
        p.name.span,
        decl_span,
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
    table.register_definition(
        TopLevelDefinition::Figure(ResolvedDeclName::from_def(
            refs.dag_id.clone(),
            f.name.value.clone(),
        )),
        f.name.span,
        decl_span,
        Some("figure".to_string()),
        Some("figure".to_string()),
        visibility,
    );
    for plot in &f.plot_names {
        table.references.push(ReferenceInfo {
            span: plot.span,
            target: refs.declaration_target(&plot.value.to_name_path()),
        });
    }
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
    table.register_definition(
        TopLevelDefinition::Layer(ResolvedDeclName::from_def(
            refs.dag_id.clone(),
            l.name.value.clone(),
        )),
        l.name.span,
        decl_span,
        Some("layer".to_string()),
        Some("layer".to_string()),
        visibility,
    );
    for plot in &l.plot_names {
        table.references.push(ReferenceInfo {
            span: plot.span,
            target: refs.declaration_target(&plot.value.to_name_path()),
        });
    }
    for field in &l.fields {
        refs.collect_body(&field.value, table);
    }
}

fn collect_dag_decl(
    d: &DagDecl,
    decl_span: Span,
    visibility: BindableVisibility,
    table: &mut SymbolTable,
    refs: &HirRefCollector<'_>,
) {
    table.register_definition(
        TopLevelDefinition::Dag(ResolvedDeclName::from_def(
            refs.dag_id.clone(),
            d.name.value.clone(),
        )),
        d.name.span,
        decl_span,
        None,
        None,
        visibility,
    );
}

fn collect_import_decl(u: &ImportDecl, table: &mut SymbolTable) {
    // Each imported name is a reference; target resolution for cross-file
    // go-to-definition is handled separately.
    collect_import_or_include_names(&u.kind, table);
}

/// Register each extern function of an `import plugin` block as an
/// alias-qualified definition, so completion offers `alias.fn` and
/// go-to-definition lands on the `fn` name inside the block.
fn collect_plugin_import_decl(
    p: &graphcal_compiler::desugar::desugared_ast::PluginImportDecl,
    source: &str,
    table: &mut SymbolTable,
    refs: &mut HirRefCollector<'_>,
) {
    for function in &p.functions {
        // The declared signature text, verbatim, is the best hover/detail
        // rendering: it is already in graphcal vocabulary.
        let signature = source
            .get(function.span.offset()..function.span.offset() + function.span.len())
            .map(|text| text.trim_end_matches(';').trim().to_string());
        table.insert_definition(
            SymbolKey::ExternFunction(ExternFunctionId::new(
                refs.dag_id.clone(),
                p.alias.value.clone(),
                function.name.value.clone(),
            )),
            DefinitionInfo {
                doc: None,
                name: format!("{}::{}", p.alias.value, function.name.value),
                category: SymbolCategory::ExternFn,
                name_span: function.name.span,
                decl_span: function.span,
                type_description: signature,
                detail: Some(format!("extern function (plugin \"{}\")", p.path.value)),
                visibility: None,
            },
        );
        for param in &function.params {
            collect_type_expr_refs(&param.type_ann, table, refs);
        }
        collect_type_expr_refs(&function.result, table, refs);
    }
}

fn collect_include_decl(
    include: &graphcal_compiler::desugar::desugared_ast::IncludeDecl,
    table: &mut SymbolTable,
    refs: &mut HirRefCollector<'_>,
) {
    let target = refs
        .resolver
        .resolve_module_path(refs.dag_id, &include.path)
        .ok();
    if let Some(target) = &target {
        if let Some(parent) = target.parent() {
            table.references.push(ReferenceInfo {
                span: include.path.segments.last().span,
                target: SymbolKey::Declaration(ResolvedDeclName::from_def(
                    parent,
                    DeclName::expect_valid(target.name()),
                ))
                .into(),
            });
        }
        for binding in &include.param_bindings {
            table.references.push(ReferenceInfo {
                span: binding.name.span,
                target: SymbolKey::Declaration(ResolvedDeclName::from_def(
                    target.clone(),
                    DeclName::from_atom(binding.name.name.clone()),
                ))
                .into(),
            });
        }
        if let graphcal_compiler::desugar::desugared_ast::ImportKind::Selective(items) =
            &include.kind
        {
            for item in items {
                table.references.push(ReferenceInfo {
                    span: item.name.span,
                    target: SymbolKey::Declaration(ResolvedDeclName::from_def(
                        target.clone(),
                        DeclName::from_atom(item.name.name.clone()),
                    ))
                    .into(),
                });
                if let Some(alias) = &item.alias {
                    table.references.push(ReferenceInfo {
                        span: alias.span,
                        target: ReferenceTarget::Unresolved(UnresolvedSymbol::Declaration(
                            SourceSymbolPath::local(alias.name.clone()),
                        )),
                    });
                }
            }
        }
    } else {
        collect_import_or_include_names(&include.kind, table);
    }
    for binding in &include.param_bindings {
        refs.collect_body(&binding.value, table);
    }
}

fn collect_import_or_include_names(
    kind: &graphcal_compiler::desugar::desugared_ast::ImportKind,
    table: &mut SymbolTable,
) {
    if let graphcal_compiler::desugar::desugared_ast::ImportKind::Selective(names) = kind {
        for import_item in names {
            let path = SourceSymbolPath::local(import_item.local_name_atom().clone());
            let unresolved = match import_item.namespace {
                graphcal_compiler::syntax::ast::ImportItemNamespace::Term => {
                    UnresolvedSymbol::Term(path)
                }
                graphcal_compiler::syntax::ast::ImportItemNamespace::Type => {
                    UnresolvedSymbol::StructType(path)
                }
                graphcal_compiler::syntax::ast::ImportItemNamespace::Dimension => {
                    UnresolvedSymbol::Dimension(path)
                }
                graphcal_compiler::syntax::ast::ImportItemNamespace::Unit => {
                    UnresolvedSymbol::Unit(path)
                }
                graphcal_compiler::syntax::ast::ImportItemNamespace::Index => {
                    UnresolvedSymbol::Index(path)
                }
            };
            let target = ReferenceTarget::Unresolved(unresolved);
            table.references.push(ReferenceInfo {
                span: import_item.name.span,
                target: target.clone(),
            });
            if let Some(alias) = &import_item.alias {
                table.references.push(ReferenceInfo {
                    span: alias.span,
                    target,
                });
            }
        }
    }
}

/// Register an expression-scoped local variable: build the `SymbolKey`,
/// insert a `LocalVar` definition into the table, and bind the name in the
/// current scope. Used by expression forms such as `Scan` and `Unfold`.
type GenericParamSymbolScope = HashMap<GenericParamName, SymbolKey>;

/// Collect references from a type expression outside a generic declaration.
fn collect_type_expr_refs(
    type_expr: &graphcal_compiler::desugar::desugared_ast::TypeExpr,
    table: &mut SymbolTable,
    refs: &mut HirRefCollector<'_>,
) {
    collect_type_expr_refs_in_scope(type_expr, None, table, refs);
}

fn collect_type_expr_refs_in_scope(
    type_expr: &graphcal_compiler::desugar::desugared_ast::TypeExpr,
    generic_scope: Option<&GenericParamSymbolScope>,
    table: &mut SymbolTable,
    refs: &mut HirRefCollector<'_>,
) {
    match &type_expr.kind {
        TypeExprKind::IndexLabel { .. }
        | TypeExprKind::Dimensionless
        | TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime => {}
        TypeExprKind::DimExpr(dim_expr) => {
            collect_dim_expr_refs_in_scope(
                dim_expr,
                generic_scope,
                UnresolvedSymbol::TypeExpression,
                table,
            );
        }
        TypeExprKind::Indexed { base, indexes } => {
            collect_type_expr_refs_in_scope(base, generic_scope, table, refs);
            for idx in indexes {
                match idx {
                    graphcal_compiler::desugar::desugared_ast::IndexExpr::Name(path) => {
                        table.references.push(ReferenceInfo {
                            span: path.span,
                            target: reference_target_in_generic_scope(
                                &path.value,
                                generic_scope,
                                UnresolvedSymbol::Index,
                            ),
                        });
                    }
                    graphcal_compiler::desugar::desugared_ast::IndexExpr::Finite {
                        cardinality,
                        ..
                    }
                    | graphcal_compiler::desugar::desugared_ast::IndexExpr::BareNat(cardinality) => {
                        collect_nat_generic_param_refs(cardinality, generic_scope, table);
                    }
                }
            }
        }
        TypeExprKind::TypeApplication { name, generic_args } => {
            table.references.push(ReferenceInfo {
                span: name.span,
                target: reference_target_in_generic_scope(
                    &name.value,
                    generic_scope,
                    UnresolvedSymbol::StructType,
                ),
            });
            for arg in generic_args {
                collect_generic_arg_refs(arg, generic_scope, table, refs);
            }
        }
        TypeExprKind::ComplexApplication { generic_args }
        | TypeExprKind::KeyApplication { generic_args } => {
            for arg in generic_args {
                collect_generic_arg_refs(arg, generic_scope, table, refs);
            }
        }
        TypeExprKind::DatetimeApplication { type_args } => {
            // `Datetime` is a built-in; no top-level reference to record.
            // Recurse into args so any user-defined names reachable from the
            // time-scale expression are still picked up by go-to-definition.
            for arg in type_args {
                collect_type_expr_refs_in_scope(arg, generic_scope, table, refs);
            }
        }
    }
    // Collect references from domain constraint bound expressions (e.g., unit names in `100 kg`).
    for bound in &type_expr.constraints {
        refs.collect_body(&bound.value, table);
    }
}

fn collect_generic_arg_refs(
    arg: &graphcal_compiler::syntax::ast::GenericArg<graphcal_compiler::syntax::phase::Desugared>,
    generic_scope: Option<&GenericParamSymbolScope>,
    table: &mut SymbolTable,
    refs: &mut HirRefCollector<'_>,
) {
    match arg {
        graphcal_compiler::syntax::ast::GenericArg::Type(type_expr) => {
            collect_type_expr_refs_in_scope(type_expr, generic_scope, table, refs);
        }
        graphcal_compiler::syntax::ast::GenericArg::Index(index) => match index {
            graphcal_compiler::syntax::ast::IndexExpr::Name(path) => {
                table.references.push(ReferenceInfo {
                    span: path.span,
                    target: reference_target_in_generic_scope(
                        &path.value,
                        generic_scope,
                        UnresolvedSymbol::Index,
                    ),
                });
            }
            graphcal_compiler::syntax::ast::IndexExpr::Finite { cardinality, .. }
            | graphcal_compiler::syntax::ast::IndexExpr::BareNat(cardinality) => {
                collect_nat_generic_param_refs(cardinality, generic_scope, table);
            }
        },
        graphcal_compiler::syntax::ast::GenericArg::Nat(expr) => {
            collect_nat_generic_param_refs(expr, generic_scope, table);
        }
        graphcal_compiler::syntax::ast::GenericArg::Ambiguous(ambiguous) => {
            collect_ambiguous_generic_arg_refs(ambiguous, generic_scope, table);
        }
    }
}

fn collect_ambiguous_generic_arg_refs(
    arg: &graphcal_compiler::syntax::ast::AmbiguousGenericArg,
    generic_scope: Option<&GenericParamSymbolScope>,
    table: &mut SymbolTable,
) {
    match arg {
        graphcal_compiler::syntax::ast::AmbiguousGenericArg::Name(ident) => {
            table.references.push(ReferenceInfo {
                span: ident.span,
                target: generic_param_symbol(&ident.name, generic_scope).map_or_else(
                    || {
                        ReferenceTarget::Unresolved(UnresolvedSymbol::GenericArgument(
                            SourceSymbolPath::local(ident.name.clone()),
                        ))
                    },
                    ReferenceTarget::Resolved,
                ),
            });
        }
        graphcal_compiler::syntax::ast::AmbiguousGenericArg::Mul(operands, _) => {
            for operand in operands {
                collect_ambiguous_generic_arg_refs(operand, generic_scope, table);
            }
        }
    }
}

fn collect_nat_generic_param_refs(
    expr: &graphcal_compiler::syntax::ast::NatExpr,
    generic_scope: Option<&GenericParamSymbolScope>,
    table: &mut SymbolTable,
) {
    match expr {
        graphcal_compiler::syntax::ast::NatExpr::Literal(..) => {}
        graphcal_compiler::syntax::ast::NatExpr::Var(ident) => {
            if let Some(target) = generic_param_symbol(&ident.name, generic_scope) {
                table.references.push(ReferenceInfo {
                    span: ident.span,
                    target: target.into(),
                });
            }
        }
        graphcal_compiler::syntax::ast::NatExpr::Add(operands, _)
        | graphcal_compiler::syntax::ast::NatExpr::Mul(operands, _) => {
            for operand in operands {
                collect_nat_generic_param_refs(operand, generic_scope, table);
            }
        }
    }
}

fn generic_param_symbol(
    name: &NameAtom,
    generic_scope: Option<&GenericParamSymbolScope>,
) -> Option<SymbolKey> {
    generic_scope?
        .get(&GenericParamName::from_atom(name.clone()))
        .cloned()
}

fn reference_target_in_generic_scope(
    path: &NamePath,
    generic_scope: Option<&GenericParamSymbolScope>,
    unresolved: fn(SourceSymbolPath) -> UnresolvedSymbol,
) -> ReferenceTarget {
    if path.qualifier_and_leaf().is_none()
        && let Some(target) = generic_param_symbol(path.leaf(), generic_scope)
    {
        return ReferenceTarget::Resolved(target);
    }
    ReferenceTarget::Unresolved(unresolved(SourceSymbolPath::from_name_path(path)))
}

/// Collect references from a dimension expression.
fn collect_dim_expr_refs(dim_expr: &DimExpr, table: &mut SymbolTable) {
    collect_dim_expr_refs_in_scope(dim_expr, None, UnresolvedSymbol::Dimension, table);
}

fn collect_dim_expr_refs_in_scope(
    dim_expr: &DimExpr,
    generic_scope: Option<&GenericParamSymbolScope>,
    unresolved: fn(SourceSymbolPath) -> UnresolvedSymbol,
    table: &mut SymbolTable,
) {
    for item in &dim_expr.terms {
        table.references.push(ReferenceInfo {
            span: item.term.span,
            target: reference_target_in_generic_scope(
                &item.term.name.value,
                generic_scope,
                unresolved,
            ),
        });
    }
}

/// Collect references from a syntax-layer unit expression.
fn collect_unit_expr_refs(unit_expr: &UnitExpr, table: &mut SymbolTable) {
    for item in &unit_expr.terms {
        let qualifier = item
            .name
            .value
            .qualifier()
            .into_iter()
            .flat_map(|owner| owner.segments().iter().cloned())
            .collect();
        let path =
            SourceSymbolPath::module_member(qualifier, item.name.value.name().atom().clone());
        table.references.push(ReferenceInfo {
            span: item.name.span,
            target: ReferenceTarget::Unresolved(UnresolvedSymbol::Unit(path)),
        });
    }
}

/// Format a domain bound expression as a human-readable string.
///
/// Handles the common cases: number literals, quantity literals, and negated forms.
fn format_bound_expr(expr: &hir::Expr) -> String {
    match &expr.kind {
        hir::ExprKind::Number(value) => format_number(*value),
        hir::ExprKind::Integer(value) => value.to_string(),
        hir::ExprKind::QuantityLiteral { value, unit } => {
            let number = format_number(*value);
            let unit = format_unit_terms_with_config(
                unit.terms
                    .iter()
                    .map(|item| (item.op, item.name.value.to_string(), item.power)),
                true,
            );
            format!("{number} {unit}")
        }
        hir::ExprKind::UnaryOp {
            op: graphcal_compiler::syntax::ast::UnaryOp::Neg,
            operand,
        } => format!("-{}", format_bound_expr(operand)),
        // For complex expressions, fall back to "..."
        _ => "...".to_string(),
    }
}

/// Format a constraint clause from domain bounds.
///
/// Returns a string like `(min: 100 kg, max: 2000 kg)` or an empty string if no constraints.
fn format_constraints(constraints: &[ResolvedDomainBound]) -> String {
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
    constraints: &[ResolvedDomainBound],
    registry: &SemanticRegistry,
) -> String {
    let constraint_str = format_constraints(constraints);
    if let ResolvedTypeExpr::Indexed { base, indexes } = resolved {
        let base_str = base.format(registry);
        let idx_strs: Vec<String> = indexes
            .iter()
            .map(|i| match i {
                ResolvedIndex::Concrete(name, _) => name.as_str().to_string(),
                ResolvedIndex::GenericParam(name, _) => name.to_string(),
                ResolvedIndex::Finite(form, _) => format!("Fin({})", form.format()),
            })
            .collect();
        format!("{base_str}{constraint_str}[{}]", idx_strs.join(", "))
    } else {
        let type_str = resolved.format(registry);
        format!("{type_str}{constraint_str}")
    }
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
    let registry = tir.registry();

    if let Some(dag) = tir.dag_registry().get(dag_id) {
        // Enrich param/node/const declarations with resolved types + constraints.
        for (name, resolved_type) in dag.resolved_decl_types() {
            let Some(identity) = dag.bound_decl_identity(name) else {
                continue;
            };
            let key = SymbolKey::Declaration(identity.clone());
            if let Some(def) = table.definitions.get_mut(&key) {
                let type_desc = dag.semantic().domain_bounds.get(identity).map_or_else(
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
        match (key, category) {
            (SymbolKey::Dimension(resolved), SymbolCategory::Dimension) => {
                let name = resolved.as_str();
                if let Some(dim) = tir.dimension(resolved)
                    && let Some(def_mut) = table.definitions.get_mut(key)
                {
                    def_mut.type_description = Some(format!(
                        "dim {name} = {}",
                        registry.dimensions.format_dimension(dim)
                    ));
                }
            }
            (SymbolKey::Unit(resolved), SymbolCategory::Unit) => {
                if let Some(unit_info) = tir.unit_info(resolved)
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
            (SymbolKey::Index(resolved), SymbolCategory::Index) => {
                if let Some(idx_def) = tir.declared_index_def(resolved)
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
                        IndexKind::Coordinate(data) => {
                            def_mut.type_description = Some(match data.spacing {
                                graphcal_compiler::registry::types::CoordinateSpacing::Step {
                                    step,
                                } => format!("range({}, {}, step: {step})", data.start, data.end),
                                graphcal_compiler::registry::types::CoordinateSpacing::Linspace => {
                                    format!(
                                        "linspace({}, {}, points: {})",
                                        data.start,
                                        data.end,
                                        idx_def.cardinality()
                                    )
                                }
                            });
                        }
                        IndexKind::RequiredNamed => {
                            def_mut.type_description = Some("(required)".to_string());
                        }
                        IndexKind::RequiredCoordinate { dimension } => {
                            def_mut.type_description = Some(format!(
                                "(required, dim: {})",
                                registry.dimensions.format_dimension(dimension)
                            ));
                        }
                        IndexKind::Finite { cardinality } => {
                            def_mut.type_description = Some(format!("Fin({})", cardinality.get()));
                        }
                    }
                }
            }
            (SymbolKey::StructType(resolved), SymbolCategory::StructType) => {
                if let Some(type_def) = tir.struct_type_def(resolved)
                    && let Some(def_mut) = table.definitions.get_mut(key)
                {
                    let desc = type_def.union_members().map_or_else(
                        || format!("{} (required)", type_def.name()),
                        |members| match type_def.record_fields() {
                            Some([]) => type_def.name().to_string(),
                            Some(fields) => {
                                let field_descs: Vec<String> = fields
                                    .iter()
                                    .map(|field| field.name().to_string())
                                    .collect();
                                format!("{} {{ {} }}", type_def.name(), field_descs.join(", "))
                            }
                            None => {
                                // Union type: show members separated by |
                                let member_descs: Vec<String> = members
                                    .iter()
                                    .map(|member| member.name().to_string())
                                    .collect();
                                member_descs.join(" | ")
                            }
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
    for (_, type_def) in tir
        .nominal_type_defs()
        .filter(|(identity, _)| identity.owner() == dag_id)
    {
        let Some(members) = type_def.union_members() else {
            continue;
        };
        for member in members {
            for field in member.fields() {
                let constructor = ResolvedConstructorName::from_def(
                    dag_id.clone(),
                    ConstructorName::expect_valid(member.name().to_string()),
                );
                let field_key = SymbolKey::Field(FieldId::new(
                    constructor,
                    FieldName::expect_valid(field.name().to_string()),
                ));
                if !table.definitions.contains_key(&field_key) {
                    table.insert_definition(
                        field_key,
                        DefinitionInfo {
                            doc: None,
                            name: field.name().to_string(),
                            category: SymbolCategory::Field,
                            name_span: Span::new(0, 0),
                            decl_span: Span::new(0, 0),
                            type_description: None,
                            detail: Some(format!("field of {}.{}", type_def.name(), member.name())),
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

    fn definition_key(table: &SymbolTable, category: SymbolCategory, name: &str) -> SymbolKey {
        table
            .definitions
            .iter()
            .find_map(|(key, definition)| {
                (definition.category == category && definition.name == name).then(|| key.clone())
            })
            .unwrap_or_else(|| panic!("missing {category:?} definition `{name}`"))
    }

    fn reference_key(table: &SymbolTable, offset: usize) -> Option<SymbolKey> {
        table
            .find_reference_at(offset)
            .and_then(|reference| table.resolve_local_target(&reference.target))
    }

    #[test]
    fn duplicate_semantic_identity_is_rejected_without_replacement() {
        let source = "const node x: Dimensionless = 1.0;\n\
                      const node x: Dimensionless = 2.0;";
        let table = table_for(source);
        let definitions = table
            .definitions
            .values()
            .filter(|definition| definition.name == "x")
            .collect::<Vec<_>>();

        assert_eq!(definitions.len(), 1);
        assert_eq!(table.definition_conflicts().len(), 1);
        assert_eq!(
            table.definition_conflicts()[0].first_span,
            definitions[0].name_span
        );
    }

    #[test]
    fn build_symbol_table_basic() {
        let source = "param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x + 1.0;";
        let raw_file = graphcal_compiler::syntax::parser::Parser::with_name(source, "test.gcl")
            .parse_file()
            .unwrap();
        let desugared = graphcal_compiler::syntax::desugar::desugar_multi_decls_in_file(raw_file);
        let file = desugared;
        let table = build_for_buffer(&file, source);

        let x_key = definition_key(&table, SymbolCategory::Param, "x");
        let y_key = definition_key(&table, SymbolCategory::Node, "y");
        assert!(table.definitions.contains_key(&x_key));
        assert!(table.definitions.contains_key(&y_key));
        assert_eq!(table.definitions[&x_key].category, SymbolCategory::Param);
        assert_eq!(table.definitions[&y_key].category, SymbolCategory::Node);

        // @x is a reference
        assert!(
            table
                .references
                .iter()
                .any(|reference| table.resolve_local_target(&reference.target)
                    == Some(x_key.clone())),
            "expected @x reference"
        );
    }

    #[test]
    fn indexes_grouped_attributes_index_bounds_and_compound_constraints() {
        let source = r"
assert first = true;
assert second = true;
#[assumes((first, second))]
node guarded: Dimensionless = 1.0;
const node START: Dimensionless = 0.0;
const node END: Dimensionless = 10.0;
const node STEP: Dimensionless = 1.0;
index Axis = range(@START, @END, step: @STEP);
const node LOWER: Dimensionless = 0.0;
param bounded: Dimensionless(min: @LOWER + 1.0) = 2.0;
";
        let table = table_for(source);

        for spelling in ["first, second", "@START", "@END", "@STEP", "@LOWER"] {
            let offset = source.find(spelling).unwrap() + usize::from(spelling != "first, second");
            assert!(
                reference_key(&table, offset).is_some(),
                "missing reference at `{spelling}`"
            );
        }
        let second = source.find("first, second").unwrap() + "first, ".len();
        assert!(reference_key(&table, second).is_some());
    }

    #[test]
    fn field_accesses_use_the_owning_constructor_when_spellings_collide() {
        let source = r"
type Item { Item(mass: Dimensionless), }
type Other { Other(mass: Dimensionless), }
param item: Item = Item(mass: 1.0);
param other: Other = Other(mass: 2.0);
node item_mass: Dimensionless = @item.mass;
node other_mass: Dimensionless = @other.mass;
";
        let table = table_for(source);
        let item_access = source.find("@item.mass").unwrap() + "@item.".len();
        let other_access = source.find("@other.mass").unwrap() + "@other.".len();
        let item_key = reference_key(&table, item_access)
            .unwrap_or_else(|| panic!("missing item field in {:#?}", table.references));
        let other_key = reference_key(&table, other_access)
            .unwrap_or_else(|| panic!("missing other field in {:#?}", table.references));

        assert_ne!(item_key, other_key);
        assert!(matches!(item_key, SymbolKey::Field(_)));
        assert!(matches!(other_key, SymbolKey::Field(_)));
        assert_eq!(table.find_all_references(&item_key).len(), 2);
        assert_eq!(table.find_all_references(&other_key).len(), 2);
    }

    #[test]
    fn recursively_indexes_dag_include_and_composition_references() {
        let source = r"
param outer: Dimensionless = 2.0;
dag d {
    param inner: Dimensionless;
    node doubled: Dimensionless = @inner * 2.0;
}
include d(inner: @outer)::{ doubled as result };
plot p = { mark: point, encode: { x: @outer } };
figure f = { plots: [p] };
";
        let table = table_for(source);
        let outer = definition_key(&table, SymbolCategory::Param, "outer");
        let inner = definition_key(&table, SymbolCategory::Param, "inner");
        let plot = definition_key(&table, SymbolCategory::Plot, "p");

        assert_eq!(table.find_all_references(&outer).len(), 2);
        assert_eq!(table.find_all_references(&inner).len(), 2);
        assert_eq!(table.find_all_references(&plot).len(), 1);
        for (spelling, relative_offset) in [
            ("@inner", 1),
            ("inner: @outer", 0),
            ("@outer", 1),
            ("plots: [p]", "plots: [".len()),
        ] {
            let offset = source.find(spelling).unwrap() + relative_offset;
            assert!(
                reference_key(&table, offset).is_some(),
                "missing reference at `{spelling}`"
            );
        }
    }

    #[test]
    fn unfold_axis_and_three_locals_are_navigable() {
        let source = r"
index Step = range(0.0 s, 1.0 s, step: 1.0 s);
node y: Dimensionless[Step] = unfold(
    Step,
    0.0,
    |prev_y, prev_t, t| prev_y + (t - prev_t) / 1.0 s
);
";
        let raw_file = graphcal_compiler::syntax::parser::Parser::with_name(source, "test.gcl")
            .parse_file()
            .unwrap();
        let file = graphcal_compiler::syntax::desugar::desugar_multi_decls_in_file(raw_file);
        let table = build_for_buffer(&file, source);

        let axis_offset = source.find("unfold(\n    Step").unwrap() + "unfold(\n    ".len();
        assert_eq!(
            reference_key(&table, axis_offset),
            Some(definition_key(&table, SymbolCategory::Index, "Step"))
        );
        for local in ["prev_y", "prev_t", "t"] {
            let key = table
                .definitions
                .iter()
                .find_map(|(key, definition)| {
                    (definition.category == SymbolCategory::LocalVar && definition.name == local)
                        .then_some(key)
                })
                .unwrap_or_else(|| panic!("missing unfold local `{local}`"));
            assert!(
                !table.find_all_references(key).is_empty(),
                "missing body reference for unfold local `{local}`"
            );
        }
    }

    #[test]
    fn generic_parameter_references_are_scoped_to_their_type() {
        let source = r"
index Component = { A };
type Sized<N: Nat, I: Index = Component, M: Nat = N + 1> {
    Sized(values: Dimensionless[I, N]),
}
";
        let raw_file = graphcal_compiler::syntax::parser::Parser::with_name(source, "test.gcl")
            .parse_file()
            .unwrap();
        let file = graphcal_compiler::syntax::desugar::desugar_multi_decls_in_file(raw_file);
        let table = build_for_buffer(&file, source);
        let n_key = definition_key(&table, SymbolCategory::GenericParam, "N");
        let i_key = definition_key(&table, SymbolCategory::GenericParam, "I");

        assert_eq!(
            table.definitions[&n_key].category,
            SymbolCategory::GenericParam
        );
        assert_eq!(table.find_all_references(&n_key).len(), 2);
        assert_eq!(table.find_all_references(&i_key).len(), 1);
        assert_eq!(
            table
                .find_all_references(&definition_key(&table, SymbolCategory::Index, "Component",))
                .len(),
            1
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

        let p_key = definition_key(&table, SymbolCategory::Param, "p");
        let q_key = definition_key(&table, SymbolCategory::Param, "q");
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
            table.resolve_local_target(&reference.unwrap().target),
            Some(definition_key(&table, SymbolCategory::Param, "x"))
        );
    }
    #[test]
    fn same_spelling_in_distinct_namespaces_has_distinct_identity() {
        let source = "pub const node scale: Dimensionless = 2.0;\n\
                      pub const unit scale: Length = 1.0 m;\n\
                      node value: Length = @scale * 1.0 scale;";
        let table = table_for(source);
        let constant = definition_key(&table, SymbolCategory::Const, "scale");
        let unit = definition_key(&table, SymbolCategory::Unit, "scale");

        assert_ne!(constant, unit);
        assert!(matches!(constant, SymbolKey::Declaration(_)));
        assert!(matches!(unit, SymbolKey::Unit(_)));
        assert_eq!(table.find_all_references(&constant).len(), 1);
        assert_eq!(table.find_all_references(&unit).len(), 1);
    }
    // --- Inline DAG invocation LSP coverage (issue #451) ---

    /// Issues #827/#828 repro: a table literal over a named index plus a
    /// qualified `Index#Variant` access.
    const TABLE_SOURCE: &str = "\
pub index Maneuver = { Departure, Correction };
param dv: Velocity[Maneuver] = table[Maneuver] {
    Departure: 2.0 km/s;
    Correction: 0.1 km/s;
};
node total: Velocity = @dv[Maneuver#Departure];
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

    #[test]
    fn unresolved_call_retains_resolved_and_unresolved_argument_references() {
        let source = "param known: Dimensionless = 1.0;\n\
                      node out: Dimensionless = mystery(@known + also_missing);";
        let table = table_for(source);

        let known = definition_key(&table, SymbolCategory::Param, "known");
        for spelling in ["mystery", "@known", "also_missing"] {
            let offset = source.find(spelling).unwrap() + usize::from(spelling.starts_with('@'));
            let reference = table
                .find_reference_at(offset)
                .unwrap_or_else(|| panic!("missing retained reference at `{spelling}`"));
            if spelling == "@known" {
                assert_eq!(
                    table.resolve_local_target(&reference.target),
                    Some(known.clone())
                );
            } else {
                assert!(matches!(reference.target, ReferenceTarget::Unresolved(_)));
            }
        }
    }

    /// Issue #827: row-key reference spans must cover exactly the variant
    /// label, not a multi-line merge starting at the table-axis token.
    #[test]
    fn table_row_key_references_are_label_precise() {
        let source = TABLE_SOURCE;
        let table = table_for(source);
        let departure_key = definition_key(&table, SymbolCategory::IndexVariant, "Departure");
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
                table.resolve_local_target(&reference.target),
                Some(definition_key(
                    &table,
                    SymbolCategory::IndexVariant,
                    variant,
                )),
                "row key must resolve to its own variant"
            );
        }
    }

    /// The table-axis token inside `table[...]` and the index segment of a
    /// qualified `Index#Variant` path resolve to the index declaration.
    #[test]
    fn index_segments_resolve_to_index_declaration() {
        let source = TABLE_SOURCE;
        let table = table_for(source);
        let axis_offset = source.find("table[Maneuver]").unwrap() + "table[".len();
        let qualifier_offset = source.find("@dv[Maneuver#Departure]").unwrap() + "@dv[".len();
        for offset in [axis_offset, qualifier_offset] {
            let reference = table
                .find_reference_at(offset)
                .expect("expected index reference at index segment");
            assert_eq!(
                table.resolve_local_target(&reference.target),
                Some(definition_key(&table, SymbolCategory::Index, "Maneuver"))
            );
        }
    }

    /// Issue #828: the reference recorded for `Maneuver#Departure` in an
    /// index access must address only the `Departure` segment so rename
    /// rewrites `@dv[Maneuver#Departure]` into `@dv[Maneuver#Begin]`.
    #[test]
    fn qualified_variant_reference_is_segment_precise() {
        let source = TABLE_SOURCE;
        let table = table_for(source);
        let departure_offset = source.find("Maneuver#Departure").unwrap() + "Maneuver.".len();
        let reference = table
            .find_reference_at(departure_offset)
            .expect("expected variant reference at `Departure` segment");
        assert_eq!(
            table.resolve_local_target(&reference.target),
            Some(definition_key(
                &table,
                SymbolCategory::IndexVariant,
                "Departure",
            ))
        );
        assert_eq!(slice(source, reference.span), "Departure");
    }

    /// Variant literals (`Season#Winter` in expression position) and match
    /// patterns get the same segment-precise spans.
    #[test]
    fn variant_literal_and_match_pattern_references_are_segment_precise() {
        let source = "\
index Season = { Winter, Summer };
node pick: Season = Season#Winter;
node out: Dimensionless = match @pick { Season#Winter => 1.0, Season#Summer => 2.0 };
";
        let table = table_for(source);
        let winter_key = definition_key(&table, SymbolCategory::IndexVariant, "Winter");
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
