//! Generic type-system leakage analysis at include boundaries.

#[allow(
    clippy::wildcard_imports,
    clippy::allow_attributes,
    reason = "leakage checks consume project compiler model types"
)]
use super::*;
use graphcal_compiler::desugar::desugared_ast::DeclKind;

/// Collect the set of type-system names declared locally in a file
/// (dims, units, indexes, types). Used to distinguish a private-local
/// symbol (V = private at the importer) from a builtin or cross-file
/// symbol for the V006 check.
pub(super) fn collect_local_type_names(
    file: &graphcal_compiler::desugar::desugared_ast::File,
) -> HashMap<String, &'static str> {
    let mut names = HashMap::new();
    for decl in &file.declarations {
        let (name, kind) = match &decl.kind {
            DeclKind::BaseDimension(d) => (d.name.value.to_string(), "dim"),
            DeclKind::Dimension(d) => (d.name.value.to_string(), "dim"),
            DeclKind::Unit(u) => (u.name.value.to_string(), "unit"),
            DeclKind::Index(idx) => (idx.name.value.to_string(), "index"),
            DeclKind::Type(t) => (t.name.value.to_string(), "type"),
            _ => continue,
        };
        names.insert(name, kind);
    }
    names
}

/// Walk a `TypeExpr` collecting every type-system name reference
/// (dimension / type / index / type-application). Used by the V006
/// check to decide which names in a re-exported signature need a
/// visibility review at the importing site.
fn collect_type_expr_names(
    type_expr: &graphcal_compiler::desugar::desugared_ast::TypeExpr,
    refs: &mut Vec<String>,
) {
    use graphcal_compiler::desugar::desugared_ast::{IndexExpr, TypeExprKind};
    match &type_expr.kind {
        TypeExprKind::DimExpr(dim_expr) => {
            for item in &dim_expr.terms {
                refs.push(item.term.name.value.display_path());
            }
        }
        TypeExprKind::Indexed { base, indexes } => {
            collect_type_expr_names(base, refs);
            for idx in indexes {
                if let IndexExpr::Name(path) = idx {
                    refs.push(path.value.display_path());
                }
            }
        }
        TypeExprKind::TypeApplication { name, generic_args } => {
            refs.push(name.value.display_path());
            for arg in generic_args {
                match arg {
                    graphcal_compiler::syntax::ast::GenericArg::Type(type_expr) => {
                        collect_type_expr_names(type_expr, refs);
                    }
                    graphcal_compiler::syntax::ast::GenericArg::Index(IndexExpr::Name(path)) => {
                        refs.push(path.value.display_path());
                    }
                    graphcal_compiler::syntax::ast::GenericArg::Index(
                        IndexExpr::Finite { .. } | IndexExpr::BareNat(_),
                    )
                    | graphcal_compiler::syntax::ast::GenericArg::Nat(_) => {}
                    graphcal_compiler::syntax::ast::GenericArg::Ambiguous(ambiguous) => {
                        collect_ambiguous_generic_names(ambiguous, refs);
                    }
                }
            }
        }
        TypeExprKind::ComplexApplication { generic_args }
        | TypeExprKind::KeyApplication { generic_args } => {
            // `Complex` and `Key` are built in; collect only names in their
            // generic argument.
            for arg in generic_args {
                match arg {
                    graphcal_compiler::syntax::ast::GenericArg::Type(type_expr) => {
                        collect_type_expr_names(type_expr, refs);
                    }
                    graphcal_compiler::syntax::ast::GenericArg::Index(IndexExpr::Name(path)) => {
                        refs.push(path.value.display_path());
                    }
                    graphcal_compiler::syntax::ast::GenericArg::Index(
                        IndexExpr::Finite { .. } | IndexExpr::BareNat(_),
                    )
                    | graphcal_compiler::syntax::ast::GenericArg::Nat(_) => {}
                    graphcal_compiler::syntax::ast::GenericArg::Ambiguous(ambiguous) => {
                        collect_ambiguous_generic_names(ambiguous, refs);
                    }
                }
            }
        }
        TypeExprKind::DatetimeApplication { type_args } => {
            // `Datetime` is a built-in — no top-level name to push.
            for arg in type_args {
                collect_type_expr_names(arg, refs);
            }
        }
        TypeExprKind::Dimensionless
        | TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime => {}
    }
}

fn collect_generic_arg_names(
    arg: &graphcal_compiler::syntax::ast::GenericArg<graphcal_compiler::syntax::phase::Desugared>,
    refs: &mut Vec<String>,
) {
    use graphcal_compiler::desugar::desugared_ast::IndexExpr;
    use graphcal_compiler::syntax::ast::GenericArg;
    match arg {
        GenericArg::Type(type_expr) => collect_type_expr_names(type_expr, refs),
        GenericArg::Index(IndexExpr::Name(path)) => refs.push(path.value.display_path()),
        GenericArg::Index(IndexExpr::Finite { .. } | IndexExpr::BareNat(_))
        | GenericArg::Nat(_) => {}
        GenericArg::Ambiguous(ambiguous) => collect_ambiguous_generic_names(ambiguous, refs),
    }
}

fn collect_ambiguous_generic_names(
    arg: &graphcal_compiler::desugar::desugared_ast::AmbiguousGenericArg,
    refs: &mut Vec<String>,
) {
    match arg {
        graphcal_compiler::desugar::desugared_ast::AmbiguousGenericArg::Name(ident) => {
            refs.push(ident.name.to_string());
        }
        graphcal_compiler::desugar::desugared_ast::AmbiguousGenericArg::Mul(operands, _) => {
            for operand in operands {
                collect_ambiguous_generic_names(operand, refs);
            }
        }
    }
}

/// A9 case 2 / V006 — re-exported decls must not name a private-at-importer
/// symbol in their effective signature.
///
/// For every dependency declaration that the importer explicitly re-exports
/// via `{ pub name }`, walk its signature, apply the include's substitution
/// map, and check each referenced type/dim/index name. If a name resolves to a
/// declaration that exists locally in the importer but is not explicitly
/// exported, the re-export leaks a private symbol.
#[expect(
    clippy::too_many_arguments,
    reason = "the check needs the dep AST, the three substitution maps, and the importer's visibility tables"
)]
pub(super) fn check_generics_leakage(
    dep_declarations: &[graphcal_compiler::desugar::desugared_ast::Declaration],
    pub_reexport_items: &HashSet<DeclName>,
    index_bindings: &IndexBindings,
    type_bindings: &HashMap<StructTypeName, StructTypeName>,
    dim_bindings: &HashMap<DimName, DimName>,
    importer_external_surface: &ExternalDeclSurface,
    importer_local_type_names: &HashMap<String, &'static str>,
    importer_src: &NamedSource<Arc<String>>,
    include_span: Span,
) -> Result<(), CompileError> {
    if pub_reexport_items.is_empty() {
        return Ok(());
    }

    for decl in dep_declarations {
        // Is this decl part of the importer's re-exported surface?
        let (decl_name, decl_kind_str) = match &decl.kind {
            DeclKind::Param(p) => (p.name.value.to_string(), "param"),
            DeclKind::Node(n) => (n.name.value.to_string(), "node"),
            DeclKind::ConstNode(c) => (c.name.value.to_string(), "const node"),
            DeclKind::BaseDimension(d) => (d.name.value.to_string(), "dim"),
            DeclKind::Dimension(d) => (d.name.value.to_string(), "dim"),
            DeclKind::Unit(u) => (u.name.value.to_string(), "unit"),
            DeclKind::Index(idx) => (idx.name.value.to_string(), "index"),
            DeclKind::Type(t) => (t.name.value.to_string(), "type"),
            _ => continue,
        };
        if !pub_reexport_items.contains(decl_name.as_str()) {
            continue;
        }

        // Collect every type-system name the signature references.
        let mut refs: Vec<String> = Vec::new();
        match &decl.kind {
            DeclKind::Param(p) => collect_type_expr_names(&p.type_ann, &mut refs),
            DeclKind::Node(n) => collect_type_expr_names(&n.type_ann, &mut refs),
            DeclKind::ConstNode(c) => collect_type_expr_names(&c.type_ann, &mut refs),
            DeclKind::Unit(u) => {
                for item in &u.dim_type.terms {
                    refs.push(item.term.name.value.display_path());
                }
            }
            DeclKind::Dimension(d) => {
                if let Some(def) = &d.definition {
                    for item in &def.terms {
                        refs.push(item.term.name.value.display_path());
                    }
                }
            }
            DeclKind::Type(t) => {
                if let graphcal_compiler::desugar::desugared_ast::TypeDeclBody::Constructors(
                    members,
                ) = &t.body
                {
                    for member in members {
                        if let Some(fields) = &member.payload {
                            for field in fields {
                                collect_type_expr_names(&field.type_ann, &mut refs);
                            }
                        }
                    }
                }
                for default in t
                    .generic_params
                    .iter()
                    .filter_map(|param| param.default.as_ref())
                {
                    collect_generic_arg_names(default, &mut refs);
                }
                // Bare references to lexical generic parameters are not
                // module API dependencies, even when a local type-system
                // declaration has the same leaf spelling.
                let generic_params = t
                    .generic_params
                    .iter()
                    .map(|param| param.name.value.as_str())
                    .collect::<HashSet<_>>();
                refs.retain(|name| !generic_params.contains(name.as_str()));
            }
            _ => {}
        }

        // Apply substitutions, then check each substituted name against the
        // importer's visibility table.
        for raw_name in refs {
            let index_target = index_bindings.get(raw_name.as_str());
            // A structural target carries no importer-local declaration whose
            // visibility could leak through the re-exported signature.
            if matches!(index_target, Some(IndexBindingTarget::Finite(_))) {
                continue;
            }
            let substituted = index_target
                .and_then(IndexBindingTarget::declared_name)
                .map(IndexName::as_str)
                .or_else(|| {
                    type_bindings
                        .get(raw_name.as_str())
                        .map(StructTypeName::as_str)
                })
                .or_else(|| dim_bindings.get(raw_name.as_str()).map(DimName::as_str))
                .unwrap_or(raw_name.as_str());

            if let Some(kind) = importer_local_type_names.get(substituted)
                && !importer_external_surface
                    .is_explicit_export(&DeclName::expect_valid(substituted))
            {
                return Err(CompileError::Eval(GraphcalError::GenericsLeakage {
                    reexport_kind: decl_kind_str.to_string(),
                    reexport_name: decl_name,
                    leaked_kind: (*kind).to_string(),
                    leaked_name: substituted.to_string(),
                    src: importer_src.clone(),
                    span: include_span.into(),
                }));
            }
        }
    }
    Ok(())
}
