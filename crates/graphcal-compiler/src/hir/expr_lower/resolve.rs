//! Name resolution for value-position references during HIR lowering.

use crate::resolved_name::{ResolvedDeclName, ResolvedIndexVariant};

use crate::builtin::{BuiltinConst, BuiltinFn};
use crate::desugar::desugared_ast as ast;
use crate::resolve::category::{DeclSymbolKind, SymbolTable};
use crate::resolve::error::{ExpectedDeclKind, ModuleResolveError, NameCategory};
use crate::resolve::namespace::Namespace;
use crate::resolve::scope::ModuleAliasRole;
use crate::semantic::time_scale::TimeScale;
use crate::syntax::ast::{Ident, IdentPath};
use crate::syntax::decl_name::{DeclName, DeclNameNamespace};
use crate::syntax::index_name::IndexVariantName;
use crate::syntax::local_name::LocalName;
use crate::syntax::module_name::{ModuleAliasName, ScopedName};
use crate::syntax::names::NamePath;
use crate::syntax::span::{Span, Spanned};

use super::error::ExprLowerError;
use super::lowerer::ExprLowerer;
use super::tolerant::Tolerant;
use crate::hir::expr::{
    ConstRef, ExprKind, ExternFnRef, ResolvedUnitExpr, ResolvedUnitExprItem, ResolvedUnitRef,
    UnappliedFunctionRef,
};

/// Attach the source span of a failed module-resolver lookup.
pub(super) fn spanned<T>(
    result: Result<T, ModuleResolveError>,
    span: Span,
) -> Result<T, ExprLowerError> {
    result.map_err(|source| ExprLowerError::ModuleResolve { source, span })
}

/// A declaration identity reached through a lexical binding that the
/// resolver does not declare.
pub(super) fn unknown_decl(resolved: &ResolvedDeclName, span: Span) -> ExprLowerError {
    ExprLowerError::ModuleResolve {
        source: ModuleResolveError::UnknownName {
            owner: resolved.owner().clone(),
            category: NameCategory::Table(SymbolTable::Decl),
            name: resolved.atom().clone(),
        },
        span,
    }
}

type DeclSymbol<'r> = crate::resolve::symbols::SymbolRef<'r, DeclNameNamespace, DeclSymbolKind>;

/// A declaration reached by a source path, before its kind is checked.
struct SourceDeclTarget<'r> {
    resolved: ResolvedDeclName,
    /// The resolver's symbol for the written path, when the resolver reached
    /// it. For an include-instance member this is the template declaration.
    symbol: Option<DeclSymbol<'r>>,
    /// Whether the path was bound by the owner's lexical binding table.
    bound: bool,
}

/// The scope boundary a graph reference crosses. It decides which
/// declaration kinds the reference may read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GraphRefBoundary {
    /// `@name`: a declaration or lexical binding of the owner itself.
    Local,
    /// `@alias::name` through an `import` alias. The alias names a reusable
    /// blueprint, not an instance, so only instance-independent constants
    /// are readable.
    ImportedDag,
    /// `@alias::name` through a module-form `include` alias, or an output of
    /// an anonymous selective include instance.
    IncludedInstance,
}

impl GraphRefBoundary {
    /// The boundary of a reference qualified by a module alias of `role`.
    pub(super) const fn through_alias(role: ModuleAliasRole) -> Self {
        match role {
            ModuleAliasRole::ImportedDag => Self::ImportedDag,
            ModuleAliasRole::IncludedInstance => Self::IncludedInstance,
        }
    }

    /// Check that a declaration of kind `actual` may be read across this
    /// boundary, returning the expected kind otherwise.
    pub(super) const fn check(self, actual: DeclSymbolKind) -> Result<(), ExpectedDeclKind> {
        match self {
            Self::ImportedDag => match actual {
                DeclSymbolKind::Const => Ok(()),
                DeclSymbolKind::Param
                | DeclSymbolKind::Node
                | DeclSymbolKind::Assert
                | DeclSymbolKind::Plot
                | DeclSymbolKind::Figure
                | DeclSymbolKind::Layer
                | DeclSymbolKind::Dag => Err(ExpectedDeclKind::InstanceIndependentConst),
            },
            Self::Local | Self::IncludedInstance => match actual {
                DeclSymbolKind::Const | DeclSymbolKind::Param | DeclSymbolKind::Node => Ok(()),
                DeclSymbolKind::Assert
                | DeclSymbolKind::Plot
                | DeclSymbolKind::Figure
                | DeclSymbolKind::Layer
                | DeclSymbolKind::Dag => Err(ExpectedDeclKind::GraphValue),
            },
        }
    }
}

#[derive(Debug, Clone)]
pub(super) enum ResolvedCallable<'r> {
    Constructor(
        crate::resolve::symbols::SymbolRef<
            'r,
            crate::syntax::type_name::ConstructorNameNamespace,
            crate::resolve::symbols::ConstructorSignature,
        >,
    ),
    Function(UnappliedFunctionRef),
}

impl<'a> ExprLowerer<'a> {
    pub(super) fn lower_unit_expr(
        &self,
        unit: &ast::UnitExpr,
    ) -> Result<ResolvedUnitExpr<ResolvedUnitRef>, ExprLowerError> {
        let terms = unit
            .terms
            .iter()
            .map(|item| {
                let reference = &item.name.value;
                let path = reference.to_name_path();
                let resolved = match self.ctx.overlay.unit_binding(reference).cloned() {
                    Some(resolved) => resolved,
                    None => match self
                        .ctx
                        .scope
                        .resolver
                        .resolve_unit_path(self.ctx.scope.owner, &path)
                        .map(crate::resolve::symbols::SymbolRef::into_resolved)
                    {
                        Ok(resolved) => resolved,
                        Err(ModuleResolveError::UnknownName { .. }) => {
                            crate::semantic::prelude::prelude_type_scope()
                                .resolve_unit_ref(reference)
                                .ok_or_else(|| ExprLowerError::UnknownUnit {
                                    name: reference.clone(),
                                    span: item.name.span,
                                })?
                        }
                        Err(source) => {
                            return Err(ExprLowerError::ModuleResolve {
                                source,
                                span: item.name.span,
                            });
                        }
                    },
                };
                Ok(ResolvedUnitExprItem {
                    op: item.op,
                    name: Spanned::new(
                        ResolvedUnitRef::new(reference.clone(), resolved),
                        item.name.span,
                    ),
                    power: item.effective_power(),
                })
            })
            .collect::<Result<Vec<_>, ExprLowerError>>()?;
        Ok(ResolvedUnitExpr {
            terms,
            span: unit.span,
        })
    }

    /// Resolve a syntactic reference path in value position.
    ///
    /// This is the single classification point of the compiler: it decides,
    /// in one pass, whether a path names a lexical local, a built-in constant,
    /// a constructor, a type-system entity, or a declaration — and resolves it
    /// to its canonical identity at the same time. Generic parameters are
    /// type-level only: they are classified solely in generic-argument
    /// positions, never here. Lexical scope shadows module symbols. Time
    /// scales are resolved only by the dedicated Static contexts that consume
    /// them.
    pub(super) fn lower_unresolved_path(
        &self,
        path: &IdentPath,
    ) -> Result<ExprKind<Tolerant>, ExprLowerError> {
        path.as_bare().map_or_else(
            || self.lower_dotted_path_ref(path),
            |ident| self.lower_bare_name_ref(ident),
        )
    }

    /// Resolve a bare identifier in value position.
    ///
    /// Priority:
    /// 1. Lexical locals (for/scan/unfold/match bindings)
    /// 2. Built-in Term constants (`PI`, `E`, ...)
    /// 3. Constructors (a bare constructor name is a nullary call)
    /// 4. Type-system names (struct types, dimensions, indexes, variants)
    /// 5. Declarations (const/node/param)
    ///
    /// A time-scale spelling participates only after Term lookup fails, to
    /// produce a targeted wrong-namespace diagnostic rather than resolving a
    /// Static atom into the expression HIR.
    pub(super) fn lower_bare_name_ref(
        &self,
        ident: &Ident,
    ) -> Result<ExprKind<Tolerant>, ExprLowerError> {
        let span = ident.span;
        if let Ok(local) = self.lookup_local(&LocalName::classify(ident.name.atom().clone()), span)
        {
            return Ok(ExprKind::LocalRef(Spanned::new(local, span)));
        }
        if let Some(builtin) = BuiltinConst::parse(ident.name.as_str()) {
            return Ok(ExprKind::ConstRef(Spanned::new(
                ConstRef::Builtin(builtin),
                span,
            )));
        }
        let path = NamePath::local(ident.name.atom().clone());
        let constructor_result = self
            .ctx
            .scope
            .resolver
            .resolve_constructor_path(self.ctx.scope.owner, &path)
            .map(crate::resolve::symbols::SymbolRef::into_resolved);
        if let Ok(constructor) = constructor_result {
            return Ok(ExprKind::ConstructorCall {
                callee: Spanned::new(constructor, span),
                generic_args: Vec::new(),
                fields: Vec::new(),
            });
        }

        match self.resolve_source_decl(&path, span) {
            Ok(SourceDeclTarget { resolved, .. }) => {
                let kind = *self
                    .ctx
                    .scope
                    .resolver
                    .symbol(&resolved)
                    .ok_or_else(|| unknown_decl(&resolved, span))?
                    .kind();
                Err(ExprLowerError::BareGraphDeclarationRef {
                    name: ScopedName::local(DeclName::classify(ident.name.atom().clone())),
                    kind,
                    span,
                })
            }
            Err(ExprLowerError::UnknownGraphRef { .. }) => {
                ident.name.as_str().parse::<TimeScale>().map_or_else(
                    |_| {
                        Err(ExprLowerError::ModuleResolve {
                            source: ModuleResolveError::UnknownName {
                                owner: self.ctx.scope.owner.clone(),
                                category: NameCategory::Namespace(Namespace::Term),
                                name: ident.name.atom().clone(),
                            },
                            span,
                        })
                    },
                    |scale| Err(ExprLowerError::TimeScaleInValuePosition { scale, span }),
                )
            }
            Err(error) => Err(error),
        }
    }

    /// Resolve a Term member selected by an explicit `::` boundary. Index
    /// labels never enter this path; the parser represents `Owner#Label`
    /// separately.
    pub(super) fn lower_dotted_path_ref(
        &self,
        path: &IdentPath,
    ) -> Result<ExprKind<Tolerant>, ExprLowerError> {
        let span = path.span();
        self.lower_const_ref(&path.to_name_path(), span)
            .map(|const_ref| ExprKind::ConstRef(Spanned::new(const_ref, span)))
    }

    pub(super) fn lower_const_ref(
        &self,
        path: &NamePath,
        span: Span,
    ) -> Result<ConstRef<ResolvedDeclName>, ExprLowerError> {
        let name = ScopedName::classify_path(path);
        if let Some(resolved) = self.bound_decl(&name) {
            let lookup = self
                .ctx
                .scope
                .resolver
                .resolve_decl_path(self.ctx.scope.owner, path);
            self.check_bound_source_access(path, lookup, &resolved, span)?;
            let actual = *self
                .ctx
                .scope
                .resolver
                .symbol(&resolved)
                .ok_or_else(|| unknown_decl(&resolved, span))?
                .kind();
            return if actual.is_const() {
                Ok(ConstRef::Decl(resolved))
            } else {
                Err(ExprLowerError::ModuleResolve {
                    source: ModuleResolveError::UnexpectedDeclKind {
                        name: resolved,
                        expected: ExpectedDeclKind::Const,
                        actual,
                    },
                    span,
                })
            };
        }

        let first_error = match self
            .ctx
            .scope
            .resolver
            .resolve_const_decl_path(self.ctx.scope.owner, path)
        {
            Ok(symbol) => return Ok(ConstRef::Decl(symbol.into_resolved())),
            Err(source) => source,
        };
        self.ctx
            .scope
            .resolver
            .resolve_constructor_path(self.ctx.scope.owner, path)
            .map(|constructor| ConstRef::Constructor(constructor.into_resolved()))
            .map_err(|_| ExprLowerError::ModuleResolve {
                source: first_error,
                span,
            })
    }

    pub(super) fn lower_graph_ref(
        &self,
        reference: &ast::GraphRef,
    ) -> Result<ExprKind<Tolerant>, ExprLowerError> {
        self.resolve_graph_ref(reference).map(ExprKind::GraphRef)
    }

    /// Resolve a `@` reference and check that its target may be read across
    /// the boundary the reference crosses.
    pub(super) fn resolve_graph_ref(
        &self,
        reference: &ast::GraphRef,
    ) -> Result<Spanned<ResolvedDeclName>, ExprLowerError> {
        match reference {
            ast::GraphRef::Source(path) => self.resolve_source_graph_ref(path),
            ast::GraphRef::IncludeOutput { span, .. } => {
                let name = reference.to_scoped_name();
                let Some(resolved) = self.bound_decl(&name) else {
                    return Err(ExprLowerError::UnknownGraphRef { name, span: *span });
                };
                self.ensure_instance_accessible(&resolved, *span)?;
                let target = SourceDeclTarget {
                    resolved,
                    symbol: None,
                    bound: true,
                };
                self.check_graph_ref_boundary(target, GraphRefBoundary::IncludedInstance, *span)
            }
        }
    }

    /// Resolve a `@` reference written in source (`@name` or
    /// `@module.child::name`).
    pub(super) fn resolve_source_graph_ref(
        &self,
        reference: &Spanned<IdentPath>,
    ) -> Result<Spanned<ResolvedDeclName>, ExprLowerError> {
        let span = reference.span;
        let path = reference.value.to_name_path();
        let target = self.resolve_source_decl(&path, span)?;
        let boundary = match path.qualifier().first() {
            None => GraphRefBoundary::Local,
            Some(head) => {
                let alias = ModuleAliasName::classify(head.clone());
                match self
                    .ctx
                    .scope
                    .resolver
                    .module_alias_role(self.ctx.scope.owner, &alias)
                {
                    Some(role) => GraphRefBoundary::through_alias(role),
                    None => {
                        return Err(ExprLowerError::ModuleResolve {
                            source: ModuleResolveError::UnknownModuleAlias {
                                owner: self.ctx.scope.owner.clone(),
                                alias,
                            },
                            span,
                        });
                    }
                }
            }
        };
        self.check_graph_ref_boundary(target, boundary, span)
    }

    fn check_graph_ref_boundary(
        &self,
        target: SourceDeclTarget<'a>,
        boundary: GraphRefBoundary,
        span: Span,
    ) -> Result<Spanned<ResolvedDeclName>, ExprLowerError> {
        let actual = match target
            .symbol
            .or_else(|| self.ctx.scope.resolver.symbol(&target.resolved))
        {
            Some(symbol) => *symbol.kind(),
            // A lexical binding the resolver does not declare (an elaborated
            // instance member) carries no kind to check.
            None if target.bound => return Ok(Spanned::new(target.resolved, span)),
            None => return Err(unknown_decl(&target.resolved, span)),
        };
        boundary
            .check(actual)
            .map_err(|expected| ExprLowerError::ModuleResolve {
                source: ModuleResolveError::UnexpectedDeclKind {
                    name: target.resolved.clone(),
                    expected,
                    actual,
                },
                span,
            })?;
        Ok(Spanned::new(target.resolved, span))
    }

    /// Resolve a source declaration path: a lexical binding of the owner
    /// first, otherwise the module resolver. An unknown name is reported as
    /// [`ExprLowerError::UnknownGraphRef`].
    fn resolve_source_decl(
        &self,
        path: &NamePath,
        span: Span,
    ) -> Result<SourceDeclTarget<'a>, ExprLowerError> {
        let name = ScopedName::classify_path(path);
        let lookup = self
            .ctx
            .scope
            .resolver
            .resolve_decl_path(self.ctx.scope.owner, path);
        if let Some(resolved) = self.bound_decl(&name) {
            let symbol = self.check_bound_source_access(path, lookup, &resolved, span)?;
            return Ok(SourceDeclTarget {
                resolved,
                symbol,
                bound: true,
            });
        }
        match lookup {
            Ok(symbol) => Ok(SourceDeclTarget {
                resolved: symbol.into_resolved(),
                symbol: Some(symbol),
                bound: false,
            }),
            Err(ModuleResolveError::UnknownName { .. }) => {
                Err(ExprLowerError::UnknownGraphRef { name, span })
            }
            Err(source) => Err(ExprLowerError::ModuleResolve { source, span }),
        }
    }

    /// The canonical identity a written name is lexically bound to, if any.
    fn bound_decl(&self, name: &ScopedName) -> Option<ResolvedDeclName> {
        self.ctx.overlay.decl_binding(name).cloned()
    }

    /// Check the access rule of a source path that is lexically bound to
    /// `resolved`, given the resolver's own `lookup` of the path.
    ///
    /// A local name needs no check. A qualified name the resolver reaches is
    /// checked by the resolver; one it cannot reach names an elaborated
    /// include-instance member, which is checked against its template.
    /// Returns the resolver's symbol when it reached the path.
    fn check_bound_source_access(
        &self,
        path: &NamePath,
        lookup: Result<DeclSymbol<'a>, ModuleResolveError>,
        resolved: &ResolvedDeclName,
        span: Span,
    ) -> Result<Option<DeclSymbol<'a>>, ExprLowerError> {
        match lookup {
            Ok(symbol) => Ok(Some(symbol)),
            Err(_) if !path.is_qualified() => Ok(None),
            Err(
                ModuleResolveError::UnknownModuleAlias { .. }
                | ModuleResolveError::UnknownName { .. },
            ) => self
                .ensure_instance_accessible(resolved, span)
                .map(|()| None),
            Err(source) => Err(ExprLowerError::ModuleResolve { source, span }),
        }
    }

    /// Require that an elaborated include-instance member is readable by the
    /// instance's consumer, judged by its declaration in the template.
    fn ensure_instance_accessible(
        &self,
        resolved: &ResolvedDeclName,
        span: Span,
    ) -> Result<(), ExprLowerError> {
        let Some(template) = self.ctx.overlay.instance_template(resolved.owner()) else {
            return Ok(());
        };
        if template == self.ctx.scope.owner {
            return Ok(());
        }

        let template_path = NamePath::local(resolved.atom().clone());
        if self
            .ctx
            .scope
            .resolver
            .resolve_decl_path(template, &template_path)
            .map_err(|source| ExprLowerError::ModuleResolve { source, span })?
            .is_instance_accessible()
        {
            Ok(())
        } else {
            Err(ExprLowerError::ModuleResolve {
                source: ModuleResolveError::PrivateName {
                    owner: resolved.owner().clone(),
                    category: NameCategory::Table(SymbolTable::Decl),
                    name: resolved.atom().clone(),
                },
                span,
            })
        }
    }

    /// Resolve one Term callee before argument-shape validation. The backing
    /// maps are an implementation detail; namespace construction guarantees
    /// that at most one callable category occupies the selected Term slot.
    pub(super) fn resolve_callable(
        &self,
        callee: &IdentPath,
    ) -> Result<ResolvedCallable<'a>, ExprLowerError> {
        let constructor = self
            .ctx
            .scope
            .resolver
            .resolve_constructor_ident_path(self.ctx.scope.owner, callee);
        constructor.map(ResolvedCallable::Constructor).or_else(|_| {
            self.lower_function_ref(callee)
                .map(ResolvedCallable::Function)
        })
    }

    pub(super) fn lower_function_ref(
        &self,
        callee: &crate::syntax::ast::IdentPath,
    ) -> Result<UnappliedFunctionRef, ExprLowerError> {
        if let Some(ident) = callee.as_bare() {
            return BuiltinFn::parse(ident.name.as_str())
                .map(UnappliedFunctionRef::Builtin)
                .ok_or_else(|| ExprLowerError::UnknownFunction {
                    path: callee.display_path(),
                    span: callee.span(),
                });
        }
        // `alias::name(...)`: an extern call when `alias` is a plugin alias in
        // scope. Extern functions are only callable in this qualified form.
        if let Some((qualifiers, leaf)) = callee.qualifier_and_leaf()
            && let [qualifier] = qualifiers.as_slice()
            && let Some(target) = self.ctx.scope.resolver.plugin_alias(
                self.ctx.scope.owner,
                &crate::syntax::module_name::ModuleAliasName::classify(
                    qualifier.name.atom().clone(),
                ),
            )
        {
            let name = crate::syntax::function_name::FnName::classify(leaf.name.atom().clone());
            if !target.functions().contains_key(&name) {
                return Err(ExprLowerError::UnknownExternFunction {
                    alias: ModuleAliasName::classify(qualifier.name.atom().clone()),
                    name,
                    span: callee.span(),
                });
            }
            return Ok(UnappliedFunctionRef::External(ExternFnRef {
                plugin: target.path().clone(),
                alias: ModuleAliasName::classify(qualifier.name.atom().clone()),
                name,
            }));
        }
        Err(ExprLowerError::UnknownFunction {
            path: callee.display_path(),
            span: callee.span(),
        })
    }

    pub(super) fn resolve_index_variant_parts(
        &self,
        index_path: &NamePath,
        variant: &IndexVariantName,
        index_span: Span,
        variant_span: Span,
    ) -> Result<ResolvedIndexVariant, ExprLowerError> {
        self.ctx
            .scope
            .resolver
            .resolve_index_variant_parts(self.ctx.scope.owner, index_path, variant)
            .map_err(|source| {
                let span = match source {
                    ModuleResolveError::UnknownIndexVariant { .. } => variant_span,
                    _ => index_span,
                };
                ExprLowerError::ModuleResolve { source, span }
            })
    }
}
