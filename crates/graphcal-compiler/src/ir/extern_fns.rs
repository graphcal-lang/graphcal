//! External plugin function signature resolution.

use crate::semantic_error::plugin::ExternSignatureError;
use std::collections::HashMap;
use std::sync::Arc;

use crate::desugar::desugared_ast::TypeExpr;
use crate::extern_struct_result::ExternStructResult;
use crate::ir::extern_function::{ExternFunctionEntry, merge_extern_function};
use crate::semantic_error::SemanticError;
use crate::semantic_error::dimension::DimensionError;
use crate::semantic_error::plugin::PluginError;
use crate::source_id::SourceId;
use crate::syntax::names::NamePath;
use crate::syntax::span::Span;

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// Extern plugin functions
// ---------------------------------------------------------------------------

/// The declaring module's view of the dimensions and record types an extern
/// signature may name.
pub(super) struct ExternSignatureScope<'s, 'a> {
    pub(super) owner: &'s crate::dag_id::DagId,
    /// Nominal types the declaring module owns, already lowered.
    pub(super) nominal_types: &'s crate::hir::nominal::NominalTypeRegistry,
    pub(super) definitions: &'s mut super::static_definitions::StaticDefinitionEvaluator<'a>,
}

impl ExternSignatureScope<'_, '_> {
    const fn resolver(&self) -> &crate::resolve::ModuleResolver {
        self.definitions.resolver()
    }

    /// The canonical definition of a nominal type visible to the declaring
    /// module: one of its own, or one lowered from its declaring module.
    fn nominal_type(
        &self,
        identity: &crate::resolved_name::ResolvedStructTypeName,
    ) -> Result<Option<Arc<crate::hir::nominal::NominalTypeDef>>, SemanticError> {
        if let Some(definition) = self.nominal_types.get(identity) {
            return Ok(Some(Arc::clone(definition)));
        }
        self.definitions
            .type_declaration(identity)
            .map(|(declaration, src)| {
                crate::outcome::without_cancellation(|cancellation| {
                    crate::hir::nominal_lower::lower_type_declaration(
                        declaration,
                        identity.clone(),
                        declaration.name.span,
                        src,
                        crate::hir::nominal_lower::NominalLowering {
                            resolver: self.resolver(),
                            cancellation,
                        },
                    )
                })
                .map(Arc::new)
            })
            .transpose()
    }

    /// What one dimension term of a signature denotes: a binder of the
    /// signature or a dimension defined in (or imported into) the declaring
    /// module. `None` when it denotes neither.
    fn dimension_term(
        &mut self,
        term: &crate::desugar::desugared_ast::DimTerm,
        generics: &ExternGenerics,
    ) -> Result<Option<ExternDimTerm>, SemanticError> {
        use crate::hir::types::DimTermTarget;

        let lowered = crate::hir::lower::lower_dim_term(term, self.type_context(generics));
        match lowered.map(|term| term.target) {
            Ok(DimTermTarget::GenericParam(id)) => Ok(generics
                .dims
                .get(&id.value)
                .cloned()
                .map(ExternDimTerm::Var)),
            Ok(DimTermTarget::Dimension(identity))
                if self.definitions.defines_dimension(&identity.value) =>
            {
                self.definitions
                    .dimension(&identity.value)
                    .map(|dimension| Some(ExternDimTerm::Fixed(dimension)))
            }
            Ok(DimTermTarget::Dimension(_)) | Err(_) => Ok(None),
        }
    }

    /// The index binder an array axis names, if it names one.
    fn index_var(
        &self,
        index: &crate::syntax::ast::IndexExpr,
        generics: &ExternGenerics,
    ) -> Option<crate::syntax::index_name::IndexVarName> {
        match crate::hir::lower::lower_index_expr(index, self.type_context(generics)) {
            Ok(crate::hir::types::IndexRef::GenericParam(id)) => {
                generics.indexes.get(&id.value).cloned()
            }
            Ok(
                crate::hir::types::IndexRef::Concrete(_) | crate::hir::types::IndexRef::Finite(_),
            )
            | Err(_) => None,
        }
    }

    const fn type_context<'c>(
        &'c self,
        generics: &'c ExternGenerics,
    ) -> crate::hir::lower::ModuleScope<'c> {
        crate::hir::lower::ModuleScope::new(self.owner, self.resolver(), &generics.scope)
    }
}

/// What one dimension term of an extern signature denotes.
enum ExternDimTerm {
    Var(crate::syntax::dimension::DimVarName),
    Fixed(crate::dimension::Dimension),
}

/// The `<...>` binders of one extern function as a lexical HIR generic
/// scope owned by the function.
struct ExternGenerics {
    scope: crate::hir::lower::GenericScope,
    dims: HashMap<crate::hir::types::GenericParamId, crate::syntax::dimension::DimVarName>,
    indexes: HashMap<crate::hir::types::GenericParamId, crate::syntax::index_name::IndexVarName>,
    /// Dimension binders in declaration order.
    dim_vars: Vec<crate::syntax::dimension::DimVarName>,
    /// Index binders in declaration order.
    index_vars: Vec<crate::syntax::index_name::IndexVarName>,
}

impl ExternGenerics {
    /// Bind `function`'s binders; binder names share one lexical namespace
    /// regardless of constraint (`<D: Dim, D: Index>` is a duplicate).
    fn new(
        key: &crate::plugin_identity::ExternFnKey,
        function: &crate::desugar::desugared_ast::ExternFnDecl,
        src: SourceId,
    ) -> Result<Self, SemanticError> {
        use crate::hir::lower::GenericParamBinding;
        use crate::hir::types::{GenericParamId, GenericParamOwner};
        use crate::syntax::ast::{ExternGenericBinder, GenericConstraint};
        use crate::syntax::type_name::GenericParamName;

        let owner = GenericParamOwner::ExternFn(key.clone());
        let mut generics = Self {
            scope: crate::hir::lower::GenericScope::new(),
            dims: HashMap::new(),
            indexes: HashMap::new(),
            dim_vars: Vec::new(),
            index_vars: Vec::new(),
        };
        for binder in &function.generics {
            let (atom, constraint, span) = match binder {
                ExternGenericBinder::Dim(var) => {
                    (var.value.atom(), GenericConstraint::Dim, var.span)
                }
                ExternGenericBinder::Index(var) => {
                    (var.value.atom(), GenericConstraint::Index, var.span)
                }
            };
            let id = GenericParamId::new(owner.clone(), GenericParamName::classify(atom.clone()));
            generics
                .scope
                .insert_binding(GenericParamBinding::new(id.clone(), constraint, span))
                .map_err(|_| {
                    SemanticError::located(
                        src,
                        binder.span(),
                        PluginError::InvalidExternSignature {
                            error: ExternSignatureError::DuplicateGenericBinder(atom.clone()),
                        },
                    )
                })?;
            match binder {
                ExternGenericBinder::Dim(var) => {
                    generics.dims.insert(id, var.value.clone());
                    generics.dim_vars.push(var.value.clone());
                }
                ExternGenericBinder::Index(var) => {
                    generics.indexes.insert(id, var.value.clone());
                    generics.index_vars.push(var.value.clone());
                }
            }
        }
        Ok(generics)
    }
}

/// Resolve every `import plugin` block's declared signatures in the
/// declaring module's scope.
///
/// Dimension names resolve in the importing scope (prelude + user dims);
/// dimension variables come from each function's explicit `<...>` binders;
/// record result types resolve through the module resolver in the
/// importing scope.
pub(super) fn resolve_plugin_imports(
    decls: &[crate::desugar::desugared_ast::PluginImportDecl],
    scope: &mut ExternSignatureScope<'_, '_>,
    src: SourceId,
) -> Result<HashMap<crate::plugin_identity::ExternFnKey, ExternFunctionEntry>, SemanticError> {
    let mut map = HashMap::new();
    for decl in decls {
        for function in &decl.functions {
            let entry = resolve_extern_function(decl, function, scope, src)?;
            merge_extern_function(&mut map, entry, src)?;
        }
    }
    Ok(map)
}

/// Resolve one extern-signature type annotation to a [`crate::function_signature::ParamKind`].
fn resolve_extern_value_kind(
    type_ann: &TypeExpr,
    generics: &ExternGenerics,
    scope: &mut ExternSignatureScope<'_, '_>,
    src: SourceId,
) -> Result<crate::function_signature::NamedParamKind, SemanticError> {
    use crate::desugar::desugared_ast::TypeExprKind;
    use crate::function_signature::ParamKind;

    if !type_ann.constraints.is_empty() {
        return Err(SemanticError::located(
            src,
            type_ann.span,
            PluginError::InvalidExternSignature {
                error: ExternSignatureError::DomainConstraint,
            },
        ));
    }
    match &type_ann.kind {
        TypeExprKind::Bool => Ok(ParamKind::bool()),
        TypeExprKind::Int => Ok(ParamKind::int()),
        TypeExprKind::Dimensionless => Ok(ParamKind::dimensionless()),
        TypeExprKind::DimExpr(dim_expr) => {
            resolve_extern_dim_monomial(dim_expr, generics, scope, src)
                .map(ParamKind::quantity_monomial)
        }
        TypeExprKind::Indexed { base, indexes } => {
            resolve_extern_array_kind(base, indexes.as_slice(), generics, scope, src)
        }
        TypeExprKind::IndexLabel { .. }
        | TypeExprKind::Datetime
        | TypeExprKind::DatetimeApplication { .. }
        | TypeExprKind::ComplexApplication { .. }
        | TypeExprKind::KeyApplication { .. }
        | TypeExprKind::TypeApplication { .. } => Err(SemanticError::located(
            src,
            type_ann.span,
            PluginError::InvalidExternSignature {
                error: ExternSignatureError::UnsupportedParameterType,
            },
        )),
    }
}

/// Resolve one `fn` declaration of an `import plugin` block to its keyed
/// [`ExternFunctionEntry`].
fn resolve_extern_function(
    decl: &crate::desugar::desugared_ast::PluginImportDecl,
    function: &crate::desugar::desugared_ast::ExternFnDecl,
    scope: &mut ExternSignatureScope<'_, '_>,
    src: SourceId,
) -> Result<ExternFunctionEntry, SemanticError> {
    let key = crate::plugin_identity::ExternFnKey {
        plugin: crate::plugin_identity::PluginIdentity::resolve(
            &decl.path.value,
            scope.owner.package(),
        ),
        name: function.name.value.clone(),
    };
    let generics = ExternGenerics::new(&key, function, src)?;
    let params = function
        .params
        .iter()
        .map(|param| {
            let kind = resolve_extern_value_kind(&param.type_ann, &generics, scope, src)?;
            Ok(crate::function_signature::FunctionParam {
                name: param.name.value.clone(),
                kind,
            })
        })
        .collect::<Result<Vec<_>, SemanticError>>()?;
    let result = resolve_extern_result_kind(&function.result, &generics, scope, src)?;
    let signature = crate::function_signature::FunctionSignature::try_from_parts(
        generics.dim_vars,
        generics.index_vars,
        params,
        result,
    )
    .map_err(|err| {
        if let crate::function_signature::SignatureError::DuplicateParamName {
            name,
            first,
            duplicate,
        } = &err
            && let (Some(first_param), Some(duplicate_param)) =
                (function.params.get(*first), function.params.get(*duplicate))
        {
            return SemanticError::located(
                src,
                duplicate_param.name.span,
                PluginError::DuplicateExternParameter {
                    name: name.clone(),
                    first: first_param.name.span,
                },
            );
        }
        SemanticError::located(
            src,
            function.span,
            PluginError::InvalidExternSignature {
                error: ExternSignatureError::Signature(err),
            },
        )
    })?;
    Ok(ExternFunctionEntry {
        plugin: key.plugin,
        alias: decl.alias.value.clone(),
        name: key.name,
        signature,
        name_span: function.name.span,
        decl_span: function.span,
        path_span: decl.path.span,
    })
}

/// Resolve an extern RESULT type annotation: any parameter kind, or a
/// record struct type in scope.
///
/// A single name in result position that is neither a dimension binder of
/// the signature nor a dimension is tried as a record type; the struct branch
/// returns the flattened field shape plus the nominal identity the
/// declaration binds it to.
fn resolve_extern_result_kind(
    type_ann: &TypeExpr,
    generics: &ExternGenerics,
    scope: &mut ExternSignatureScope<'_, '_>,
    src: SourceId,
) -> Result<crate::function_signature::NamedResultKind<ExternStructResult>, SemanticError> {
    use crate::desugar::desugared_ast::TypeExprKind;

    if type_ann.constraints.is_empty()
        && let TypeExprKind::DimExpr(dim_expr) = &type_ann.kind
        && let [item] = dim_expr.terms.as_slice()
        && item.term.power.is_none()
        && scope.dimension_term(&item.term, generics)?.is_none()
    {
        // Not a dimension: the only remaining reading is a record type.
        return resolve_extern_struct_return(
            &item.term.name.value,
            item.term.name.span,
            scope,
            src,
        );
    }
    if let TypeExprKind::TypeApplication { .. } = &type_ann.kind {
        return Err(SemanticError::located(
            src,
            type_ann.span,
            PluginError::InvalidExternSignature {
                error: ExternSignatureError::GenericStructReturn,
            },
        ));
    }
    resolve_extern_value_kind(type_ann, generics, scope, src).map(Into::into)
}

/// Resolve a record-type extern result: nominal identity through the
/// module resolver, flattened field shape from its canonical HIR definition.
pub(super) fn resolve_extern_struct_return(
    path: &NamePath,
    span: Span,
    scope: &mut ExternSignatureScope<'_, '_>,
    src: SourceId,
) -> Result<crate::function_signature::NamedResultKind<ExternStructResult>, SemanticError> {
    use crate::function_signature::{ResultKind, StructShape, StructShapeField};

    let invalid = |error: ExternSignatureError| {
        SemanticError::located(src, span, PluginError::InvalidExternSignature { error })
    };
    let Ok(resolved_type) = scope
        .resolver()
        .resolve_struct_type_path(scope.owner, path)
        .map(crate::resolve::symbols::SymbolRef::into_resolved)
    else {
        // Neither a dimension nor a type in scope: report it the way any
        // other unknown dimension-position name is reported.
        return Err(SemanticError::located(
            src,
            span,
            DimensionError::UnknownDimension { name: path.clone() },
        ));
    };
    let leaf = resolved_type.to_unowned_def_name();
    let Some(type_def) = scope.nominal_type(&resolved_type)? else {
        return Err(invalid(ExternSignatureError::UndeclaredRecordType(leaf)));
    };
    if !type_def.generic_params().is_empty() {
        return Err(invalid(ExternSignatureError::GenericRecordType(leaf)));
    }
    let (Some(fields), Some([record])) = (type_def.record_fields(), type_def.union_members())
    else {
        return Err(invalid(ExternSignatureError::NotARecordType(leaf)));
    };

    let shape_fields = fields
        .iter()
        .map(|field| {
            let kind = resolve_extern_struct_field(field, scope, src)?;
            Ok(StructShapeField {
                name: field.name().clone(),
                kind,
            })
        })
        .collect::<Result<Vec<_>, SemanticError>>()?;
    let shape = StructShape::try_new(shape_fields)
        .map_err(|err| invalid(ExternSignatureError::Signature(err)))?;
    Ok(ResultKind::Struct(ExternStructResult::new(
        resolved_type,
        record.name(),
        shape,
    )))
}

/// Resolve one record field to its concrete boundary kind.
fn resolve_extern_struct_field(
    field: &crate::hir::nominal::NominalField,
    scope: &mut ExternSignatureScope<'_, '_>,
    src: SourceId,
) -> Result<crate::function_signature::StructFieldKind, SemanticError> {
    use crate::function_signature::StructFieldKind;
    use crate::hir::types::{BuiltinType, DeclType, DimTermTarget, ValueTypeKind};

    let annotation = field.type_annotation();
    let unsupported = || {
        SemanticError::located(
            src,
            annotation.span,
            PluginError::InvalidExternSignature {
                error: ExternSignatureError::UnsupportedStructField(field.name().clone()),
            },
        )
    };
    if !annotation.domain_bounds.is_empty() {
        return Err(unsupported());
    }
    let DeclType::Value(value) = &annotation.decl_type else {
        return Err(unsupported());
    };
    match &value.kind {
        ValueTypeKind::Builtin(BuiltinType::Bool) => Ok(StructFieldKind::Bool),
        ValueTypeKind::Builtin(BuiltinType::Int) => Ok(StructFieldKind::Int),
        ValueTypeKind::Builtin(BuiltinType::Dimensionless) => Ok(StructFieldKind::Quantity(
            crate::dimension::Dimension::dimensionless(),
        )),
        ValueTypeKind::DimExpr(expr) => {
            // No dimension variables are in scope inside a record's fields;
            // the dimension is therefore concrete by construction.
            let overflow =
                || SemanticError::located(src, annotation.span, DimensionError::DimensionOverflow);
            let mut dimension = crate::dimension::Dimension::dimensionless();
            for item in &expr.terms {
                let DimTermTarget::Dimension(name) = &item.term.target else {
                    return Err(unsupported());
                };
                let factor = scope
                    .definitions
                    .dimension(&name.value)?
                    .pow(item.term.power)
                    .map_err(|_| overflow())?;
                dimension = match item.op {
                    crate::syntax::ast::MulDivOp::Mul => dimension.checked_mul(&factor),
                    crate::syntax::ast::MulDivOp::Div => dimension.checked_div(&factor),
                }
                .map_err(|_| overflow())?;
            }
            Ok(StructFieldKind::Quantity(dimension))
        }
        ValueTypeKind::Builtin(BuiltinType::Datetime(_))
        | ValueTypeKind::Struct(_)
        | ValueTypeKind::GenericTypeParam(_)
        | ValueTypeKind::Complex(_)
        | ValueTypeKind::Key(_)
        | ValueTypeKind::TypeApplication { .. } => Err(unsupported()),
    }
}

/// Resolve an array type annotation (`D[I]`, `Velocity[I, J]`) to
/// [`crate::function_signature::ParamKind::Indexed`].
///
/// Every axis must name one of the signature's `Index` binders. Concrete
/// declared/structural indexes remain declaration-site concerns: an extern
/// function is generic over the axes it receives, and every result axis must
/// come from an input.
fn resolve_extern_array_kind(
    base: &TypeExpr,
    indexes: &[crate::syntax::ast::IndexExpr],
    generics: &ExternGenerics,
    scope: &mut ExternSignatureScope<'_, '_>,
    src: SourceId,
) -> Result<crate::function_signature::NamedParamKind, SemanticError> {
    use crate::desugar::desugared_ast::TypeExprKind;
    use crate::function_signature::{DimMonomial, ParamKind, ScalarValueKind};

    let resolved_indexes = indexes
        .iter()
        .map(|index_expr| {
            scope.index_var(index_expr, generics).ok_or_else(|| {
                SemanticError::located(
                    src,
                    index_expr.span(),
                    PluginError::InvalidExternSignature {
                        error: ExternSignatureError::ArrayAxesMustBeBinders,
                    },
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let indexes =
        crate::syntax::non_empty::NonEmpty::try_from_vec(resolved_indexes).map_err(|_| {
            SemanticError::located(
                src,
                type_ann_indexes_span(indexes, base),
                PluginError::InvalidExternSignature {
                    error: ExternSignatureError::ArrayWithoutAxes,
                },
            )
        })?;

    if !base.constraints.is_empty() {
        return Err(SemanticError::located(
            src,
            base.span,
            PluginError::InvalidExternSignature {
                error: ExternSignatureError::DomainConstraint,
            },
        ));
    }
    let element = match &base.kind {
        TypeExprKind::Bool => ScalarValueKind::Bool,
        TypeExprKind::Int => ScalarValueKind::Int,
        TypeExprKind::Dimensionless => ScalarValueKind::Quantity(DimMonomial::dimensionless()),
        TypeExprKind::DimExpr(dim_expr) => {
            ScalarValueKind::Quantity(resolve_extern_dim_monomial(dim_expr, generics, scope, src)?)
        }
        _ => {
            return Err(SemanticError::located(
                src,
                base.span,
                PluginError::InvalidExternSignature {
                    error: ExternSignatureError::ArrayElementKind,
                },
            ));
        }
    };
    Ok(ParamKind::Indexed { element, indexes })
}

/// Span covering an array annotation's index list (falls back to the base
/// type's span when the list is empty, which the parser never produces).
fn type_ann_indexes_span(
    indexes: &[crate::syntax::ast::IndexExpr],
    base: &TypeExpr,
) -> crate::syntax::span::Span {
    match (indexes.first(), indexes.last()) {
        (Some(first), Some(last)) => first.span().merge(last.span()),
        _ => base.span,
    }
}

/// Resolve a dimension expression over the signature's dimension binders and
/// the dimensions of the declaring module into a
/// [`crate::function_signature::DimMonomial`].
fn resolve_extern_dim_monomial(
    dim_expr: &crate::desugar::desugared_ast::DimExpr,
    generics: &ExternGenerics,
    scope: &mut ExternSignatureScope<'_, '_>,
    src: SourceId,
) -> Result<crate::function_signature::NamedDimMonomial, SemanticError> {
    use crate::syntax::ast::MulDivOp;

    let overflow =
        |span: Span| SemanticError::located(src, span, DimensionError::DimensionOverflow);

    let mut vars = Vec::new();
    let mut fixed = crate::dimension::Dimension::dimensionless();
    for item in &dim_expr.terms {
        let term = &item.term;
        let power = term.effective_power();
        match scope.dimension_term(term, generics)? {
            Some(ExternDimTerm::Var(var)) => {
                let power = match item.op {
                    MulDivOp::Mul => power,
                    MulDivOp::Div => -power,
                };
                vars.push((var, power));
            }
            Some(ExternDimTerm::Fixed(dim)) => {
                let powered = dim.pow(power).map_err(|_| overflow(term.span))?;
                fixed = match item.op {
                    MulDivOp::Mul => fixed.checked_mul(&powered),
                    MulDivOp::Div => fixed.checked_div(&powered),
                }
                .map_err(|_| overflow(term.span))?;
            }
            None => {
                return Err(SemanticError::located(
                    src,
                    term.name.span,
                    DimensionError::UnknownDimension {
                        name: term.name.value.clone(),
                    },
                ));
            }
        }
    }
    crate::function_signature::DimMonomial::try_new(vars, fixed).map_err(|error| {
        SemanticError::located(
            src,
            dim_expr.span,
            PluginError::InvalidExternSignature {
                error: ExternSignatureError::Signature(
                    crate::function_signature::SignatureError::from(error),
                ),
            },
        )
    })
}
