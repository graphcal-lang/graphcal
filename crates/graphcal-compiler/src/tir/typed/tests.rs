use super::*;
use crate::desugar::desugared_ast::TypeExpr;
use crate::dimension::{BaseDimId, Rational};
use crate::registry::prelude::load_prelude;
use crate::registry::time_scale::TimeScale;
use crate::registry::types::RegistryBuilder;
use crate::syntax::index_name::ResolvedIndexName;
use crate::syntax::parser::Parser;
use crate::syntax::type_name::StructTypeName;

fn make_registry() -> Registry {
    let mut b = RegistryBuilder::new();
    load_prelude(&mut b).unwrap();
    b.try_build().unwrap()
}

fn make_dim_term_name(name: &str) -> crate::syntax::span::Spanned<crate::syntax::names::NamePath> {
    crate::syntax::span::Spanned::new(crate::syntax::names::NamePath::from(name), Span::new(0, 0))
}

/// Create a simple dimension `TypeExpr` from a name string like `"Velocity"`.
fn make_dim_type_expr(name: &str) -> crate::desugar::desugared_ast::TypeExpr {
    crate::desugar::desugared_ast::TypeExpr {
        kind: crate::desugar::desugared_ast::TypeExprKind::DimExpr(
            crate::desugar::desugared_ast::DimExpr {
                terms: vec![crate::desugar::desugared_ast::DimExprItem {
                    op: crate::desugar::desugared_ast::MulDivOp::Mul,
                    term: crate::desugar::desugared_ast::DimTerm {
                        name: make_dim_term_name(name),
                        power: None,
                        span: Span::new(0, 0),
                    },
                }],
                span: Span::new(0, 0),
            },
        ),
        constraints: vec![],
        span: Span::new(0, 0),
    }
}

fn make_registry_with_struct() -> Registry {
    let mut b = RegistryBuilder::new();
    load_prelude(&mut b).unwrap();
    b.register_type(crate::registry::types::TypeDef {
        name: StructTypeName::expect_valid("TransferResult"),
        generic_params: vec![],
        kind: crate::registry::types::TypeDefKind::Union {
            members: vec![crate::registry::types::UnionMemberDef {
                name: crate::syntax::type_name::ConstructorName::expect_valid("TransferResult"),
                fields: vec![
                    crate::registry::types::StructField {
                        name: crate::syntax::type_name::FieldName::expect_valid("dv1"),
                        type_ann: make_dim_type_expr("Velocity"),
                    },
                    crate::registry::types::StructField {
                        name: crate::syntax::type_name::FieldName::expect_valid("dv2"),
                        type_ann: make_dim_type_expr("Velocity"),
                    },
                ],
            }],
        },
    });
    b.try_build().unwrap()
}

fn make_registry_with_index() -> Registry {
    let mut b = RegistryBuilder::new();
    load_prelude(&mut b).unwrap();
    b.register_index(crate::registry::types::IndexDef {
        name: IndexName::expect_valid("Maneuver"),
        kind: crate::registry::types::IndexKind::Named {
            variants: vec![
                crate::syntax::index_name::IndexVariantName::expect_valid("Departure"),
                crate::syntax::index_name::IndexVariantName::expect_valid("Insertion"),
            ],
        },
    });
    b.try_build().unwrap()
}

fn make_src() -> NamedSource<Arc<String>> {
    NamedSource::new("test", Arc::new(String::new()))
}

/// Parse a type annotation from a param declaration and return the `TypeExpr`.
fn parse_type(source: &str) -> TypeExpr {
    // Wrap in a param declaration so the parser can handle it
    let full = format!("param x: {source} = 0.0;");
    let raw_file = Parser::new(&full).parse_file().unwrap();
    let desugared = crate::syntax::desugar::desugar_multi_decls_in_file(raw_file);
    let file = desugared;
    match &file.declarations[0].kind {
        crate::desugar::desugared_ast::DeclKind::Param(p) => p.type_ann.clone(),
        _ => panic!("expected param"),
    }
}

#[test]
fn resolve_dimensionless() {
    let r = make_registry();
    let te = parse_type("Dimensionless");
    let resolved = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap();
    assert_eq!(resolved, ResolvedTypeExpr::Dimensionless);
}

#[test]
fn resolve_bool() {
    let r = make_registry();
    let te = parse_type("Bool");
    let resolved = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap();
    assert_eq!(resolved, ResolvedTypeExpr::Bool);
}

#[test]
fn resolve_int() {
    let r = make_registry();
    let te = parse_type("Int");
    let resolved = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap();
    assert_eq!(resolved, ResolvedTypeExpr::Int);
}

#[test]
fn resolve_concrete_dimension() {
    let r = make_registry();
    let te = parse_type("Length");
    let resolved = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap();
    assert_eq!(
        resolved,
        ResolvedTypeExpr::Scalar(Dimension::base(BaseDimId::Prelude("Length".to_string())))
    );
}

#[test]
fn resolve_compound_dimension() {
    let r = make_registry();
    let te = parse_type("Length / Time^2");
    let resolved = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap();
    let expected = (Dimension::base(BaseDimId::Prelude("Length".to_string()))
        / Dimension::base(BaseDimId::Prelude("Time".to_string()))
            .pow(2)
            .unwrap())
    .unwrap();
    assert_eq!(resolved, ResolvedTypeExpr::Scalar(expected));
}

#[test]
fn resolve_struct_type() {
    let r = make_registry_with_struct();
    let te = parse_type("TransferResult");
    let resolved = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap();
    assert!(
        matches!(resolved, ResolvedTypeExpr::Struct(name, _) if name.as_str() == "TransferResult")
    );
}

#[test]
fn resolve_generic_dim_param() {
    let r = make_registry();
    let dim_params = vec![GenericParamName::expect_valid("D")];
    let te = parse_type("D");
    let resolved = resolve_type_expr(&te, &r, &dim_params, &[], &[], &make_src()).unwrap();
    assert!(matches!(resolved, ResolvedTypeExpr::GenericDimParam(name, _) if name.as_str() == "D"));
}

#[test]
fn resolve_generic_dim_expr_with_power() {
    let r = make_registry();
    let dim_params = vec![GenericParamName::expect_valid("D")];
    let te = parse_type("D^2");
    let resolved = resolve_type_expr(&te, &r, &dim_params, &[], &[], &make_src()).unwrap();
    match resolved {
        ResolvedTypeExpr::GenericDimExpr { terms, .. } => {
            assert_eq!(terms.len(), 1);
            match &terms[0] {
                ResolvedDimTerm::GenericParam { name, power, .. } => {
                    assert_eq!(name.as_str(), "D");
                    assert_eq!(*power, Rational::from(2));
                }
                ResolvedDimTerm::Concrete { .. } => panic!("expected GenericParam term"),
            }
        }
        _ => panic!("expected GenericDimExpr"),
    }
}

#[test]
fn resolve_mixed_generic_concrete() {
    let r = make_registry();
    let dim_params = vec![GenericParamName::expect_valid("D")];
    // D * Length  — this is a DimExpr with a generic and a concrete term
    let te = parse_type("D * Length");
    let resolved = resolve_type_expr(&te, &r, &dim_params, &[], &[], &make_src()).unwrap();
    match resolved {
        ResolvedTypeExpr::GenericDimExpr { terms, .. } => {
            assert_eq!(terms.len(), 2);
            assert!(
                matches!(&terms[0], ResolvedDimTerm::GenericParam { name, .. } if name.as_str() == "D")
            );
            assert!(matches!(&terms[1], ResolvedDimTerm::Concrete { .. }));
        }
        _ => panic!("expected GenericDimExpr, got {resolved:?}"),
    }
}

#[test]
fn resolve_concrete_indexed() {
    let r = make_registry_with_index();
    let te = parse_type("Length[Maneuver]");
    let resolved = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap();
    match resolved {
        ResolvedTypeExpr::Indexed { base, indexes } => {
            assert_eq!(
                *base,
                ResolvedTypeExpr::Scalar(Dimension::base(BaseDimId::Prelude("Length".to_string())))
            );
            assert_eq!(indexes.len(), 1);
            assert!(
                matches!(&indexes[0], ResolvedIndex::Concrete(name, _) if name.as_str() == "Maneuver")
            );
        }
        _ => panic!("expected Indexed"),
    }
}

#[test]
fn resolve_generic_indexed() {
    let r = make_registry();
    let dim_params = vec![GenericParamName::expect_valid("D")];
    let index_params = vec![GenericParamName::expect_valid("I")];
    let te = parse_type("D[I]");
    let resolved =
        resolve_type_expr(&te, &r, &dim_params, &index_params, &[], &make_src()).unwrap();
    match resolved {
        ResolvedTypeExpr::Indexed { base, indexes } => {
            assert!(
                matches!(*base, ResolvedTypeExpr::GenericDimParam(ref name, _) if name.as_str() == "D")
            );
            assert_eq!(indexes.len(), 1);
            assert!(
                matches!(&indexes[0], ResolvedIndex::GenericParam(name, _) if name.as_str() == "I")
            );
        }
        _ => panic!("expected Indexed"),
    }
}

#[test]
fn resolve_unknown_dimension_error() {
    let r = make_registry();
    let te = parse_type("UnknownDim");
    let err = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap_err();
    assert!(matches!(err, GraphcalError::UnknownDimension { .. }));
}

#[test]
fn resolve_unknown_index_error() {
    let r = make_registry();
    let te = parse_type("Length[UnknownIdx]");
    let err = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap_err();
    assert!(matches!(err, GraphcalError::UnknownIndex { .. }));
}

#[test]
fn resolve_struct_takes_priority_over_dim_param() {
    // When a name matches both a struct and a generic param,
    // struct should win (structs are concrete, params are only
    // in scope inside a function that has that param).
    // In practice this shouldn't happen because struct names are
    // PascalCase and generic params are single letters, but let's
    // make sure the priority is correct.
    let r = make_registry_with_struct();
    let dim_params = vec![GenericParamName::expect_valid("TransferResult")];
    let te = parse_type("TransferResult");
    let resolved = resolve_type_expr(&te, &r, &dim_params, &[], &[], &make_src()).unwrap();
    assert!(matches!(resolved, ResolvedTypeExpr::Struct(..)));
}

#[test]
fn resolve_velocity_derived_dimension() {
    let r = make_registry();
    let te = parse_type("Velocity");
    let resolved = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap();
    let expected = (Dimension::base(BaseDimId::Prelude("Length".to_string()))
        / Dimension::base(BaseDimId::Prelude("Time".to_string())))
    .unwrap();
    assert_eq!(resolved, ResolvedTypeExpr::Scalar(expected));
}

// --- module-aware type resolution integration tests ---

/// Single-file integration helper: lower + type-resolve + compile each
/// inline dag body using the dumb `lower_dag_body_to_ir` primitive
/// directly (no self-import preprocessing — fixtures exercised here
/// either don't use self-imports or are expected to surface errors that
/// fall out of the unprocessed body).
fn parse_and_type_resolve(source: &str) -> Result<TIR, GraphcalError> {
    let raw_file = Parser::new(source).parse_file().unwrap();
    let desugared = crate::syntax::desugar::desugar_multi_decls_in_file(raw_file);
    let file = desugared;
    let src = NamedSource::new("test.gcl", Arc::new(source.to_string()));
    let ir = crate::ir::lower::lower(&file, &src)?;
    let parent_dag_id =
        crate::dag_id::DagId::from_virtual_relative_path(std::path::Path::new("test.gcl")).unwrap();
    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(parent_dag_id.clone(), &file.declarations)
        .map_err(|err| {
            internal_error(
                format!("test module resolver failed for root module: {err}"),
                &src,
                Span::new(0, 0),
            )
        })?;
    for decl in &file.declarations {
        if let crate::desugar::desugared_ast::DeclKind::Dag(dag) = &decl.kind {
            resolver
                .add_module(parent_dag_id.child(dag.name.value.as_str()), &dag.body)
                .map_err(|err| {
                    internal_error(
                        format!(
                            "test module resolver failed for inline dag `{}`: {err}",
                            dag.name.value
                        ),
                        &src,
                        Span::new(0, 0),
                    )
                })?;
        }
    }
    let mut module_types = ModuleTypeRegistry::default();
    module_types.insert_graphcal_prelude().map_err(|err| {
        internal_error(
            format!("test module type prelude failed: {err}"),
            &src,
            Span::new(0, 0),
        )
    })?;
    module_types.insert_registry(&parent_dag_id, &ir.registry);
    let mut tir =
        type_resolve_with_modules(ir, parent_dag_id.clone(), &src, &resolver, &module_types)?;
    compile_inline_dag_bodies_test(&mut tir, &src, &parent_dag_id, &file.declarations)?;
    Ok(tir)
}

/// Compile each inline dag body in `tir` with no self-import
/// preprocessing. Used by compiler-side integration tests that don't
/// have access to the eval crate's project pipeline.
fn compile_inline_dag_bodies_test(
    tir: &mut TIR,
    src: &NamedSource<Arc<String>>,
    parent_dag_id: &crate::dag_id::DagId,
    parent_declarations: &[crate::desugar::desugared_ast::Declaration],
) -> Result<(), GraphcalError> {
    let dag_bodies = tir
        .registry
        .dags
        .all_dags()
        .map(|(name, dag)| (name.clone(), dag.body.clone()))
        .collect::<Vec<_>>();
    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(parent_dag_id.clone(), parent_declarations)
        .map_err(|err| {
            internal_error(
                format!("test module resolver failed for parent module: {err}"),
                src,
                Span::new(0, 0),
            )
        })?;
    for (name, body) in &dag_bodies {
        resolver
            .add_module(parent_dag_id.child(name.as_str()), body)
            .map_err(|err| {
                internal_error(
                    format!("test module resolver failed for inline dag `{name}`: {err}"),
                    src,
                    Span::new(0, 0),
                )
            })?;
    }
    let mut module_types = ModuleTypeRegistry::default();
    module_types.insert_graphcal_prelude().map_err(|err| {
        internal_error(
            format!("test module type prelude failed: {err}"),
            src,
            Span::new(0, 0),
        )
    })?;
    module_types.insert_registry(parent_dag_id, &tir.registry);

    for (name, body) in dag_bodies {
        let dag_body_ir = crate::ir::lower::lower_dag_body_to_ir(
            name.as_str(),
            &body,
            &tir.registry,
            &resolver,
            &crate::ir::resolve::ImportedValueNames::default(),
            HashMap::new(),
            HashMap::new(),
            src,
            parent_dag_id,
        )?;
        let dag_id = parent_dag_id.child(name.as_str());
        let mut compiled_dag =
            type_resolve_single_with_modules(dag_body_ir, &dag_id, src, &resolver, &module_types)?;
        compiled_dag.populate_pub_nodes(&body);
        tir.dags.insert(dag_id, compiled_dag);
    }
    Ok(())
}

#[test]
fn module_aware_type_resolve_records_semantic_deps() {
    let source = "const node C: Dimensionless = 1.0;\n\
                  const node D: Dimensionless = C;\n\
                  param p: Dimensionless;\n\
                  node x: Dimensionless = @p + D;";
    let raw_file = Parser::new(source).parse_file().unwrap();
    let desugared = crate::syntax::desugar::desugar_multi_decls_in_file(raw_file);
    let file = desugared;
    let src = NamedSource::new("test.gcl", Arc::new(source.to_string()));
    let dag_id =
        crate::dag_id::DagId::from_virtual_relative_path(std::path::Path::new("test.gcl")).unwrap();
    let ir = crate::ir::lower::lower(&file, &src).unwrap();
    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(dag_id.clone(), &file.declarations)
        .unwrap();
    let mut module_types = ModuleTypeRegistry::default();
    module_types.insert_graphcal_prelude().unwrap();
    module_types.insert_registry(&dag_id, &ir.registry);

    let tir =
        type_resolve_with_modules(ir, dag_id.clone(), &src, &resolver, &module_types).unwrap();
    let deps = &tir.root().semantic.dependencies;
    let c = ResolvedDeclName::from_def(dag_id.clone(), DeclName::expect_valid("C"));
    let d = ResolvedDeclName::from_def(dag_id.clone(), DeclName::expect_valid("D"));
    let p = ResolvedDeclName::from_def(dag_id.clone(), DeclName::expect_valid("p"));
    let x = ResolvedDeclName::from_def(dag_id, DeclName::expect_valid("x"));

    assert!(deps.const_deps[&d].contains(&c));
    assert!(deps.const_deps[&c].is_empty());
    assert!(deps.runtime_deps[&x].contains(&p));
    assert!(deps.runtime_deps[&p].is_empty());
}

#[test]
fn type_resolve_rocket() {
    let source = include_str!("../../../../../tests/fixtures/valid/rocket.gcl");
    let tir = parse_and_type_resolve(source).unwrap();
    // All declarations should have resolved types
    assert!(
        tir.root()
            .resolved_decl_types
            .contains_key(&ScopedName::local("dry_mass"))
    );
    assert!(
        tir.root()
            .resolved_decl_types
            .contains_key(&ScopedName::local("delta_v"))
    );
    assert!(
        tir.root()
            .resolved_decl_types
            .contains_key(&ScopedName::local("g0"))
    );
}

#[test]
fn type_resolve_indexed() {
    let source = include_str!("../../../../../tests/fixtures/valid/indexed.gcl");
    let tir = parse_and_type_resolve(source).unwrap();
    // delta_v should be Velocity[Maneuver]
    let dv_type = &tir.root().resolved_decl_types[&ScopedName::local("delta_v")];
    assert!(matches!(dv_type, ResolvedTypeExpr::Indexed { .. }));
}

#[test]
fn type_resolve_hohmann() {
    // hohmann.gcl uses DAG+include. Project-level `graphcal check`
    // accepts it (see the CLI tests), but single-file TIR resolution
    // rejects it: there's no project loader to resolve cross-DAG
    // references like `import hohmann.{...}`, and `@transfer` from the
    // unexpanded include surfaces as an unresolved reference during HIR
    // lowering. Resolution fails on the first unresolved name it
    // encounters.
    let source = include_str!("../../../../../tests/fixtures/valid/hohmann.gcl");
    let err = parse_and_type_resolve(source).unwrap_err();
    assert!(
        err.to_string().contains("transfer"),
        "unexpected error: {err}"
    );
}

#[test]
fn type_resolve_generics() {
    let source = include_str!("../../../../../tests/fixtures/valid/generics.gcl");
    let tir = parse_and_type_resolve(source).unwrap();
    // pos_eci should be a GenericStruct with type args
    let pos_type = &tir.root().resolved_decl_types[&ScopedName::local("pos_eci")];
    match pos_type {
        ResolvedTypeExpr::GenericStruct {
            name, type_args, ..
        } => {
            assert_eq!(name.as_str(), "Vec3");
            assert_eq!(type_args.len(), 2);
            assert_eq!(
                type_args[0],
                ResolvedTypeExpr::Scalar(Dimension::base(BaseDimId::Prelude("Length".to_string())))
            );
            assert!(matches!(&type_args[1], ResolvedTypeExpr::Struct(n, _) if n.as_str() == "Eci"));
        }
        other => panic!("expected GenericStruct, got {other:?}"),
    }
    // x_pos should be scalar Length
    assert_eq!(
        tir.root().resolved_decl_types[&ScopedName::local("x_pos")],
        ResolvedTypeExpr::Scalar(Dimension::base(BaseDimId::Prelude("Length".to_string())))
    );
}

#[test]
fn type_resolve_default_type_params() {
    let source = include_str!("../../../../../tests/fixtures/valid/generics.gcl");
    let tir = parse_and_type_resolve(source).unwrap();

    // pos3_eci: Pos3<Length, Eci> — explicit, 2 type args
    let pos3_eci = &tir.root().resolved_decl_types[&ScopedName::local("pos3_eci")];
    match pos3_eci {
        ResolvedTypeExpr::GenericStruct {
            name, type_args, ..
        } => {
            assert_eq!(name.as_str(), "Pos3");
            assert_eq!(type_args.len(), 2);
            assert_eq!(
                type_args[0],
                ResolvedTypeExpr::Scalar(Dimension::base(BaseDimId::Prelude("Length".to_string())))
            );
            assert!(matches!(&type_args[1], ResolvedTypeExpr::Struct(n, _) if n.as_str() == "Eci"));
        }
        other => panic!("expected GenericStruct, got {other:?}"),
    }

    // pos3_default: Pos3<Length> — default fills in Unframed
    let pos3_default = &tir.root().resolved_decl_types[&ScopedName::local("pos3_default")];
    match pos3_default {
        ResolvedTypeExpr::GenericStruct {
            name, type_args, ..
        } => {
            assert_eq!(name.as_str(), "Pos3");
            assert_eq!(type_args.len(), 2);
            assert_eq!(
                type_args[0],
                ResolvedTypeExpr::Scalar(Dimension::base(BaseDimId::Prelude("Length".to_string())))
            );
            assert!(
                matches!(&type_args[1], ResolvedTypeExpr::Struct(n, _) if n.as_str() == "Unframed"),
                "expected Struct(Unframed), got {:?}",
                type_args[1]
            );
        }
        other => panic!("expected GenericStruct, got {other:?}"),
    }
}

// --- resolved_to_declared_type() tests ---

use crate::registry::declared_type::{DeclaredType, IndexTypeRef, StructTypeRef};

#[test]
fn generic_index_substitution_preserves_resolved_owner() {
    use crate::tir::dim_check::{InferredIndex, InferredType};

    let src = make_src();
    let registry = make_registry();
    let owner = crate::dag_id::DagId::root_in_package("test", "a");
    let resolved_index = ResolvedIndexName::from_def(owner, IndexName::expect_valid("Phase"));
    let generic = GenericParamName::expect_valid("I");
    let resolved_type = ResolvedTypeExpr::Indexed {
        base: Box::new(ResolvedTypeExpr::Dimensionless),
        indexes: vec![ResolvedIndex::GenericParam(
            generic.clone(),
            Span::new(0, 0),
        )],
    };
    let actual = InferredType::Indexed {
        element: Box::new(InferredType::Scalar(Dimension::dimensionless())),
        index: InferredIndex::from_resolved(resolved_index.clone()),
    };
    let mut dim_sub = HashMap::new();
    let mut index_sub = HashMap::new();
    let mut nat_sub = HashMap::new();

    unify_resolved_type(
        &resolved_type,
        &actual,
        &mut dim_sub,
        &mut index_sub,
        &mut nat_sub,
        &registry,
        &src,
        Span::new(0, 0),
    )
    .unwrap();
    assert_eq!(
        index_sub[&generic].declared_resolved(),
        Some(&resolved_index)
    );

    let substituted =
        substitute_resolved_type(&resolved_type, &dim_sub, &index_sub, &nat_sub, &src).unwrap();
    let InferredType::Indexed { index, .. } = substituted else {
        panic!("expected indexed type after substitution");
    };
    assert_eq!(index.declared_resolved(), Some(&resolved_index));
}

#[test]
fn convert_dimensionless() {
    let dt = resolved_to_declared_type(&ResolvedTypeExpr::Dimensionless, &make_src()).unwrap();
    assert_eq!(dt, DeclaredType::Scalar(Dimension::dimensionless()));
}

#[test]
fn convert_bool() {
    let dt = resolved_to_declared_type(&ResolvedTypeExpr::Bool, &make_src()).unwrap();
    assert_eq!(dt, DeclaredType::Bool);
}

#[test]
fn convert_int() {
    let dt = resolved_to_declared_type(&ResolvedTypeExpr::Int, &make_src()).unwrap();
    assert_eq!(dt, DeclaredType::Int);
}

#[test]
fn convert_scalar() {
    let dim = Dimension::base(BaseDimId::Prelude("Length".to_string()));
    let dt =
        resolved_to_declared_type(&ResolvedTypeExpr::Scalar(dim.clone()), &make_src()).unwrap();
    assert_eq!(dt, DeclaredType::Scalar(dim));
}

#[test]
fn convert_struct() {
    let owner = crate::dag_id::DagId::root_in_package("test", "test");
    let resolved = ResolvedStructTypeName::from_def(owner, StructTypeName::expect_valid("Foo"));
    let dt = resolved_to_declared_type(
        &ResolvedTypeExpr::Struct(resolved.clone(), Span::new(0, 0)),
        &make_src(),
    )
    .unwrap();
    assert_eq!(
        dt,
        DeclaredType::Struct(StructTypeRef::from_resolved(resolved), vec![])
    );
}

#[test]
fn convert_indexed() {
    let owner = crate::dag_id::DagId::root_in_package("test", "test");
    let resolved_index = ResolvedIndexName::from_def(owner, IndexName::expect_valid("M"));
    let dt = resolved_to_declared_type(
        &ResolvedTypeExpr::Indexed {
            base: Box::new(ResolvedTypeExpr::Scalar(Dimension::base(
                BaseDimId::Prelude("Length".to_string()),
            ))),
            indexes: vec![ResolvedIndex::Concrete(
                resolved_index.clone(),
                Span::new(0, 0),
            )],
        },
        &make_src(),
    )
    .unwrap();
    assert_eq!(
        dt,
        DeclaredType::Indexed {
            element: Box::new(DeclaredType::Scalar(Dimension::base(BaseDimId::Prelude(
                "Length".to_string()
            )))),
            index: IndexTypeRef::from_resolved(resolved_index),
        }
    );
}

#[test]
fn convert_generic_dim_param_fails() {
    let err = resolved_to_declared_type(
        &ResolvedTypeExpr::GenericDimParam(GenericParamName::expect_valid("D"), Span::new(0, 0)),
        &make_src(),
    )
    .unwrap_err();
    assert!(matches!(err, GraphcalError::EvalError { .. }));
}

#[test]
fn convert_generic_index_fails() {
    let err = resolved_to_declared_type(
        &ResolvedTypeExpr::Indexed {
            base: Box::new(ResolvedTypeExpr::Dimensionless),
            indexes: vec![ResolvedIndex::GenericParam(
                GenericParamName::expect_valid("I"),
                Span::new(0, 0),
            )],
        },
        &make_src(),
    )
    .unwrap_err();
    assert!(matches!(err, GraphcalError::EvalError { .. }));
}

// --- Datetime type resolution tests ---

#[test]
fn resolve_bare_datetime() {
    let r = make_registry();
    let te = parse_type("Datetime");
    let resolved = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap();
    assert_eq!(resolved, ResolvedTypeExpr::Datetime(TimeScale::UTC));
}

#[test]
fn resolve_datetime_utc() {
    let r = make_registry();
    let te = parse_type("Datetime<UTC>");
    let resolved = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap();
    assert_eq!(resolved, ResolvedTypeExpr::Datetime(TimeScale::UTC));
}

#[test]
fn resolve_datetime_tt() {
    let r = make_registry();
    let te = parse_type("Datetime<TT>");
    let resolved = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap();
    assert_eq!(resolved, ResolvedTypeExpr::Datetime(TimeScale::TT));
}

#[test]
fn resolve_datetime_tai() {
    let r = make_registry();
    let te = parse_type("Datetime<TAI>");
    let resolved = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap();
    assert_eq!(resolved, ResolvedTypeExpr::Datetime(TimeScale::TAI));
}

#[test]
fn resolve_datetime_gpst() {
    let r = make_registry();
    let te = parse_type("Datetime<GPST>");
    let resolved = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap();
    assert_eq!(resolved, ResolvedTypeExpr::Datetime(TimeScale::GPST));
}

#[test]
fn resolve_datetime_unknown_scale_error() {
    let r = make_registry();
    let te = parse_type("Datetime<XYZ>");
    let err = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap_err();
    assert!(matches!(err, GraphcalError::EvalError { .. }));
}

#[test]
fn convert_datetime_utc() {
    let dt = resolved_to_declared_type(&ResolvedTypeExpr::Datetime(TimeScale::UTC), &make_src())
        .unwrap();
    assert_eq!(dt, DeclaredType::Datetime(TimeScale::UTC));
}

#[test]
fn convert_datetime_tt() {
    let dt =
        resolved_to_declared_type(&ResolvedTypeExpr::Datetime(TimeScale::TT), &make_src()).unwrap();
    assert_eq!(dt, DeclaredType::Datetime(TimeScale::TT));
}

// -----------------------------------------------------------------------
// NatPolyForm::is_leq tests
// -----------------------------------------------------------------------

#[test]
fn nat_leq_constant_equal() {
    let a = NatPolyForm::from_constant(3);
    let b = NatPolyForm::from_constant(3);
    assert!(a.is_leq(&b));
}

#[test]
fn nat_leq_constant_less() {
    let a = NatPolyForm::from_constant(2);
    let b = NatPolyForm::from_constant(5);
    assert!(a.is_leq(&b));
}

#[test]
fn nat_leq_constant_greater() {
    let a = NatPolyForm::from_constant(5);
    let b = NatPolyForm::from_constant(3);
    assert!(!a.is_leq(&b));
}

#[test]
fn nat_leq_same_var() {
    // N <= N
    let a = NatPolyForm::from_var(GenericParamName::expect_valid("N"));
    let b = NatPolyForm::from_var(GenericParamName::expect_valid("N"));
    assert!(a.is_leq(&b));
}

#[test]
fn nat_leq_var_plus_constant() {
    // N <= N + 1
    let a = NatPolyForm::from_var(GenericParamName::expect_valid("N"));
    let b = NatPolyForm::from_var(GenericParamName::expect_valid("N"))
        .add(&NatPolyForm::from_constant(1))
        .unwrap();
    assert!(a.is_leq(&b));
}

#[test]
fn nat_leq_var_plus_constant_reverse() {
    // N + 1 <= N → false
    let a = NatPolyForm::from_var(GenericParamName::expect_valid("N"))
        .add(&NatPolyForm::from_constant(1))
        .unwrap();
    let b = NatPolyForm::from_var(GenericParamName::expect_valid("N"));
    assert!(!a.is_leq(&b));
}

#[test]
fn nat_leq_different_vars() {
    // N <= M → false (N could be larger)
    let a = NatPolyForm::from_var(GenericParamName::expect_valid("N"));
    let b = NatPolyForm::from_var(GenericParamName::expect_valid("M"));
    assert!(!a.is_leq(&b));
}

#[test]
fn nat_leq_zero_leq_anything() {
    // 0 <= N
    let a = NatPolyForm::from_constant(0);
    let b = NatPolyForm::from_var(GenericParamName::expect_valid("N"));
    assert!(a.is_leq(&b));
}

// -----------------------------------------------------------------------
// NatRangeIndexIdentity typed-reference tests
// -----------------------------------------------------------------------

#[test]
fn nat_range_identity_concrete_to_index_type_ref() -> Result<(), Box<dyn std::error::Error>> {
    let reference = NatPolyForm::from_constant(3)
        .to_nat_range_identity()?
        .to_index_type_ref()?;
    assert_eq!(
        reference
            .nat_range()
            .map(crate::registry::types::NatRangeIndex::size_u64),
        Some(3)
    );
    assert_eq!(reference.display_name().as_str(), "range(3)");
    Ok(())
}

#[test]
fn nat_range_identity_symbolic_to_display_only_index_type_ref()
-> Result<(), Box<dyn std::error::Error>> {
    let reference = NatPolyForm::from_var(GenericParamName::expect_valid("N"))
        .add(&NatPolyForm::from_constant(1))
        .unwrap()
        .to_nat_range_identity()?
        .to_index_type_ref()?;
    assert_eq!(reference.nat_range(), None);
    assert_eq!(reference.display_name().as_str(), "range(N + 1)");
    Ok(())
}

// -----------------------------------------------------------------------
// NatPolyForm multiplication tests (Level 2)
// -----------------------------------------------------------------------

#[test]
fn nat_mul_constants() {
    let a = NatPolyForm::from_constant(3);
    let b = NatPolyForm::from_constant(4);
    assert_eq!(a.mul(&b).unwrap(), NatPolyForm::from_constant(12));
}

#[test]
fn nat_mul_var_by_constant() {
    // N * 3
    let n = NatPolyForm::from_var(GenericParamName::expect_valid("N"));
    let three = NatPolyForm::from_constant(3);
    let result = n.mul(&three).unwrap();
    // Should format as "3 * N"
    assert_eq!(result.format(), "3 * N");
    // Evaluate with N=5 → 15
    let mut bindings = HashMap::new();
    bindings.insert(GenericParamName::expect_valid("N"), 5);
    assert_eq!(result.evaluate(&bindings), Some(15));
}

#[test]
fn nat_mul_two_vars() {
    // M * N
    let m = NatPolyForm::from_var(GenericParamName::expect_valid("M"));
    let n = NatPolyForm::from_var(GenericParamName::expect_valid("N"));
    let result = m.mul(&n).unwrap();
    assert_eq!(result.format(), "M * N");
    let mut bindings = HashMap::new();
    bindings.insert(GenericParamName::expect_valid("M"), 3);
    bindings.insert(GenericParamName::expect_valid("N"), 4);
    assert_eq!(result.evaluate(&bindings), Some(12));
}

#[test]
fn nat_mul_distributive() {
    // (M + 1) * N = M * N + N
    let m = NatPolyForm::from_var(GenericParamName::expect_valid("M"));
    let n = NatPolyForm::from_var(GenericParamName::expect_valid("N"));
    let m_plus_1 = m.add(&NatPolyForm::from_constant(1)).unwrap();
    let result = m_plus_1.mul(&n).unwrap();
    // Evaluate with M=2, N=3 → (2+1)*3 = 9
    let mut bindings = HashMap::new();
    bindings.insert(GenericParamName::expect_valid("M"), 2);
    bindings.insert(GenericParamName::expect_valid("N"), 3);
    assert_eq!(result.evaluate(&bindings), Some(9));
}

#[test]
fn nat_mul_mixed_add() {
    // M * N + 1
    let m = NatPolyForm::from_var(GenericParamName::expect_valid("M"));
    let n = NatPolyForm::from_var(GenericParamName::expect_valid("N"));
    let result = m
        .mul(&n)
        .unwrap()
        .add(&NatPolyForm::from_constant(1))
        .unwrap();
    assert_eq!(result.format(), "M * N + 1");
    let mut bindings = HashMap::new();
    bindings.insert(GenericParamName::expect_valid("M"), 2);
    bindings.insert(GenericParamName::expect_valid("N"), 3);
    assert_eq!(result.evaluate(&bindings), Some(7));
}

#[test]
fn nat_poly_is_constant() {
    let c = NatPolyForm::from_constant(5);
    assert!(c.is_constant());

    let n = NatPolyForm::from_var(GenericParamName::expect_valid("N"));
    assert!(!n.is_constant());

    let mn = NatPolyForm::from_var(GenericParamName::expect_valid("M"))
        .mul(&NatPolyForm::from_var(GenericParamName::expect_valid("N")))
        .unwrap();
    assert!(!mn.is_constant());
}

#[test]
fn nat_poly_leq_with_mul() {
    // M * N <= M * N + 1
    let mn = NatPolyForm::from_var(GenericParamName::expect_valid("M"))
        .mul(&NatPolyForm::from_var(GenericParamName::expect_valid("N")))
        .unwrap();
    let mn_plus_1 = mn.add(&NatPolyForm::from_constant(1)).unwrap();
    assert!(mn.is_leq(&mn_plus_1));
    assert!(!mn_plus_1.is_leq(&mn));
}

#[test]
fn nat_add_overflow_errors() {
    // Regression: coefficient addition used to wrap silently, letting a
    // wrapped form unify with an unrelated type.
    let a = NatPolyForm::from_constant(u64::MAX);
    let b = NatPolyForm::from_constant(1);
    assert!(a.add(&b).is_err());
}

#[test]
fn nat_mul_overflow_errors() {
    // Regression: coefficient multiplication used to wrap silently.
    let a = NatPolyForm::from_constant(u64::MAX);
    let b = NatPolyForm::from_constant(2);
    assert!(a.mul(&b).is_err());
}

#[test]
fn nat_unify_substituted_term_overflow_errors() {
    // Regression: `unify_nat_poly_form` multiplied a term coefficient by
    // a substituted binding without overflow checking (debug panic,
    // release wraparound). `2 * N` with N bound near u64::MAX must report
    // a mismatch instead.
    let form = NatPolyForm::from_constant(2)
        .mul(&NatPolyForm::from_var(GenericParamName::expect_valid("N")))
        .unwrap();
    let mut nat_sub = HashMap::new();
    nat_sub.insert(GenericParamName::expect_valid("N"), u64::MAX / 2 + 1);
    let src = NamedSource::new("<test>", Arc::new(String::new()));
    let result = unify_nat_poly_form(
        &form,
        4,
        &mut nat_sub,
        &IndexName::expect_valid("range(4)"),
        &src,
        Span::new(0, 0),
    );
    assert!(result.is_err());
}

#[test]
fn nat_poly_format_zero() {
    let z = NatPolyForm::from_constant(0);
    assert_eq!(z.format(), "0");
}
