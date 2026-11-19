//! HIR reference collection: expression references and lexical locals from tolerantly lowered bodies.

use std::collections::HashMap;

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::hir;
use graphcal_compiler::resolve::ModuleResolver;
use graphcal_compiler::resolve::error::ModuleResolveError;
use graphcal_compiler::resolve::symbols::SymbolRef;
use graphcal_compiler::resolve::tables::NamespaceTables;
use graphcal_compiler::resolved_name::ResolvedName;
use graphcal_compiler::syntax::index_name::IndexVariantName;
use graphcal_compiler::syntax::names::{NameAtom, NameDef, NamePath};
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::syntax::span::Spanned;

use graphcal_compiler::builtin::BuiltinFn;
use graphcal_compiler::semantic::time_zone::TimeZoneRegistry;

use crate::nominal_type_index::NominalTypeIndex;
use crate::symbol_identity::{
    ExternFunctionId, FieldId, IndexVariantId, LocalSymbolId, ReferenceTarget, SourceSymbolPath,
    SymbolId, UnresolvedSymbol,
};

use super::index::{
    DefinitionInfo, ReferenceInfo, SymbolCategory, SymbolTable, dag_declaration_name,
};

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
pub(super) struct HirRefCollector<'a> {
    pub(super) dag_id: &'a DagId,
    pub(super) resolver: &'a ModuleResolver,
    nominal_types: &'a NominalTypeIndex,
    time_zones: TimeZoneRegistry,
    /// Lexical local definitions of the current body, keyed by HIR identity.
    locals: HashMap<hir::expr::LocalId, SymbolId>,
    /// Span of the expression currently being lowered. Together with the DAG
    /// owner it scopes HIR-local IDs, which are only body-local identities.
    body_span: Span,
}

impl<'a> HirRefCollector<'a> {
    pub(super) fn new(
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

    /// The identity of the current module's own declaration `name`.
    pub(super) fn declared<Ns: NamespaceTables>(
        &self,
        name: &NameDef<Ns>,
    ) -> Option<ResolvedName<Ns>> {
        self.resolver
            .declaration(self.dag_id, name)
            .ok()
            .map(SymbolRef::into_resolved)
    }

    pub(super) fn declaration_target(&self, path: &NamePath) -> ReferenceTarget {
        self.resolver
            .resolve_decl_path(self.dag_id, path)
            .map(graphcal_compiler::resolve::symbols::SymbolRef::into_resolved)
            .map_or_else(
                |_| {
                    ReferenceTarget::Unresolved(UnresolvedSymbol::Declaration(
                        SourceSymbolPath::from_name_path(path),
                    ))
                },
                |name| SymbolId::Declaration(name).into(),
            )
    }

    fn collect_resolved_unit_expr_refs(
        unit: &hir::expr::ResolvedUnitExpr<hir::expr::ResolvedUnitRef>,
        table: &mut SymbolTable,
    ) {
        for item in &unit.terms {
            table.references.push(ReferenceInfo {
                span: item.name.span,
                target: SymbolId::Unit(item.name.value.resolved().clone()).into(),
            });
        }
    }

    /// Record an attribute's `Index#Label` (`#[expected_fail(Mode#Boost)]`):
    /// the index path and, when it names a variant of that index, the label.
    pub(super) fn attribute_label(
        &self,
        index: &Spanned<NamePath>,
        label: &Spanned<IndexVariantName>,
        table: &mut SymbolTable,
    ) {
        match self
            .resolver
            .resolve_index_variant_parts(self.dag_id, &index.value, &label.value)
        {
            Ok(variant) => {
                Self::reference(table, label.span, Self::variant_key(&variant));
                Self::reference(table, index.span, SymbolId::Index(variant.index().clone()));
            }
            Err(_) => table.references.push(ReferenceInfo {
                span: index.span,
                target: ReferenceTarget::Unresolved(UnresolvedSymbol::Index(
                    SourceSymbolPath::from_name_path(&index.value),
                )),
            }),
        }
    }

    fn variant_key(variant: &graphcal_compiler::resolved_name::ResolvedIndexVariant) -> SymbolId {
        SymbolId::IndexVariant(IndexVariantId::from_resolved(variant))
    }

    /// Lower one declaration body and record its references.
    pub(super) fn collect_body(
        &mut self,
        expr: &graphcal_compiler::desugar::desugared_ast::Expr,
        table: &mut SymbolTable,
    ) {
        self.collect_lowered(expr, table, BodyHoles::Rejected);
    }

    /// Collect a constant or node formula, which may contain `todo<T> { … }`
    /// holes.
    pub(super) fn collect_formula_body(
        &mut self,
        expr: &graphcal_compiler::desugar::desugared_ast::Expr,
        table: &mut SymbolTable,
    ) {
        self.collect_lowered(expr, table, BodyHoles::Allowed);
    }

    fn collect_lowered(
        &mut self,
        expr: &graphcal_compiler::desugar::desugared_ast::Expr,
        table: &mut SymbolTable,
        holes: BodyHoles,
    ) {
        let generic_scope = hir::lower::GenericScope::new();
        let ctx = hir::ExprLoweringContext::new(
            hir::lower::ModuleScope::new(self.dag_id, self.resolver, &generic_scope),
            &self.time_zones,
        );
        let ctx = match holes {
            BodyHoles::Rejected => ctx,
            BodyHoles::Allowed => ctx.allowing_todo_holes(),
        };
        let lowered = hir::lower_expr_tolerant(expr, ctx);
        self.locals.clear();
        self.body_span = expr.span;
        self.walk(&lowered, table);
        for diagnostic in lowered.diagnostics() {
            Self::record_unresolved(diagnostic, table);
        }
    }

    /// Record a reference for an unresolved name from a lowering diagnostic,
    /// keyed by its written spelling so IDE features answer on broken code.
    fn record_unresolved(err: &hir::ExprLowerError, table: &mut SymbolTable) {
        let (target, span) = match err {
            hir::ExprLowerError::UnknownGraphRef { name, span } => {
                // A name under an anonymous include scope has no written spelling.
                let Some(qualifier) = name
                    .qualifier()
                    .iter()
                    .map(|segment| segment.alias().map(|alias| alias.atom().clone()))
                    .collect::<Option<Vec<_>>>()
                else {
                    return;
                };
                let path = SourceSymbolPath::module_member(qualifier, name.leaf().atom().clone());
                (UnresolvedSymbol::Declaration(path), *span)
            }
            hir::ExprLowerError::UnknownLocalRef { name, span } => (
                UnresolvedSymbol::Declaration(SourceSymbolPath::local(name.atom().clone())),
                *span,
            ),
            hir::ExprLowerError::UnknownUnit { name, span } => {
                let qualifier = name.qualifier().to_vec();
                let path = SourceSymbolPath::module_member(qualifier, name.leaf().atom().clone());
                (UnresolvedSymbol::Unit(path), *span)
            }
            hir::ExprLowerError::UnknownFunction { path, span } => {
                let Some(name) = path.as_bare() else {
                    return;
                };
                (
                    UnresolvedSymbol::Function(SourceSymbolPath::local(name.clone())),
                    *span,
                )
            }
            // A removed built-in (#2246) resolves to nothing either; it only
            // has a dedicated diagnostic.
            hir::ExprLowerError::RemovedBuiltinFunction { function, span } => {
                let Ok(name) = NameAtom::parse(function.as_str()) else {
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
            } => (
                UnresolvedSymbol::Declaration(SourceSymbolPath::local(name.clone())),
                *span,
            ),
            _ => return,
        };
        table.references.push(ReferenceInfo {
            span,
            target: ReferenceTarget::Unresolved(target),
        });
    }

    /// Register an expression-scoped local variable: build its `SymbolKey`,
    /// insert a `LocalVar` definition into the table, and record the key for
    /// the local's later references. Used by expression forms such as `for`,
    /// `scan`, and `unfold`.
    fn define_local(
        &mut self,
        local: &hir::expr::LocalDef,
        detail: String,
        table: &mut SymbolTable,
    ) {
        let key = SymbolId::Local(LocalSymbolId::new(
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

    fn reference(table: &mut SymbolTable, span: Span, target: SymbolId) {
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
        let key = SymbolId::Index(index.clone());
        for index_span in variant.index_spans() {
            Self::reference(table, index_span, key.clone());
        }
    }

    fn walk(&mut self, expr: &hir::expr::Expr<hir::Tolerant>, table: &mut SymbolTable) {
        graphcal_compiler::stack::with_stack_growth(|| self.walk_inner(expr, table));
    }

    #[expect(
        clippy::too_many_lines,
        reason = "reference extraction handles every HIR ExprKind variant"
    )]
    fn walk_inner(&mut self, expr: &hir::expr::Expr<hir::Tolerant>, table: &mut SymbolTable) {
        match expr.kind() {
            hir::expr::ExprKind::Error(failure) => {
                for child in failure.children() {
                    self.walk(child, table);
                }
            }
            hir::expr::ExprKind::Number(_)
            | hir::expr::ExprKind::Integer(_)
            | hir::expr::ExprKind::Bool(_)
            | hir::expr::ExprKind::StringLiteral(_)
            | hir::expr::ExprKind::OffsetDateTimeLiteral(_)
            | hir::expr::ExprKind::CivilDateTimeLiteral(_)
            | hir::expr::ExprKind::EpochLiteral(_)
            | hir::expr::ExprKind::ZonedDateTimeLiteral(_)
            | hir::expr::ExprKind::IanaTimeZoneLiteral(_) => {}
            hir::expr::ExprKind::GraphRef(target) => {
                Self::reference(
                    table,
                    target.span,
                    SymbolId::Declaration(target.value.clone()),
                );
            }
            hir::expr::ExprKind::TodoHole(hole) => {
                for entry in &hole.interface.value {
                    self.walk(entry, table);
                }
            }
            hir::expr::ExprKind::ConstRef(const_ref) => {
                let target = match &const_ref.value {
                    hir::expr::ConstRef::Decl(name) => SymbolId::Declaration(name.clone()),
                    hir::expr::ConstRef::Constructor(name) => SymbolId::Constructor(name.clone()),
                    hir::expr::ConstRef::Builtin(builtin) => SymbolId::BuiltinConstant(*builtin),
                };
                Self::reference(table, const_ref.span, target);
            }
            hir::expr::ExprKind::LocalRef(local) => {
                if let Some(key) = self.locals.get(&local.value) {
                    Self::reference(table, local.span, key.clone());
                }
            }
            hir::expr::ExprKind::VariantLiteral(variant) => {
                Self::variant_reference(variant, table);
            }
            hir::expr::ExprKind::BinOp { lhs, rhs, .. } => {
                self.walk(lhs, table);
                self.walk(rhs, table);
            }
            hir::expr::ExprKind::UnaryOp { operand, .. }
            | hir::expr::ExprKind::DisplayTimezone { expr: operand, .. } => {
                self.walk(operand, table);
            }
            hir::expr::ExprKind::FieldAccess { expr, field } => {
                self.walk(expr, table);
                let target = self.nominal_types.expression_constructor(expr).map_or_else(
                    || ReferenceTarget::Unresolved(UnresolvedSymbol::Field(field.value.clone())),
                    |constructor| {
                        SymbolId::Field(FieldId::new(constructor, field.value.clone())).into()
                    },
                );
                table.references.push(ReferenceInfo {
                    span: field.span,
                    target,
                });
            }
            hir::expr::ExprKind::FnCall { callee, args } => {
                match &callee.value {
                    hir::expr::FunctionRef::Builtin(builtin) => {
                        Self::reference(
                            table,
                            callee.span,
                            SymbolId::BuiltinFunction(builtin.function()),
                        );
                    }
                    hir::expr::FunctionRef::Epoch { scale } => {
                        Self::reference(
                            table,
                            callee.span,
                            SymbolId::BuiltinFunction(BuiltinFn::EPOCH),
                        );
                        Self::reference(table, scale.span, SymbolId::TimeScale(scale.value));
                    }
                    hir::expr::FunctionRef::External(ext) => Self::reference(
                        table,
                        callee.span,
                        SymbolId::ExternFunction(ExternFunctionId::new(
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
            hir::expr::ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.walk(condition, table);
                self.walk(then_branch, table);
                self.walk(else_branch, table);
            }
            hir::expr::ExprKind::QuantityLiteral { unit, .. } => {
                Self::collect_resolved_unit_expr_refs(unit, table);
            }
            hir::expr::ExprKind::Convert {
                expr: inner,
                target,
            } => {
                self.walk(inner, table);
                Self::collect_resolved_unit_expr_refs(target, table);
            }
            hir::expr::ExprKind::ConstructorCall {
                callee,
                generic_args,
                fields,
            } => {
                let constructor = callee.value.clone();
                Self::reference(
                    table,
                    callee.span,
                    SymbolId::Constructor(constructor.clone()),
                );
                for generic_arg in generic_args {
                    self.walk_generic_arg(generic_arg, table);
                }
                for field in fields {
                    Self::reference(
                        table,
                        field.name.span,
                        SymbolId::Field(FieldId::new(
                            constructor.clone(),
                            field.name.value.clone(),
                        )),
                    );
                    self.walk(&field.value, table);
                }
            }
            hir::expr::ExprKind::MapLiteral { entries } => {
                for entry in entries {
                    for key in &entry.keys {
                        if let hir::expr::MapEntryKey::IndexVariant(variant) = key {
                            Self::variant_reference(variant, table);
                        }
                    }
                    self.walk(&entry.value, table);
                }
            }
            hir::expr::ExprKind::ForComp { bindings, body } => {
                for binding in bindings {
                    let detail = match &binding.index {
                        hir::expr::ForBindingIndex::Named(index) => {
                            Self::reference(
                                table,
                                index.span,
                                SymbolId::Index(index.value.clone()),
                            );
                            format!("loop variable over {}", index.value.as_str())
                        }
                        hir::expr::ForBindingIndex::Finite { cardinality, .. } => {
                            format!("loop variable over Fin({})", nat_expr_label(cardinality))
                        }
                    };
                    self.define_local(&binding.local, detail, table);
                }
                self.walk(body, table);
            }
            hir::expr::ExprKind::IndexAccess { expr: inner, args } => {
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
            hir::expr::ExprKind::Scan {
                source,
                init,
                acc,
                val,
                body,
            } => {
                self.walk(source, table);
                self.walk(init, table);
                self.define_local(acc, "scan accumulator".into(), table);
                self.define_local(val, "scan value".into(), table);
                self.walk(body, table);
            }
            hir::expr::ExprKind::Unfold {
                recurrence,
                init,
                body,
            } => {
                let axis = &recurrence.axis;
                Self::reference(table, axis.span, SymbolId::Index(axis.value.clone()));
                self.walk(init, table);
                self.define_local(
                    &recurrence.previous_state,
                    "unfold previous state".into(),
                    table,
                );
                self.define_local(
                    &recurrence.previous_index,
                    "unfold previous index".into(),
                    table,
                );
                self.define_local(
                    &recurrence.current_index,
                    "unfold current index".into(),
                    table,
                );
                self.walk(body, table);
            }
            hir::expr::ExprKind::KeyForm { axis, arg, .. } => {
                if let hir::expr::ForBindingIndex::Named(axis) = axis {
                    Self::reference(table, axis.span, SymbolId::Index(axis.value.clone()));
                }
                self.walk(arg, table);
            }
            hir::expr::ExprKind::Match { scrutinee, arms } => {
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
                                SymbolId::Constructor(constructor_id.clone()),
                            );
                            for binding in bindings {
                                match binding {
                                    hir::expr::PatternBinding::Bind { field, local } => {
                                        Self::reference(
                                            table,
                                            field.span,
                                            SymbolId::Field(FieldId::new(
                                                constructor_id.clone(),
                                                field.value.clone(),
                                            )),
                                        );
                                        self.define_local(
                                            local,
                                            format!("bound from {}", constructor.value),
                                            table,
                                        );
                                    }
                                    hir::expr::PatternBinding::Wildcard { field, .. } => {
                                        Self::reference(
                                            table,
                                            field.span,
                                            SymbolId::Field(FieldId::new(
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
            hir::expr::ExprKind::DagCall {
                target,
                args,
                output,
                ..
            } => {
                if let Some(declaration) = dag_declaration_name(&target.value) {
                    Self::reference(table, target.span, SymbolId::Declaration(declaration));
                }
                Self::reference(
                    table,
                    output.span,
                    SymbolId::Declaration(output.value.clone()),
                );
                for arg in args {
                    Self::reference(
                        table,
                        arg.target.span,
                        SymbolId::Declaration(arg.target.value.clone()),
                    );
                    self.walk(&arg.value, table);
                }
            }
        }
    }

    fn walk_type(&self, value_type: &hir::types::ValueType, table: &mut SymbolTable) {
        match &value_type.kind {
            hir::types::ValueTypeKind::Builtin(_)
            | hir::types::ValueTypeKind::GenericTypeParam(_) => {}
            hir::types::ValueTypeKind::Complex(dimension) => {
                if let hir::types::DimArg::Expr(dim_expr) = dimension {
                    for item in &dim_expr.terms {
                        if let hir::types::DimTermTarget::Dimension(name) = &item.term.target {
                            Self::reference(
                                table,
                                name.span,
                                SymbolId::Dimension(name.value.clone()),
                            );
                        }
                    }
                }
            }
            hir::types::ValueTypeKind::DimExpr(dim_expr) => {
                for item in &dim_expr.terms {
                    if let hir::types::DimTermTarget::Dimension(name) = &item.term.target {
                        Self::reference(table, name.span, SymbolId::Dimension(name.value.clone()));
                    }
                }
            }
            hir::types::ValueTypeKind::Key(index) => {
                if let hir::types::IndexRef::Concrete(name) = index {
                    Self::reference(table, name.span, SymbolId::Index(name.value.clone()));
                }
            }
            hir::types::ValueTypeKind::Struct(name) => {
                Self::reference(table, name.span, SymbolId::StructType(name.value.clone()));
            }
            hir::types::ValueTypeKind::TypeApplication { name, generic_args } => {
                Self::reference(table, name.span, SymbolId::StructType(name.value.clone()));
                for arg in generic_args {
                    self.walk_generic_arg(arg, table);
                }
            }
        }
    }

    fn walk_generic_arg(&self, arg: &hir::types::GenericArg, table: &mut SymbolTable) {
        match arg {
            hir::types::GenericArg::Dim(hir::types::DimArg::Dimensionless(_))
            | hir::types::GenericArg::Index(
                hir::types::IndexRef::GenericParam(_) | hir::types::IndexRef::Finite(_),
            )
            | hir::types::GenericArg::Nat(_) => {}
            hir::types::GenericArg::Dim(hir::types::DimArg::Expr(dim_expr)) => {
                for item in &dim_expr.terms {
                    if let hir::types::DimTermTarget::Dimension(name) = &item.term.target {
                        Self::reference(table, name.span, SymbolId::Dimension(name.value.clone()));
                    }
                }
            }
            hir::types::GenericArg::Index(hir::types::IndexRef::Concrete(name)) => {
                Self::reference(table, name.span, SymbolId::Index(name.value.clone()));
            }
            hir::types::GenericArg::Type(value_type) => self.walk_type(value_type, table),
        }
    }
}

/// Short display label for a normalized type-level natural-number form,
/// used in local-variable hover details: a constant, a lone parameter, or
/// `..` for anything larger.
fn nat_expr_label(
    form: &graphcal_compiler::syntax::span::Spanned<graphcal_compiler::nat::NatPolyForm>,
) -> String {
    match (form.value.constant_value(), form.value.as_variable()) {
        (Some(value), _) => value.to_string(),
        (None, Some(parameter)) => parameter.name.to_string(),
        (None, None) => "..".to_string(),
    }
}

/// Whether a lowered body may contain `todo<T> { … }` holes (constant and
/// node formulas).
#[derive(Debug, Clone, Copy)]
pub(super) enum BodyHoles {
    Rejected,
    Allowed,
}
