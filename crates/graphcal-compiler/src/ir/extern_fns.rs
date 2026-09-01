//! External plugin function signature resolution.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::desugared_ast::TypeExpr;
use crate::registry::error::GraphcalError;
use crate::registry::types::Registry;
use crate::syntax::names::NamePath;
use crate::syntax::span::Span;

// ---------------------------------------------------------------------------
// Extern plugin functions
// ---------------------------------------------------------------------------

/// A resolved extern function declared by an `import plugin` block.
///
/// The signature is stored in structural (base-dimension exponent) form —
/// this is what Phase B of the plugin plan (#25) compares against embedded
/// plugin manifests.
#[derive(Debug, Clone)]
pub struct ExternFunctionEntry {
    /// Canonical plugin identity (the `import plugin "…"` path string).
    pub plugin: crate::syntax::plugin::PluginPath,
    /// The alias the declaring file bound the plugin to (diagnostics only).
    pub alias: crate::syntax::module_name::ModuleAliasName,
    /// The function leaf name.
    pub name: crate::syntax::function_name::FnName,
    /// The resolved dimensional signature.
    pub signature: crate::function_signature::FunctionSignature,
    /// The nominal identity of the record type a struct-returning function
    /// produces. `Some` exactly when the signature's result is
    /// [`crate::function_signature::ValueKind::Struct`]: the manifest side
    /// is structural, but the declaration binds the shape to this type, and
    /// checking/evaluation carry the nominal identity from here.
    pub result_struct: Option<ExternStructResult>,
    /// Span of the function name inside the declaring block.
    pub name_span: Span,
    /// Span of the whole `fn ...;` declaration.
    pub decl_span: Span,
    /// Span of the `import plugin "…"` path string, for diagnostics about
    /// the plugin as a whole (unloadable file, hash mismatch, …).
    pub path_span: Span,
}

impl ExternFunctionEntry {
    /// Whether two entries describe the same callable definition.
    ///
    /// Source aliases and spans are diagnostic provenance, not signature
    /// identity, so independently compiled copies may differ in those fields.
    pub(crate) fn has_same_callable_definition(&self, other: &Self) -> bool {
        self.plugin == other.plugin
            && self.name == other.name
            && self.signature == other.signature
            && self.result_struct.as_ref().map(|result| &result.resolved)
                == other.result_struct.as_ref().map(|result| &result.resolved)
    }
}

/// The record type a struct-returning extern function was declared with.
#[derive(Debug, Clone)]
pub struct ExternStructResult {
    /// Canonical identity of the record type named at the declaration site.
    pub resolved: crate::syntax::type_name::ResolvedStructTypeName,
}

/// Resolve every `import plugin` block's declared signatures against the
/// frozen registry.
///
/// Dimension names resolve in the importing scope (prelude + user dims);
/// dimension variables come from each function's explicit `<...>` binders;
/// record result types resolve through the module resolver in the
/// importing scope.
pub(super) fn resolve_plugin_imports(
    decls: &[crate::desugar::desugared_ast::PluginImportDecl],
    registry: &Registry,
    owner: &crate::dag_id::DagId,
    resolver: &crate::syntax::module_resolve::ModuleResolver,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<crate::syntax::plugin::ExternFnKey, ExternFunctionEntry>, GraphcalError> {
    use std::collections::hash_map::Entry;

    let mut map = HashMap::new();
    for decl in decls {
        for function in &decl.functions {
            let (key, entry) =
                resolve_extern_function(decl, function, registry, owner, resolver, src)?;
            match map.entry(key) {
                Entry::Occupied(existing) => {
                    // The same plugin function may be declared under several
                    // aliases, but its signature is a single fact about the
                    // plugin — conflicting declarations are rejected.
                    // Comparison is structural: renaming dimension variables
                    // or parameters does not make a different signature.
                    let existing: &ExternFunctionEntry = existing.get();
                    if !existing.signature.structurally_equivalent(&entry.signature) {
                        return Err(GraphcalError::InvalidExternSignature {
                            message: format!(
                                "function `{}` of plugin \"{}\" is declared elsewhere with a different signature",
                                entry.name, entry.plugin
                            ),
                            src: src.clone(),
                            span: function.span.into(),
                        });
                    }
                    // A struct return is nominal at the declaration site:
                    // two aliases must also agree on WHICH record type the
                    // shared shape produces.
                    let existing_struct = existing.result_struct.as_ref().map(|s| &s.resolved);
                    let entry_struct = entry.result_struct.as_ref().map(|s| &s.resolved);
                    if existing_struct != entry_struct {
                        return Err(GraphcalError::InvalidExternSignature {
                            message: format!(
                                "function `{}` of plugin \"{}\" is declared elsewhere with a different result type",
                                entry.name, entry.plugin
                            ),
                            src: src.clone(),
                            span: function.span.into(),
                        });
                    }
                }
                Entry::Vacant(slot) => {
                    slot.insert(entry);
                }
            }
        }
    }
    Ok(map)
}

/// Resolve one extern-signature type annotation to a [`crate::function_signature::ValueKind`].
fn resolve_extern_value_kind(
    type_ann: &TypeExpr,
    dim_vars: &[crate::syntax::dimension::DimVarName],
    index_vars: &[crate::syntax::index_name::IndexVarName],
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<crate::function_signature::ValueKind, GraphcalError> {
    use crate::desugar::desugared_ast::TypeExprKind;
    use crate::function_signature::ValueKind;

    if !type_ann.constraints.is_empty() {
        return Err(GraphcalError::InvalidExternSignature {
            message: "domain constraints are not allowed in extern function signatures".to_string(),
            src: src.clone(),
            span: type_ann.span.into(),
        });
    }
    match &type_ann.kind {
        TypeExprKind::Bool => Ok(ValueKind::bool()),
        TypeExprKind::Int => Ok(ValueKind::int()),
        TypeExprKind::Dimensionless => Ok(ValueKind::dimensionless()),
        TypeExprKind::DimExpr(dim_expr) => {
            resolve_extern_dim_monomial(dim_expr, dim_vars, registry, src)
                .map(ValueKind::quantity_monomial)
        }
        TypeExprKind::Indexed { base, indexes } => {
            resolve_extern_array_kind(
                base,
                indexes.as_slice(),
                dim_vars,
                index_vars,
                registry,
                src,
            )
        }
        TypeExprKind::IndexLabel { .. }
        | TypeExprKind::Datetime
        | TypeExprKind::DatetimeApplication { .. }
        | TypeExprKind::ComplexApplication { .. }
        | TypeExprKind::KeyApplication { .. }
        | TypeExprKind::TypeApplication { .. } => Err(GraphcalError::InvalidExternSignature {
            message:
                "extern function signatures support Bool, Int, quantity types, and indexed scalar collections over one or more declared index variables"
                    .to_string(),
            src: src.clone(),
            span: type_ann.span.into(),
        }),
    }
}

/// Resolve one `fn` declaration of an `import plugin` block to its keyed
/// [`ExternFunctionEntry`].
fn resolve_extern_function(
    decl: &crate::desugar::desugared_ast::PluginImportDecl,
    function: &crate::desugar::desugared_ast::ExternFnDecl,
    registry: &Registry,
    owner: &crate::dag_id::DagId,
    resolver: &crate::syntax::module_resolve::ModuleResolver,
    src: &NamedSource<Arc<String>>,
) -> Result<(crate::syntax::plugin::ExternFnKey, ExternFunctionEntry), GraphcalError> {
    // Binder idents share one lexical namespace regardless of
    // constraint: `<D: Dim, D: Index>` is a duplicate declaration.
    let mut seen_binders: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for binder in &function.generics {
        if !seen_binders.insert(binder.name_str()) {
            return Err(GraphcalError::InvalidExternSignature {
                message: format!(
                    "generic binder `{}` is declared more than once",
                    binder.name_str()
                ),
                src: src.clone(),
                span: binder.span().into(),
            });
        }
    }
    let dim_vars: Vec<crate::syntax::dimension::DimVarName> = function
        .generics
        .iter()
        .filter_map(|binder| match binder {
            crate::syntax::ast::ExternGenericBinder::Dim(var) => Some(var.value.clone()),
            crate::syntax::ast::ExternGenericBinder::Index(_) => None,
        })
        .collect();
    let index_vars: Vec<crate::syntax::index_name::IndexVarName> = function
        .generics
        .iter()
        .filter_map(|binder| match binder {
            crate::syntax::ast::ExternGenericBinder::Index(var) => Some(var.value.clone()),
            crate::syntax::ast::ExternGenericBinder::Dim(_) => None,
        })
        .collect();
    let params = function
        .params
        .iter()
        .map(|param| {
            let kind =
                resolve_extern_value_kind(&param.type_ann, &dim_vars, &index_vars, registry, src)?;
            Ok(crate::function_signature::FunctionParam {
                name: param.name.value.clone(),
                kind,
            })
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;
    let (result, result_struct) = resolve_extern_result_kind(
        &function.result,
        &dim_vars,
        &index_vars,
        registry,
        owner,
        resolver,
        src,
    )?;
    let signature =
        crate::function_signature::FunctionSignature::try_new(dim_vars, index_vars, params, result)
            .map_err(|err| {
                if let crate::function_signature::SignatureError::DuplicateParamName {
                    name,
                    first,
                    duplicate,
                } = &err
                    && let (Some(first_param), Some(duplicate_param)) =
                        (function.params.get(*first), function.params.get(*duplicate))
                {
                    return GraphcalError::DuplicateExternParameter {
                        name: name.clone(),
                        src: src.clone(),
                        duplicate: duplicate_param.name.span.into(),
                        first: first_param.name.span.into(),
                    };
                }
                GraphcalError::InvalidExternSignature {
                    message: err.to_string(),
                    src: src.clone(),
                    span: function.span.into(),
                }
            })?;
    let key = crate::syntax::plugin::ExternFnKey {
        plugin: decl.path.value.clone(),
        name: function.name.value.clone(),
    };
    let entry = ExternFunctionEntry {
        plugin: decl.path.value.clone(),
        alias: decl.alias.value.clone(),
        name: function.name.value.clone(),
        signature,
        result_struct,
        name_span: function.name.span,
        decl_span: function.span,
        path_span: decl.path.span,
    };
    Ok((key, entry))
}

/// Resolve an extern RESULT type annotation: any parameter kind, or a
/// record struct type in scope.
///
/// A bare name in result position that is neither a declared dimension
/// variable nor a dimension is tried as a record type; the struct branch
/// returns the flattened field shape plus the nominal identity the
/// declaration binds it to.
fn resolve_extern_result_kind(
    type_ann: &TypeExpr,
    dim_vars: &[crate::syntax::dimension::DimVarName],
    index_vars: &[crate::syntax::index_name::IndexVarName],
    registry: &Registry,
    owner: &crate::dag_id::DagId,
    resolver: &crate::syntax::module_resolve::ModuleResolver,
    src: &NamedSource<Arc<String>>,
) -> Result<
    (
        crate::function_signature::ValueKind,
        Option<ExternStructResult>,
    ),
    GraphcalError,
> {
    use crate::desugar::desugared_ast::TypeExprKind;

    if type_ann.constraints.is_empty()
        && let TypeExprKind::DimExpr(dim_expr) = &type_ann.kind
        && let [item] = dim_expr.terms.as_slice()
        && item.term.power.is_none()
        && let Some(atom) = item.term.name.value.as_bare()
        && !dim_vars.iter().any(|var| var.as_str() == atom.as_str())
        && registry.dimensions.get_dimension(atom.as_str()).is_none()
    {
        // Not a dimension: the only remaining reading is a record type.
        return resolve_extern_struct_return(
            &item.term.name.value,
            item.term.name.span,
            registry,
            owner,
            resolver,
            src,
        );
    }
    if let TypeExprKind::TypeApplication { .. } = &type_ann.kind {
        return Err(GraphcalError::InvalidExternSignature {
            message: "generic struct returns are not supported in this phase; use a record \
                      type with concrete field types"
                .to_string(),
            src: src.clone(),
            span: type_ann.span.into(),
        });
    }
    resolve_extern_value_kind(type_ann, dim_vars, index_vars, registry, src)
        .map(|kind| (kind, None))
}

/// Resolve a record-type extern result: nominal identity through the
/// module resolver, flattened field shape from the registry's definition.
pub(super) fn resolve_extern_struct_return(
    path: &NamePath,
    span: Span,
    registry: &Registry,
    owner: &crate::dag_id::DagId,
    resolver: &crate::syntax::module_resolve::ModuleResolver,
    src: &NamedSource<Arc<String>>,
) -> Result<
    (
        crate::function_signature::ValueKind,
        Option<ExternStructResult>,
    ),
    GraphcalError,
> {
    use crate::function_signature::{StructShape, StructShapeField, ValueKind};

    let invalid = |message: String| GraphcalError::InvalidExternSignature {
        message,
        src: src.clone(),
        span: span.into(),
    };
    let Ok(resolved_type) = resolver.resolve_struct_type_path(owner, path) else {
        // Neither a dimension nor a type in scope: report it the way any
        // other unknown dimension-position name is reported.
        return Err(GraphcalError::UnknownDimension {
            name: path.clone(),
            src: src.clone(),
            span: span.into(),
        });
    };
    let leaf = resolved_type.to_unowned_def_name();
    let Some(type_def) = registry.types.get_type(leaf.as_str()) else {
        return Err(invalid(format!(
            "record type `{leaf}` is not available in this file's registry; extern struct \
             returns must use a type declared in (or imported into) the declaring file"
        )));
    };
    if !type_def.generic_params().is_empty() {
        return Err(invalid(format!(
            "record type `{leaf}` is generic; generic struct returns are not supported in \
             this phase"
        )));
    }
    let Some(fields) = type_def.record_fields() else {
        return Err(invalid(format!(
            "`{leaf}` is not a record type; extern struct returns need a single constructor \
             named after the type"
        )));
    };

    let shape_fields = fields
        .iter()
        .map(|field| {
            let kind = resolve_extern_struct_field(field, registry, src)?;
            Ok(StructShapeField {
                name: field.name().clone(),
                kind,
            })
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;
    let shape = StructShape::try_new(shape_fields).map_err(|err| invalid(err.to_string()))?;
    Ok((
        ValueKind::Struct(shape),
        Some(ExternStructResult {
            resolved: resolved_type,
        }),
    ))
}

/// Resolve one record field to its concrete boundary kind.
fn resolve_extern_struct_field(
    field: &crate::registry::type_def::StructField,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<crate::function_signature::StructFieldKind, GraphcalError> {
    use crate::desugar::desugared_ast::TypeExprKind;
    use crate::function_signature::StructFieldKind;

    let unsupported = || GraphcalError::InvalidExternSignature {
        message: format!(
            "field `{}` has a type that cannot cross the plugin boundary; struct-return \
             fields support Bool, Int, and quantity types in this phase",
            field.name()
        ),
        src: src.clone(),
        span: field.type_ann().span.into(),
    };
    if !field.type_ann().constraints.is_empty() {
        return Err(unsupported());
    }
    match &field.type_ann().kind {
        TypeExprKind::Bool => Ok(StructFieldKind::Bool),
        TypeExprKind::Int => Ok(StructFieldKind::Int),
        TypeExprKind::Dimensionless => Ok(StructFieldKind::Quantity(
            crate::dimension::Dimension::dimensionless(),
        )),
        TypeExprKind::DimExpr(dim_expr) => {
            // No dimension variables are in scope inside a record's fields;
            // the monomial is therefore concrete by construction.
            let monomial = resolve_extern_dim_monomial(dim_expr, &[], registry, src)?;
            Ok(StructFieldKind::Quantity(monomial.fixed))
        }
        TypeExprKind::IndexLabel { .. }
        | TypeExprKind::Datetime
        | TypeExprKind::DatetimeApplication { .. }
        | TypeExprKind::ComplexApplication { .. }
        | TypeExprKind::KeyApplication { .. }
        | TypeExprKind::Indexed { .. }
        | TypeExprKind::TypeApplication { .. } => Err(unsupported()),
    }
}

/// Resolve an array type annotation (`D[I]`, `Velocity[I, J]`) to
/// [`crate::function_signature::ValueKind::Indexed`].
///
/// Every axis must name one of the signature's `Index` binders. Concrete
/// declared/structural indexes remain declaration-site concerns: an extern
/// function is generic over the axes it receives, and every result axis must
/// come from an input.
fn resolve_extern_array_kind(
    base: &TypeExpr,
    indexes: &[crate::syntax::ast::IndexExpr],
    dim_vars: &[crate::syntax::dimension::DimVarName],
    index_vars: &[crate::syntax::index_name::IndexVarName],
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<crate::function_signature::ValueKind, GraphcalError> {
    use crate::desugar::desugared_ast::TypeExprKind;
    use crate::function_signature::{DimMonomial, ScalarValueKind, ValueKind};
    use crate::syntax::ast::IndexExpr;

    let resolved_indexes = indexes
        .iter()
        .map(|index_expr| {
            let index_var = match index_expr {
                IndexExpr::Name(name) => name.value.as_bare().and_then(|atom| {
                    index_vars
                        .iter()
                        .find(|var| var.as_str() == atom.as_str())
                        .cloned()
                }),
                IndexExpr::Finite { .. } | IndexExpr::BareNat(_) => None,
            };
            index_var.ok_or_else(|| GraphcalError::InvalidExternSignature {
                message: "extern array axes must name the signature's `Index` binders \
                          (concrete indexes and `Fin(N)` axes cannot appear in the declaration)"
                    .to_string(),
                src: src.clone(),
                span: index_expr.span().into(),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let indexes =
        crate::syntax::non_empty::NonEmpty::try_from_vec(resolved_indexes).map_err(|_| {
            GraphcalError::InvalidExternSignature {
                message: "extern arrays must have at least one axis".to_string(),
                src: src.clone(),
                span: type_ann_indexes_span(indexes, base),
            }
        })?;

    if !base.constraints.is_empty() {
        return Err(GraphcalError::InvalidExternSignature {
            message: "domain constraints are not allowed in extern function signatures".to_string(),
            src: src.clone(),
            span: base.span.into(),
        });
    }
    let element = match &base.kind {
        TypeExprKind::Bool => ScalarValueKind::Bool,
        TypeExprKind::Int => ScalarValueKind::Int,
        TypeExprKind::Dimensionless => ScalarValueKind::Quantity(DimMonomial::dimensionless()),
        TypeExprKind::DimExpr(dim_expr) => ScalarValueKind::Quantity(resolve_extern_dim_monomial(
            dim_expr, dim_vars, registry, src,
        )?),
        _ => {
            return Err(GraphcalError::InvalidExternSignature {
                message: "extern array elements must be Bool, Int, or quantities".to_string(),
                src: src.clone(),
                span: base.span.into(),
            });
        }
    };
    Ok(ValueKind::Indexed { element, indexes })
}

/// Span covering an array annotation's index list (falls back to the base
/// type's span when the list is empty, which the parser never produces).
fn type_ann_indexes_span(
    indexes: &[crate::syntax::ast::IndexExpr],
    base: &TypeExpr,
) -> miette::SourceSpan {
    match (indexes.first(), indexes.last()) {
        (Some(first), Some(last)) => first.span().merge(last.span()).into(),
        _ => base.span.into(),
    }
}

/// Resolve a dimension expression over declared dim variables and in-scope
/// dimensions into a [`crate::function_signature::DimMonomial`].
fn resolve_extern_dim_monomial(
    dim_expr: &crate::desugar::desugared_ast::DimExpr,
    dim_vars: &[crate::syntax::dimension::DimVarName],
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<crate::function_signature::DimMonomial, GraphcalError> {
    use crate::function_signature::DimVarPower;
    use crate::syntax::ast::MulDivOp;

    let overflow = |span: Span| GraphcalError::DimensionOverflow {
        src: src.clone(),
        span: span.into(),
    };

    let mut vars: Vec<DimVarPower> = Vec::new();
    let mut fixed = crate::dimension::Dimension::dimensionless();
    for item in &dim_expr.terms {
        let term = &item.term;
        let power = term.effective_power();
        let path = &term.name.value;
        if let Some(atom) = path.as_bare()
            && let Some(var) = dim_vars.iter().find(|v| v.as_str() == atom.as_str())
        {
            let power = match item.op {
                MulDivOp::Mul => power,
                MulDivOp::Div => power.checked_neg().map_err(|_| overflow(term.span))?,
            };
            vars.push(DimVarPower {
                var: var.clone(),
                power,
            });
            continue;
        }
        let Some(leaf) = path.as_bare() else {
            return Err(GraphcalError::InvalidExternSignature {
                message: format!(
                    "module-qualified dimension `{}` is not supported in extern function signatures",
                    path.display_path()
                ),
                src: src.clone(),
                span: term.span.into(),
            });
        };
        let Some(dim) = registry.dimensions.get_dimension(leaf.as_str()) else {
            return Err(GraphcalError::UnknownDimension {
                name: NamePath::from(leaf.clone()),
                src: src.clone(),
                span: term.name.span.into(),
            });
        };
        let powered = dim.pow(power).map_err(|_| overflow(term.span))?;
        fixed = match item.op {
            MulDivOp::Mul => fixed.checked_mul(&powered),
            MulDivOp::Div => fixed.checked_div(&powered),
        }
        .map_err(|_| overflow(term.span))?;
    }
    Ok(crate::function_signature::DimMonomial { vars, fixed })
}
