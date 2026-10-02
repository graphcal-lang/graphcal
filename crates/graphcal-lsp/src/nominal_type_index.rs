//! Nominal expression types used to identify record-field occurrences.
//!
//! HIR resolves declarations and constructors canonically but intentionally
//! leaves a field access as a bare field name. This focused index preserves
//! enough declared nominal type information to attach that spelling to its
//! owning constructor without relying on globally unique field names.

use std::collections::HashMap;

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::desugar::desugared_ast::{
    DeclKind, Declaration, TypeDeclBody, TypeExpr, TypeExprKind,
};
use graphcal_compiler::dimension::Rational;
use graphcal_compiler::hir;
use graphcal_compiler::resolve::ModuleResolver;
use graphcal_compiler::resolved_name::{ResolvedConstructorName, ResolvedDeclName};
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::phase::never;

use crate::symbol_identity::FieldId;

/// Canonical nominal types for declarations and record fields in one source
/// module tree.
#[derive(Debug, Default)]
pub struct NominalTypeIndex {
    declaration_types: HashMap<ResolvedDeclName, ResolvedConstructorName>,
    field_types: HashMap<FieldId, ResolvedConstructorName>,
}

impl NominalTypeIndex {
    pub fn build(declarations: &[Declaration], owner: &DagId, resolver: &ModuleResolver) -> Self {
        let mut index = Self::default();
        collect_declarations(declarations, owner, resolver, &mut index);
        index
    }

    /// Resolve the record constructor produced by an expression when its
    /// nominal type follows directly from HIR and declared types.
    ///
    /// IDE trees name their references' source definitions, which is what
    /// the declared types are keyed by.
    pub fn expression_constructor(
        &self,
        expr: &hir::expr::Expr<hir::Tolerant>,
    ) -> Option<ResolvedConstructorName> {
        match expr.kind() {
            hir::expr::ExprKind::GraphRef(target) => {
                self.declaration_types.get(&target.value).cloned()
            }
            hir::expr::ExprKind::ConstRef(target) => match &target.value {
                hir::expr::ConstRef::Decl(name) => self.declaration_types.get(name).cloned(),
                hir::expr::ConstRef::Constructor(constructor) => Some(constructor.clone()),
                hir::expr::ConstRef::Builtin(_) => None,
            },
            hir::expr::ExprKind::ConstructorCall { callee, .. } => Some(callee.value.clone()),
            hir::expr::ExprKind::IndexAccess { expr, .. }
            | hir::expr::ExprKind::Convert { expr, .. }
            | hir::expr::ExprKind::DisplayTimezone { expr, .. } => {
                self.expression_constructor(expr)
            }
            hir::expr::ExprKind::FieldAccess { expr, field } => {
                let owner = self.expression_constructor(expr)?;
                self.field_types
                    .get(&FieldId::new(owner, field.value.clone()))
                    .cloned()
            }
            hir::expr::ExprKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                let then_type = self.expression_constructor(then_branch)?;
                (self.expression_constructor(else_branch).as_ref() == Some(&then_type))
                    .then_some(then_type)
            }
            _ => None,
        }
    }
}

fn collect_declarations(
    declarations: &[Declaration],
    owner: &DagId,
    resolver: &ModuleResolver,
    index: &mut NominalTypeIndex,
) {
    for declaration in declarations {
        match &declaration.kind {
            DeclKind::Param(param) => {
                collect_declared_value(&param.name.value, &param.type_ann, owner, resolver, index);
            }
            DeclKind::Node(node) => {
                collect_declared_value(&node.name.value, &node.type_ann, owner, resolver, index);
            }
            DeclKind::ConstNode(constant) => collect_declared_value(
                &constant.name.value,
                &constant.type_ann,
                owner,
                resolver,
                index,
            ),
            DeclKind::Type(type_decl) => {
                let members = match &type_decl.body {
                    TypeDeclBody::Required => &[][..],
                    TypeDeclBody::Constructors(members) => members.as_slice(),
                };
                let Ok(type_id) = resolver.declaration(owner, &type_decl.name.value) else {
                    continue;
                };
                for member in members {
                    let constructor = type_id.resolved().constructor(member.name.value.clone());
                    for field in member.payload.iter().flatten() {
                        if let Some(field_type) =
                            nominal_constructor(&field.type_ann, owner, resolver)
                        {
                            index.field_types.insert(
                                FieldId::new(constructor.clone(), field.name.value.clone()),
                                field_type,
                            );
                        }
                    }
                }
            }
            DeclKind::Dag(dag) => collect_declarations(
                &dag.body,
                &owner.inline_dag_child(dag.name.value.clone()),
                resolver,
                index,
            ),
            // Declarations without a declared value type or record fields.
            DeclKind::BaseDimension(_)
            | DeclKind::Dimension(_)
            | DeclKind::Unit(_)
            | DeclKind::Index(_)
            | DeclKind::Import(_)
            | DeclKind::PluginImport(_)
            | DeclKind::Include(_)
            | DeclKind::Assert(_)
            | DeclKind::Plot(_)
            | DeclKind::Figure(_)
            | DeclKind::Layer(_) => {}
            #[expect(
                clippy::uninhabited_references,
                reason = "Sugar(Infallible) proves this arm unreachable"
            )]
            DeclKind::Sugar(sugar) => never(*sugar),
        }
    }
}

fn collect_declared_value(
    name: &DeclName,
    type_expr: &TypeExpr,
    owner: &DagId,
    resolver: &ModuleResolver,
    index: &mut NominalTypeIndex,
) {
    if let Some(constructor) = nominal_constructor(type_expr, owner, resolver)
        && let Ok(declaration) = resolver.declaration(owner, name)
    {
        index
            .declaration_types
            .insert(declaration.into_resolved(), constructor);
    }
}

fn nominal_constructor(
    type_expr: &TypeExpr,
    owner: &DagId,
    resolver: &ModuleResolver,
) -> Option<ResolvedConstructorName> {
    let type_name = match &type_expr.element.kind {
        TypeExprKind::TypeApplication { name, .. } => resolver
            .resolve_struct_type_path(owner, &name.value)
            .map(graphcal_compiler::resolve::symbols::SymbolRef::into_resolved)
            .ok()?,
        TypeExprKind::DimExpr(dimension) => {
            let [term] = dimension.terms.as_slice() else {
                return None;
            };
            if term.term.effective_power() != Rational::ONE {
                return None;
            }
            resolver
                .resolve_struct_type_path(owner, &term.term.name.value)
                .map(graphcal_compiler::resolve::symbols::SymbolRef::into_resolved)
                .ok()?
        }
        _ => return None,
    };
    Some(type_name.constructor(
        graphcal_compiler::syntax::type_name::record_constructor_name(
            &type_name.to_unowned_def_name(),
        ),
    ))
}
