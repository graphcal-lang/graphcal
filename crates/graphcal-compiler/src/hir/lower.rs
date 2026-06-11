//! Lowering from the desugared syntax AST into HIR type-level nodes.
//!
//! This module is the first consumer of the module-aware resolver at the HIR
//! boundary. It deliberately resolves source `NamePath`s into canonical
//! `ResolvedName<Ns>` values or lexical `GenericParamId`s instead of carrying
//! syntax paths forward.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use thiserror::Error;

use crate::dag_id::DagId;
use crate::desugar::desugared_ast as ast;
use crate::registry::time_scale::TimeScale;
use crate::syntax::ast::GenericConstraint;
use crate::syntax::module_resolve::{ModuleResolveError, ModuleResolver};
use crate::syntax::names::{
    DimName, GenericParamName, NameAtom, NamePath, ResolvedName, TimeScaleName, namespace,
};
use crate::syntax::span::{Span, Spanned};

use super::types::{
    BuiltinType, DimExpr, DimExprItem, DimTermRef, DimTermTarget, GenericParamDef, GenericParamId,
    GenericParamOwner, IndexRef, NatExpr, TypeExpr, TypeExprKind,
};

/// Errors produced while lowering syntax type expressions into HIR.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HirLowerError {
    /// A module-aware lookup failed at the use site.
    #[error("{source}")]
    ModuleResolve {
        #[source]
        source: ModuleResolveError,
        span: Span,
    },
    /// A type-level path was not found in any namespace valid for that syntax position.
    #[error("unknown type-level name `{path}`")]
    UnknownTypePath { path: String, span: Span },
    /// A natural-number expression referenced a non-Nat generic parameter.
    #[error(
        "generic parameter `{name}` has constraint `{actual:?}`, but this position expects {expected}"
    )]
    GenericConstraintMismatch {
        name: GenericParamName,
        actual: GenericConstraint,
        expected: &'static str,
        span: Span,
    },
    /// A natural-number expression referenced a name that is not a generic parameter.
    #[error("unknown generic parameter `{name}`")]
    UnknownGenericParam { name: GenericParamName, span: Span },
    /// A generic parameter list declared the same name twice.
    #[error("duplicate generic parameter `{name}`")]
    DuplicateGenericParam {
        name: GenericParamName,
        first: Span,
        duplicate: Span,
    },
    /// `Datetime<...>` has the wrong number of arguments.
    #[error("type `Datetime` expects 0 or 1 type argument(s), got {got}")]
    WrongDatetimeArgCount { got: usize, span: Span },
    /// `Datetime<...>` argument was not a bare time-scale name.
    #[error("expected a time scale name (e.g., UTC, TAI, TT, TDB, GPST)")]
    ExpectedTimeScaleName { span: Span },
    /// `Datetime<...>` argument was a bare name, but not a supported time scale.
    #[error("unknown time scale `{name}`; expected one of: {expected}")]
    UnknownTimeScale {
        name: String,
        expected: &'static str,
        span: Span,
    },
}

/// Implicit prelude type-level symbols visible without an import.
///
/// The module resolver intentionally resolves source module aliases only. The
/// Graphcal prelude is different: it is implicitly in scope in every module but
/// still needs a canonical owner once we cross into HIR. This small typed scope
/// models that boundary without falling back to flat strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreludeTypeScope {
    owner: DagId,
    dimensions: HashSet<DimName>,
}

impl PreludeTypeScope {
    /// Create a prelude type scope from its canonical owner and dimension names.
    #[must_use]
    pub fn new(owner: DagId, dimensions: impl IntoIterator<Item = DimName>) -> Self {
        Self {
            owner,
            dimensions: dimensions.into_iter().collect(),
        }
    }

    /// Create the built-in Graphcal prelude type scope.
    #[must_use]
    pub fn graphcal() -> Self {
        Self::new(
            crate::registry::prelude::prelude_dag_id(),
            crate::registry::prelude::PRELUDE_DIMENSION_NAMES
                .iter()
                .copied()
                .map(DimName::new),
        )
    }

    fn resolve_dimension_path(&self, path: &NamePath) -> Option<ResolvedName<namespace::Dim>> {
        let atom = path.as_bare()?;
        self.dimensions
            .contains(atom.as_str())
            .then(|| ResolvedName::new(self.owner.clone(), atom.clone()))
    }
}

/// A generic parameter binding in a lexical generic scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericParamBinding {
    pub id: GenericParamId,
    pub constraint: GenericConstraint,
    pub span: Span,
}

impl GenericParamBinding {
    /// Create a lexical generic-parameter binding.
    #[must_use]
    pub const fn new(id: GenericParamId, constraint: GenericConstraint, span: Span) -> Self {
        Self {
            id,
            constraint,
            span,
        }
    }

    fn spanned_id(&self, span: Span) -> Spanned<GenericParamId> {
        Spanned::new(self.id.clone(), span)
    }
}

/// Lexical generic parameters visible while lowering one type expression.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GenericScope {
    params: HashMap<GenericParamName, GenericParamBinding>,
}

impl GenericScope {
    /// Create an empty generic scope.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a generic scope from syntax generic parameter definitions.
    ///
    /// # Errors
    ///
    /// Returns [`HirLowerError::DuplicateGenericParam`] if the list contains
    /// the same leaf name twice.
    pub fn from_params(
        owner: &GenericParamOwner,
        params: &[ast::GenericParam],
    ) -> Result<Self, HirLowerError> {
        params.iter().try_fold(Self::new(), |mut scope, param| {
            scope.insert(owner, param)?;
            Ok(scope)
        })
    }

    /// Insert one syntax generic parameter into this scope.
    ///
    /// # Errors
    ///
    /// Returns [`HirLowerError::DuplicateGenericParam`] if a parameter with the
    /// same leaf name is already in scope.
    pub fn insert(
        &mut self,
        owner: &GenericParamOwner,
        param: &ast::GenericParam,
    ) -> Result<(), HirLowerError> {
        let id = GenericParamId::new(owner.clone(), param.name.value.clone());
        self.insert_binding(GenericParamBinding::new(
            id,
            param.constraint,
            param.name.span,
        ))
    }

    /// Insert an already-built generic parameter binding.
    ///
    /// # Errors
    ///
    /// Returns [`HirLowerError::DuplicateGenericParam`] if a parameter with the
    /// same leaf name is already in scope.
    pub fn insert_binding(&mut self, binding: GenericParamBinding) -> Result<(), HirLowerError> {
        let name = binding.id.name.clone();
        match self.params.entry(name.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(binding);
                Ok(())
            }
            Entry::Occupied(entry) => Err(HirLowerError::DuplicateGenericParam {
                name,
                first: entry.get().span,
                duplicate: binding.span,
            }),
        }
    }

    /// Look up a generic parameter by its leaf name.
    #[must_use]
    pub fn get(&self, name: &GenericParamName) -> Option<&GenericParamBinding> {
        self.params.get(name)
    }

    fn get_atom(&self, atom: &NameAtom) -> Option<&GenericParamBinding> {
        let name = GenericParamName::from_atom(atom.clone());
        self.get(&name)
    }
}

/// Context required to lower one type expression into HIR.
#[derive(Debug, Clone, Copy)]
pub struct TypeLoweringContext<'a> {
    pub owner: &'a DagId,
    pub resolver: &'a ModuleResolver,
    pub generic_scope: &'a GenericScope,
    pub prelude: Option<&'a PreludeTypeScope>,
}

impl<'a> TypeLoweringContext<'a> {
    /// Create a type-lowering context.
    #[must_use]
    pub const fn new(
        owner: &'a DagId,
        resolver: &'a ModuleResolver,
        generic_scope: &'a GenericScope,
    ) -> Self {
        Self {
            owner,
            resolver,
            generic_scope,
            prelude: None,
        }
    }

    /// Add implicit prelude type symbols to this lowering context.
    #[must_use]
    pub const fn with_prelude(self, prelude: &'a PreludeTypeScope) -> Self {
        Self {
            owner: self.owner,
            resolver: self.resolver,
            generic_scope: self.generic_scope,
            prelude: Some(prelude),
        }
    }

    fn resolve_prelude_dimension_path(
        self,
        path: &NamePath,
    ) -> Option<ResolvedName<namespace::Dim>> {
        self.prelude
            .and_then(|prelude| prelude.resolve_dimension_path(path))
    }
}

/// Lower syntax generic parameter declarations into HIR definitions and return
/// the lexical scope they introduce.
///
/// Defaults are lowered after the full parameter scope is built, so a default
/// may refer to another parameter in the same list.
///
/// # Errors
///
/// Returns [`HirLowerError`] if a generic parameter is duplicated or a default
/// type expression cannot be resolved.
pub fn lower_generic_params(
    owner: &GenericParamOwner,
    params: &[ast::GenericParam],
    module_owner: &DagId,
    resolver: &ModuleResolver,
) -> Result<(GenericScope, Vec<GenericParamDef>), HirLowerError> {
    let scope = GenericScope::from_params(owner, params)?;
    let ctx = TypeLoweringContext::new(module_owner, resolver, &scope);
    let defs = params
        .iter()
        .map(|param| {
            let id = Spanned::new(
                GenericParamId::new(owner.clone(), param.name.value.clone()),
                param.name.span,
            );
            let default = param
                .default
                .as_ref()
                .map(|default| lower_type_expr(default, ctx))
                .transpose()?;
            Ok(GenericParamDef {
                id,
                constraint: param.constraint,
                default,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((scope, defs))
}

/// Lower a syntax type expression into a HIR type expression.
///
/// # Errors
///
/// Returns [`HirLowerError`] when a source path cannot be resolved to the
/// namespace required by its syntactic position.
pub fn lower_type_expr(
    type_ann: &ast::TypeExpr,
    ctx: TypeLoweringContext<'_>,
) -> Result<TypeExpr, HirLowerError> {
    let kind = match &type_ann.kind {
        ast::TypeExprKind::Dimensionless => TypeExprKind::Builtin(BuiltinType::Dimensionless),
        ast::TypeExprKind::Bool => TypeExprKind::Builtin(BuiltinType::Bool),
        ast::TypeExprKind::Int => TypeExprKind::Builtin(BuiltinType::Int),
        ast::TypeExprKind::Datetime => TypeExprKind::Builtin(BuiltinType::datetime_utc()),
        ast::TypeExprKind::DatetimeApplication { type_args } => {
            TypeExprKind::Builtin(lower_datetime_application(type_ann.span, type_args)?)
        }
        ast::TypeExprKind::DimExpr(dim_expr) => lower_dim_expr_as_type(dim_expr, ctx)?,
        ast::TypeExprKind::Indexed { base, indexes } => TypeExprKind::Indexed {
            base: Box::new(lower_type_expr(base, ctx)?),
            indexes: indexes
                .iter()
                .map(|index| lower_index_expr(index, ctx))
                .collect::<Result<Vec<_>, _>>()?,
        },
        ast::TypeExprKind::TypeApplication { name, type_args } => TypeExprKind::TypeApplication {
            name: Spanned::new(
                ctx.resolver
                    .resolve_struct_type_path(ctx.owner, &name.value)
                    .map_err(|source| HirLowerError::ModuleResolve {
                        source,
                        span: name.span,
                    })?,
                name.span,
            ),
            type_args: type_args
                .iter()
                .map(|arg| lower_type_expr(arg, ctx))
                .collect::<Result<Vec<_>, _>>()?,
        },
    };

    Ok(TypeExpr::new(kind, type_ann.span))
}

fn lower_datetime_application(
    span: Span,
    type_args: &[ast::TypeExpr],
) -> Result<BuiltinType, HirLowerError> {
    match type_args {
        [arg] => Ok(BuiltinType::Datetime(lower_time_scale_arg(arg)?)),
        args => Err(HirLowerError::WrongDatetimeArgCount {
            got: args.len(),
            span,
        }),
    }
}

fn lower_time_scale_arg(arg: &ast::TypeExpr) -> Result<TimeScaleName, HirLowerError> {
    let ast::TypeExprKind::DimExpr(dim_expr) = &arg.kind else {
        return Err(HirLowerError::ExpectedTimeScaleName { span: arg.span });
    };
    let [item] = dim_expr.terms.as_slice() else {
        return Err(HirLowerError::ExpectedTimeScaleName { span: arg.span });
    };
    if item.term.power.is_some() {
        return Err(HirLowerError::ExpectedTimeScaleName { span: arg.span });
    }
    let Some(atom) = item.term.name.value.as_bare() else {
        return Err(HirLowerError::ExpectedTimeScaleName { span: arg.span });
    };
    atom.as_str()
        .parse::<TimeScale>()
        .map(TimeScaleName::new)
        .map_err(|_| HirLowerError::UnknownTimeScale {
            name: atom.to_string(),
            expected: "UTC, TAI, TT, TDB, ET, GPST, GST, BDT, QZSST",
            span: item.term.name.span,
        })
}

fn lower_dim_expr_as_type(
    dim_expr: &ast::DimExpr,
    ctx: TypeLoweringContext<'_>,
) -> Result<TypeExprKind, HirLowerError> {
    match lower_single_term_nominal_type(dim_expr, ctx)? {
        NominalTypeLookup::Found(kind) => Ok(kind),
        NominalTypeLookup::Absent { deferred_error } => match lower_dim_expr(dim_expr, ctx) {
            Ok(dim_expr) => Ok(TypeExprKind::DimExpr(dim_expr)),
            Err(HirLowerError::UnknownTypePath { path, span }) => deferred_error.map_or(
                Err(HirLowerError::UnknownTypePath { path, span }),
                |source| Err(HirLowerError::ModuleResolve { source, span }),
            ),
            Err(err) => Err(err),
        },
    }
}

fn lower_single_term_nominal_type(
    dim_expr: &ast::DimExpr,
    ctx: TypeLoweringContext<'_>,
) -> Result<NominalTypeLookup, HirLowerError> {
    let [item] = dim_expr.terms.as_slice() else {
        return Ok(NominalTypeLookup::absent());
    };
    if item.term.power.is_some() {
        return Ok(NominalTypeLookup::absent());
    }

    let path = &item.term.name.value;
    let mut deferred_error = None;

    match resolve_optional(ctx.resolver.resolve_index_path(ctx.owner, path)) {
        LookupCandidate::Found(index) => {
            return Ok(NominalTypeLookup::Found(TypeExprKind::Label(Spanned::new(
                index,
                item.term.name.span,
            ))));
        }
        LookupCandidate::Absent => {}
        LookupCandidate::Error(source) => {
            deferred_error.get_or_insert(source);
        }
    }

    match resolve_optional(ctx.resolver.resolve_struct_type_path(ctx.owner, path)) {
        LookupCandidate::Found(struct_type) => {
            return Ok(NominalTypeLookup::Found(TypeExprKind::Struct(
                Spanned::new(struct_type, item.term.name.span),
            )));
        }
        LookupCandidate::Absent => {}
        LookupCandidate::Error(source) => {
            deferred_error.get_or_insert(source);
        }
    }

    if let Some(atom) = path.as_bare()
        && let Some(binding) = ctx.generic_scope.get_atom(atom)
    {
        match binding.constraint {
            GenericConstraint::Type => {
                return Ok(NominalTypeLookup::Found(TypeExprKind::GenericTypeParam(
                    binding.spanned_id(item.term.name.span),
                )));
            }
            GenericConstraint::Dim => return Ok(NominalTypeLookup::Absent { deferred_error }),
            GenericConstraint::Index | GenericConstraint::Nat => {
                return Err(HirLowerError::GenericConstraintMismatch {
                    name: GenericParamName::from_atom(atom.clone()),
                    actual: binding.constraint,
                    expected: "Dim or Type",
                    span: item.term.name.span,
                });
            }
        }
    }

    Ok(NominalTypeLookup::Absent { deferred_error })
}

fn lower_dim_expr(
    dim_expr: &ast::DimExpr,
    ctx: TypeLoweringContext<'_>,
) -> Result<DimExpr, HirLowerError> {
    let terms = dim_expr
        .terms
        .iter()
        .map(|item| lower_dim_expr_item(item, ctx))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(DimExpr {
        terms,
        span: dim_expr.span,
    })
}

fn lower_dim_expr_item(
    item: &ast::DimExprItem,
    ctx: TypeLoweringContext<'_>,
) -> Result<DimExprItem, HirLowerError> {
    Ok(DimExprItem {
        op: item.op,
        term: lower_dim_term(&item.term, ctx)?,
    })
}

fn lower_dim_term(
    term: &ast::DimTerm,
    ctx: TypeLoweringContext<'_>,
) -> Result<DimTermRef, HirLowerError> {
    if let Some(atom) = term.name.value.as_bare()
        && let Some(binding) = ctx.generic_scope.get_atom(atom)
    {
        return match binding.constraint {
            GenericConstraint::Dim => Ok(DimTermRef {
                target: DimTermTarget::GenericParam(binding.spanned_id(term.name.span)),
                power: term.power,
                span: term.span,
            }),
            GenericConstraint::Index | GenericConstraint::Nat | GenericConstraint::Type => {
                Err(HirLowerError::GenericConstraintMismatch {
                    name: GenericParamName::from_atom(atom.clone()),
                    actual: binding.constraint,
                    expected: "Dim",
                    span: term.name.span,
                })
            }
        };
    }

    let resolved = match ctx
        .resolver
        .resolve_dimension_path(ctx.owner, &term.name.value)
    {
        Ok(resolved) => resolved,
        Err(ModuleResolveError::UnknownName { .. }) => ctx
            .resolve_prelude_dimension_path(&term.name.value)
            .ok_or_else(|| HirLowerError::UnknownTypePath {
                path: term.name.value.display_path(),
                span: term.name.span,
            })?,
        Err(source) => {
            return Err(HirLowerError::ModuleResolve {
                source,
                span: term.name.span,
            });
        }
    };

    Ok(DimTermRef {
        target: DimTermTarget::Dimension(Spanned::new(resolved, term.name.span)),
        power: term.power,
        span: term.span,
    })
}

fn lower_index_expr(
    index: &ast::IndexExpr,
    ctx: TypeLoweringContext<'_>,
) -> Result<IndexRef, HirLowerError> {
    match index {
        ast::IndexExpr::Name(path) => lower_index_expr_name(path, ctx),
        ast::IndexExpr::NatExpr(nat_expr) => Ok(IndexRef::NatExpr(lower_nat_expr(nat_expr, ctx)?)),
    }
}

fn lower_index_expr_name(
    path: &Spanned<NamePath>,
    ctx: TypeLoweringContext<'_>,
) -> Result<IndexRef, HirLowerError> {
    if let Some(atom) = path.value.as_bare()
        && let Some(binding) = ctx.generic_scope.get_atom(atom)
    {
        return match binding.constraint {
            GenericConstraint::Index => Ok(IndexRef::GenericParam(binding.spanned_id(path.span))),
            GenericConstraint::Nat => Ok(IndexRef::NatExpr(NatExpr::Param(
                binding.spanned_id(path.span),
            ))),
            GenericConstraint::Dim | GenericConstraint::Type => {
                Err(HirLowerError::GenericConstraintMismatch {
                    name: GenericParamName::from_atom(atom.clone()),
                    actual: binding.constraint,
                    expected: "Index or Nat",
                    span: path.span,
                })
            }
        };
    }

    ctx.resolver
        .resolve_index_path(ctx.owner, &path.value)
        .map(|index| IndexRef::Concrete(Spanned::new(index, path.span)))
        .map_err(|source| match source {
            ModuleResolveError::UnknownName { .. } => HirLowerError::UnknownTypePath {
                path: path.value.display_path(),
                span: path.span,
            },
            source => HirLowerError::ModuleResolve {
                source,
                span: path.span,
            },
        })
}

/// Lower a syntax type-level natural-number expression into HIR.
///
/// # Errors
///
/// Returns [`HirLowerError`] if the expression references an unknown generic
/// parameter or a generic parameter whose constraint is not `Nat`.
pub fn lower_nat_expr(
    nat_expr: &ast::NatExpr,
    ctx: TypeLoweringContext<'_>,
) -> Result<NatExpr, HirLowerError> {
    match nat_expr {
        ast::NatExpr::Literal(value, span) => Ok(NatExpr::Literal(*value, *span)),
        ast::NatExpr::Var(ident) => {
            let name = ident.as_generic_param_name();
            let binding =
                ctx.generic_scope
                    .get(&name)
                    .ok_or_else(|| HirLowerError::UnknownGenericParam {
                        name: name.clone(),
                        span: ident.span,
                    })?;
            if binding.constraint != GenericConstraint::Nat {
                return Err(HirLowerError::GenericConstraintMismatch {
                    name,
                    actual: binding.constraint,
                    expected: "Nat",
                    span: ident.span,
                });
            }
            Ok(NatExpr::Param(binding.spanned_id(ident.span)))
        }
        ast::NatExpr::Add(lhs, rhs, span) => Ok(NatExpr::Add(
            Box::new(lower_nat_expr(lhs, ctx)?),
            Box::new(lower_nat_expr(rhs, ctx)?),
            *span,
        )),
        ast::NatExpr::Mul(lhs, rhs, span) => Ok(NatExpr::Mul(
            Box::new(lower_nat_expr(lhs, ctx)?),
            Box::new(lower_nat_expr(rhs, ctx)?),
            *span,
        )),
    }
}

enum NominalTypeLookup {
    Found(TypeExprKind),
    Absent {
        deferred_error: Option<ModuleResolveError>,
    },
}

impl NominalTypeLookup {
    const fn absent() -> Self {
        Self::Absent {
            deferred_error: None,
        }
    }
}

enum LookupCandidate<T> {
    Found(T),
    Absent,
    Error(ModuleResolveError),
}

fn resolve_optional<T>(result: Result<T, ModuleResolveError>) -> LookupCandidate<T> {
    match result {
        Ok(value) => LookupCandidate::Found(value),
        Err(ModuleResolveError::UnknownName { .. }) => LookupCandidate::Absent,
        Err(err) => LookupCandidate::Error(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::names::{ResolvedName, StructTypeName};
    use crate::syntax::parser::Parser;

    fn desugared_source(source: &str) -> ast::File {
        let raw = Parser::new(source).parse_file().unwrap();
        crate::syntax::desugar::desugar_multi_decls_in_file(raw)
    }

    fn first_import(file: &ast::File) -> (&ast::ModulePath, &ast::ImportKind) {
        file.declarations
            .iter()
            .find_map(|decl| match &decl.kind {
                ast::DeclKind::Import(import) => Some((&import.path, &import.kind)),
                _ => None,
            })
            .expect("source should contain an import")
    }

    fn first_param_type(file: &ast::File) -> &ast::TypeExpr {
        file.declarations
            .iter()
            .find_map(|decl| match &decl.kind {
                ast::DeclKind::Param(param) => Some(&param.type_ann),
                _ => None,
            })
            .expect("source should contain a param")
    }

    fn first_type_decl(file: &ast::File) -> &ast::TypeDecl {
        file.declarations
            .iter()
            .find_map(|decl| match &decl.kind {
                ast::DeclKind::Type(type_decl) => Some(type_decl),
                _ => None,
            })
            .expect("source should contain a type declaration")
    }

    #[test]
    fn lowers_qualified_type_level_paths_to_canonical_owners() {
        let lib_id = DagId::root("lib");
        let main_id = DagId::root("main");
        let lib = desugared_source(
            "pub base dim Length; pub index Phase = { Burn }; pub type Vec3<D: Dim> { Vec3(x: D) }",
        );
        let main = desugared_source(
            "import lib as physics; param v: physics.Vec3<physics.Length>[physics.Phase];",
        );
        let (import_path, import_kind) = first_import(&main);

        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(lib_id.clone(), &lib.declarations)
            .unwrap();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();
        resolver
            .register_import(&main_id, import_path, import_kind, &lib_id)
            .unwrap();

        let scope = GenericScope::new();
        let lowered = lower_type_expr(
            first_param_type(&main),
            TypeLoweringContext::new(&main_id, &resolver, &scope),
        )
        .unwrap();

        let TypeExprKind::Indexed { base, indexes } = lowered.kind else {
            panic!("expected indexed type, got {lowered:?}");
        };
        let [IndexRef::Concrete(index)] = indexes.as_slice() else {
            panic!("expected one concrete index, got {indexes:?}");
        };
        assert_eq!(index.value.owner(), &lib_id);
        assert_eq!(index.value.as_str(), "Phase");

        let TypeExprKind::TypeApplication { name, type_args } = base.kind else {
            panic!("expected type application, got {base:?}");
        };
        assert_eq!(name.value.owner(), &lib_id);
        assert_eq!(name.value.as_str(), "Vec3");

        let [arg] = type_args.as_slice() else {
            panic!("expected one type argument, got {type_args:?}");
        };
        let TypeExprKind::DimExpr(dim_expr) = &arg.kind else {
            panic!("expected dimension type argument, got {arg:?}");
        };
        let [term] = dim_expr.terms.as_slice() else {
            panic!("expected one dimension term, got {dim_expr:?}");
        };
        let DimTermTarget::Dimension(dim) = &term.term.target else {
            panic!("expected concrete dimension term, got {term:?}");
        };
        assert_eq!(dim.value.owner(), &lib_id);
        assert_eq!(dim.value.as_str(), "Length");
    }

    #[test]
    fn lowers_generic_scope_references_to_lexical_ids() {
        let owner_id = DagId::root("main");
        let file = desugared_source(
            "type Series<D: Dim, I: Index, N: Nat, F: Type> { Series(value: F, samples: D[I, N]) }",
        );
        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(owner_id.clone(), &file.declarations)
            .unwrap();

        let type_decl = first_type_decl(&file);
        let type_owner = GenericParamOwner::Type(ResolvedName::from_def(
            owner_id.clone(),
            StructTypeName::new("Series"),
        ));
        let (scope, defs) =
            lower_generic_params(&type_owner, &type_decl.generic_params, &owner_id, &resolver)
                .unwrap();
        assert_eq!(defs.len(), 4);

        let members = match &type_decl.body {
            ast::TypeDeclBody::Constructors(members) => members,
            ast::TypeDeclBody::Required => panic!("expected constructor body"),
        };
        let payload = members[0]
            .payload
            .as_ref()
            .expect("Series constructor should have payload");
        let value_type = lower_type_expr(
            &payload[0].type_ann,
            TypeLoweringContext::new(&owner_id, &resolver, &scope),
        )
        .unwrap();
        let TypeExprKind::GenericTypeParam(value_param) = value_type.kind else {
            panic!("expected generic type parameter, got {value_type:?}");
        };
        assert_eq!(value_param.value.name.as_str(), "F");

        let samples_type = lower_type_expr(
            &payload[1].type_ann,
            TypeLoweringContext::new(&owner_id, &resolver, &scope),
        )
        .unwrap();
        let TypeExprKind::Indexed { base, indexes } = samples_type.kind else {
            panic!("expected indexed type, got {samples_type:?}");
        };
        let TypeExprKind::DimExpr(dim_expr) = base.kind else {
            panic!("expected dimension base, got {base:?}");
        };
        let [dim_item] = dim_expr.terms.as_slice() else {
            panic!("expected one dimension term, got {dim_expr:?}");
        };
        let DimTermTarget::GenericParam(dim_param) = &dim_item.term.target else {
            panic!("expected generic dimension param, got {dim_item:?}");
        };
        assert_eq!(dim_param.value.name.as_str(), "D");

        let [
            IndexRef::GenericParam(index_param),
            IndexRef::NatExpr(NatExpr::Param(nat_param)),
        ] = indexes.as_slice()
        else {
            panic!("expected generic index and nat params, got {indexes:?}");
        };
        assert_eq!(index_param.value.name.as_str(), "I");
        assert_eq!(nat_param.value.name.as_str(), "N");
    }
}
