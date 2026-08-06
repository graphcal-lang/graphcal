use super::*;
use crate::dimension::{BaseDimId, Rational};
use crate::registry::prelude::load_prelude;
use crate::registry::time_scale::TimeScale;
use crate::registry::types::RegistryBuilder;
use crate::syntax::index_name::ResolvedIndexName;
use crate::syntax::parser::Parser;
use crate::syntax::type_name::{ResolvedStructTypeName, StructTypeName};

fn make_registry() -> Registry {
    let mut b = RegistryBuilder::new();
    load_prelude(&mut b).unwrap();
    b.try_build().unwrap()
}

fn make_src() -> NamedSource<Arc<String>> {
    NamedSource::new("test", Arc::new(String::new()))
}

/// Resolve a source type through the production AST → HIR → TIR path.
///
/// Generic parameters are declared on a synthetic nominal type so HIR builds
/// the same typed [`hir::GenericScope`] used for real generic field signatures.
fn resolve_source_type(
    source_type: &str,
    dim_params: &[GenericParamName],
    index_params: &[GenericParamName],
    nat_params: &[GenericParamName],
) -> Result<ResolvedTypeExpr, GraphcalError> {
    let params = dim_params
        .iter()
        .map(|name| format!("{name}: Dim"))
        .chain(index_params.iter().map(|name| format!("{name}: Index")))
        .chain(nat_params.iter().map(|name| format!("{name}: Nat")))
        .collect::<Vec<_>>();
    if params.is_empty() {
        let tir = parse_and_type_resolve(&format!("param x: {source_type};"))?;
        return Ok(tir.root().resolved_decl_types[&ScopedName::parse("x").unwrap()].clone());
    }

    let source = format!(
        "type ResolutionSubject<{}> {{ ResolutionSubject(value: {source_type}) }}",
        params.join(", ")
    );
    let tir = parse_and_type_resolve(&source)?;
    tir.root()
        .semantic
        .type_defs
        .fields()
        .find(|(key, _)| {
            key.owning_type.as_str() == "ResolutionSubject" && key.field.as_str() == "value"
        })
        .map(|(_, field)| field.resolved_type().clone())
        .ok_or_else(|| GraphcalError::InternalError {
            message: "test type field was not resolved through HIR".to_string(),
            src: NamedSource::new("test.gcl", Arc::new(source)),
            span: Span::new(0, 0).into(),
        })
}

fn resolved_param_type(program: &str, name: &str) -> Result<ResolvedTypeExpr, GraphcalError> {
    let tir = parse_and_type_resolve(program)?;
    Ok(tir.root().resolved_decl_types[&ScopedName::parse(name).unwrap()].clone())
}

#[test]
fn resolve_dimensionless() {
    let resolved = resolve_source_type("Dimensionless", &[], &[], &[]).unwrap();
    assert_eq!(resolved, ResolvedTypeExpr::Dimensionless);
}

#[test]
fn resolve_bool() {
    let resolved = resolve_source_type("Bool", &[], &[], &[]).unwrap();
    assert_eq!(resolved, ResolvedTypeExpr::Bool);
}

#[test]
fn resolve_int() {
    let resolved = resolve_source_type("Int", &[], &[], &[]).unwrap();
    assert_eq!(resolved, ResolvedTypeExpr::Int);
}

#[test]
fn resolve_concrete_dimension() {
    let resolved = resolve_source_type("Length", &[], &[], &[]).unwrap();
    assert_eq!(
        resolved,
        ResolvedTypeExpr::Quantity(Dimension::base(BaseDimId::Prelude(
            crate::dimension::PreludeBaseDimension::Length
        )))
    );
}

#[test]
fn resolve_compound_dimension() {
    let resolved = resolve_source_type("Length / Time^2", &[], &[], &[]).unwrap();
    let expected = (Dimension::base(BaseDimId::Prelude(
        crate::dimension::PreludeBaseDimension::Length,
    )) / Dimension::base(BaseDimId::Prelude(
        crate::dimension::PreludeBaseDimension::Time,
    ))
    .pow(2)
    .unwrap())
    .unwrap();
    assert_eq!(resolved, ResolvedTypeExpr::Quantity(expected));
}

#[test]
fn resolve_struct_type() {
    let resolved = resolved_param_type(
        "pub type TransferResult { TransferResult(value: Velocity) }\nparam x: TransferResult;",
        "x",
    )
    .unwrap();
    assert!(
        matches!(resolved, ResolvedTypeExpr::Struct(name, _) if name.as_str() == "TransferResult")
    );
}

#[test]
fn resolve_generic_dim_param() {
    let dim_params = vec![GenericParamName::expect_valid("D")];
    let resolved = resolve_source_type("D", &dim_params, &[], &[]).unwrap();
    assert!(matches!(resolved, ResolvedTypeExpr::GenericDimParam(name, _) if name.as_str() == "D"));
}

#[test]
fn resolve_generic_dim_expr_with_power() {
    let dim_params = vec![GenericParamName::expect_valid("D")];
    let resolved = resolve_source_type("D^2", &dim_params, &[], &[]).unwrap();
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
    let dim_params = vec![GenericParamName::expect_valid("D")];
    let resolved = resolve_source_type("D * Length", &dim_params, &[], &[]).unwrap();
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
    let resolved = resolved_param_type(
        "pub index Maneuver = { Departure, Insertion };\nparam x: Length[Maneuver];",
        "x",
    )
    .unwrap();
    match resolved {
        ResolvedTypeExpr::Indexed { base, indexes } => {
            assert_eq!(
                *base,
                ResolvedTypeExpr::Quantity(Dimension::base(BaseDimId::Prelude(
                    crate::dimension::PreludeBaseDimension::Length,
                )))
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
    let dim_params = vec![GenericParamName::expect_valid("D")];
    let index_params = vec![GenericParamName::expect_valid("I")];
    let resolved = resolve_source_type("D[I]", &dim_params, &index_params, &[]).unwrap();
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
    let err = resolve_source_type("UnknownDim", &[], &[], &[]).unwrap_err();
    assert!(matches!(err, GraphcalError::UnknownDimension { .. }));
}

#[test]
fn quantity_is_semantic_not_a_source_type_constructor() {
    let error = resolve_source_type("Quantity", &[], &[], &[]).unwrap_err();
    assert!(matches!(error, GraphcalError::UnknownDimension { .. }));
}

#[test]
fn resolve_unknown_index_error() {
    let err = resolve_source_type("Length[UnknownIdx]", &[], &[], &[]).unwrap_err();
    assert!(matches!(err, GraphcalError::UnknownIndex { .. }));
}

#[test]
fn resolve_struct_takes_priority_over_dim_param() {
    let tir = parse_and_type_resolve(
        "type TransferResult { TransferResult(value: Velocity) }\n\
         type ResolutionSubject<TransferResult: Dim> {\n\
             ResolutionSubject(value: TransferResult)\n\
         }",
    )
    .unwrap();
    let resolved = tir
        .root()
        .semantic
        .type_defs
        .fields()
        .find(|(key, _)| {
            key.owning_type.as_str() == "ResolutionSubject" && key.field.as_str() == "value"
        })
        .map(|(_, field)| field.resolved_type())
        .expect("ResolutionSubject.value should resolve through HIR");
    assert!(matches!(resolved, ResolvedTypeExpr::Struct(..)));
}

#[test]
fn resolve_velocity_derived_dimension() {
    let resolved = resolve_source_type("Velocity", &[], &[], &[]).unwrap();
    let expected = (Dimension::base(BaseDimId::Prelude(
        crate::dimension::PreludeBaseDimension::Length,
    )) / Dimension::base(BaseDimId::Prelude(
        crate::dimension::PreludeBaseDimension::Time,
    )))
    .unwrap();
    assert_eq!(resolved, ResolvedTypeExpr::Quantity(expected));
}

// --- module-aware type resolution integration tests ---

#[test]
fn field_constraint_resolution_error_uses_definition_source() {
    let schema_source = "pub base dim Currency;\n\
                         pub type Price { Price(amount: Currency(min: 0.0 missing)) }\n";
    let raw_file = Parser::new(schema_source).parse_file().unwrap();
    let file = crate::syntax::desugar::desugar_multi_decls_in_file(raw_file);
    let schema_src = NamedSource::new("schema.gcl", Arc::new(schema_source.to_string()));
    let schema_id = crate::dag_id::DagId::root_in_package("test", "schema");
    let ir = crate::ir::lower::lower(&file, &schema_src).unwrap();
    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(schema_id.clone(), &file.declarations)
        .unwrap();
    let mut project_types = ProjectTypeStore::default();
    project_types.insert_graphcal_prelude().unwrap();
    project_types.insert_local_registry(&schema_id, &ir.registry, schema_src);

    // Emulate an importer whose ambient source is unrelated to the imported
    // type definition. The diagnostic must still use `schema_src`.
    let consumer_src = NamedSource::new("consumer.gcl", Arc::new("param p: Price;".to_string()));
    let error = type_resolve_with_modules(ir, &schema_id, &consumer_src, &resolver, &project_types)
        .unwrap_err();

    match error {
        GraphcalError::UnknownUnit { name, src, span } => {
            assert_eq!(name.to_string(), "missing");
            assert_eq!(src.name(), "schema.gcl");
            assert!(span.offset() + span.len() <= src.inner().len());
            assert_eq!(
                &src.inner()[span.offset()..span.offset() + span.len()],
                "missing"
            );
        }
        other => panic!("expected UnknownUnit against schema.gcl, got {other:?}"),
    }
}

#[test]
fn dag_type_indexes_share_the_project_store_definition_handle() {
    let tir = parse_and_type_resolve(
        "pub type Item { Item(value: Dimensionless) }\nparam item: Item = Item(value: 1.0);\n",
    )
    .unwrap();
    let name = ResolvedStructTypeName::from_def(
        tir.root_dag_id().clone(),
        StructTypeName::expect_valid("Item"),
    );
    let indexed = tir
        .root()
        .semantic
        .type_defs
        .struct_types
        .get(&name)
        .unwrap();
    let canonical = tir
        .project_type_store()
        .get_struct_type_handle(&name)
        .unwrap();
    assert!(Arc::ptr_eq(indexed, canonical));
}

#[test]
fn repeated_store_insertion_preserves_canonical_definition_handles() {
    let source = "pub index Axis = { A };\npub type Item { Item(value: Dimensionless) }\n";
    let raw_file = Parser::new(source).parse_file().unwrap();
    let file = crate::syntax::desugar::desugar_multi_decls_in_file(raw_file);
    let src = NamedSource::new("store.gcl", Arc::new(source.to_string()));
    let owner = crate::dag_id::DagId::root_in_package("test", "store");
    let ir = crate::ir::lower::lower(&file, &src).unwrap();
    let index_name = ResolvedIndexName::from_def(
        owner.clone(),
        crate::syntax::index_name::IndexName::expect_valid("Axis"),
    );
    let type_name =
        ResolvedStructTypeName::from_def(owner.clone(), StructTypeName::expect_valid("Item"));
    let mut store = ProjectTypeStore::default();
    store.insert_local_registry(&owner, &ir.registry, src.clone());
    let first_index = Arc::clone(store.get_index_handle(&index_name).unwrap());
    let first_type = Arc::clone(store.get_struct_type_handle(&type_name).unwrap());

    store.insert_local_registry(&owner, &ir.registry, src);

    assert!(Arc::ptr_eq(
        &first_index,
        store.get_index_handle(&index_name).unwrap()
    ));
    assert!(Arc::ptr_eq(
        &first_type,
        store.get_struct_type_handle(&type_name).unwrap()
    ));
}

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
    let mut project_types = ProjectTypeStore::default();
    project_types.insert_graphcal_prelude().map_err(|err| {
        internal_error(
            format!("test module type prelude failed: {err}"),
            &src,
            Span::new(0, 0),
        )
    })?;
    project_types.insert_local_registry(&parent_dag_id, &ir.registry, src.clone());
    let mut builder = type_resolve_builder_with_modules_and_cancellation(
        ir,
        &parent_dag_id,
        &src,
        &resolver,
        &project_types,
        &crate::cancellation::CancellationToken::unbounded(),
    )?;
    compile_inline_dag_bodies_test(&mut builder, &src, &parent_dag_id, &file.declarations)?;
    Ok(builder.finish())
}

/// Compile each inline dag body in `tir` with no self-import
/// preprocessing. Used by compiler-side integration tests that don't
/// have access to the eval crate's project pipeline.
fn compile_inline_dag_bodies_test(
    tir: &mut TirBuilder,
    src: &NamedSource<Arc<String>>,
    parent_dag_id: &crate::dag_id::DagId,
    parent_declarations: &[crate::desugar::desugared_ast::Declaration],
) -> Result<(), GraphcalError> {
    let dag_bodies = tir
        .registry()
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
    let mut project_types = ProjectTypeStore::default();
    project_types.insert_graphcal_prelude().map_err(|err| {
        internal_error(
            format!("test module type prelude failed: {err}"),
            src,
            Span::new(0, 0),
        )
    })?;
    project_types.insert_local_registry(parent_dag_id, tir.registry(), src.clone());

    for (name, body) in dag_bodies {
        let dag_body_ir = crate::ir::lower::lower_dag_body_to_ir(
            name.as_str(),
            &body,
            tir.registry(),
            &resolver,
            &crate::ir::resolve::ImportedValueNames::default(),
            HashMap::new(),
            src,
            parent_dag_id,
        )?;
        let dag_id = parent_dag_id.child(name.as_str());
        let mut compiled_dag =
            type_resolve_single_with_modules(dag_body_ir, &dag_id, src, &resolver, &project_types)?;
        compiled_dag.populate_projectable_outputs(&body);
        tir.insert_dag(compiled_dag)
            .map_err(|error| internal_error(error.to_string(), src, Span::new(0, 0)))?;
    }
    Ok(())
}

#[test]
fn tir_builder_preserves_root_and_rejects_duplicate_dag_identity() {
    let source = "node value: Dimensionless = 1.0;";
    let raw_file = Parser::new(source).parse_file().unwrap();
    let file = crate::syntax::desugar::desugar_multi_decls_in_file(raw_file);
    let src = NamedSource::new("test.gcl", Arc::new(source.to_string()));
    let root_id =
        crate::dag_id::DagId::from_virtual_relative_path(std::path::Path::new("test.gcl")).unwrap();
    let ir = crate::ir::lower::lower(&file, &src).unwrap();
    let mut resolver = ModuleResolver::default();
    resolver
        .add_module(root_id.clone(), &file.declarations)
        .unwrap();
    let mut project_types = ProjectTypeStore::default();
    project_types.insert_graphcal_prelude().unwrap();
    project_types.insert_local_registry(&root_id, &ir.registry, src.clone());
    let mut builder = type_resolve_builder_with_modules_and_cancellation(
        ir,
        &root_id,
        &src,
        &resolver,
        &project_types,
        &crate::cancellation::CancellationToken::unbounded(),
    )
    .unwrap();

    assert_eq!(builder.root().dag_id(), &root_id);
    let duplicate = builder.root().clone();
    assert!(matches!(
        builder.insert_dag(duplicate),
        Err(DagRegistryError::DuplicateDag { dag_id }) if dag_id == root_id
    ));

    let tir = builder.finish();
    assert_eq!(tir.root_dag_id(), &root_id);
    assert_eq!(tir.root().dag_id(), &root_id);
    assert_eq!(tir.dag_registry().len(), 1);
    assert!(tir.dag_registry().get(&root_id).is_some());
}

#[test]
fn finalized_tir_keeps_inline_dags_in_the_checked_registry() {
    let tir = parse_and_type_resolve(
        "dag child { pub node output: Dimensionless = 1.0; }\n\
         node result: Dimensionless = @child().output;",
    )
    .unwrap();
    let child_id = tir.root_dag_id().child("child");

    assert_eq!(tir.local_dags().count(), 2);
    assert_eq!(
        tir.dag_registry().get(&child_id).unwrap().dag_id(),
        &child_id
    );
    assert!(
        tir.dag_registry()
            .iter()
            .all(|(dag_id, dag)| dag_id == dag.dag_id())
    );
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
    let mut project_types = ProjectTypeStore::default();
    project_types.insert_graphcal_prelude().unwrap();
    project_types.insert_local_registry(&dag_id, &ir.registry, src.clone());

    let tir = type_resolve_with_modules(ir, &dag_id, &src, &resolver, &project_types).unwrap();
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
            .contains_key(&ScopedName::parse("dry_mass").unwrap())
    );
    assert!(
        tir.root()
            .resolved_decl_types
            .contains_key(&ScopedName::parse("delta_v").unwrap())
    );
    assert!(
        tir.root()
            .resolved_decl_types
            .contains_key(&ScopedName::parse("g0").unwrap())
    );
}

#[test]
fn type_resolve_indexed() {
    let source = include_str!("../../../../../tests/fixtures/valid/indexed.gcl");
    let tir = parse_and_type_resolve(source).unwrap();
    // delta_v should be Velocity[Maneuver]
    let dv_type = &tir.root().resolved_decl_types[&ScopedName::parse("delta_v").unwrap()];
    assert!(matches!(dv_type, ResolvedTypeExpr::Indexed { .. }));
}

#[test]
fn type_resolve_complex() {
    let source = include_str!("../../../../../tests/fixtures/valid/complex.gcl");
    let tir = parse_and_type_resolve(source).unwrap();

    assert!(matches!(
        &tir.root().resolved_decl_types[&ScopedName::parse("a").unwrap()],
        ResolvedTypeExpr::Complex {
            dimension: ResolvedDimArg::Concrete(dimension),
            ..
        } if *dimension == Dimension::base(BaseDimId::Prelude(crate::dimension::PreludeBaseDimension::Length))
    ));
    assert!(matches!(
        &tir.root().resolved_decl_types[&ScopedName::parse("series").unwrap()],
        ResolvedTypeExpr::Indexed { base, .. }
            if matches!(base.as_ref(), ResolvedTypeExpr::Complex { .. })
    ));
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
fn generic_index_param_shadows_same_named_module_index_in_type_args() {
    let source = r"
pub index I = { A };
pub type Box<I: Index> {
    Box(values: Dimensionless[I]),
}
pub type Wrap<I: Index> {
    Wrap(boxed: Box<I>, values: Dimensionless[I]),
}
";
    let tir = parse_and_type_resolve(source).unwrap();
    let boxed_field = tir
        .root()
        .semantic
        .type_defs
        .fields()
        .find_map(|(key, field)| (key.field.as_str() == "boxed").then(|| field.resolved_type()))
        .expect("Wrap.boxed field type");

    let ResolvedTypeExpr::GenericStruct { generic_args, .. } = boxed_field else {
        panic!("expected generic Box<I>, got {boxed_field:?}");
    };
    assert!(
        matches!(&generic_args[0], ResolvedGenericArg::Index(ResolvedIndex::GenericParam(name, _)) if name.as_str() == "I"),
        "generic argument should bind to the index parameter, got {:?}",
        generic_args[0]
    );
}

#[test]
fn type_resolve_generics() {
    let source = include_str!("../../../../../tests/fixtures/valid/generics.gcl");
    let tir = parse_and_type_resolve(source).unwrap();
    // pos_eci should be a GenericStruct with type args
    let pos_type = &tir.root().resolved_decl_types[&ScopedName::parse("pos_eci").unwrap()];
    match pos_type {
        ResolvedTypeExpr::GenericStruct {
            name, generic_args, ..
        } => {
            assert_eq!(name.as_str(), "Vec3");
            assert_eq!(generic_args.len(), 2);
            assert_eq!(
                generic_args[0],
                ResolvedGenericArg::Dim(ResolvedDimArg::Concrete(Dimension::base(
                    BaseDimId::Prelude(crate::dimension::PreludeBaseDimension::Length)
                )))
            );
            assert!(
                matches!(&generic_args[1], ResolvedGenericArg::Type(ResolvedTypeExpr::Struct(n, _)) if n.as_str() == "Eci")
            );
        }
        other => panic!("expected GenericStruct, got {other:?}"),
    }
    // x_pos should be quantity Length
    assert_eq!(
        tir.root().resolved_decl_types[&ScopedName::parse("x_pos").unwrap()],
        ResolvedTypeExpr::Quantity(Dimension::base(BaseDimId::Prelude(
            crate::dimension::PreludeBaseDimension::Length
        )))
    );
}

#[test]
fn type_resolve_default_type_params() {
    let source = include_str!("../../../../../tests/fixtures/valid/generics.gcl");
    let tir = parse_and_type_resolve(source).unwrap();

    // pos3_eci: Pos3<Length, Eci> — explicit, 2 type args
    let pos3_eci = &tir.root().resolved_decl_types[&ScopedName::parse("pos3_eci").unwrap()];
    match pos3_eci {
        ResolvedTypeExpr::GenericStruct {
            name, generic_args, ..
        } => {
            assert_eq!(name.as_str(), "Pos3");
            assert_eq!(generic_args.len(), 2);
            assert_eq!(
                generic_args[0],
                ResolvedGenericArg::Dim(ResolvedDimArg::Concrete(Dimension::base(
                    BaseDimId::Prelude(crate::dimension::PreludeBaseDimension::Length)
                )))
            );
            assert!(
                matches!(&generic_args[1], ResolvedGenericArg::Type(ResolvedTypeExpr::Struct(n, _)) if n.as_str() == "Eci")
            );
        }
        other => panic!("expected GenericStruct, got {other:?}"),
    }

    // pos3_default: Pos3<Length> — default fills in Unframed
    let pos3_default = &tir.root().resolved_decl_types[&ScopedName::parse("pos3_default").unwrap()];
    match pos3_default {
        ResolvedTypeExpr::GenericStruct {
            name, generic_args, ..
        } => {
            assert_eq!(name.as_str(), "Pos3");
            assert_eq!(generic_args.len(), 2);
            assert_eq!(
                generic_args[0],
                ResolvedGenericArg::Dim(ResolvedDimArg::Concrete(Dimension::base(
                    BaseDimId::Prelude(crate::dimension::PreludeBaseDimension::Length)
                )))
            );
            assert!(
                matches!(&generic_args[1], ResolvedGenericArg::Type(ResolvedTypeExpr::Struct(n, _)) if n.as_str() == "Unframed"),
                "expected Struct(Unframed), got {:?}",
                generic_args[1]
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
        element: Box::new(InferredType::Quantity(Dimension::dimensionless())),
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
    assert_eq!(dt, DeclaredType::Quantity(Dimension::dimensionless()));
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
fn convert_quantity() {
    let dim = Dimension::base(BaseDimId::Prelude(
        crate::dimension::PreludeBaseDimension::Length,
    ));
    let dt =
        resolved_to_declared_type(&ResolvedTypeExpr::Quantity(dim.clone()), &make_src()).unwrap();
    assert_eq!(dt, DeclaredType::Quantity(dim));
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
            base: Box::new(ResolvedTypeExpr::Quantity(Dimension::base(
                BaseDimId::Prelude(crate::dimension::PreludeBaseDimension::Length),
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
            element: Box::new(DeclaredType::Quantity(Dimension::base(BaseDimId::Prelude(
                crate::dimension::PreludeBaseDimension::Length,
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
    let resolved = resolve_source_type("Datetime", &[], &[], &[]).unwrap();
    assert_eq!(resolved, ResolvedTypeExpr::Datetime(TimeScale::UTC));
}

#[test]
fn resolve_datetime_utc() {
    let resolved = resolve_source_type("Datetime<UTC>", &[], &[], &[]).unwrap();
    assert_eq!(resolved, ResolvedTypeExpr::Datetime(TimeScale::UTC));
}

#[test]
fn resolve_datetime_tt() {
    let resolved = resolve_source_type("Datetime<TT>", &[], &[], &[]).unwrap();
    assert_eq!(resolved, ResolvedTypeExpr::Datetime(TimeScale::TT));
}

#[test]
fn resolve_datetime_tai() {
    let resolved = resolve_source_type("Datetime<TAI>", &[], &[], &[]).unwrap();
    assert_eq!(resolved, ResolvedTypeExpr::Datetime(TimeScale::TAI));
}

#[test]
fn resolve_datetime_gpst() {
    let resolved = resolve_source_type("Datetime<GPST>", &[], &[], &[]).unwrap();
    assert_eq!(resolved, ResolvedTypeExpr::Datetime(TimeScale::GPST));
}

#[test]
fn resolve_datetime_unknown_scale_error() {
    let err = resolve_source_type("Datetime<XYZ>", &[], &[], &[]).unwrap_err();
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
// FiniteIndexIdentity typed-reference tests
// -----------------------------------------------------------------------

#[test]
fn finite_index_identity_concrete_to_index_type_ref() -> Result<(), Box<dyn std::error::Error>> {
    let reference = NatPolyForm::from_constant(3)
        .to_finite_index_identity()?
        .to_index_type_ref()?;
    assert_eq!(
        reference
            .finite_index()
            .map(crate::registry::types::FiniteIndex::size_u64),
        Some(3)
    );
    assert_eq!(reference.display_name().as_str(), "Fin(3)");
    Ok(())
}

#[test]
fn finite_index_identity_symbolic_to_display_only_index_type_ref()
-> Result<(), Box<dyn std::error::Error>> {
    let reference = NatPolyForm::from_var(GenericParamName::expect_valid("N"))
        .add(&NatPolyForm::from_constant(1))
        .unwrap()
        .to_finite_index_identity()?
        .to_index_type_ref()?;
    assert_eq!(reference.finite_index(), None);
    assert_eq!(reference.display_name().as_str(), "Fin(N + 1)");
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
        &IndexName::expect_valid("Fin(4)"),
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
