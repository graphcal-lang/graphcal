//! Expression lowering entry points and the structural lowering walk.

use crate::resolved_name::ResolvedDeclName;

use crate::dag_id::DagId;
use crate::desugar::desugared_ast as ast;
use crate::resolve::error::{ModuleResolveError, NameCategory};
use crate::resolve::namespace::Namespace;
use crate::syntax::ast::{InputBindingCategory, UnresolvedRef};
use crate::syntax::local_name::LocalName;
use crate::syntax::module_name::ScopedName;
use crate::syntax::names::NamePath;
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::phase::never;
use crate::syntax::span::{Span, Spanned};

use super::context::ExprLoweringContext;
use super::error::ExprLowerError;
use super::lowerer::ExprLowerer;
use super::resolve::{ResolvedCallable, spanned};
use crate::hir::expr::{
    AssertBody, DagCallIndexBinding, DagCallStaticBindings, Expr, ExprKind, FieldInit, ForBinding,
    ForBindingIndex, IndexArg, IndexVariantRef, MapEntry, MapEntryKey, MatchArm, MatchPattern,
    ParamBinding, PatternBinding, UnfoldRecurrence,
};
use crate::hir::expr::{CheckedAssertBody, CheckedExpr};
use crate::hir::lower::{lower_generic_args, lower_nat_expr};

/// Lower a syntax expression into HIR, accumulating diagnostics.
///
/// References that cannot be resolved become [`ExprKind::Error`] nodes and
/// their diagnostics are returned alongside the lowered tree, so consumers
/// that must keep working on incomplete code (the LSP) still get a tree with
/// spans for every position that did resolve.
#[must_use]
pub fn lower_expr_tolerant(
    expr: &ast::Expr,
    ctx: ExprLoweringContext<'_>,
) -> (Expr, Vec<ExprLowerError>) {
    let mut lowerer = ExprLowerer::new(ctx);
    let hir_expr = lowerer.lower_expr(expr);
    (hir_expr, lowerer.diagnostics)
}

/// Lower a syntax expression into HIR, rejecting unresolved references.
///
/// This is the batch-pipeline boundary: the lowered tree is guaranteed to
/// contain no [`ExprKind::Error`] node.
///
/// # Errors
///
/// Returns the first [`ExprLowerError`] if any expression-level reference
/// cannot be resolved to a canonical module identity or lexical local binding.
pub fn lower_expr(
    expr: &ast::Expr,
    ctx: ExprLoweringContext<'_>,
) -> Result<CheckedExpr, ExprLowerError> {
    let (lowered, mut diagnostics) = lower_expr_tolerant(expr, ctx);
    if diagnostics.is_empty() {
        CheckedExpr::finish(lowered).map_err(|source| ExprLowerError::ExpressionIdentity {
            source,
            span: expr.span,
        })
    } else {
        Err(diagnostics.swap_remove(0))
    }
}

/// Resolve a declaration reference without constructing an expression.
pub fn lower_graph_reference(
    reference: &Spanned<ScopedName>,
    ctx: ExprLoweringContext<'_>,
) -> Result<Spanned<ResolvedDeclName>, ExprLowerError> {
    ExprLowerer::new(ctx).resolve_graph_ref(reference)
}

/// Lower a syntax assertion body into HIR, accumulating diagnostics.
///
/// Each assertion body owns an independent lexical local-id space. Assertion
/// expressions cannot share locals across the `actual`/`expected`/`tolerance`
/// slots of a tolerance assertion, so each slot is lowered with a fresh lowerer.
#[must_use]
fn lower_assert_body_tolerant(
    body: &ast::AssertBody,
    ctx: ExprLoweringContext<'_>,
) -> (AssertBody, Vec<ExprLowerError>) {
    match body {
        ast::AssertBody::Expr(expr) => {
            let (lowered, diagnostics) = lower_expr_tolerant(expr, ctx);
            (AssertBody::Expr(Box::new(lowered)), diagnostics)
        }
        ast::AssertBody::Tolerance {
            actual,
            expected,
            tolerance,
        } => {
            let (actual, mut diagnostics) = lower_expr_tolerant(actual, ctx);
            let (expected, expected_diags) = lower_expr_tolerant(expected, ctx);
            let (tolerance, tolerance_diags) = lower_expr_tolerant(tolerance, ctx);
            diagnostics.extend(expected_diags);
            diagnostics.extend(tolerance_diags);
            (
                AssertBody::Tolerance {
                    actual: Box::new(actual),
                    expected: Box::new(expected),
                    tolerance: Box::new(tolerance),
                },
                diagnostics,
            )
        }
    }
}

/// Lower a syntax assertion body into HIR, rejecting unresolved references.
///
/// # Errors
///
/// Returns the first [`ExprLowerError`] if any reference cannot be resolved.
pub fn lower_assert_body(
    body: &ast::AssertBody,
    ctx: ExprLoweringContext<'_>,
) -> Result<CheckedAssertBody, ExprLowerError> {
    let (lowered, mut diagnostics) = lower_assert_body_tolerant(body, ctx);
    if diagnostics.is_empty() {
        let span = match &lowered {
            AssertBody::Expr(expr) => expr.span,
            AssertBody::Tolerance { actual, .. } => actual.span,
        };
        CheckedAssertBody::finish(lowered)
            .map_err(|source| ExprLowerError::ExpressionIdentity { source, span })
    } else {
        Err(diagnostics.swap_remove(0))
    }
}

fn static_binding_value_path(
    binding: &ast::ParamBinding,
    owner: &DagId,
) -> Result<NamePath, ExprLowerError> {
    match &binding.value.kind {
        ast::ExprKind::UnresolvedRef(UnresolvedRef::Path(path)) => Ok(path.to_name_path()),
        _ => Err(ExprLowerError::ModuleResolve {
            source: ModuleResolveError::UnknownName {
                owner: owner.clone(),
                category: NameCategory::Namespace(Namespace::Static),
                name: binding.name.name.atom().clone(),
            },
            span: binding.value.span,
        }),
    }
}

impl ExprLowerer<'_> {
    /// Lower one expression level, localizing failures.
    ///
    /// A failed parent becomes an [`ExprKind::Error`] node that retains every
    /// independently lowerable descendant expression. Any tentative child
    /// lowering performed before the parent failed is rolled back first, so
    /// diagnostics and lexical IDs are emitted exactly once.
    pub(super) fn lower_expr(&mut self, expr: &ast::Expr) -> Expr {
        let diagnostics_checkpoint = self.diagnostics.len();
        let next_local_checkpoint = self.next_local;
        let scope_depth_checkpoint = self.local_scopes.len();

        // Recursion choke point: lowering recurses once per tree level
        // (unbounded for left-nested operator chains).
        crate::stack::with_stack_growth(|| match self.lower_expr_inner(expr) {
            Ok(lowered) => lowered,
            Err(err) => {
                self.diagnostics.truncate(diagnostics_checkpoint);
                self.next_local = next_local_checkpoint;
                self.local_scopes.truncate(scope_depth_checkpoint);
                self.diagnostics.push(err);
                let children = self.lower_error_children(expr);
                Expr::new(ExprKind::Error { children }, expr.span)
            }
        })
    }

    /// Lower only expression-valued children of a failed parent.
    ///
    /// Binder metadata is intentionally ignored: when a `for`, `scan`,
    /// `unfold`, or match pattern fails, its lexical bindings were never
    /// established, so retaining a body must not fabricate those locals.
    pub(super) fn lower_error_children(&mut self, expr: &ast::Expr) -> Vec<Expr> {
        let children: Vec<&ast::Expr> = match &expr.kind {
            ast::ExprKind::Number(_)
            | ast::ExprKind::Integer(_)
            | ast::ExprKind::Bool(_)
            | ast::ExprKind::StringLiteral(_)
            | ast::ExprKind::UnresolvedRef(_)
            | ast::ExprKind::GraphRef(_)
            | ast::ExprKind::QuantityLiteral { .. } => Vec::new(),
            ast::ExprKind::BinOp { lhs, rhs, .. } => vec![lhs, rhs],
            ast::ExprKind::UnaryOp { operand, .. } => vec![operand],
            ast::ExprKind::FnCall { args, .. } => args.iter().collect(),
            ast::ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => vec![condition, then_branch, else_branch],
            ast::ExprKind::Convert { expr, .. }
            | ast::ExprKind::DisplayTimezone { expr, .. }
            | ast::ExprKind::FieldAccess { expr, .. } => vec![expr],
            ast::ExprKind::ConstructorCall { fields, .. } => {
                fields.iter().map(|field| &field.value).collect()
            }
            ast::ExprKind::MapLiteral { entries } => {
                entries.iter().map(|entry| &entry.value).collect()
            }
            ast::ExprKind::ForComp { body, .. } => vec![body],
            ast::ExprKind::IndexAccess { expr, args } => std::iter::once(expr.as_ref())
                .chain(args.iter().filter_map(|arg| match arg {
                    ast::IndexArg::Expr(expr) => Some(expr.as_ref()),
                    ast::IndexArg::Variant { .. } | ast::IndexArg::Var(_) => None,
                }))
                .collect(),
            ast::ExprKind::Scan {
                source, init, body, ..
            } => vec![source, init, body],
            ast::ExprKind::Unfold { init, body, .. } => vec![init, body],
            ast::ExprKind::KeyForm { arg, .. } => vec![arg],
            ast::ExprKind::Match { scrutinee, arms } => std::iter::once(scrutinee.as_ref())
                .chain(arms.iter().map(|arm| &arm.body))
                .collect(),
            ast::ExprKind::InlineDagRef { args, .. } => args.iter().map(|arg| &arg.value).collect(),
            #[expect(
                clippy::uninhabited_references,
                reason = "Sugar(Infallible) proves this arm unreachable"
            )]
            ast::ExprKind::Sugar(sugar) => never(*sugar),
        };
        children
            .into_iter()
            .map(|child| self.lower_expr(child))
            .collect()
    }

    #[expect(clippy::too_many_lines, reason = "exhaustive ExprKind lowering")]
    pub(super) fn lower_expr_inner(&mut self, expr: &ast::Expr) -> Result<Expr, ExprLowerError> {
        let kind = match &expr.kind {
            ast::ExprKind::Number(value) => ExprKind::Number(*value),
            ast::ExprKind::Integer(value) => ExprKind::Integer(*value),
            ast::ExprKind::Bool(value) => ExprKind::Bool(*value),
            ast::ExprKind::StringLiteral(value) => ExprKind::StringLiteral(value.clone()),
            ast::ExprKind::UnresolvedRef(unresolved) => match unresolved {
                UnresolvedRef::Path(path) => self.lower_unresolved_path(path)?,
                UnresolvedRef::IndexLabel { index, label, .. } => {
                    let index_path = index.to_name_path();
                    let resolved = self.resolve_index_variant_parts(
                        &index_path,
                        &label.value,
                        index.span(),
                        label.span,
                    )?;
                    ExprKind::VariantLiteral(IndexVariantRef {
                        variant: resolved,
                        index_span: Some(index.span()),
                        additional_index_spans: Vec::new(),
                        variant_span: label.span,
                    })
                }
            },
            ast::ExprKind::GraphRef(name) => self.lower_graph_ref(name)?,
            ast::ExprKind::BinOp { op, lhs, rhs } => ExprKind::BinOp {
                op: *op,
                lhs: Box::new(self.lower_expr(lhs)),
                rhs: Box::new(self.lower_expr(rhs)),
            },
            ast::ExprKind::UnaryOp { op, operand } => ExprKind::UnaryOp {
                op: *op,
                operand: Box::new(self.lower_expr(operand)),
            },
            ast::ExprKind::FnCall {
                callee,
                generic_args,
                args,
            } => match self.resolve_callable(callee)? {
                ResolvedCallable::Function(function_ref) => {
                    let function_ref = Self::lower_function_application(
                        function_ref,
                        generic_args,
                        callee.display_path(),
                        callee.span(),
                    )?;
                    Self::check_function_arity(&function_ref, args.len(), callee.span())?;
                    let args = self.lower_function_args(&function_ref, args)?;
                    ExprKind::FnCall {
                        callee: Spanned::new(function_ref, callee.span()),
                        args,
                    }
                }
                ResolvedCallable::Constructor(constructor) => {
                    let constructor = constructor.into_resolved();
                    return Err(if args.is_empty() {
                        ExprLowerError::EmptyParenthesizedConstructor {
                            constructor,
                            span: expr.span,
                        }
                    } else {
                        ExprLowerError::PositionalArgumentsOnConstructor {
                            constructor,
                            span: expr.span,
                        }
                    });
                }
            },
            ast::ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => ExprKind::If {
                condition: Box::new(self.lower_expr(condition)),
                then_branch: Box::new(self.lower_expr(then_branch)),
                else_branch: Box::new(self.lower_expr(else_branch)),
            },
            ast::ExprKind::QuantityLiteral { value, unit } => ExprKind::QuantityLiteral {
                value: *value,
                unit: self.lower_unit_expr(unit)?,
            },
            ast::ExprKind::Convert { expr, target } => ExprKind::Convert {
                expr: Box::new(self.lower_expr(expr)),
                target: self.lower_unit_expr(target)?,
            },
            ast::ExprKind::DisplayTimezone {
                expr: operand,
                timezone,
            } => ExprKind::DisplayTimezone {
                expr: Box::new(self.lower_expr(operand)),
                timezone: self.lower_iana_time_zone_id(timezone, expr.span)?,
            },
            ast::ExprKind::FieldAccess { expr, field } => ExprKind::FieldAccess {
                expr: Box::new(self.lower_expr(expr)),
                field: field.clone(),
            },
            ast::ExprKind::ConstructorCall {
                callee,
                generic_args,
                fields,
            } => {
                let constructor = match self.resolve_callable(callee)? {
                    ResolvedCallable::Constructor(constructor) => constructor,
                    ResolvedCallable::Function(function) => {
                        return Err(ExprLowerError::NamedArgumentsOnFunction {
                            function,
                            argument_names: fields
                                .iter()
                                .map(|field| field.name.value.clone())
                                .collect(),
                            span: expr.span,
                        });
                    }
                };
                let resolved = constructor.into_resolved();
                let lowered_args = lower_generic_args(
                    resolved.as_str(),
                    constructor.kind().generic_params(),
                    generic_args,
                    expr.span,
                    self.ctx.type_context(),
                )?;
                ExprKind::ConstructorCall {
                    callee: Spanned::new(resolved, callee.span()),
                    generic_args: lowered_args,
                    fields: fields
                        .iter()
                        .map(|field| self.lower_field_init(field))
                        .collect(),
                }
            }
            ast::ExprKind::MapLiteral { entries } => ExprKind::MapLiteral {
                entries: entries
                    .iter()
                    .map(|entry| self.lower_map_entry(entry, expr.span))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            ast::ExprKind::ForComp { bindings, body } => {
                let bindings = bindings
                    .iter()
                    .map(|binding| self.lower_for_binding(binding))
                    .collect::<Result<Vec<_>, _>>()?;
                let locals = bindings
                    .iter()
                    .map(|binding| binding.local.clone())
                    .collect::<Vec<_>>();
                self.push_scope(locals)?;
                let body = Box::new(self.lower_expr(body));
                self.pop_scope();
                ExprKind::ForComp { bindings, body }
            }
            ast::ExprKind::IndexAccess { expr, args } => ExprKind::IndexAccess {
                expr: Box::new(self.lower_expr(expr)),
                args: args.try_map_ref(|arg| self.lower_index_arg(arg))?,
            },
            ast::ExprKind::Scan {
                source,
                init,
                acc_name,
                val_name,
                body,
            } => {
                let source = Box::new(self.lower_expr(source));
                let init = Box::new(self.lower_expr(init));
                let acc = self.allocate_local(acc_name.value.clone(), acc_name.span)?;
                let val = self.allocate_local(val_name.value.clone(), val_name.span)?;
                self.push_scope(vec![acc.clone(), val.clone()])?;
                let body = Box::new(self.lower_expr(body));
                self.pop_scope();
                ExprKind::Scan {
                    source,
                    init,
                    acc,
                    val,
                    body,
                }
            }
            ast::ExprKind::Unfold {
                axis,
                init,
                prev_state_name,
                prev_index_name,
                index_name,
                body,
            } => {
                let resolved_axis = self
                    .ctx
                    .resolver
                    .resolve_index_path(self.ctx.owner, &axis.value)
                    .map(crate::resolve::symbols::SymbolRef::into_resolved)
                    .map_err(|source| ExprLowerError::ModuleResolve {
                        source,
                        span: axis.span,
                    })?;
                let init = Box::new(self.lower_expr(init));
                let prev_state =
                    self.allocate_local(prev_state_name.value.clone(), prev_state_name.span)?;
                let prev_index =
                    self.allocate_local(prev_index_name.value.clone(), prev_index_name.span)?;
                let index = self.allocate_local(index_name.value.clone(), index_name.span)?;
                self.push_scope(vec![prev_state.clone(), prev_index.clone(), index.clone()])?;
                let body = Box::new(self.lower_expr(body));
                self.pop_scope();
                ExprKind::Unfold {
                    recurrence: Box::new(UnfoldRecurrence {
                        axis: Spanned::new(resolved_axis, axis.span),
                        previous_state: prev_state,
                        previous_index: prev_index,
                        current_index: index,
                    }),
                    init,
                    body,
                }
            }
            ast::ExprKind::KeyForm { kind, axis, arg } => {
                let lowered_axis = match axis {
                    crate::syntax::ast::IndexExpr::Name(path) => {
                        let resolved = self
                            .ctx
                            .resolver
                            .resolve_index_path(self.ctx.owner, &path.value)
                            .map(crate::resolve::symbols::SymbolRef::into_resolved)
                            .map_err(|source| ExprLowerError::ModuleResolve {
                                source,
                                span: path.span,
                            })?;
                        ForBindingIndex::Named(Spanned::new(resolved, path.span))
                    }
                    crate::syntax::ast::IndexExpr::Finite { cardinality, span } => {
                        ForBindingIndex::Finite {
                            cardinality: lower_nat_expr(cardinality, self.ctx.type_context())?,
                            span: *span,
                        }
                    }
                    crate::syntax::ast::IndexExpr::BareNat(nat_expr) => {
                        return Err(crate::hir::lower::HirLowerError::ExpectedIndexFoundNat {
                            expression: nat_expr.to_string(),
                            span: nat_expr.span(),
                        }
                        .into());
                    }
                };
                ExprKind::KeyForm {
                    kind: *kind,
                    axis: lowered_axis,
                    axis_span: axis.span(),
                    arg: Box::new(self.lower_expr(arg)),
                }
            }
            ast::ExprKind::Match { scrutinee, arms } => ExprKind::Match {
                scrutinee: Box::new(self.lower_expr(scrutinee)),
                arms: arms
                    .iter()
                    .map(|arm| self.lower_match_arm(arm))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            ast::ExprKind::InlineDagRef { path, args, output } => {
                let target = self
                    .ctx
                    .resolver
                    .resolve_module_path(self.ctx.owner, path)
                    .map_err(|source| ExprLowerError::ModuleResolve {
                        source,
                        span: path.span(),
                    })?;
                let (lowered_args, static_bindings) =
                    self.lower_dag_call_bindings(&target, args)?;
                let output_path = NamePath::local(output.value.atom().clone());
                let lowered_output = self
                    .ctx
                    .resolver
                    .resolve_decl_path(&target, &output_path)
                    .map(crate::resolve::symbols::SymbolRef::into_resolved)
                    .map_err(|source| ExprLowerError::ModuleResolve {
                        source,
                        span: output.span,
                    })?;
                ExprKind::DagCall {
                    target: Spanned::new(target, path.span()),
                    args: lowered_args,
                    static_bindings,
                    output: Spanned::new(lowered_output, output.span),
                }
            }
            // `Sugar(_)` payload is `Infallible` post-desugar.
            #[expect(
                clippy::uninhabited_references,
                reason = "Sugar(Infallible) proves this arm unreachable"
            )]
            ast::ExprKind::Sugar(s) => never(*s),
        };
        Ok(Expr::new(kind, expr.span))
    }

    pub(super) fn lower_field_init(&mut self, field: &ast::FieldInit) -> FieldInit {
        FieldInit {
            name: field.name.clone(),
            value: self.lower_expr(&field.value),
        }
    }

    pub(super) fn lower_dag_call_bindings(
        &mut self,
        target: &DagId,
        bindings: &[ast::ParamBinding],
    ) -> Result<(Vec<ParamBinding>, DagCallStaticBindings), ExprLowerError> {
        let mut params = Vec::new();
        let mut static_bindings = DagCallStaticBindings::default();
        for binding in bindings {
            let input_path = NamePath::local(binding.name.name.atom().clone());
            match binding.category {
                InputBindingCategory::Unmarked => {
                    params.push(self.lower_param_binding(target, binding)?);
                }
                InputBindingCategory::Type => {
                    let resolver = self.ctx.resolver;
                    let input = spanned(
                        resolver.resolve_struct_type_path(target, &input_path),
                        binding.name.span,
                    )?
                    .into_resolved();
                    let value_path = static_binding_value_path(binding, self.ctx.owner)?;
                    let value = spanned(
                        resolver.resolve_struct_type_path(self.ctx.owner, &value_path),
                        binding.value.span,
                    )?
                    .into_resolved();
                    static_bindings.types.insert(input, value);
                }
                InputBindingCategory::Dimension => {
                    let input = spanned(
                        self.ctx
                            .resolver
                            .resolve_dimension_path(target, &input_path),
                        binding.name.span,
                    )?
                    .into_resolved();
                    let value_path = static_binding_value_path(binding, self.ctx.owner)?;
                    let value = match self
                        .ctx
                        .resolver
                        .resolve_dimension_path(self.ctx.owner, &value_path)
                        .map(crate::resolve::symbols::SymbolRef::into_resolved)
                    {
                        Ok(value) => value,
                        Err(source @ ModuleResolveError::UnknownName { .. }) => self
                            .ctx
                            .resolve_prelude_dimension_path(&value_path)
                            .ok_or(ExprLowerError::ModuleResolve {
                                source,
                                span: binding.value.span,
                            })?,
                        Err(source) => {
                            return Err(ExprLowerError::ModuleResolve {
                                source,
                                span: binding.value.span,
                            });
                        }
                    };
                    static_bindings.dimensions.insert(input, value);
                }
                InputBindingCategory::Index => {
                    let input = spanned(
                        self.ctx.resolver.resolve_index_path(target, &input_path),
                        binding.name.span,
                    )?
                    .into_resolved();
                    let value = match binding.value.index_binding_arg() {
                        Some(ast::IndexExpr::Name(path)) => self
                            .ctx
                            .resolver
                            .resolve_index_path(self.ctx.owner, &path.value)
                            .map(crate::resolve::symbols::SymbolRef::into_resolved)
                            .map(DagCallIndexBinding::Declared)
                            .map_err(|source| ExprLowerError::ModuleResolve {
                                source,
                                span: binding.value.span,
                            })?,
                        Some(ast::IndexExpr::Finite {
                            cardinality: ast::NatExpr::Literal(cardinality, _),
                            ..
                        }) => crate::registry::types::FiniteIndex::try_from_u64(cardinality)
                            .map(DagCallIndexBinding::Finite)
                            .map_err(|_| ExprLowerError::InvalidStaticBindingValue {
                                name: binding.name.name.atom().clone(),
                                span: binding.value.span,
                            })?,
                        _ => {
                            return Err(ExprLowerError::InvalidStaticBindingValue {
                                name: binding.name.name.atom().clone(),
                                span: binding.value.span,
                            });
                        }
                    };
                    static_bindings.indexes.insert(input, value);
                }
            }
        }
        Ok((params, static_bindings))
    }

    pub(super) fn lower_param_binding(
        &mut self,
        target: &DagId,
        binding: &ast::ParamBinding,
    ) -> Result<ParamBinding, ExprLowerError> {
        let path = NamePath::local(binding.name.name.atom().clone());
        let target_name = self
            .ctx
            .resolver
            .resolve_decl_path(target, &path)
            .map(crate::resolve::symbols::SymbolRef::into_resolved)
            .map_err(|source| ExprLowerError::ModuleResolve {
                source,
                span: binding.name.span,
            })?;
        Ok(ParamBinding {
            target: Spanned::new(target_name, binding.name.span),
            value: self.lower_expr(&binding.value),
        })
    }

    pub(super) fn lower_map_entry(
        &mut self,
        entry: &ast::MapEntry,
        map_span: Span,
    ) -> Result<MapEntry, ExprLowerError> {
        let keys = entry
            .keys
            .iter()
            .map(|key| self.lower_map_entry_key(key, map_span))
            .collect::<Result<Vec<_>, _>>()?;
        let mut keys = keys.into_iter();
        let Some(first) = keys.next() else {
            return Err(ExprLowerError::EmptyMapEntry {
                span: entry.value.span,
            });
        };
        Ok(MapEntry {
            keys: NonEmpty::new(first, keys.collect()),
            value: self.lower_expr(&entry.value),
        })
    }

    pub(super) fn lower_map_entry_key(
        &self,
        key: &ast::MapEntryKey,
        map_span: Span,
    ) -> Result<MapEntryKey, ExprLowerError> {
        match key {
            ast::MapEntryKey::Named {
                index,
                additional_index_spans,
                variant,
            } => {
                let resolved = self
                    .resolve_index_variant_parts(
                        &index.value,
                        &variant.value,
                        index.span,
                        variant.span,
                    )
                    .map_err(|err| match err {
                        ExprLowerError::ModuleResolve {
                            source: ModuleResolveError::UnknownIndexVariant { index, variant },
                            ..
                        } => ExprLowerError::ExtraMapVariant {
                            index_name: index.to_unowned_def_name(),
                            variant_name: variant,
                            span: map_span,
                        },
                        err => err,
                    })?;
                Ok(MapEntryKey::IndexVariant(IndexVariantRef {
                    variant: resolved,
                    index_span: Some(index.span),
                    additional_index_spans: additional_index_spans.clone(),
                    variant_span: variant.span,
                }))
            }
            ast::MapEntryKey::Finite { position, .. } => Ok(MapEntryKey::FinitePosition {
                size: position.value.cardinality(),
                position: Spanned::new(position.value.position(), position.span),
            }),
        }
    }

    pub(super) fn lower_for_binding(
        &mut self,
        binding: &ast::ForBinding,
    ) -> Result<ForBinding, ExprLowerError> {
        let local = self.allocate_local(binding.var.value.clone(), binding.var.span)?;
        let index = match &binding.index {
            ast::ForBindingIndex::Named(index) => {
                let resolved = self
                    .ctx
                    .resolver
                    .resolve_index_path(self.ctx.owner, &index.value)
                    .map(crate::resolve::symbols::SymbolRef::into_resolved)
                    .map_err(|source| ExprLowerError::ModuleResolve {
                        source,
                        span: index.span,
                    })?;
                ForBindingIndex::Named(Spanned::new(resolved, index.span))
            }
            ast::ForBindingIndex::Finite { cardinality, span } => ForBindingIndex::Finite {
                cardinality: lower_nat_expr(cardinality, self.ctx.type_context())?,
                span: *span,
            },
        };
        Ok(ForBinding { local, index })
    }

    pub(super) fn lower_index_arg(
        &mut self,
        arg: &ast::IndexArg,
    ) -> Result<IndexArg, ExprLowerError> {
        match arg {
            ast::IndexArg::Variant { index, variant } => {
                let resolved = self.resolve_index_variant_parts(
                    &index.value,
                    &variant.value,
                    index.span,
                    variant.span,
                )?;
                Ok(IndexArg::Variant(IndexVariantRef {
                    variant: resolved,
                    index_span: Some(index.span),
                    additional_index_spans: Vec::new(),
                    variant_span: variant.span,
                }))
            }
            ast::IndexArg::Var(ident) => Ok(IndexArg::Var(Spanned::new(
                self.lookup_local(&LocalName::classify(ident.name.atom().clone()), ident.span)?,
                ident.span,
            ))),
            ast::IndexArg::Expr(expr) => Ok(IndexArg::Expr(Box::new(self.lower_expr(expr)))),
        }
    }

    pub(super) fn lower_match_arm(
        &mut self,
        arm: &ast::MatchArm,
    ) -> Result<MatchArm, ExprLowerError> {
        let pattern = self.lower_match_pattern(&arm.pattern)?;
        self.push_scope(pattern.bound_locals())?;
        let body = self.lower_expr(&arm.body);
        self.pop_scope();
        Ok(MatchArm {
            pattern,
            body,
            span: arm.span,
        })
    }

    pub(super) fn lower_match_pattern(
        &mut self,
        pattern: &ast::MatchPattern,
    ) -> Result<MatchPattern, ExprLowerError> {
        match pattern {
            ast::MatchPattern::Constructor {
                name,
                bindings,
                span,
            } => Ok(MatchPattern::Constructor {
                constructor: Spanned::new(
                    self.ctx
                        .resolver
                        .resolve_constructor_path(
                            self.ctx.owner,
                            &NamePath::local(name.value.atom().clone()),
                        )
                        .map(crate::resolve::symbols::SymbolRef::into_resolved)
                        .map_err(|source| ExprLowerError::ModuleResolve {
                            source,
                            span: name.span,
                        })?,
                    name.span,
                ),
                bindings: self.lower_pattern_bindings(bindings)?,
                span: *span,
            }),
            ast::MatchPattern::IndexLabel {
                index,
                variant,
                span,
            } => {
                let resolved = self.resolve_index_variant_parts(
                    &index.value,
                    &variant.value,
                    index.span,
                    variant.span,
                )?;
                Ok(MatchPattern::IndexLabel {
                    variant: IndexVariantRef {
                        variant: resolved,
                        index_span: Some(index.span),
                        additional_index_spans: Vec::new(),
                        variant_span: variant.span,
                    },
                    span: *span,
                })
            }
            ast::MatchPattern::Path {
                path,
                bindings,
                span,
            } => self.lower_path_pattern(path, bindings, *span),
        }
    }

    pub(super) fn lower_path_pattern(
        &mut self,
        path: &crate::syntax::ast::IdentPath,
        bindings: &ast::PatternBindings<ast::PatternBinding>,
        span: Span,
    ) -> Result<MatchPattern, ExprLowerError> {
        let name_path = path.to_name_path();
        match self
            .ctx
            .resolver
            .resolve_constructor_path(self.ctx.owner, &name_path)
            .map(crate::resolve::symbols::SymbolRef::into_resolved)
        {
            Ok(constructor) => Ok(MatchPattern::Constructor {
                constructor: Spanned::new(constructor, path.span()),
                bindings: self.lower_pattern_bindings(bindings)?,
                span,
            }),
            Err(source) => match source {
                ModuleResolveError::UnknownName { .. }
                | ModuleResolveError::UnknownModuleAlias { .. }
                | ModuleResolveError::UnknownModule { .. } => Err(ExprLowerError::UnknownPattern {
                    path: path.display_path(),
                    span,
                }),
                source => Err(ExprLowerError::ModuleResolve { source, span }),
            },
        }
    }

    pub(super) fn lower_pattern_bindings(
        &mut self,
        bindings: &ast::PatternBindings<ast::PatternBinding>,
    ) -> Result<ast::PatternBindings<PatternBinding>, ExprLowerError> {
        match bindings {
            ast::PatternBindings::Bare => Ok(ast::PatternBindings::Bare),
            ast::PatternBindings::Parenthesized(bindings) => bindings
                .iter()
                .map(|binding| self.lower_pattern_binding(binding))
                .collect::<Result<Vec<_>, _>>()
                .map(ast::PatternBindings::Parenthesized),
        }
    }

    pub(super) fn lower_pattern_binding(
        &mut self,
        binding: &ast::PatternBinding,
    ) -> Result<PatternBinding, ExprLowerError> {
        match binding {
            ast::PatternBinding::Bind { field, var } => Ok(PatternBinding::Bind {
                field: field.clone(),
                local: self
                    .allocate_local(LocalName::classify(var.name.atom().clone()), var.span)?,
            }),
            ast::PatternBinding::Wildcard { field, span } => Ok(PatternBinding::Wildcard {
                field: field.clone(),
                span: *span,
            }),
        }
    }
}
