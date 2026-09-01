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
use graphcal_compiler::syntax::decl_name::ResolvedDeclName;
use graphcal_compiler::syntax::module_resolve::ModuleResolver;
use graphcal_compiler::syntax::type_name::{ConstructorName, ResolvedConstructorName};

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
    pub fn expression_constructor(&self, expr: &hir::Expr) -> Option<ResolvedConstructorName> {
        match &expr.kind {
            hir::ExprKind::GraphRef(target) => self.declaration_types.get(&target.value).cloned(),
            hir::ExprKind::ConstRef(target) => match &target.value {
                hir::ConstRef::Decl(name) => self.declaration_types.get(name).cloned(),
                hir::ConstRef::Constructor(constructor) => Some(constructor.clone()),
                hir::ConstRef::Builtin(_) | hir::ConstRef::GenericNatParam(_) => None,
            },
            hir::ExprKind::ConstructorCall { callee, .. } => Some(callee.value.clone()),
            hir::ExprKind::IndexAccess { expr, .. }
            | hir::ExprKind::Convert { expr, .. }
            | hir::ExprKind::DisplayTimezone { expr, .. } => self.expression_constructor(expr),
            hir::ExprKind::FieldAccess { expr, field } => {
                let owner = self.expression_constructor(expr)?;
                self.field_types
                    .get(&FieldId::new(owner, field.value.clone()))
                    .cloned()
            }
            hir::ExprKind::If {
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
        let declared_value = match &declaration.kind {
            DeclKind::Param(param) => Some((&param.name, &param.type_ann)),
            DeclKind::Node(node) => Some((&node.name, &node.type_ann)),
            DeclKind::ConstNode(constant) => Some((&constant.name, &constant.type_ann)),
            _ => None,
        };
        if let Some((name, type_expr)) = declared_value
            && let Some(constructor) = nominal_constructor(type_expr, owner, resolver)
        {
            index.declaration_types.insert(
                ResolvedDeclName::from_def(owner.clone(), name.value.clone()),
                constructor,
            );
        }

        match &declaration.kind {
            DeclKind::Type(type_decl) => {
                let members = match &type_decl.body {
                    TypeDeclBody::Required => &[][..],
                    TypeDeclBody::Constructors(members) => members.as_slice(),
                };
                for member in members {
                    let constructor =
                        ResolvedConstructorName::from_def(owner.clone(), member.name.value.clone());
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
                &owner.child(dag.name.value.as_str()),
                resolver,
                index,
            ),
            _ => {}
        }
    }
}

fn nominal_constructor(
    type_expr: &TypeExpr,
    owner: &DagId,
    resolver: &ModuleResolver,
) -> Option<ResolvedConstructorName> {
    let type_name = match &type_expr.kind {
        TypeExprKind::Indexed { base, .. } => {
            return nominal_constructor(base, owner, resolver);
        }
        TypeExprKind::TypeApplication { name, .. } => {
            resolver.resolve_struct_type_path(owner, &name.value).ok()?
        }
        TypeExprKind::DimExpr(dimension) => {
            let [term] = dimension.terms.as_slice() else {
                return None;
            };
            if term.term.effective_power() != Rational::ONE {
                return None;
            }
            resolver
                .resolve_struct_type_path(owner, &term.term.name.value)
                .ok()?
        }
        _ => return None,
    };
    Some(ResolvedConstructorName::from_def(
        type_name.owner().clone(),
        ConstructorName::from_atom(type_name.atom().clone()),
    ))
}
