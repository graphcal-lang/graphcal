//! Module-aware classification of ambiguous alias field accesses.

use std::borrow::Cow;
use std::collections::HashSet;

use graphcal_compiler::desugar::desugared_ast::{DeclKind, Expr, ExprKind, File};
use graphcal_compiler::ir::resolve::{ImportedValueNames, ScopedName};
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::module_name::ModuleAliasName;
use graphcal_compiler::syntax::phase::Desugared;
use graphcal_compiler::syntax::span::Spanned;
use graphcal_compiler::syntax::visitor::ExprVisitorMut;

use super::IncludeInstanceRequest;

/// `(module, member)` pair identifying a qualified import alias.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct QualifiedMember {
    module: ModuleAliasName,
    member: DeclName,
}

struct AliasFieldAccessRewriter<'a> {
    qualified_pairs: &'a HashSet<QualifiedMember>,
}

impl ExprVisitorMut<Desugared> for AliasFieldAccessRewriter<'_> {
    type Error = std::convert::Infallible;

    fn visit_expr_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        self.dispatch_mut(expr)?;

        let promote = if let ExprKind::FieldAccess { expr: inner, field } = &expr.kind
            && let ExprKind::GraphRef(qualifier_name) = &inner.kind
            && !qualifier_name.value.is_qualified()
            && self.qualified_pairs.contains(&QualifiedMember {
                module: ModuleAliasName::from_atom(qualifier_name.value.member().atom().clone()),
                member: DeclName::from_atom(field.value.atom().clone()),
            }) {
            let merged_span = qualifier_name.span.merge(field.span);
            Some(ExprKind::GraphRef(Spanned {
                value: ScopedName::qualified(
                    ModuleAliasName::from_atom(qualifier_name.value.member().atom().clone()),
                    DeclName::from_atom(field.value.atom().clone()),
                ),
                span: merged_span,
            }))
        } else {
            None
        };
        if let Some(kind) = promote {
            expr.kind = kind;
        }
        Ok(())
    }
}

fn rewrite_alias_field_access(expr: &mut Expr, qualified_pairs: &HashSet<QualifiedMember>) {
    if qualified_pairs.is_empty() {
        return;
    }
    let mut rewriter = AliasFieldAccessRewriter { qualified_pairs };
    let _ = rewriter.visit_expr_mut(expr);
}

/// Resolve namespace-alias graph references in one compilation body and its
/// pending instance bindings.
pub(super) fn rewrite_qualified_refs_in_compilation_body<'a>(
    ast: &'a File,
    imported_names: &ImportedValueNames,
    include_instances: &mut [IncludeInstanceRequest],
) -> Cow<'a, File> {
    let alias_pairs = collect_qualified_pairs(imported_names);
    if alias_pairs.is_empty() {
        return Cow::Borrowed(ast);
    }

    for binding in include_instances
        .iter_mut()
        .flat_map(|include| include.bindings.values_mut())
    {
        rewrite_alias_field_access(binding, &alias_pairs);
    }

    let mut ast = ast.clone();
    for declaration in &mut ast.declarations {
        rewrite_decl_exprs(declaration, &alias_pairs);
    }
    Cow::Owned(ast)
}

fn collect_qualified_pairs(imported: &ImportedValueNames) -> HashSet<QualifiedMember> {
    imported
        .const_names
        .iter()
        .chain(imported.param_names.iter())
        .chain(imported.node_names.iter())
        .filter_map(|(scoped, _)| {
            let [module] = scoped.qualifier() else {
                return None;
            };
            Some(QualifiedMember {
                module: module.clone(),
                member: scoped.member().clone(),
            })
        })
        .collect()
}

fn rewrite_decl_exprs(
    declaration: &mut graphcal_compiler::desugar::desugared_ast::Declaration,
    alias_pairs: &HashSet<QualifiedMember>,
) {
    let rewrite = |expr: &mut Expr| rewrite_alias_field_access(expr, alias_pairs);
    match &mut declaration.kind {
        DeclKind::Param(param) => {
            if let Some(value) = &mut param.value {
                rewrite(value);
            }
        }
        DeclKind::Node(node) => rewrite(&mut node.value),
        DeclKind::ConstNode(constant) => rewrite(&mut constant.value),
        DeclKind::Assert(assertion) => match &mut assertion.body {
            graphcal_compiler::desugar::desugared_ast::AssertBody::Expr(expr) => rewrite(expr),
            graphcal_compiler::desugar::desugared_ast::AssertBody::Tolerance {
                actual,
                expected,
                tolerance,
                ..
            } => {
                rewrite(actual);
                rewrite(expected);
                rewrite(tolerance);
            }
        },
        DeclKind::Include(include) => include
            .param_bindings
            .iter_mut()
            .for_each(|binding| rewrite(&mut binding.value)),
        _ => {}
    }
}
