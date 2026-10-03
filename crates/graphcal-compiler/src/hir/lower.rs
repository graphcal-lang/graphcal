//! Lowering from the desugared syntax AST into HIR type-level nodes.
//!
//! This module is the first consumer of the module-aware resolver at the HIR
//! boundary. It deliberately resolves source `NamePath`s into canonical
//! `ResolvedName<Ns>` values or lexical `GenericParamId`s instead of carrying
//! syntax paths forward.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use thiserror::Error;

use crate::dag_id::DagId;
use crate::desugar::desugared_ast as ast;
use crate::generic_param::{
    GenericApplicationTarget, GenericArgArity, render_accepted_constraints,
};
use crate::resolve::ModuleResolver;
use crate::resolve::category::SurfaceNameKind;
use crate::resolve::error::ModuleResolveError;
use crate::semantic::time_scale::TimeScale;
use crate::syntax::ast::GenericConstraint;
use crate::syntax::index_name::IndexVariantName;
use crate::syntax::names::{NameAtom, NamePath};
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::span::{Span, Spanned};
use crate::syntax::type_name::GenericParamName;

use super::types::{
    BuiltinType, DeclType, DimArg, DimExpr, DimExprItem, DimTermRef, DimTermTarget, GenericArg,
    GenericParamId, IndexRef, ValueType, ValueTypeKind,
};
use crate::nat::{NatOverflowError, NatPolyForm};

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
    UnknownTypePath {
        path: NamePath,
        slot: TypePathSlot,
        span: Span,
    },
    /// An index label appeared in a type-expression slot.
    #[error("index label `{index}#{label}` cannot be used as a type")]
    IndexLabelAsType {
        index: NamePath,
        label: IndexVariantName,
        span: Span,
    },
    /// An index name appeared where a value type is required.
    #[error("index `{index}` cannot be used as a type")]
    IndexAsType { index: IndexRef },
    /// A natural-number expression referenced a non-Nat generic parameter.
    #[error(
        "generic parameter `{name}` has constraint `{actual:?}`, but this position expects {}",
        render_accepted_constraints(expected)
    )]
    GenericConstraintMismatch {
        name: GenericParamName,
        actual: GenericConstraint,
        /// Every constraint this position accepts, in diagnostic order.
        expected: &'static [GenericConstraint],
        span: Span,
    },
    /// A Nat was supplied where an explicit Index is required.
    #[error("expected Index, found Nat `{expression}`; write `Fin({expression})`")]
    ExpectedIndexFoundNat {
        expression: crate::semantic_error::index::FoundNat,
        span: Span,
    },
    /// A type-level natural-number expression's normalized form overflowed.
    #[error("{source}")]
    NatOverflow {
        #[source]
        source: NatOverflowError,
        span: Span,
    },
    /// A natural-number expression referenced a name that is not a generic parameter.
    #[error("unknown generic parameter `{name}`")]
    UnknownGenericParam { name: GenericParamName, span: Span },
    /// An application supplied the wrong number of generic arguments.
    #[error("`{target}` expects {expected} generic argument(s), got {got}")]
    WrongGenericArgCount {
        target: GenericApplicationTarget,
        expected: GenericArgArity,
        got: usize,
        span: Span,
    },
    /// A generic argument's source category does not satisfy its declared sort.
    #[error(
        "generic parameter `{parameter}` expects an argument of sort `{expected}`, got {actual}"
    )]
    GenericArgumentSortMismatch {
        parameter: GenericParamName,
        expected: GenericConstraint,
        actual: &'static str,
        span: Span,
    },
    /// A generic parameter list declared the same name twice.
    #[error("duplicate generic parameter `{name}`")]
    DuplicateGenericParam {
        name: GenericParamName,
        first: Span,
        duplicate: Span,
    },
    /// A Static generic binder reused a visible Static slot.
    #[error("generic parameter `{name}` shadows a visible Static name")]
    GenericParamShadowsStatic {
        name: GenericParamName,
        original: Option<Span>,
        duplicate: Span,
    },
    /// `Datetime<...>` has the wrong number of arguments.
    #[error("type `Datetime` expects 0 or 1 type argument(s), got {got}")]
    WrongDatetimeArgCount { got: usize, span: Span },
    /// `Datetime<...>` argument was not a bare time scale.
    #[error("expected a time scale name (e.g., UTC, TAI, TT, TDB, GPST)")]
    ExpectedTimeScale { span: Span },
    /// `Datetime<...>` argument was a bare name, but not a supported time scale.
    #[error("unknown time scale `{name}`; expected one of: {expected}")]
    UnknownTimeScale {
        name: NameAtom,
        expected: &'static str,
        span: Span,
    },
}

/// The syntactic position of a type-level path that resolved to nothing.
///
/// Each position has one namespace, so the diagnostic can name what was
/// missing without re-walking the syntax tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypePathSlot {
    /// An index axis of a declaration type: `I` in `T[I]`.
    IndexAxis,
    /// A term of a dimension expression in a type position: `L` in `L / T`.
    DimensionTerm,
    /// The applied type of a generic type application: `Vec` in `Vec<L>`.
    TypeApplication,
}

/// A generic parameter binding in a lexical generic scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericParamBinding {
    pub(crate) id: GenericParamId,
    pub(crate) constraint: GenericConstraint,
    span: Span,
}

impl GenericParamBinding {
    /// Create a lexical generic-parameter binding.
    #[must_use]
    pub(crate) const fn new(id: GenericParamId, constraint: GenericConstraint, span: Span) -> Self {
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

    /// Insert an already-built generic parameter binding.
    ///
    /// # Errors
    ///
    /// Returns [`HirLowerError::DuplicateGenericParam`] if a parameter with the
    /// same leaf name is already in scope.
    pub(crate) fn insert_binding(
        &mut self,
        binding: GenericParamBinding,
    ) -> Result<(), HirLowerError> {
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
    pub(crate) fn get(&self, name: &GenericParamName) -> Option<&GenericParamBinding> {
        self.params.get(name)
    }

    fn get_atom(&self, atom: &NameAtom) -> Option<&GenericParamBinding> {
        let name = GenericParamName::classify(atom.clone());
        self.get(&name)
    }
}

/// The module-level scope one type or expression lowering runs in.
///
/// Source paths resolve as seen from one owner module, with its lexical
/// generic parameters, through the module-aware resolver. The implicit
/// Graphcal prelude ([`crate::resolve::prelude::prelude_type_scope`]) is in scope everywhere,
/// so it is not a field.
#[derive(Debug, Clone, Copy)]
pub struct ModuleScope<'a> {
    pub(crate) owner: &'a DagId,
    pub(crate) resolver: &'a ModuleResolver,
    pub(crate) generic_scope: &'a GenericScope,
}

impl<'a> ModuleScope<'a> {
    /// Create the scope of `owner` with the lexical generic parameters `generic_scope`.
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
        }
    }
}

/// A single type-syntax slot lowered before its position decides whether an
/// index is acceptable there.
///
/// A single-term name in type syntax (`Phase`, `I`) may denote an index, but
/// an index is never a [`ValueType`]: only `Index`-sorted generic arguments
/// (including `Key<I>`) accept one. This private classification lets each
/// syntactic position report its own diagnostic without ever placing an index
/// inside a value type.
enum TypeSlot {
    Value(ValueType),
    Index(IndexRef),
}

impl TypeSlot {
    /// Commit this slot to a value-type position.
    fn into_value_type(self) -> Result<ValueType, HirLowerError> {
        match self {
            Self::Value(value_type) => Ok(value_type),
            Self::Index(index) => Err(HirLowerError::IndexAsType { index }),
        }
    }
}

/// Type syntax lowered without committing to a syntactic position.
enum LoweredTypeSyntax {
    Slot(TypeSlot),
    Indexed {
        element: TypeSlot,
        indexes: NonEmpty<IndexRef>,
        span: Span,
    },
}

/// Lower a syntax declaration type annotation into a HIR [`DeclType`].
///
/// # Errors
///
/// Returns [`HirLowerError`] when a source path cannot be resolved to the
/// namespace required by its syntactic position, or when an index name is
/// used as a value type.
pub(crate) fn lower_decl_type(
    type_ann: &ast::TypeExpr,
    ctx: ModuleScope<'_>,
) -> Result<DeclType, HirLowerError> {
    match lower_type_syntax(type_ann, ctx)? {
        LoweredTypeSyntax::Slot(slot) => slot.into_value_type().map(DeclType::Value),
        LoweredTypeSyntax::Indexed {
            element,
            indexes,
            span,
        } => Ok(DeclType::Indexed {
            element: element.into_value_type()?,
            indexes,
            span,
        }),
    }
}

fn lower_type_syntax(
    type_ann: &ast::TypeExpr,
    ctx: ModuleScope<'_>,
) -> Result<LoweredTypeSyntax, HirLowerError> {
    let element = lower_type_slot(&type_ann.element, ctx)?;
    match &type_ann.indexes {
        Some(indexes) => Ok(LoweredTypeSyntax::Indexed {
            element,
            indexes: indexes.try_map_ref(|index| lower_index_expr(index, ctx))?,
            span: type_ann.span,
        }),
        None => Ok(LoweredTypeSyntax::Slot(element)),
    }
}

fn lower_type_slot(
    type_ann: &ast::ElementTypeExpr,
    ctx: ModuleScope<'_>,
) -> Result<TypeSlot, HirLowerError> {
    let kind = match &type_ann.kind {
        ast::TypeExprKind::Dimensionless => ValueTypeKind::Builtin(BuiltinType::Dimensionless),
        ast::TypeExprKind::Bool => ValueTypeKind::Builtin(BuiltinType::Bool),
        ast::TypeExprKind::Int => ValueTypeKind::Builtin(BuiltinType::Int),
        ast::TypeExprKind::Datetime => ValueTypeKind::Builtin(BuiltinType::datetime_utc()),
        ast::TypeExprKind::DatetimeApplication { type_args } => ValueTypeKind::Builtin(
            lower_datetime_application(type_ann.span, type_args.as_slice())?,
        ),
        ast::TypeExprKind::ComplexApplication { generic_args } => {
            ValueTypeKind::Complex(lower_complex_application(type_ann.span, generic_args, ctx)?)
        }
        ast::TypeExprKind::KeyApplication { generic_args } => {
            ValueTypeKind::Key(lower_key_application(type_ann.span, generic_args, ctx)?)
        }
        ast::TypeExprKind::IndexLabel { index, label } => {
            return Err(HirLowerError::IndexLabelAsType {
                index: index.clone().into_spanned_name_path().value,
                label: label.value.clone(),
                span: type_ann.span,
            });
        }
        ast::TypeExprKind::DimExpr(dim_expr) => {
            return lower_dim_expr_as_type(dim_expr, type_ann.span, ctx);
        }
        ast::TypeExprKind::TypeApplication { name, generic_args } => {
            let struct_type = ctx
                .resolver
                .resolve_struct_type_path(ctx.owner, &name.value)
                .map_err(|source| match source {
                    ModuleResolveError::UnknownName { .. } => HirLowerError::UnknownTypePath {
                        path: name.value.clone(),
                        slot: TypePathSlot::TypeApplication,
                        span: name.span,
                    },
                    source => HirLowerError::ModuleResolve {
                        source,
                        span: name.span,
                    },
                })?;
            let resolved_name = struct_type.into_resolved();
            let generic_args = lower_generic_args(
                GenericApplicationTarget::StructType(resolved_name.clone()),
                struct_type.kind(),
                generic_args.as_slice(),
                type_ann.span,
                ctx,
            )?;
            ValueTypeKind::TypeApplication {
                name: Spanned::new(resolved_name, name.span),
                generic_args,
            }
        }
    };

    Ok(TypeSlot::Value(ValueType::new(kind, type_ann.span)))
}

fn lower_complex_application(
    span: Span,
    args: &[ast::GenericArg],
    ctx: ModuleScope<'_>,
) -> Result<DimArg, HirLowerError> {
    let [arg] = args else {
        return Err(HirLowerError::WrongGenericArgCount {
            target: GenericApplicationTarget::Complex,
            expected: GenericArgArity::exactly(1),
            got: args.len(),
            span,
        });
    };
    let parameter = GenericParamName::expect_valid("D");
    match lower_generic_arg_for_constraint(arg, GenericConstraint::Dim, &parameter, ctx)? {
        GenericArg::Dim(dimension) => Ok(dimension),
        GenericArg::Index(_) | GenericArg::Nat(_) | GenericArg::Type(_) => {
            Err(generic_arg_sort_mismatch(
                &parameter,
                GenericConstraint::Dim,
                "non-dimension type argument",
                arg.span(),
            ))
        }
    }
}

fn lower_key_application(
    span: Span,
    args: &[ast::GenericArg],
    ctx: ModuleScope<'_>,
) -> Result<IndexRef, HirLowerError> {
    let [arg] = args else {
        return Err(HirLowerError::WrongGenericArgCount {
            target: GenericApplicationTarget::Key,
            expected: GenericArgArity::exactly(1),
            got: args.len(),
            span,
        });
    };
    let parameter = GenericParamName::expect_valid("I");
    match lower_generic_arg_for_constraint(arg, GenericConstraint::Index, &parameter, ctx)? {
        GenericArg::Index(index) => Ok(index),
        GenericArg::Dim(_) | GenericArg::Nat(_) | GenericArg::Type(_) => {
            Err(generic_arg_sort_mismatch(
                &parameter,
                GenericConstraint::Index,
                "non-index type argument",
                arg.span(),
            ))
        }
    }
}

pub(crate) fn lower_generic_args(
    target: GenericApplicationTarget,
    params: &[crate::resolve::symbols::GenericParamSignature],
    args: &[ast::GenericArg],
    span: Span,
    ctx: ModuleScope<'_>,
) -> Result<Vec<GenericArg>, HirLowerError> {
    check_generic_arg_count(target, params, args.len(), span)?;
    params
        .iter()
        .zip(args)
        .map(|(param, arg)| {
            lower_generic_arg_for_constraint(arg, param.constraint, &param.name, ctx)
        })
        .collect()
}

fn check_generic_arg_count(
    target: GenericApplicationTarget,
    params: &[crate::resolve::symbols::GenericParamSignature],
    got: usize,
    span: Span,
) -> Result<(), HirLowerError> {
    let expected = GenericArgArity::of_defaults(params.iter().map(|param| param.has_default));
    if expected.accepts(got) {
        Ok(())
    } else {
        Err(HirLowerError::WrongGenericArgCount {
            target,
            expected,
            got,
            span,
        })
    }
}

pub(crate) fn lower_generic_arg_for_constraint(
    arg: &ast::GenericArg,
    constraint: GenericConstraint,
    parameter: &GenericParamName,
    ctx: ModuleScope<'_>,
) -> Result<GenericArg, HirLowerError> {
    match constraint {
        GenericConstraint::Dim => {
            match lower_generic_arg_as_type_syntax(arg, parameter, constraint, ctx)? {
                LoweredTypeSyntax::Slot(TypeSlot::Value(ValueType {
                    kind: ValueTypeKind::Builtin(BuiltinType::Dimensionless),
                    span,
                })) => Ok(GenericArg::Dim(DimArg::Dimensionless(span))),
                LoweredTypeSyntax::Slot(TypeSlot::Value(ValueType {
                    kind: ValueTypeKind::DimExpr(dim_expr),
                    ..
                })) => Ok(GenericArg::Dim(DimArg::Expr(dim_expr))),
                LoweredTypeSyntax::Slot(TypeSlot::Value(_) | TypeSlot::Index(_))
                | LoweredTypeSyntax::Indexed { .. } => Err(generic_arg_sort_mismatch(
                    parameter,
                    constraint,
                    "non-dimension type argument",
                    arg.span(),
                )),
            }
        }
        GenericConstraint::Index => match arg {
            ast::GenericArg::Index(index) => lower_index_expr(index, ctx).map(GenericArg::Index),
            ast::GenericArg::Nat(nat) => Err(HirLowerError::ExpectedIndexFoundNat {
                expression: crate::semantic_error::index::FoundNat::Expression(nat.clone()),
                span: nat.span(),
            }),
            ast::GenericArg::Type(_) | ast::GenericArg::Ambiguous(_) => {
                match lower_generic_arg_as_type_syntax(arg, parameter, constraint, ctx)? {
                    LoweredTypeSyntax::Slot(TypeSlot::Index(index)) => Ok(GenericArg::Index(index)),
                    LoweredTypeSyntax::Slot(TypeSlot::Value(_))
                    | LoweredTypeSyntax::Indexed { .. } => Err(generic_arg_sort_mismatch(
                        parameter,
                        constraint,
                        "non-index type argument",
                        arg.span(),
                    )),
                }
            }
        },
        GenericConstraint::Nat => match arg {
            ast::GenericArg::Nat(nat) => lower_nat_expr(nat, ctx).map(GenericArg::Nat),
            ast::GenericArg::Ambiguous(ambiguous) => {
                if let Some(actual) = non_nat_sort_for_ambiguous_arg(ambiguous, ctx) {
                    return Err(generic_arg_sort_mismatch(
                        parameter,
                        constraint,
                        actual,
                        ambiguous.span(),
                    ));
                }
                let nat = ambiguous_generic_arg_as_nat(ambiguous);
                lower_nat_expr(&nat, ctx).map(GenericArg::Nat)
            }
            ast::GenericArg::Type(_) => Err(generic_arg_sort_mismatch(
                parameter,
                constraint,
                "type argument",
                arg.span(),
            )),
            ast::GenericArg::Index(_) => Err(generic_arg_sort_mismatch(
                parameter,
                constraint,
                "Index argument",
                arg.span(),
            )),
        },
        GenericConstraint::Type => {
            let actual = match lower_generic_arg_as_type_syntax(arg, parameter, constraint, ctx)? {
                LoweredTypeSyntax::Slot(TypeSlot::Value(value_type)) => {
                    return Ok(GenericArg::Type(value_type));
                }
                LoweredTypeSyntax::Slot(TypeSlot::Index(_)) => "Index argument",
                LoweredTypeSyntax::Indexed { .. } => "indexed declaration type",
            };
            Err(generic_arg_sort_mismatch(
                parameter,
                constraint,
                actual,
                arg.span(),
            ))
        }
    }
}

fn lower_generic_arg_as_type_syntax(
    arg: &ast::GenericArg,
    parameter: &GenericParamName,
    constraint: GenericConstraint,
    ctx: ModuleScope<'_>,
) -> Result<LoweredTypeSyntax, HirLowerError> {
    match arg {
        ast::GenericArg::Type(type_expr) => lower_type_syntax(type_expr, ctx),
        ast::GenericArg::Ambiguous(ambiguous) => {
            lower_type_syntax(&ambiguous_generic_arg_as_type(ambiguous), ctx)
        }
        ast::GenericArg::Index(_) => Err(generic_arg_sort_mismatch(
            parameter,
            constraint,
            "Index argument",
            arg.span(),
        )),
        ast::GenericArg::Nat(_) => Err(generic_arg_sort_mismatch(
            parameter,
            constraint,
            "Nat argument",
            arg.span(),
        )),
    }
}

fn non_nat_sort_for_ambiguous_arg(
    arg: &ast::AmbiguousGenericArg,
    ctx: ModuleScope<'_>,
) -> Option<&'static str> {
    match arg {
        ast::AmbiguousGenericArg::Name(ident) => {
            if ctx.generic_scope.get_atom(ident.name.atom()).is_some() {
                return None;
            }
            let path = NamePath::local(ident.name.atom().clone());
            if ctx
                .resolver
                .resolve_index_path(ctx.owner, &path)
                .map(crate::resolve::symbols::SymbolRef::into_resolved)
                .is_ok()
            {
                return Some("Index argument");
            }
            if ctx
                .resolver
                .resolve_struct_type_path(ctx.owner, &path)
                .map(crate::resolve::symbols::SymbolRef::into_resolved)
                .is_ok()
            {
                return Some("Type argument");
            }
            if ctx
                .resolver
                .resolve_dimension_path(ctx.owner, &path)
                .map(crate::resolve::symbols::SymbolRef::into_resolved)
                .is_ok()
                || crate::resolve::prelude::prelude_type_scope()
                    .resolve_dimension_path(&path)
                    .is_some()
            {
                return Some("Dim argument");
            }
            None
        }
        ast::AmbiguousGenericArg::Mul(operands, _) => operands
            .iter()
            .find_map(|operand| non_nat_sort_for_ambiguous_arg(operand, ctx)),
    }
}

fn generic_arg_sort_mismatch(
    parameter: &GenericParamName,
    expected: GenericConstraint,
    actual: &'static str,
    span: Span,
) -> HirLowerError {
    HirLowerError::GenericArgumentSortMismatch {
        parameter: parameter.clone(),
        expected,
        actual,
        span,
    }
}

pub(crate) fn ambiguous_generic_arg_as_nat(arg: &ast::AmbiguousGenericArg) -> ast::NatExpr {
    match arg {
        ast::AmbiguousGenericArg::Name(ident) => ast::NatExpr::Var(ident.clone()),
        ast::AmbiguousGenericArg::Mul(operands, span) => {
            ast::NatExpr::Mul(operands.map_ref(ambiguous_generic_arg_as_nat), *span)
        }
    }
}

pub(crate) fn ambiguous_generic_arg_as_type(arg: &ast::AmbiguousGenericArg) -> ast::TypeExpr {
    let mut terms = Vec::new();
    collect_ambiguous_dim_terms(arg, &mut terms);
    ast::TypeExpr::unindexed(ast::ElementTypeExpr {
        kind: ast::TypeExprKind::DimExpr(ast::DimExpr {
            terms,
            span: arg.span(),
        }),
        constraints: Vec::new(),
        span: arg.span(),
    })
}

fn collect_ambiguous_dim_terms(arg: &ast::AmbiguousGenericArg, terms: &mut Vec<ast::DimExprItem>) {
    match arg {
        ast::AmbiguousGenericArg::Name(ident) => terms.push(ast::DimExprItem {
            op: ast::MulDivOp::Mul,
            term: ast::DimTerm {
                name: ast::DimTermName::Path(Spanned::new(
                    NamePath::local(ident.name.atom().clone()),
                    ident.span,
                )),
                power: None,
                span: ident.span,
            },
        }),
        ast::AmbiguousGenericArg::Mul(operands, _) => {
            for operand in operands {
                collect_ambiguous_dim_terms(operand, terms);
            }
        }
    }
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

fn lower_time_scale_arg(arg: &ast::TypeExpr) -> Result<TimeScale, HirLowerError> {
    let (ast::TypeExprKind::DimExpr(dim_expr), None) = (&arg.element.kind, &arg.indexes) else {
        return Err(HirLowerError::ExpectedTimeScale { span: arg.span });
    };
    let [item] = dim_expr.terms.as_slice() else {
        return Err(HirLowerError::ExpectedTimeScale { span: arg.span });
    };
    if item.term.power.is_some() {
        return Err(HirLowerError::ExpectedTimeScale { span: arg.span });
    }
    let Some((atom, atom_span)) = item
        .term
        .name
        .as_path()
        .and_then(|path| Some((path.value.as_bare()?, path.span)))
    else {
        return Err(HirLowerError::ExpectedTimeScale { span: arg.span });
    };
    atom.as_str()
        .parse::<TimeScale>()
        .map_err(|_| HirLowerError::UnknownTimeScale {
            name: atom.clone(),
            expected: "UTC, TAI, TT, TDB, ET, GPST, GST, BDT, QZSST",
            span: atom_span,
        })
}

fn lower_dim_expr_as_type(
    dim_expr: &ast::DimExpr,
    span: Span,
    ctx: ModuleScope<'_>,
) -> Result<TypeSlot, HirLowerError> {
    match lower_single_term_nominal_type(dim_expr, span, ctx)? {
        NominalTypeLookup::Found(slot) => Ok(slot),
        NominalTypeLookup::Absent { deferred_error } => match lower_dim_expr(dim_expr, ctx) {
            Ok(dim_expr) => Ok(TypeSlot::Value(ValueType::new(
                ValueTypeKind::DimExpr(dim_expr),
                span,
            ))),
            Err(HirLowerError::UnknownTypePath { path, slot, span }) => deferred_error.map_or(
                Err(HirLowerError::UnknownTypePath { path, slot, span }),
                |source| Err(HirLowerError::ModuleResolve { source, span }),
            ),
            Err(HirLowerError::ModuleResolve { source, span }) => {
                Err(HirLowerError::ModuleResolve {
                    source: type_position_wrong_universe(source),
                    span,
                })
            }
            Err(err) => Err(err),
        },
    }
}

fn lower_single_term_nominal_type(
    dim_expr: &ast::DimExpr,
    type_span: Span,
    ctx: ModuleScope<'_>,
) -> Result<NominalTypeLookup, HirLowerError> {
    let [item] = dim_expr.terms.as_slice() else {
        return Ok(NominalTypeLookup::absent());
    };
    if item.term.power.is_some() {
        return Ok(NominalTypeLookup::absent());
    }

    let Some(name) = item.term.name.as_path() else {
        return Ok(NominalTypeLookup::absent());
    };
    let path = &name.value;
    if let Some(atom) = path.as_bare()
        && let Some(binding) = ctx.generic_scope.get_atom(atom)
    {
        match binding.constraint {
            GenericConstraint::Type => {
                return Ok(NominalTypeLookup::Found(TypeSlot::Value(ValueType::new(
                    ValueTypeKind::GenericTypeParam(binding.spanned_id(name.span)),
                    type_span,
                ))));
            }
            // A concrete nominal type with the same leaf takes precedence;
            // if none exists, dimension lowering below resolves this binding.
            GenericConstraint::Dim => {}
            GenericConstraint::Index => {
                return Ok(NominalTypeLookup::Found(TypeSlot::Index(
                    IndexRef::GenericParam(binding.spanned_id(name.span)),
                )));
            }
            GenericConstraint::Nat => {
                return Err(HirLowerError::GenericConstraintMismatch {
                    name: GenericParamName::classify(atom.clone()),
                    actual: binding.constraint,
                    expected: &[GenericConstraint::Dim, GenericConstraint::Type],
                    span: name.span,
                });
            }
        }
    }

    let mut deferred_error = None;

    match resolve_optional(
        ctx.resolver
            .resolve_index_path(ctx.owner, path)
            .map(crate::resolve::symbols::SymbolRef::into_resolved),
    ) {
        LookupCandidate::Found(index) => {
            return Ok(NominalTypeLookup::Found(TypeSlot::Index(
                IndexRef::Concrete(Spanned::new(index, name.span)),
            )));
        }
        LookupCandidate::Absent => {}
        LookupCandidate::Error(source) => {
            deferred_error.get_or_insert(source);
        }
    }

    match resolve_optional(ctx.resolver.resolve_struct_type_path(ctx.owner, path)) {
        LookupCandidate::Found(symbol) => {
            let generic_params = symbol.kind();
            let struct_type = symbol.into_resolved();
            let kind = if generic_params.is_empty() {
                ValueTypeKind::Struct(Spanned::new(struct_type, name.span))
            } else {
                check_generic_arg_count(
                    GenericApplicationTarget::StructType(struct_type.clone()),
                    generic_params,
                    0,
                    name.span,
                )?;
                ValueTypeKind::TypeApplication {
                    name: Spanned::new(struct_type, name.span),
                    generic_args: Vec::new(),
                }
            };
            return Ok(NominalTypeLookup::Found(TypeSlot::Value(ValueType::new(
                kind, type_span,
            ))));
        }
        LookupCandidate::Absent => {}
        LookupCandidate::Error(source) => {
            deferred_error.get_or_insert(source);
        }
    }

    Ok(NominalTypeLookup::Absent { deferred_error })
}

fn lower_dim_expr(dim_expr: &ast::DimExpr, ctx: ModuleScope<'_>) -> Result<DimExpr, HirLowerError> {
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
    ctx: ModuleScope<'_>,
) -> Result<DimExprItem, HirLowerError> {
    Ok(DimExprItem {
        op: item.op,
        term: lower_dim_term(&item.term, ctx)?,
    })
}

pub(crate) fn lower_dim_term(
    term: &ast::DimTerm,
    ctx: ModuleScope<'_>,
) -> Result<DimTermRef, HirLowerError> {
    let name = match &term.name {
        ast::DimTermName::Dimensionless(_) => {
            return Ok(DimTermRef {
                target: DimTermTarget::Dimensionless,
                power: term.effective_power(),
                span: term.span,
            });
        }
        ast::DimTermName::Path(name) => name,
    };
    if let Some(atom) = name.value.as_bare()
        && let Some(binding) = ctx.generic_scope.get_atom(atom)
    {
        return match binding.constraint {
            GenericConstraint::Dim => Ok(DimTermRef {
                target: DimTermTarget::GenericParam(binding.spanned_id(name.span)),
                power: term.effective_power(),
                span: term.span,
            }),
            GenericConstraint::Index | GenericConstraint::Nat | GenericConstraint::Type => {
                Err(HirLowerError::GenericConstraintMismatch {
                    name: GenericParamName::classify(atom.clone()),
                    actual: binding.constraint,
                    expected: &[GenericConstraint::Dim],
                    span: name.span,
                })
            }
        };
    }

    let resolved = match ctx
        .resolver
        .resolve_dimension_path(ctx.owner, &name.value)
        .map(crate::resolve::symbols::SymbolRef::into_resolved)
    {
        Ok(resolved) => resolved,
        Err(ModuleResolveError::UnknownName { .. }) => {
            crate::resolve::prelude::prelude_type_scope()
                .resolve_dimension_path(&name.value)
                .ok_or_else(|| HirLowerError::UnknownTypePath {
                    path: name.value.clone(),
                    slot: TypePathSlot::DimensionTerm,
                    span: name.span,
                })?
        }
        Err(source) => {
            return Err(HirLowerError::ModuleResolve {
                source,
                span: name.span,
            });
        }
    };

    Ok(DimTermRef {
        target: DimTermTarget::Dimension(Spanned::new(resolved, name.span)),
        power: term.effective_power(),
        span: term.span,
    })
}

pub(crate) fn lower_index_expr(
    index: &ast::IndexExpr,
    ctx: ModuleScope<'_>,
) -> Result<IndexRef, HirLowerError> {
    match index {
        ast::IndexExpr::Name(path) => lower_index_expr_name(path, ctx),
        ast::IndexExpr::Finite { cardinality, .. } => {
            lower_nat_expr(cardinality, ctx).map(IndexRef::Finite)
        }
        ast::IndexExpr::BareNat(nat_expr) => Err(HirLowerError::ExpectedIndexFoundNat {
            expression: crate::semantic_error::index::FoundNat::Expression(nat_expr.clone()),
            span: nat_expr.span(),
        }),
    }
}

fn lower_index_expr_name(
    path: &Spanned<NamePath>,
    ctx: ModuleScope<'_>,
) -> Result<IndexRef, HirLowerError> {
    if let Some(atom) = path.value.as_bare()
        && let Some(binding) = ctx.generic_scope.get_atom(atom)
    {
        return match binding.constraint {
            GenericConstraint::Index => Ok(IndexRef::GenericParam(binding.spanned_id(path.span))),
            GenericConstraint::Nat => Err(HirLowerError::ExpectedIndexFoundNat {
                expression: crate::semantic_error::index::FoundNat::Parameter(atom.clone()),
                span: path.span,
            }),
            GenericConstraint::Dim | GenericConstraint::Type => {
                Err(HirLowerError::GenericConstraintMismatch {
                    name: GenericParamName::classify(atom.clone()),
                    actual: binding.constraint,
                    expected: &[GenericConstraint::Index],
                    span: path.span,
                })
            }
        };
    }

    ctx.resolver
        .resolve_index_path(ctx.owner, &path.value)
        .map(crate::resolve::symbols::SymbolRef::into_resolved)
        .map(|index| IndexRef::Concrete(Spanned::new(index, path.span)))
        .map_err(|source| match source {
            ModuleResolveError::UnknownName { .. } => HirLowerError::UnknownTypePath {
                path: path.value.clone(),
                slot: TypePathSlot::IndexAxis,
                span: path.span,
            },
            source => HirLowerError::ModuleResolve {
                source,
                span: path.span,
            },
        })
}

/// Lower a syntax type-level natural-number expression into its normalized
/// HIR form.
///
/// Normalization happens here, at the AST-to-HIR boundary, so every later
/// phase consumes one canonical [`NatPolyForm`] whose variables are
/// owner-qualified [`GenericParamId`]s.
///
/// # Errors
///
/// Returns [`HirLowerError`] if the expression references an unknown generic
/// parameter, a generic parameter whose constraint is not `Nat`, or if its
/// normalized coefficients overflow.
pub(crate) fn lower_nat_expr(
    nat_expr: &ast::NatExpr,
    ctx: ModuleScope<'_>,
) -> Result<Spanned<NatPolyForm>, HirLowerError> {
    let span = nat_expr.span();
    normalize_nat_expr(nat_expr, ctx)?
        .map(|form| Spanned::new(form, span))
        .map_err(|source| HirLowerError::NatOverflow { source, span })
}

/// Normalize one Nat expression. Name errors are reported before overflow,
/// in source order; overflow is reported once for the whole expression.
fn normalize_nat_expr(
    nat_expr: &ast::NatExpr,
    ctx: ModuleScope<'_>,
) -> Result<Result<NatPolyForm, NatOverflowError>, HirLowerError> {
    match nat_expr {
        ast::NatExpr::Literal(value, _) => Ok(Ok(NatPolyForm::from_constant(*value))),
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
                    expected: &[GenericConstraint::Nat],
                    span: ident.span,
                });
            }
            Ok(Ok(NatPolyForm::from_var(binding.id.clone())))
        }
        ast::NatExpr::Add(operands, _) => {
            let operands = operands
                .iter()
                .map(|operand| normalize_nat_expr(operand, ctx))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(operands
                .into_iter()
                .try_fold(NatPolyForm::from_constant(0), |sum, operand| {
                    sum.add(&operand?)
                }))
        }
        ast::NatExpr::Mul(operands, _) => {
            let operands = operands
                .iter()
                .map(|operand| normalize_nat_expr(operand, ctx))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(operands
                .into_iter()
                .try_fold(NatPolyForm::from_constant(1), |product, operand| {
                    product.mul(&operand?)
                }))
        }
    }
}

enum NominalTypeLookup {
    Found(TypeSlot),
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
        Err(
            ModuleResolveError::UnknownName { .. } | ModuleResolveError::WrongUniverseName { .. },
        ) => LookupCandidate::Absent,
        Err(err) => LookupCandidate::Error(err),
    }
}

fn type_position_wrong_universe(source: ModuleResolveError) -> ModuleResolveError {
    match source {
        ModuleResolveError::WrongUniverseName {
            owner,
            name,
            actual,
            ..
        } => ModuleResolveError::WrongUniverseName {
            owner,
            name,
            expected: SurfaceNameKind::Type,
            actual,
        },
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir::types::GenericParamOwner;
    use crate::resolved_name::ResolvedStructTypeName;
    use crate::syntax::parser::Parser;
    use crate::syntax::type_name::StructTypeName;

    fn desugared_source(source: &str) -> ast::File {
        let raw = Parser::new(source).parse_file().unwrap();
        crate::desugar::desugared_ast::File::from(raw)
    }

    fn first_import(file: &ast::File) -> &ast::ImportDecl {
        file.declarations
            .iter()
            .find_map(|decl| match &decl.kind {
                ast::DeclKind::Import(import) => Some(import),
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
        let lib_id = DagId::root_in_package("test", "lib");
        let main_id = DagId::root_in_package("test", "main");
        let lib = desugared_source(
            "pub base dim Length; pub index Phase = { Burn }; pub type Vec3<D: Dim> { Vec3(x: D) }",
        );
        let main = desugared_source(
            "import lib as physics; param v: physics::Vec3<physics::Length>[physics::Phase];",
        );
        let import = first_import(&main);

        let mut modules = crate::resolve::builder::TestModules::default();
        modules.add(lib_id.clone(), &lib.declarations);
        modules.add(main_id.clone(), &main.declarations);
        modules.import(&main_id, import, &lib_id);
        let resolver = modules.build().unwrap();

        let scope = GenericScope::new();
        let lowered = lower_decl_type(
            first_param_type(&main),
            ModuleScope::new(&main_id, &resolver, &scope),
        )
        .unwrap();

        let DeclType::Indexed {
            element: base,
            indexes,
            ..
        } = lowered
        else {
            panic!("expected indexed type, got {lowered:?}");
        };
        let [IndexRef::Concrete(index)] = indexes.as_slice() else {
            panic!("expected one concrete index, got {indexes:?}");
        };
        assert_eq!(index.value.owner(), &lib_id);
        assert_eq!(index.value.as_str(), "Phase");

        let ValueTypeKind::TypeApplication { name, generic_args } = base.kind else {
            panic!("expected type application, got {base:?}");
        };
        assert_eq!(name.value.owner(), &lib_id);
        assert_eq!(name.value.as_str(), "Vec3");

        let [GenericArg::Dim(DimArg::Expr(dim_expr))] = generic_args.as_slice() else {
            panic!("expected one dimension argument, got {generic_args:?}");
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
        let owner_id = DagId::root_in_package("test", "main");
        let file = desugared_source(
            "type Series<D: Dim, I: Index, N: Nat, F: Type> { Series(value: F, samples: D[I, Fin(N)]) }",
        );
        let mut modules = crate::resolve::builder::TestModules::default();
        modules.add(owner_id.clone(), &file.declarations);
        let resolver = modules.build().unwrap();

        let type_decl = first_type_decl(&file);
        let type_owner = GenericParamOwner::Type(ResolvedStructTypeName::for_test(
            owner_id.clone(),
            StructTypeName::expect_valid("Series"),
        ));
        let mut scope = GenericScope::new();
        for param in &type_decl.generic_params {
            scope
                .insert_binding(GenericParamBinding::new(
                    GenericParamId::new(type_owner.clone(), param.name.value.clone()),
                    param.constraint,
                    param.name.span,
                ))
                .unwrap();
        }

        let members = match &type_decl.body {
            ast::TypeDeclBody::Constructors(members) => members,
            ast::TypeDeclBody::Required => panic!("expected constructor body"),
        };
        let payload = members[0]
            .payload
            .as_ref()
            .expect("Series constructor should have payload");
        let value_type = lower_decl_type(
            &payload[0].type_ann,
            ModuleScope::new(&owner_id, &resolver, &scope),
        )
        .unwrap();
        let DeclType::Value(ValueType {
            kind: ValueTypeKind::GenericTypeParam(value_param),
            ..
        }) = value_type
        else {
            panic!("expected generic type parameter, got {value_type:?}");
        };
        assert_eq!(value_param.value.name.as_str(), "F");

        let samples_type = lower_decl_type(
            &payload[1].type_ann,
            ModuleScope::new(&owner_id, &resolver, &scope),
        )
        .unwrap();
        let DeclType::Indexed {
            element: base,
            indexes,
            ..
        } = samples_type
        else {
            panic!("expected indexed type, got {samples_type:?}");
        };
        let ValueTypeKind::DimExpr(dim_expr) = base.kind else {
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
            IndexRef::Finite(cardinality),
        ] = indexes.as_slice()
        else {
            panic!("expected generic index and Fin(N), got {indexes:?}");
        };
        assert_eq!(index_param.value.name.as_str(), "I");
        // The cardinality is normalized during lowering and names the
        // owner-qualified `N` of the same generic scope as `I`.
        assert_eq!(
            cardinality.value,
            NatPolyForm::from_var(GenericParamId::new(
                index_param.value.owner().clone(),
                GenericParamName::expect_valid("N"),
            ))
        );
    }

    const INDEX_PRELUDE: &str = "base dim Length; index M = { A, B }; index J = { X, Y }; \
         type Box<F: Type> { Box(value: Dimensionless) } \
         type Vec<D: Dim> { Vec(x: D) } \
         type Axis<I: Index> { Axis(v: Dimensionless[I]) }";

    /// Lower every payload field type of `type Grid<N: Nat, M: Nat, D: Dim>`.
    fn lower_grid_fields(
        fields: &str,
    ) -> (GenericParamOwner, Vec<Result<DeclType, HirLowerError>>) {
        let owner_id = DagId::root_in_package("test", "main");
        let file = desugared_source(&format!(
            "type Grid<N: Nat, M: Nat, D: Dim> {{ Grid({fields}) }}"
        ));
        let mut modules = crate::resolve::builder::TestModules::default();
        modules.add(owner_id.clone(), &file.declarations);
        let resolver = modules.build().unwrap();
        let type_decl = first_type_decl(&file);
        let type_owner = GenericParamOwner::Type(ResolvedStructTypeName::for_test(
            owner_id.clone(),
            StructTypeName::expect_valid("Grid"),
        ));
        let mut scope = GenericScope::new();
        for param in &type_decl.generic_params {
            scope
                .insert_binding(GenericParamBinding::new(
                    GenericParamId::new(type_owner.clone(), param.name.value.clone()),
                    param.constraint,
                    param.name.span,
                ))
                .unwrap();
        }
        let ast::TypeDeclBody::Constructors(members) = &type_decl.body else {
            panic!("expected constructor body");
        };
        let lowered = members[0]
            .payload
            .as_ref()
            .expect("Grid constructor should have payload")
            .iter()
            .map(|field| {
                lower_decl_type(
                    &field.type_ann,
                    ModuleScope::new(&owner_id, &resolver, &scope),
                )
            })
            .collect();
        (type_owner, lowered)
    }

    fn finite_cardinality(lowered: &Result<DeclType, HirLowerError>) -> &Spanned<NatPolyForm> {
        let Ok(DeclType::Indexed { indexes, .. }) = lowered else {
            panic!("expected an indexed type, got {lowered:?}");
        };
        let [IndexRef::Finite(cardinality)] = indexes.as_slice() else {
            panic!("expected one finite axis, got {indexes:?}");
        };
        cardinality
    }

    #[test]
    fn nat_expressions_are_normalized_during_lowering() {
        let (owner, lowered) =
            lower_grid_fields("a: Dimensionless[Fin(N * M + N + N)], b: Dimensionless[Fin(2 + 3)]");
        let n = NatPolyForm::from_var(GenericParamId::new(
            owner.clone(),
            GenericParamName::expect_valid("N"),
        ));
        let m = NatPolyForm::from_var(GenericParamId::new(
            owner,
            GenericParamName::expect_valid("M"),
        ));
        // N * M + N + N = M * N + 2 * N
        let expected = n
            .mul(&m)
            .and_then(|mn| mn.add(&n))
            .and_then(|sum| sum.add(&n))
            .unwrap();
        assert_eq!(finite_cardinality(&lowered[0]).value, expected);
        assert_eq!(
            finite_cardinality(&lowered[1]).value,
            NatPolyForm::from_constant(5)
        );
    }

    #[test]
    fn nat_lowering_reports_names_before_overflow() {
        let (_, lowered) = lower_grid_fields(
            "a: Dimensionless[Fin(18446744073709551615 + 1)], \
             b: Dimensionless[Fin(18446744073709551615 + 1 + K)], \
             c: Dimensionless[Fin(D + 1)]",
        );
        let Err(HirLowerError::NatOverflow { span, .. }) = &lowered[0] else {
            panic!("expected Nat overflow, got {:?}", lowered[0]);
        };
        assert!(span.len() > 1, "overflow blames the whole expression");
        assert!(matches!(
            &lowered[1],
            Err(HirLowerError::UnknownGenericParam { name, .. }) if name.as_str() == "K"
        ));
        assert!(matches!(
            &lowered[2],
            Err(HirLowerError::GenericConstraintMismatch { name, .. }) if name.as_str() == "D"
        ));
    }

    fn lower_param_type(param_type: &str) -> Result<DeclType, HirLowerError> {
        let owner_id = DagId::root_in_package("test", "main");
        let file = desugared_source(&format!("{INDEX_PRELUDE} param p: {param_type};"));
        let mut modules = crate::resolve::builder::TestModules::default();
        modules.add(owner_id.clone(), &file.declarations);
        let resolver = modules.build().unwrap();
        let scope = GenericScope::new();
        lower_decl_type(
            first_param_type(&file),
            ModuleScope::new(&owner_id, &resolver, &scope),
        )
    }

    #[test]
    fn index_names_are_rejected_in_every_value_type_position() {
        // (declaration type, expected diagnostic)
        let cases = [
            ("M", "index `M` cannot be used as a type"),
            ("M[J]", "index `M` cannot be used as a type"),
            (
                "Box<M>",
                "generic parameter `F` expects an argument of sort `Type`, got Index argument",
            ),
            (
                "Box<M[J]>",
                "generic parameter `F` expects an argument of sort `Type`, got indexed declaration type",
            ),
            (
                "Box<Length[J]>",
                "generic parameter `F` expects an argument of sort `Type`, got indexed declaration type",
            ),
            (
                "Vec<M>",
                "generic parameter `D` expects an argument of sort `Dim`, got non-dimension type argument",
            ),
            (
                "Complex<M>",
                "generic parameter `D` expects an argument of sort `Dim`, got non-dimension type argument",
            ),
            (
                "Axis<Length>",
                "generic parameter `I` expects an argument of sort `Index`, got non-index type argument",
            ),
            (
                "Axis<Length[J]>",
                "generic parameter `I` expects an argument of sort `Index`, got non-index type argument",
            ),
        ];
        for (param_type, expected) in cases {
            let error = lower_param_type(param_type).expect_err(param_type);
            assert_eq!(error.to_string(), expected, "for `{param_type}`");
        }
    }

    #[test]
    fn index_as_type_error_points_at_the_index_name() {
        let error = lower_param_type("M[J]").unwrap_err();
        let HirLowerError::IndexAsType { index } = &error else {
            panic!("expected IndexAsType, got {error:?}");
        };
        let IndexRef::Concrete(name) = index else {
            panic!("expected concrete index, got {index:?}");
        };
        assert_eq!(name.value.as_str(), "M");
        let source = format!("{INDEX_PRELUDE} param p: M[J];");
        assert_eq!(
            &source[name.span.offset()..name.span.offset() + name.span.len()],
            "M"
        );
    }

    #[test]
    fn index_positions_accept_index_names() {
        let DeclType::Value(ValueType {
            kind: ValueTypeKind::Key(IndexRef::Concrete(key)),
            ..
        }) = lower_param_type("Key<M>").unwrap()
        else {
            panic!("expected Key<M>");
        };
        assert_eq!(key.value.as_str(), "M");

        let DeclType::Value(ValueType {
            kind: ValueTypeKind::TypeApplication { generic_args, .. },
            ..
        }) = lower_param_type("Axis<M>").unwrap()
        else {
            panic!("expected Axis<M>");
        };
        assert!(matches!(
            generic_args.as_slice(),
            [GenericArg::Index(IndexRef::Concrete(name))] if name.value.as_str() == "M"
        ));

        let DeclType::Indexed { element, .. } = lower_param_type("Box<Length>[M]").unwrap() else {
            panic!("expected indexed Box<Length>");
        };
        assert!(matches!(
            element.kind,
            ValueTypeKind::TypeApplication { ref generic_args, .. }
                if matches!(generic_args.as_slice(), [GenericArg::Type(_)])
        ));
    }

    #[test]
    fn index_refs_render_their_leaf_spelling() {
        let owner = GenericParamOwner::Type(ResolvedStructTypeName::for_test(
            DagId::root_in_package("test", "main"),
            StructTypeName::expect_valid("T"),
        ));
        let span = Span::new(0, 1);
        let n = NatPolyForm::from_var(GenericParamId::new(
            owner.clone(),
            GenericParamName::expect_valid("N"),
        ));
        let sum = NatPolyForm::from_constant(2)
            .mul(&n)
            .and_then(|doubled| doubled.add(&NatPolyForm::from_constant(1)))
            .unwrap();
        assert_eq!(
            IndexRef::Finite(Spanned::new(sum, span)).to_string(),
            "Fin(2 * N + 1)"
        );
        assert_eq!(
            IndexRef::GenericParam(Spanned::new(
                GenericParamId::new(owner, GenericParamName::expect_valid("I")),
                span,
            ))
            .to_string(),
            "I"
        );
    }

    fn unknown_type_path(param_type: &str) -> (NamePath, TypePathSlot) {
        match lower_param_type(param_type) {
            Err(HirLowerError::UnknownTypePath { path, slot, .. }) => (path, slot),
            other => panic!("expected an unknown type path for `{param_type}`, got {other:?}"),
        }
    }

    #[test]
    fn unknown_type_paths_record_their_syntactic_slot() {
        let missing = NamePath::local(NameAtom::try_from("Missing").unwrap());
        for (param_type, slot) in [
            ("Dimensionless[Missing]", TypePathSlot::IndexAxis),
            ("Length[M, Missing]", TypePathSlot::IndexAxis),
            ("Missing", TypePathSlot::DimensionTerm),
            ("Length / Missing", TypePathSlot::DimensionTerm),
            ("Vec<Missing>", TypePathSlot::DimensionTerm),
            // An ambiguous product argument is a dimension product.
            ("Vec<Length * Missing>", TypePathSlot::DimensionTerm),
            ("Box<Length / Missing>", TypePathSlot::DimensionTerm),
            ("Missing<Length>", TypePathSlot::TypeApplication),
        ] {
            assert_eq!(
                unknown_type_path(param_type),
                (missing.clone(), slot),
                "`{param_type}`"
            );
        }
    }

    #[test]
    fn generic_arg_arity_accepts_the_defaultable_range() {
        let exact = GenericArgArity::exactly(1);
        assert!(!exact.accepts(0));
        assert!(exact.accepts(1));
        assert!(!exact.accepts(2));
        assert_eq!(exact.to_string(), "1");

        let ranged = GenericArgArity::of_defaults([false, true, true]);
        assert!(!ranged.accepts(0));
        assert!(ranged.accepts(1));
        assert!(ranged.accepts(3));
        assert!(!ranged.accepts(4));
        assert_eq!(ranged.to_string(), "1..3");
    }

    #[test]
    fn generic_arg_arity_requires_every_parameter_before_the_last_undefaulted_one() {
        assert_eq!(
            GenericArgArity::of_defaults([]),
            GenericArgArity::exactly(0)
        );
        assert_eq!(
            GenericArgArity::of_defaults([false, true]).to_string(),
            "1..2"
        );
        assert_eq!(
            GenericArgArity::of_defaults([true, false]),
            GenericArgArity::exactly(2)
        );
        assert_eq!(
            GenericArgArity::of_defaults([true, true]).to_string(),
            "0..2"
        );
    }

    #[test]
    fn wrong_generic_arg_count_names_its_target() {
        for (param_type, message) in [
            (
                "Complex<Length, Length>",
                "`Complex` expects 1 generic argument(s), got 2",
            ),
            ("Key<M, J>", "`Key` expects 1 generic argument(s), got 2"),
            (
                "Vec<Length, Length>",
                "`Vec` expects 1 generic argument(s), got 2",
            ),
            ("Vec", "`Vec` expects 1 generic argument(s), got 0"),
        ] {
            let error = lower_param_type(param_type).unwrap_err();
            assert!(
                matches!(error, HirLowerError::WrongGenericArgCount { .. }),
                "`{param_type}`: {error:?}"
            );
            assert_eq!(error.to_string(), message, "`{param_type}`");
        }
    }

    #[test]
    fn generic_constraint_mismatch_renders_every_accepted_constraint() {
        let mismatch =
            |expected: &'static [GenericConstraint]| HirLowerError::GenericConstraintMismatch {
                name: GenericParamName::expect_valid("N"),
                actual: GenericConstraint::Nat,
                expected,
                span: Span::new(0, 1),
            };
        assert_eq!(
            mismatch(&[GenericConstraint::Dim, GenericConstraint::Type]).to_string(),
            "generic parameter `N` has constraint `Nat`, but this position expects Dim or Type"
        );
        assert_eq!(
            mismatch(&[GenericConstraint::Index]).to_string(),
            "generic parameter `N` has constraint `Nat`, but this position expects Index"
        );
    }
}
