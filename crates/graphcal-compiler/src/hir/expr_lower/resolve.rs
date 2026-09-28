//! Name resolution for value-position references during HIR lowering.

use crate::resolved_name::{ResolvedDeclName, ResolvedIndexVariant};

use crate::builtin::{BuiltinConst, BuiltinFn};
use crate::desugar::desugared_ast as ast;
use crate::registry::time_scale::TimeScale;
use crate::resolve::category::{DeclSymbolKind, SymbolTable};
use crate::resolve::error::{ExpectedDeclKind, ModuleResolveError, NameCategory};
use crate::resolve::namespace::Namespace;
use crate::resolve::scope::ModuleAliasRole;
use crate::syntax::ast::{Ident, IdentPath};
use crate::syntax::decl_name::DeclName;
use crate::syntax::index_name::IndexVariantName;
use crate::syntax::local_name::LocalName;
use crate::syntax::module_name::{ModuleAliasName, ScopeSegment, ScopedName};
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
    ) -> Result<ResolvedUnitExpr, ExprLowerError> {
        let terms = unit
            .terms
            .iter()
            .map(|item| {
                let reference = &item.name.value;
                let path = reference.to_name_path();
                let resolved = match self
                    .ctx
                    .unit_bindings
                    .and_then(|bindings| bindings.get(reference))
                    .cloned()
                {
                    Some(resolved) => resolved,
                    None => match self
                        .ctx
                        .resolver
                        .resolve_unit_path(self.ctx.owner, &path)
                        .map(crate::resolve::symbols::SymbolRef::into_resolved)
                    {
                        Ok(resolved) => resolved,
                        Err(ModuleResolveError::UnknownName { .. }) => self
                            .ctx
                            .resolve_prelude_unit_ref(reference)
                            .or_else(|| self.ctx.resolve_registry_unit_ref(reference))
                            .ok_or_else(|| ExprLowerError::UnknownUnit {
                                name: reference.clone(),
                                span: item.name.span,
                            })?,
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
            .resolver
            .resolve_constructor_path(self.ctx.owner, &path)
            .map(crate::resolve::symbols::SymbolRef::into_resolved);
        if let Ok(constructor) = constructor_result {
            return Ok(ExprKind::ConstructorCall {
                callee: Spanned::new(constructor, span),
                generic_args: Vec::new(),
                fields: Vec::new(),
            });
        }

        let scoped_name = ScopedName::local(DeclName::classify(ident.name.atom().clone()));
        match self.resolve_decl_scoped_name(&scoped_name, span) {
            Ok(resolved) => {
                let kind = *self
                    .ctx
                    .resolver
                    .symbol(&resolved)
                    .ok_or_else(|| unknown_decl(&resolved, span))?
                    .kind();
                Err(ExprLowerError::BareGraphDeclarationRef {
                    name: scoped_name,
                    kind,
                    span,
                })
            }
            Err(ExprLowerError::UnknownGraphRef { .. }) => {
                ident.name.as_str().parse::<TimeScale>().map_or_else(
                    |_| {
                        Err(ExprLowerError::ModuleResolve {
                            source: ModuleResolveError::UnknownName {
                                owner: self.ctx.owner.clone(),
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
        let scoped = ScopedName::classify_path(&path.to_name_path());
        self.lower_const_ref(&scoped, span)
            .map(|const_ref| ExprKind::ConstRef(Spanned::new(const_ref, span)))
    }

    pub(super) fn lower_const_ref(
        &self,
        name: &ScopedName,
        span: Span,
    ) -> Result<ConstRef, ExprLowerError> {
        let mut first_error = None;

        if let Some(resolved) = self
            .ctx
            .decl_bindings
            .and_then(|bindings| bindings.get(name))
            .cloned()
        {
            self.ensure_bound_decl_access(name, &resolved, span)?;
            let actual = *self
                .ctx
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

        // An anonymous include qualifier has no source path; such a name is
        // only reachable through `decl_bindings` above.
        if let Some(path) = name.to_name_path() {
            match self
                .ctx
                .resolver
                .resolve_const_decl_path(self.ctx.owner, &path)
                .map(crate::resolve::symbols::SymbolRef::into_resolved)
            {
                Ok(resolved) => return Ok(ConstRef::Decl(resolved)),
                Err(err) => first_error.get_or_insert(err),
            };
            if let Some(resolved) = self.resolve_synthetic_child_decl_path(&path)
                && self
                    .ctx
                    .resolver
                    .symbol(&resolved)
                    .is_some_and(|symbol| symbol.kind().is_const())
            {
                return Ok(ConstRef::Decl(resolved));
            }
            match self
                .ctx
                .resolver
                .resolve_constructor_path(self.ctx.owner, &path)
                .map(crate::resolve::symbols::SymbolRef::into_resolved)
            {
                Ok(resolved) => return Ok(ConstRef::Constructor(resolved)),
                Err(err) => first_error.get_or_insert(err),
            };
        }

        first_error.map_or_else(
            // Only a name qualified by an anonymous include instance has no
            // source path to resolve; without a binding it is unknown.
            || {
                Err(ExprLowerError::UnknownGraphRef {
                    name: name.clone(),
                    span,
                })
            },
            |source| Err(ExprLowerError::ModuleResolve { source, span }),
        )
    }

    pub(super) fn lower_graph_ref(
        &self,
        name: &Spanned<ScopedName>,
    ) -> Result<ExprKind<Tolerant>, ExprLowerError> {
        self.resolve_graph_ref(name).map(ExprKind::GraphRef)
    }

    pub(super) fn resolve_graph_ref(
        &self,
        name: &Spanned<ScopedName>,
    ) -> Result<Spanned<ResolvedDeclName>, ExprLowerError> {
        let resolved = self.resolve_decl_scoped_name(&name.value, name.span)?;
        let path_symbol = match name
            .value
            .to_name_path()
            .map(|path| self.ctx.resolver.resolve_decl_path(self.ctx.owner, &path))
        {
            Some(Ok(symbol)) => Some(symbol),
            Some(Err(source @ ModuleResolveError::PrivateName { .. })) => {
                return Err(ExprLowerError::ModuleResolve {
                    source,
                    span: name.span,
                });
            }
            Some(Err(_)) | None => None,
        };
        let kind = match path_symbol.or_else(|| self.ctx.resolver.symbol(&resolved)) {
            Some(symbol) => *symbol.kind(),
            None if self
                .ctx
                .decl_bindings
                .is_some_and(|bindings| bindings.contains_key(&name.value)) =>
            {
                return Ok(Spanned::new(resolved, name.span));
            }
            None => return Err(unknown_decl(&resolved, name.span)),
        };
        let role = name
            .value
            .qualifier()
            .first()
            .and_then(ScopeSegment::alias)
            .and_then(|alias| self.ctx.resolver.module_alias_role(self.ctx.owner, alias));
        let permitted = match role {
            Some(ModuleAliasRole::ImportedDag) => kind == DeclSymbolKind::Const,
            Some(ModuleAliasRole::IncludedInstance) | None => {
                matches!(
                    kind,
                    DeclSymbolKind::Const | DeclSymbolKind::Param | DeclSymbolKind::Node
                )
            }
        };
        if !permitted {
            return Err(ExprLowerError::ModuleResolve {
                source: ModuleResolveError::UnexpectedDeclKind {
                    name: resolved,
                    expected: match role {
                        Some(ModuleAliasRole::ImportedDag) => {
                            ExpectedDeclKind::InstanceIndependentConst
                        }
                        Some(ModuleAliasRole::IncludedInstance) | None => {
                            ExpectedDeclKind::GraphValue
                        }
                    },
                    actual: kind,
                },
                span: name.span,
            });
        }
        Ok(Spanned::new(resolved, name.span))
    }

    pub(super) fn resolve_decl_scoped_name(
        &self,
        name: &ScopedName,
        span: Span,
    ) -> Result<ResolvedDeclName, ExprLowerError> {
        if let Some(resolved) = self
            .ctx
            .decl_bindings
            .and_then(|bindings| bindings.get(name))
            .cloned()
        {
            self.ensure_bound_decl_access(name, &resolved, span)?;
            return Ok(resolved);
        }
        // An anonymous include qualifier has no source path; such a name is
        // only reachable through `decl_bindings` above.
        let Some(path) = name.to_name_path() else {
            return Err(ExprLowerError::UnknownGraphRef {
                name: name.clone(),
                span,
            });
        };
        let resolved = match self
            .ctx
            .resolver
            .resolve_decl_path(self.ctx.owner, &path)
            .map(crate::resolve::symbols::SymbolRef::into_resolved)
        {
            Ok(resolved) => Ok(resolved),
            // Synthetic include-instance children intentionally are not module
            // aliases. Retry only when the qualifier itself is absent; once
            // the resolver reaches a real alias, its rejection is authoritative.
            Err(source @ ModuleResolveError::UnknownModuleAlias { .. }) => {
                self.resolve_synthetic_child_decl_path(&path).ok_or(source)
            }
            Err(source) => Err(source),
        };
        resolved.map_err(|source| match source {
            ModuleResolveError::UnknownName { .. } => ExprLowerError::UnknownGraphRef {
                name: name.clone(),
                span,
            },
            source => ExprLowerError::ModuleResolve { source, span },
        })
    }

    pub(super) fn ensure_bound_decl_access(
        &self,
        name: &ScopedName,
        resolved: &ResolvedDeclName,
        span: Span,
    ) -> Result<(), ExprLowerError> {
        if !name.is_qualified() {
            return Ok(());
        }

        match name.to_name_path().map(|path| {
            self.ctx
                .resolver
                .resolve_decl_path(self.ctx.owner, &path)
                .map(crate::resolve::symbols::SymbolRef::into_resolved)
        }) {
            Some(Ok(_)) => Ok(()),
            // An anonymous include qualifier is never a module alias.
            None
            | Some(Err(
                ModuleResolveError::UnknownModuleAlias { .. }
                | ModuleResolveError::UnknownName { .. },
            )) => {
                let Some(template) = self
                    .ctx
                    .instance_templates
                    .and_then(|templates| templates.get(resolved.owner()))
                else {
                    return Ok(());
                };
                if template == self.ctx.owner {
                    return Ok(());
                }

                let template_path = NamePath::local(resolved.atom().clone());
                if self
                    .ctx
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
            Some(Err(source)) => Err(ExprLowerError::ModuleResolve { source, span }),
        }
    }

    pub(super) fn resolve_synthetic_child_decl_path(
        &self,
        path: &NamePath,
    ) -> Option<ResolvedDeclName> {
        let (qualifier, leaf) = path.qualifier_and_leaf()?;
        let owner = qualifier
            .iter()
            .fold(self.ctx.owner.clone(), |owner, segment| {
                owner.inline_dag_child(DeclName::classify(segment.clone()))
            });
        self.ctx.resolver.symbols(&owner).and_then(|module| {
            let decl_name = DeclName::classify(leaf.clone());
            module
                .decls()
                .contains_key(&decl_name)
                .then(|| ResolvedDeclName::from_def(owner, decl_name))
        })
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
            .resolver
            .resolve_constructor_ident_path(self.ctx.owner, callee);
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
            && let Some(target) = self.ctx.resolver.plugin_alias(
                self.ctx.owner,
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
            .resolver
            .resolve_index_variant_parts(self.ctx.owner, index_path, variant)
            .map_err(|source| {
                let span = match source {
                    ModuleResolveError::UnknownIndexVariant { .. } => variant_span,
                    _ => index_span,
                };
                ExprLowerError::ModuleResolve { source, span }
            })
    }
}
