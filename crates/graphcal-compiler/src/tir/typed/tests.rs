use super::type_expr::internal_error;
use super::*;
use crate::dimension::{BaseDimId, Dimension, Rational};
use crate::display::formatting_registry::FormattingRegistry;
use crate::generic_param::test_support::type_param;
use crate::resolved_name::{ResolvedIndexName, ResolvedStructTypeName, ResolvedUnitName};
use crate::semantic::time_scale::TimeScale;
use crate::semantic_error::SemanticErrorKind;
use crate::semantic_error::dimension::DimensionError;
use crate::semantic_error::index::IndexError;
use crate::semantic_error::name::NameError;
use crate::semantic_error::structure::StructError;
use crate::syntax::dimension::UnitName;
use crate::syntax::index_name::IndexName;
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::parser::Parser;
use crate::syntax::type_name::{GenericParamName, StructTypeName};

fn make_registry() -> FormattingRegistry {
    FormattingRegistry::graphcal_prelude().unwrap()
}

fn make_src() -> crate::source_id::SourceId {
    crate::source_registry::SourceRegistry::new().register("test", Arc::new(String::new()))
}

/// Resolve a source type through the production AST → HIR → TIR path.
///
/// Generic parameters are declared on a synthetic nominal type so HIR builds
/// the same typed [`hir::lower::GenericScope`] used for real generic field signatures.
fn resolve_source_type(
    source_type: &str,
    dim_params: &[GenericParamName],
    index_params: &[GenericParamName],
    nat_params: &[GenericParamName],
) -> Result<ResolvedDeclType, SemanticError> {
    let params = dim_params
        .iter()
        .map(|name| format!("{name}: Dim"))
        .chain(index_params.iter().map(|name| format!("{name}: Index")))
        .chain(nat_params.iter().map(|name| format!("{name}: Nat")))
        .collect::<Vec<_>>();
    if params.is_empty() {
        let tir = parse_and_type_resolve(&format!("param x: {source_type};"))?;
        return Ok(root_decl_type(&tir, "x").clone());
    }

    let source = format!(
        "type ResolutionSubject<{}> {{ ResolutionSubject(value: {source_type}) }}",
        params.join(", ")
    );
    let tir = parse_and_type_resolve(&source)?;
    tir.root()
        .semantic()
        .type_defs
        .nominals()
        .filter(|nominal| nominal.identity().as_str() == "ResolutionSubject")
        .flat_map(|nominal| {
            nominal
                .members()
                .flat_map(crate::tir::typed::NominalMember::fields)
        })
        .find(|field| field.field().name().as_str() == "value")
        .map(|field| field.semantics().resolved_type().clone())
        .ok_or_else(|| {
            SemanticError::internal_error(
                "test type field was not resolved through HIR".to_string(),
                crate::source_registry::SourceRegistry::new()
                    .register("test.gcl", Arc::new(source)),
                crate::diagnostic_anchor::DiagnosticAnchor::Source(Span::new(0, 0)),
            )
        })
}

fn resolved_param_type(program: &str, name: &str) -> Result<ResolvedDeclType, SemanticError> {
    let tir = parse_and_type_resolve(program)?;
    Ok(root_decl_type(&tir, name).clone())
}

/// The checked type of a root declaration written as `name`, if any.
fn root_decl_type_opt<'a>(tir: &'a CheckedTir, name: &str) -> Option<&'a ResolvedDeclType> {
    let written = ScopedName::local(crate::syntax::decl_name::DeclName::expect_valid(name));
    let identity = tir.root().body().bound_decl_identity(&written)?;
    tir.decl_type(identity).map(CheckedDeclType::resolved)
}

/// The checked type of a root declaration written as `name`.
fn root_decl_type<'a>(tir: &'a CheckedTir, name: &str) -> &'a ResolvedDeclType {
    root_decl_type_opt(tir, name).unwrap_or_else(|| panic!("`{name}` has no checked type"))
}

/// The value type of a scalar declaration type.
fn scalar(resolved: ResolvedDeclType) -> ResolvedValueType {
    match resolved {
        ResolvedDeclType::Value(value_type) => value_type,
        indexed @ ResolvedDeclType::Indexed { .. } => {
            panic!("expected a scalar type, got {indexed:?}")
        }
    }
}

/// A concrete scalar quantity type.
fn concrete_quantity(dimension: Dimension) -> ResolvedValueType {
    ResolvedValueType::Quantity(ResolvedDim::Concrete(dimension))
}

#[test]
fn value_declaration_records_carry_their_checked_types() {
    let tir = parse_and_type_resolve(
        "const node k: Int = 2;\nparam p: Length = 1.0 m;\nnode n: Bool = true;\n\
         assert a = @n;",
    )
    .unwrap();
    let length = Dimension::base(BaseDimId::Prelude(
        crate::dimension::PreludeBaseDimension::Length,
    ));
    for (name, resolved, declared) in [
        ("k", ResolvedValueType::Int, CheckedType::Int),
        (
            "p",
            concrete_quantity(length.clone()),
            CheckedType::Quantity(length),
        ),
        ("n", ResolvedValueType::Bool, CheckedType::Bool),
    ] {
        let written = ScopedName::local(crate::syntax::decl_name::DeclName::expect_valid(name));
        let identity = tir.root().body().bound_decl_identity(&written).unwrap();
        let checked = tir.decl_type(identity).unwrap();
        assert_eq!(
            checked.resolved(),
            &ResolvedDeclType::Value(resolved),
            "{name}"
        );
        assert_eq!(checked.declared(), &declared, "{name}");
        assert_eq!(tir.root().value_decl_type(identity), Some(checked));
        assert_eq!(
            tir.dag_containing_declaration(identity)
                .map(super::checked_dag::CheckedDag::dag_id),
            Some(tir.root_dag_id())
        );
    }
    assert_eq!(tir.root().value_decl_types().count(), 3);

    // Assertions are declarations without a value type.
    let assertion = tir
        .root()
        .body()
        .bound_decl_identity(&ScopedName::local(
            crate::syntax::decl_name::DeclName::expect_valid("a"),
        ))
        .unwrap();
    assert!(tir.decl_type(assertion).is_none());
    assert!(tir.dag_containing_declaration(assertion).is_none());
}

#[test]
fn decl_type_rejects_identities_owned_by_unknown_dags() {
    let tir = parse_and_type_resolve("node n: Bool = true;").unwrap();
    let foreign = crate::resolved_name::ResolvedDeclName::for_test(
        crate::dag_id::DagId::root_in_package("elsewhere", "elsewhere"),
        crate::syntax::decl_name::DeclName::expect_valid("n"),
    );
    assert!(tir.decl_type(&foreign).is_none());
    assert!(tir.dag_containing_declaration(&foreign).is_none());
}

#[test]
fn checked_decl_type_requires_a_concrete_type() {
    let src =
        crate::source_registry::SourceRegistry::new().register("test.gcl", Arc::new(String::new()));
    let generic = ResolvedDeclType::Value(ResolvedValueType::GenericTypeParam(
        type_param("T"),
        Span::new(0, 0),
    ));
    assert!(CheckedDeclType::new(generic, src).is_err());
    let checked =
        CheckedDeclType::new(ResolvedDeclType::Value(ResolvedValueType::Int), src).unwrap();
    assert_eq!(checked.declared(), &CheckedType::Int);
}

#[test]
fn declaration_identity_lookup_keeps_unknown_names_as_typed_probes() {
    let tir =
        parse_and_type_resolve("node known: Dimensionless = 1.0;\nassert valid = @known == 1.0;")
            .unwrap();

    let known = ScopedName::local(crate::syntax::decl_name::DeclName::expect_valid("known"));
    match tir.root().body().lookup_decl_identity(&known) {
        DeclarationIdentityLookup::Bound(identity) => {
            assert_eq!(identity.owner(), tir.root_dag_id());
            assert_eq!(identity.as_str(), "known");
        }
        DeclarationIdentityLookup::DiagnosticProbe(probe) => {
            panic!("known declaration became a diagnostic probe: {probe}");
        }
    }

    let unknown = ScopedName::in_scope(
        crate::syntax::module_name::ModuleAliasName::expect_valid("dependency"),
        crate::syntax::decl_name::DeclName::expect_valid("missing"),
    );
    match tir.root().body().lookup_decl_identity(&unknown) {
        DeclarationIdentityLookup::DiagnosticProbe(probe) => {
            assert_eq!(probe.dag_id(), tir.root_dag_id());
            assert_eq!(probe.name(), &unknown);
        }
        DeclarationIdentityLookup::Bound(identity) => {
            panic!("unknown declaration received fabricated identity `{identity}`");
        }
    }
}

#[test]
fn resolve_dimensionless() {
    let resolved = resolve_source_type("Dimensionless", &[], &[], &[]).unwrap();
    assert_eq!(
        scalar(resolved),
        concrete_quantity(Dimension::dimensionless())
    );
}

#[test]
fn resolve_bool() {
    let resolved = resolve_source_type("Bool", &[], &[], &[]).unwrap();
    assert_eq!(scalar(resolved), ResolvedValueType::Bool);
}

#[test]
fn resolve_int() {
    let resolved = resolve_source_type("Int", &[], &[], &[]).unwrap();
    assert_eq!(scalar(resolved), ResolvedValueType::Int);
}

#[test]
fn resolve_concrete_dimension() {
    let resolved = resolve_source_type("Length", &[], &[], &[]).unwrap();
    assert_eq!(
        scalar(resolved),
        concrete_quantity(Dimension::base(BaseDimId::Prelude(
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
    assert_eq!(scalar(resolved), concrete_quantity(expected));
}

#[test]
fn resolve_struct_type() {
    let resolved = resolved_param_type(
        "pub type TransferResult { TransferResult(value: Velocity) }\nparam x: TransferResult;",
        "x",
    )
    .unwrap();
    assert!(matches!(
        scalar(resolved),
        ResolvedValueType::Struct { name, generic_args, .. }
            if name.as_str() == "TransferResult" && generic_args.is_empty()
    ));
}

#[test]
fn resolve_generic_dim_param() {
    let dim_params = vec![GenericParamName::expect_valid("D")];
    let resolved = resolve_source_type("D", &dim_params, &[], &[]).unwrap();
    let ResolvedValueType::Quantity(dimension) = scalar(resolved) else {
        panic!("expected a quantity type");
    };
    assert!(matches!(dimension.lone_generic_param(), Some((name, _)) if name.name.as_str() == "D"));
}

#[test]
fn resolve_generic_dim_expr_with_power() {
    let dim_params = vec![GenericParamName::expect_valid("D")];
    let resolved = resolve_source_type("D^2", &dim_params, &[], &[]).unwrap();
    match scalar(resolved) {
        ResolvedValueType::Quantity(dimension @ ResolvedDim::Symbolic { .. }) => {
            assert_eq!(dimension.lone_generic_param(), None);
            let ResolvedDim::Symbolic { terms, .. } = dimension else {
                panic!("expected a symbolic quantity");
            };
            assert_eq!(terms.len(), 1);
            match &terms[0] {
                ResolvedDimTerm::GenericParam { name, power, .. } => {
                    assert_eq!(name.name.as_str(), "D");
                    assert_eq!(*power, Rational::from(2));
                }
                ResolvedDimTerm::Concrete { .. } => panic!("expected GenericParam term"),
            }
        }
        _ => panic!("expected a symbolic quantity"),
    }
}

#[test]
fn resolve_mixed_generic_concrete() {
    let dim_params = vec![GenericParamName::expect_valid("D")];
    let resolved = resolve_source_type("D * Length", &dim_params, &[], &[]).unwrap();
    match scalar(resolved) {
        ResolvedValueType::Quantity(ResolvedDim::Symbolic { terms, .. }) => {
            assert_eq!(terms.len(), 2);
            assert!(
                matches!(&terms[0], ResolvedDimTerm::GenericParam { name, .. } if name.name.as_str() == "D")
            );
            assert!(matches!(&terms[1], ResolvedDimTerm::Concrete { .. }));
        }
        other => panic!("expected a symbolic quantity, got {other:?}"),
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
        ResolvedDeclType::Indexed { element, indexes } => {
            assert_eq!(
                element,
                concrete_quantity(Dimension::base(BaseDimId::Prelude(
                    crate::dimension::PreludeBaseDimension::Length,
                )))
            );
            assert_eq!(indexes.len(), 1);
            assert!(
                matches!(&indexes[0], ResolvedIndex::Concrete(name, _) if name.as_str() == "Maneuver")
            );
        }
        ResolvedDeclType::Value(_) => panic!("expected Indexed"),
    }
}

#[test]
fn resolve_generic_indexed() {
    let dim_params = vec![GenericParamName::expect_valid("D")];
    let index_params = vec![GenericParamName::expect_valid("I")];
    let resolved = resolve_source_type("D[I]", &dim_params, &index_params, &[]).unwrap();
    match resolved {
        ResolvedDeclType::Indexed { element, indexes } => {
            assert!(matches!(
                element,
                ResolvedValueType::Quantity(ref dimension)
                    if matches!(dimension.lone_generic_param(), Some((name, _)) if name.name.as_str() == "D")
            ));
            assert_eq!(indexes.len(), 1);
            assert!(
                matches!(&indexes[0], ResolvedIndex::GenericParam(name, _) if name.name.as_str() == "I")
            );
        }
        ResolvedDeclType::Value(_) => panic!("expected Indexed"),
    }
}

#[test]
fn resolve_unknown_dimension_error() {
    let err = resolve_source_type("UnknownDim", &[], &[], &[]).unwrap_err();
    assert!(matches!(
        err,
        SemanticError::Located(crate::diagnostic::Diagnostic {
            kind: SemanticErrorKind::Dimension(DimensionError::UnknownDimension { .. }),
            ..
        })
    ));
}

#[test]
fn quantity_is_semantic_not_a_source_type_constructor() {
    let error = resolve_source_type("Quantity", &[], &[], &[]).unwrap_err();
    assert!(matches!(
        error,
        SemanticError::Located(crate::diagnostic::Diagnostic {
            kind: SemanticErrorKind::Dimension(DimensionError::UnknownDimension { .. }),
            ..
        })
    ));
}

#[test]
fn resolve_unknown_index_error() {
    let err = resolve_source_type("Length[UnknownIdx]", &[], &[], &[]).unwrap_err();
    assert!(matches!(
        err,
        SemanticError::Located(crate::diagnostic::Diagnostic {
            kind: SemanticErrorKind::Index(IndexError::UnknownIndex { .. }),
            ..
        })
    ));
}

#[test]
fn generic_dim_param_cannot_shadow_struct_type() {
    let result = parse_and_type_resolve(
        "type TransferResult { TransferResult(value: Velocity) }\n\
         type ResolutionSubject<TransferResult: Dim> {\n\
             ResolutionSubject(value: TransferResult)\n\
         }",
    );
    assert!(matches!(
        result,
        Err(SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Name(kind @ NameError::GenericParamShadowsStatic { .. }), .. }))
            if kind.to_string().contains("shadows a visible Static name")
    ));
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
    assert_eq!(scalar(resolved), concrete_quantity(expected));
}

// --- module-aware type resolution integration tests ---

#[test]
fn field_constraint_hir_error_uses_definition_source() {
    let schema_source = "pub base dim Currency;\n\
                         pub type Price { Price(amount: Currency(min: 0.0 missing)) }\n";
    let raw_file = Parser::new(schema_source).parse_file().unwrap();
    let file = crate::desugar::desugared_ast::File::from(raw_file);
    let schema_src = crate::source_registry::SourceRegistry::new()
        .register("schema.gcl", Arc::new(schema_source.to_string()));

    // Nominal field bounds now cross into HIR with their definition. An
    // unresolved unit therefore fails before a TIR or consumer scope exists.
    let error = crate::ir::lower::lower(&file, "schema.gcl", schema_src).unwrap_err();

    match error {
        SemanticError::Located(crate::diagnostic::Diagnostic {
            src,
            primary: span,
            kind: SemanticErrorKind::Dimension(DimensionError::UnknownUnit { name }),
        }) => {
            assert_eq!(name.to_string(), "missing");
            assert_eq!(src, schema_src);
            assert!(span.offset() + span.len() <= schema_source.len());
            assert_eq!(
                &schema_source[span.offset()..span.offset() + span.len()],
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
    let name = ResolvedStructTypeName::for_test(
        tir.root_dag_id().clone(),
        StructTypeName::expect_valid("Item"),
    );
    let indexed = tir
        .root()
        .semantic()
        .type_defs
        .nominal(&name)
        .unwrap()
        .definition();
    let canonical = tir
        .project_type_store()
        .get_struct_type_handle(&name)
        .unwrap();
    assert!(Arc::ptr_eq(indexed, canonical));
}

#[test]
fn tir_index_lookup_uses_the_project_store_for_declared_and_finite_indexes() {
    let tir =
        parse_and_type_resolve("index Axis = { A, B };\nparam values: Dimensionless[Fin(3)];\n")
            .unwrap();
    let declared = crate::semantic::checked_type::IndexTypeRef::<Concrete>::from_resolved(
        ResolvedIndexName::for_test(
            tir.root_dag_id().clone(),
            crate::syntax::index_name::IndexName::expect_valid("Axis"),
        ),
    );
    let finite = crate::semantic::checked_type::IndexTypeRef::<Concrete>::from_finite_index(
        crate::semantic::index_def::FiniteIndex::try_from_u64(3).unwrap(),
    );

    assert!(matches!(
        &tir.index_def(&declared).unwrap().kind,
        crate::semantic::index_def::IndexKind::Concrete(
            crate::semantic::index_def::ConcreteIndexKind::Named { variants }
        ) if variants.len().get() == 2
    ));
    assert_eq!(
        tir.index_def(&finite)
            .and_then(|definition| definition.concrete_cardinality())
            .map(crate::semantic::index_def::IndexCardinality::get),
        Some(3)
    );
}

#[test]
fn repeated_store_insertion_preserves_canonical_definition_handles() {
    let source = "pub index Axis = { A };\npub type Item { Item(value: Dimensionless) }\n";
    let raw_file = Parser::new(source).parse_file().unwrap();
    let file = crate::desugar::desugared_ast::File::from(raw_file);
    let src = crate::source_registry::SourceRegistry::new()
        .register("store.gcl", Arc::new(source.to_string()));
    let ir = crate::ir::lower::lower(&file, "store.gcl", src).unwrap();
    let owner = ir.dag_id().clone();
    let index_name = ResolvedIndexName::for_test(
        owner.clone(),
        crate::syntax::index_name::IndexName::expect_valid("Axis"),
    );
    let type_name = ResolvedStructTypeName::for_test(owner, StructTypeName::expect_valid("Item"));
    let mut store = ProjectTypeStore::default();
    store.insert_module(ir.definitions()).unwrap();
    let first_index = Arc::clone(store.get_index_handle(&index_name).unwrap());
    let first_type = Arc::clone(store.get_struct_type_handle(&type_name).unwrap());

    store.insert_module(ir.definitions()).unwrap();

    assert!(Arc::ptr_eq(
        &first_index,
        store.get_index_handle(&index_name).unwrap()
    ));
    assert!(Arc::ptr_eq(
        &first_type,
        store.get_struct_type_handle(&type_name).unwrap()
    ));
}

#[test]
fn dag_store_clones_share_canonical_body_handles() {
    let tir = parse_and_type_resolve("node x: Dimensionless = 1.0;").unwrap();
    let owner = tir.root_dag_id().clone();
    let store = tir.freeze_local_dag_store().unwrap();
    let first = store.handle(&owner).unwrap();
    let cloned = store.clone();
    let second = cloned.handle(&owner).unwrap();

    assert!(Arc::ptr_eq(first, second));
}

/// Instantiate and check a draft without external override summaries.
fn check_draft(
    draft: TirDraft,
    src: crate::source_id::SourceId,
) -> Result<CheckedTir, SemanticError> {
    let instantiated = draft.instantiate(&CheckedOverrideDependencies::default(), src)?;
    crate::outcome::without_cancellation(|cancellation| instantiated.check(src, cancellation))
}

fn importer_tir(path: &str, stores: &[&DagStore]) -> CheckedTir {
    let source = "node x: Dimensionless = 1.0;";
    let mut builder = parse_and_type_resolve_builder_named(source, path).unwrap();
    builder
        .install_shared_dag_stores(stores.iter().copied())
        .unwrap();
    check_draft(
        builder,
        crate::source_registry::SourceRegistry::new().register(path, Arc::new(source.to_string())),
    )
    .unwrap()
}

fn unit_overlay_tir(path: &str) -> (CheckedTir, ResolvedUnitName) {
    unit_overlay_tir_with(path, |_, _| {})
}

/// A checked body whose own dynamic unit is also installed as a runtime unit,
/// after `extra` edits the runtime-unit overlay.
fn unit_overlay_tir_with(
    path: &str,
    extra: impl FnOnce(&mut UncheckedTir, &ResolvedUnitName),
) -> (CheckedTir, ResolvedUnitName) {
    let source = "const unit local_step: Length = 2.0 m; node distance: Length = 1.0 local_step;";
    let mut tir = parse_and_type_resolve_builder_named(source, path)
        .unwrap()
        .finish();
    let unit = ResolvedUnitName::for_test(
        tir.root_dag_id().clone(),
        UnitName::expect_valid("local_step"),
    );
    let info = tir.unit_info(&unit).unwrap().clone();
    tir.insert_runtime_unit(unit.clone(), info).unwrap();
    extra(&mut tir, &unit);
    let instances = super::instance_graph::InstanceGraph::new(&tir.dags);
    let tir = InstantiatedTir { tir, instances }
        .check(
            crate::source_registry::SourceRegistry::new()
                .register(path, Arc::new(source.to_string())),
            &crate::cancellation::CancellationToken::unbounded(),
        )
        .unwrap();
    (tir, unit)
}

#[test]
fn imported_store_diamonds_share_bodies_and_units_without_republishing_imports() {
    let (leaf, unit) = unit_overlay_tir("leaf.gcl");
    let owner = leaf.root_dag_id().clone();
    let leaf = leaf.freeze_local_dag_store().unwrap();
    let left = importer_tir("left.gcl", &[&leaf]);
    let right = importer_tir("right.gcl", &[&leaf]);
    for importer in [&left, &right] {
        assert!(std::ptr::eq(
            leaf.get(&owner).unwrap(),
            importer.dag_registry().get(&owner).unwrap()
        ));
        assert!(std::ptr::eq(
            leaf.unit_info(&unit).unwrap(),
            importer.unit_info(&unit).unwrap()
        ));
    }
    let left = left.freeze_local_dag_store().unwrap();
    let right = right.freeze_local_dag_store().unwrap();
    for published in [&left, &right] {
        assert_eq!(published.len(), 1);
        assert!(published.get(&owner).is_none());
        assert!(published.unit_info(&unit).is_none());
    }
    let root = importer_tir("root.gcl", &[&leaf, &left, &right]);
    assert_eq!(root.dag_registry().len(), 4);
    assert!(std::ptr::eq(
        leaf.get(&owner).unwrap(),
        root.dag_registry().get(&owner).unwrap()
    ));
    assert!(std::ptr::eq(
        leaf.unit_info(&unit).unwrap(),
        root.unit_info(&unit).unwrap()
    ));
}

#[test]
fn registry_positions_follow_identity_order_whatever_the_install_order() {
    let stores = ["b.gcl", "c.gcl", "a.gcl"]
        .map(|path| importer_tir(path, &[]).freeze_local_dag_store().unwrap());
    let positions = |order: [usize; 3]| {
        let tir = importer_tir("root.gcl", &order.map(|store| &stores[store]));
        tir.dag_registry()
            .positioned()
            .map(|(position, dag)| (position.index(), dag.dag_id().clone()))
            .collect::<Vec<_>>()
    };
    let forward = positions([0, 1, 2]);
    assert_eq!(forward, positions([2, 1, 0]));
    assert_eq!(forward, positions([1, 2, 0]));
    let imported = forward[1..]
        .iter()
        .map(|(_, dag)| dag.clone())
        .collect::<Vec<_>>();
    let mut sorted = imported.clone();
    sorted.sort();
    assert_eq!(imported, sorted);
}

#[test]
fn installed_stores_bring_every_dag_their_bodies_call() {
    let leaf = importer_tir("leaf.gcl", &[])
        .freeze_local_dag_store()
        .unwrap();
    let (leaf_id, _) = leaf.iter().next().unwrap();
    let caller =
        crate::dag_id::DagId::from_virtual_relative_path(std::path::Path::new("caller.gcl"))
            .unwrap();
    // A store whose only body calls the leaf's body.
    let calling = DagStore {
        dags: HashMap::new(),
        runtime_units: HashMap::new(),
        external_callees: std::collections::BTreeMap::from([(leaf_id.clone(), caller.clone())]),
    };
    let draft = || {
        parse_and_type_resolve_builder_named("node x: Dimensionless = 1.0;", "root.gcl").unwrap()
    };

    let mut alone = draft();
    assert!(matches!(
        alone.install_shared_dag_stores([&calling]),
        Err(DagStoreInsertError::MissingCallee { caller: found, target })
            if found == caller && &target == leaf_id
    ));
    assert_eq!(alone.finish().dags.len(), 1, "nothing is installed");

    let mut together = draft();
    together
        .install_shared_dag_stores([&calling, &leaf])
        .unwrap();
    assert_eq!(together.finish().dags.len(), 2);

    let mut callee_first = draft();
    callee_first.install_shared_dag_stores([&leaf]).unwrap();
    callee_first.install_shared_dag_stores([&calling]).unwrap();
    assert_eq!(callee_first.finish().dags.len(), 2);
}

#[test]
fn local_and_shared_body_collisions_fail_in_both_insertion_orders() {
    let leaf = importer_tir("leaf.gcl", &[])
        .freeze_local_dag_store()
        .unwrap();
    let (owner, body) = leaf.iter().next().unwrap();
    for shared_first in [false, true] {
        let mut root =
            parse_and_type_resolve_builder_named("node x: Dimensionless = 1.0;", "root.gcl")
                .unwrap();
        if shared_first {
            root.install_shared_dag_stores([&leaf]).unwrap();
            assert!(
                matches!(root.insert_dag(body.body().clone()), Err(DagRegistryError::DuplicateDag { dag_id }) if &dag_id == owner)
            );
        } else {
            root.insert_dag(body.body().clone()).unwrap();
            assert!(
                matches!(root.install_shared_dag_stores([&leaf]), Err(DagStoreInsertError::Registry(DagRegistryError::DuplicateDag { dag_id })) if &dag_id == owner)
            );
        }
        assert_eq!(root.finish().dags.len(), 2);
    }
}

#[test]
fn publication_rejects_runtime_units_without_a_defining_body() {
    let mut missing = None;
    let (tir, _) = unit_overlay_tir_with("root.gcl", |tir, unit| {
        let identity = ResolvedUnitName::for_test(
            tir.root_dag_id()
                .inline_dag_child(crate::syntax::decl_name::DeclName::expect_valid("missing")),
            unit.to_unowned_def_name(),
        );
        let info = tir.unit_info(unit).unwrap().clone();
        tir.insert_runtime_unit(identity.clone(), info).unwrap();
        missing = Some(identity);
    });
    let missing = missing.unwrap();
    assert!(
        matches!(tir.freeze_local_dag_store(), Err(DagStoreFreezeError::MissingUnitOwner { identity }) if identity == missing)
    );
}

fn lower_store_hir(source: &str) -> crate::ir::model::HirDag {
    let raw = Parser::new(source).parse_file().unwrap();
    let file = crate::desugar::desugared_ast::File::from(raw);
    let src = crate::source_registry::SourceRegistry::new()
        .register("same.gcl", Arc::new(source.to_string()));
    crate::ir::lower::lower(&file, "same.gcl", src).unwrap()
}

#[test]
fn project_type_store_rejects_competing_dimension_definitions() {
    let first = lower_store_hir("dim Custom = Length;");
    let competing = lower_store_hir("dim Custom = Time;");
    let identity = crate::resolved_name::ResolvedDimName::for_test(
        first.dag_id().clone(),
        crate::syntax::dimension::DimName::expect_valid("Custom"),
    );
    let mut store = ProjectTypeStore::default();
    store.insert_module(first.definitions()).unwrap();

    assert!(matches!(
        store.insert_module(competing.definitions()),
        Err(ProjectTypeStoreInsertError::CompetingDimensionDefinition {
            identity: found,
        }) if found == identity
    ));
}

#[test]
fn project_type_store_rejects_competing_unit_definitions() {
    let first = lower_store_hir("const unit custom: Length = 2.0 m;");
    let competing = lower_store_hir("const unit custom: Length = 3.0 m;");
    let identity = crate::resolved_name::ResolvedUnitName::for_test(
        first.dag_id().clone(),
        crate::syntax::dimension::UnitName::expect_valid("custom"),
    );
    let mut store = ProjectTypeStore::default();
    store.insert_module(first.definitions()).unwrap();

    assert!(matches!(
        store.insert_module(competing.definitions()),
        Err(ProjectTypeStoreInsertError::CompetingUnitDefinition {
            identity: found,
        }) if found == identity
    ));
}

#[test]
fn project_type_store_rejects_competing_index_definitions() {
    let first = lower_store_hir("index Axis = { A };");
    let competing = lower_store_hir("index Axis = { B };");
    let identity = ResolvedIndexName::for_test(
        first.dag_id().clone(),
        crate::syntax::index_name::IndexName::expect_valid("Axis"),
    );
    let mut store = ProjectTypeStore::default();
    store.insert_module(first.definitions()).unwrap();

    assert!(matches!(
        store.insert_module(competing.definitions()),
        Err(ProjectTypeStoreInsertError::CompetingIndexDefinition {
            identity: found,
        }) if found == identity
    ));
}

#[test]
fn project_type_store_rejects_competing_nominal_definitions() {
    let first = lower_store_hir("type Item { Item(value: Dimensionless) }");
    let competing = lower_store_hir("type Item { Item(value: Bool) }");
    let identity = crate::resolved_name::ResolvedStructTypeName::for_test(
        first.dag_id().clone(),
        StructTypeName::expect_valid("Item"),
    );
    let mut store = ProjectTypeStore::default();
    store.insert_module(first.definitions()).unwrap();

    assert!(matches!(
        store.insert_module(competing.definitions()),
        Err(ProjectTypeStoreInsertError::CompetingNominalDefinition {
            identity: found,
        }) if found == identity
    ));
}

#[test]
fn tir_builder_accepts_identical_externs_and_rejects_competing_signatures() {
    let mut builder = parse_and_type_resolve_builder(
        r#"import plugin "graphcal:demo" as first {
            fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D;
        }"#,
    )
    .unwrap();
    let identical = parse_and_type_resolve(
        r#"
        import plugin "graphcal:demo" as second {
            fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D;
        }"#,
    )
    .unwrap();
    let (key, function) = identical.extern_functions().iter().next().unwrap();
    builder
        .insert_extern_function(key.clone(), function.clone())
        .unwrap();

    let competing = parse_and_type_resolve(
        r#"import plugin "graphcal:demo" as third {
            fn lerp(a: Length, b: Length) -> Length;
        }"#,
    )
    .unwrap();
    let (key, function) = competing.extern_functions().iter().next().unwrap();
    let expected_key = key.clone();
    let error = builder
        .insert_extern_function(key.clone(), function.clone())
        .unwrap_err();

    assert_eq!(error.plugin, expected_key.plugin);
    assert_eq!(error.name, expected_key.name);
}

/// Single-file integration helper: lower + type-resolve + compile each
/// inline dag body using the dumb `lower_dag_body_to_ir` primitive
/// directly (no self-import preprocessing — fixtures exercised here
/// either don't use self-imports or are expected to surface errors that
/// fall out of the unprocessed body).
fn parse_and_type_resolve(source: &str) -> Result<CheckedTir, SemanticError> {
    let draft = parse_and_type_resolve_builder(source)?;
    check_draft(
        draft,
        crate::source_registry::SourceRegistry::new()
            .register("test.gcl", Arc::new(source.to_string())),
    )
}

fn parse_and_type_resolve_builder(source: &str) -> Result<TirDraft, SemanticError> {
    parse_and_type_resolve_builder_named(source, "test.gcl")
}

fn parse_and_type_resolve_builder_named(
    source: &str,
    path: &str,
) -> Result<TirDraft, SemanticError> {
    let raw_file = Parser::new(source).parse_file().unwrap();
    let desugared = crate::desugar::desugared_ast::File::from(raw_file);
    let file = desugared;
    let src =
        crate::source_registry::SourceRegistry::new().register(path, Arc::new(source.to_string()));
    let lowered = crate::ir::lower::lower_file_with_inline_dags_for_test(&file, path, src)?;
    let resolver = lowered.resolver;
    let mut project_types = ProjectTypeStore::default();
    project_types.insert_graphcal_prelude().map_err(|err| {
        internal_error(
            format!("test module type prelude failed: {err}"),
            src,
            Span::new(0, 0),
        )
    })?;
    for dag in std::iter::once(&lowered.root).chain(&lowered.inline_dags) {
        project_types
            .insert_module(dag.definitions())
            .map_err(|error| internal_error(error.to_string(), src, Span::new(0, 0)))?;
    }
    let mut builder = crate::outcome::without_cancellation(|cancellation| {
        let signed = resolve_hir_signature_with_modules_and_cancellation(
            lowered.root,
            src,
            &resolver,
            &project_types,
            cancellation,
        )?;
        TirDraft::resolve_root(
            signed,
            &|_| None,
            src,
            &resolver,
            Arc::new(project_types.clone()),
            cancellation,
        )
    })?;
    for dag_body_ir in lowered.inline_dags {
        let compiled_dag =
            type_resolve_single_with_modules(dag_body_ir, src, &resolver, &project_types)?;
        builder
            .insert_dag(compiled_dag)
            .map_err(|error| internal_error(error.to_string(), src, Span::new(0, 0)))?;
    }
    Ok(builder)
}

#[test]
fn tir_builder_preserves_root_and_rejects_duplicate_dag_identity() {
    let source = "node value: Dimensionless = 1.0;";
    let raw_file = Parser::new(source).parse_file().unwrap();
    let file = crate::desugar::desugared_ast::File::from(raw_file);
    let src = crate::source_registry::SourceRegistry::new()
        .register("test.gcl", Arc::new(source.to_string()));
    let root_id =
        crate::dag_id::DagId::from_virtual_relative_path(std::path::Path::new("test.gcl")).unwrap();
    let ir = crate::ir::lower::lower(&file, "test.gcl", src).unwrap();
    let mut modules = crate::resolve::builder::TestModules::default();
    modules.add(root_id.clone(), &file.declarations);
    let resolver = modules.build().unwrap();
    let mut project_types = ProjectTypeStore::default();
    project_types.insert_graphcal_prelude().unwrap();
    project_types.insert_module(ir.definitions()).unwrap();
    let mut builder = type_resolve_draft(ir, src, &resolver, Arc::new(project_types)).unwrap();

    assert_eq!(builder.root().dag_id(), &root_id);
    let duplicate = builder.root().clone();
    assert!(matches!(
        builder.insert_dag(duplicate),
        Err(DagRegistryError::DuplicateDag { dag_id }) if dag_id == root_id
    ));

    let tir = check_draft(builder, src).unwrap();
    assert_eq!(tir.root_dag_id(), &root_id);
    assert_eq!(tir.root().dag_id(), &root_id);
    assert_eq!(tir.dag_registry().len(), 1);
    assert!(tir.dag_registry().get(&root_id).is_some());
}

#[test]
fn finalized_tir_keeps_inline_dags_in_the_checked_registry() {
    let tir = parse_and_type_resolve(
        "dag child { pub node output: Dimensionless = 1.0; }\n\
         node result: Dimensionless = @child()::output;",
    )
    .unwrap();
    let child_id = tir
        .root_dag_id()
        .inline_dag_child(crate::syntax::decl_name::DeclName::expect_valid("child"));

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
fn closing_a_registry_resolves_every_call_slot_or_names_the_missing_callee() {
    let tir = parse_and_type_resolve(
        "dag child { pub node output: Dimensionless = 1.0; }\n\
         node result: Dimensionless = @child()::output;",
    )
    .unwrap();
    let child_id = tir
        .root_dag_id()
        .inline_dag_child(crate::syntax::decl_name::DeclName::expect_valid("child"));
    let registry = &tir.dags;
    let (child, _) = registry.get_positioned(&child_id).unwrap();
    assert_eq!(
        registry.callee_positions(crate::tir::typed::dag_position::DagPosition::ROOT),
        [child]
    );

    let closed = CheckedDagRegistry::close(registry.dags.clone()).unwrap();
    assert_eq!(
        closed.callee_positions(crate::tir::typed::dag_position::DagPosition::ROOT),
        [child]
    );

    let error = CheckedDagRegistry::close(super::dag_slots::DagSlots::new(registry.root().clone()))
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "DAG `{}` calls DAG `{child_id}`, which is not in its program",
            tir.root_dag_id()
        )
    );
}

#[test]
fn declaration_views_expose_each_kind_without_bodies() {
    use super::declaration_view::{DeclarationKind, ValueDeclaration};
    use crate::declaration_category::{DeclCategory, ValueDeclCategory};
    use crate::plot_visibility::PlotVisibility;

    let tir = parse_and_type_resolve(
        "const node BASE: Dimensionless = 1.0;\n\
         param required: Dimensionless;\n\
         param defaulted: Dimensionless = 2.0;\n\
         node total: Dimensionless = @BASE + @required + @defaulted;\n\
         assert positive = @total > 0.0;\n\
         plot trend = { mark: line, encode: { x: @total, y: @total } };\n\
         figure summary = { plots: [trend] };",
    )
    .unwrap();
    let dag = tir.root();
    let views = dag.declarations().collect::<Vec<_>>();
    assert_eq!(
        views
            .iter()
            .map(|view| view.name().as_str())
            .collect::<Vec<_>>(),
        [
            "BASE",
            "required",
            "defaulted",
            "total",
            "positive",
            "trend",
            "summary"
        ]
    );
    let value = |index: usize| views[index].value().unwrap();
    let categories = (0..4)
        .map(|index| {
            let ValueDeclaration {
                category,
                has_default,
                ..
            } = value(index);
            (category, has_default)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        categories,
        [
            (ValueDeclCategory::Const, false),
            (ValueDeclCategory::Param, false),
            (ValueDeclCategory::Param, true),
            (ValueDeclCategory::Node, false),
        ]
    );
    assert_eq!(
        views[3].category(),
        DeclCategory::Value(ValueDeclCategory::Node)
    );
    assert!(matches!(views[4].kind(), DeclarationKind::Assert { .. }));
    assert_eq!(views[4].category(), DeclCategory::Assert);
    assert!(views[4].value().is_none());
    assert!(matches!(
        views[5].kind(),
        DeclarationKind::Plot {
            visibility: PlotVisibility::Standalone
        }
    ));
    assert_eq!(views[5].category(), DeclCategory::Plot);
    let DeclarationKind::Figure { plot_names } = views[6].kind() else {
        panic!("a figure view lists its plots");
    };
    assert_eq!(plot_names.len(), 1);
    assert_eq!(views[6].category(), DeclCategory::Figure);

    for view in &views {
        let found = dag.declaration(view.identity()).unwrap();
        assert_eq!(found.identity(), view.identity());
        assert_eq!(found.category(), view.category());
        assert_eq!(view.identity().owner(), tir.root_dag_id());
    }
}

#[test]
fn module_aware_type_resolve_records_semantic_deps() {
    let source = "const node C: Dimensionless = 1.0;\n\
                  const node D: Dimensionless = @C;\n\
                  param p: Dimensionless;\n\
                  node x: Dimensionless = @p + @D;";
    let raw_file = Parser::new(source).parse_file().unwrap();
    let desugared = crate::desugar::desugared_ast::File::from(raw_file);
    let file = desugared;
    let src = crate::source_registry::SourceRegistry::new()
        .register("test.gcl", Arc::new(source.to_string()));
    let dag_id =
        crate::dag_id::DagId::from_virtual_relative_path(std::path::Path::new("test.gcl")).unwrap();
    let ir = crate::ir::lower::lower(&file, "test.gcl", src).unwrap();
    let mut modules = crate::resolve::builder::TestModules::default();
    modules.add(dag_id.clone(), &file.declarations);
    let resolver = modules.build().unwrap();
    let mut project_types = ProjectTypeStore::default();
    project_types.insert_graphcal_prelude().unwrap();
    project_types.insert_module(ir.definitions()).unwrap();

    let tir = type_resolve_draft(ir, src, &resolver, Arc::new(project_types))
        .unwrap()
        .finish();
    let deps = &tir.root().semantic.dependencies;
    let c = ResolvedDeclName::for_test(dag_id.clone(), DeclName::expect_valid("C"));
    let d = ResolvedDeclName::for_test(dag_id.clone(), DeclName::expect_valid("D"));
    let p = ResolvedDeclName::for_test(dag_id.clone(), DeclName::expect_valid("p"));
    let x = ResolvedDeclName::for_test(dag_id, DeclName::expect_valid("x"));

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
    assert!(root_decl_type_opt(&tir, "dry_mass").is_some());
    assert!(root_decl_type_opt(&tir, "delta_v").is_some());
    assert!(root_decl_type_opt(&tir, "g0").is_some());
}

#[test]
fn type_resolve_indexed() {
    let source = include_str!("../../../../../tests/fixtures/valid/indexed.gcl");
    let tir = parse_and_type_resolve(source).unwrap();
    // delta_v should be Velocity[Maneuver]
    let dv_type = root_decl_type(&tir, "delta_v");
    assert!(matches!(dv_type, ResolvedDeclType::Indexed { .. }));
}

#[test]
fn type_resolve_complex() {
    let source = include_str!("../../../../../tests/fixtures/valid/complex.gcl");
    let tir = parse_and_type_resolve(source).unwrap();

    assert!(matches!(
        root_decl_type(&tir, "a"),
        ResolvedDeclType::Value(ResolvedValueType::Complex {
            dimension: ResolvedDim::Concrete(dimension),
            ..
        }) if *dimension == Dimension::base(BaseDimId::Prelude(crate::dimension::PreludeBaseDimension::Length))
    ));
    assert!(matches!(
        root_decl_type(&tir, "series"),
        ResolvedDeclType::Indexed {
            element: ResolvedValueType::Complex { .. },
            ..
        }
    ));
}

#[test]
fn type_resolve_hohmann() {
    // hohmann.gcl uses DAG+include. Project-level `graphcal check`
    // accepts it (see the CLI tests), but single-file TIR resolution
    // rejects it: there's no project loader to resolve cross-DAG
    // references like `import hohmann::{...}`, and `@transfer` from the
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
fn generic_index_param_cannot_shadow_module_index() {
    let source = r"
pub index I = { A };
pub type Box<I: Index> {
    Box(values: Dimensionless[I]),
}
pub type Wrap<I: Index> {
    Wrap(boxed: Box<I>, values: Dimensionless[I]),
}
";
    assert!(matches!(
        parse_and_type_resolve(source),
        Err(SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Name(kind @ NameError::GenericParamShadowsStatic { .. }), .. }))
            if kind.to_string().contains("shadows a visible Static name")
    ));
}

#[test]
fn type_resolve_generics() {
    let source = include_str!("../../../../../tests/fixtures/valid/generics.gcl");
    let tir = parse_and_type_resolve(source).unwrap();
    // pos_eci should be a struct application with type args
    let pos_type = root_decl_type(&tir, "pos_eci");
    match pos_type {
        ResolvedDeclType::Value(ResolvedValueType::Struct {
            name, generic_args, ..
        }) => {
            assert_eq!(name.as_str(), "Vec3");
            assert_eq!(generic_args.len(), 2);
            assert_eq!(
                generic_args[0],
                ResolvedGenericArg::Dim(ResolvedDim::Concrete(Dimension::base(
                    BaseDimId::Prelude(crate::dimension::PreludeBaseDimension::Length)
                )))
            );
            assert!(
                matches!(&generic_args[1], ResolvedGenericArg::Type(ResolvedValueType::Struct { name: n, .. }) if n.as_str() == "Eci")
            );
        }
        other => panic!("expected a struct application, got {other:?}"),
    }
    // x_pos should be quantity Length
    assert_eq!(
        *root_decl_type(&tir, "x_pos"),
        ResolvedDeclType::Value(concrete_quantity(Dimension::base(BaseDimId::Prelude(
            crate::dimension::PreludeBaseDimension::Length
        ))))
    );
}

#[test]
fn type_resolve_default_type_params() {
    let source = include_str!("../../../../../tests/fixtures/valid/generics.gcl");
    let tir = parse_and_type_resolve(source).unwrap();

    // pos3_eci: Pos3<Length, Eci> — explicit, 2 type args
    let pos3_eci = root_decl_type(&tir, "pos3_eci");
    match pos3_eci {
        ResolvedDeclType::Value(ResolvedValueType::Struct {
            name, generic_args, ..
        }) => {
            assert_eq!(name.as_str(), "Pos3");
            assert_eq!(generic_args.len(), 2);
            assert_eq!(
                generic_args[0],
                ResolvedGenericArg::Dim(ResolvedDim::Concrete(Dimension::base(
                    BaseDimId::Prelude(crate::dimension::PreludeBaseDimension::Length)
                )))
            );
            assert!(
                matches!(&generic_args[1], ResolvedGenericArg::Type(ResolvedValueType::Struct { name: n, .. }) if n.as_str() == "Eci")
            );
        }
        other => panic!("expected a struct application, got {other:?}"),
    }

    // pos3_default: Pos3<Length> — default fills in Unframed
    let pos3_default = root_decl_type(&tir, "pos3_default");
    match pos3_default {
        ResolvedDeclType::Value(ResolvedValueType::Struct {
            name, generic_args, ..
        }) => {
            assert_eq!(name.as_str(), "Pos3");
            assert_eq!(generic_args.len(), 2);
            assert_eq!(
                generic_args[0],
                ResolvedGenericArg::Dim(ResolvedDim::Concrete(Dimension::base(
                    BaseDimId::Prelude(crate::dimension::PreludeBaseDimension::Length)
                )))
            );
            assert!(
                matches!(&generic_args[1], ResolvedGenericArg::Type(ResolvedValueType::Struct { name: n, .. }) if n.as_str() == "Unframed"),
                "expected Struct(Unframed), got {:?}",
                generic_args[1]
            );
        }
        other => panic!("expected a struct application, got {other:?}"),
    }
}

// --- to_checked_type() tests ---

use crate::semantic::checked_type::{CheckedType, Concrete, IndexTypeRef, StructTypeRef};

#[test]
fn generic_index_substitution_preserves_resolved_owner() {
    let src = make_src();
    let owner = crate::dag_id::DagId::root_in_package("test", "a");
    let resolved_index = ResolvedIndexName::for_test(owner, IndexName::expect_valid("Phase"));
    let generic = type_param("I");
    let resolved_type = ResolvedDeclType::Indexed {
        element: concrete_quantity(Dimension::dimensionless()),
        indexes: NonEmpty::singleton(ResolvedIndex::GenericParam(
            generic.clone(),
            Span::new(0, 0),
        )),
    };
    let mut substitution = Substitution::default();
    substitution.bind(
        generic,
        ResolvedGenericArg::Index(ResolvedIndex::Concrete(
            resolved_index.clone(),
            Span::new(0, 0),
        )),
    );
    let substituted = substitution
        .apply(&resolved_type)
        .unwrap()
        .to_checked_type(src)
        .unwrap();
    let CheckedType::Indexed { index, .. } = substituted else {
        panic!("expected indexed type after substitution");
    };
    assert_eq!(index.declared_resolved(), Some(&resolved_index));
}

#[test]
fn convert_dimensionless() {
    let dt = concrete_quantity(Dimension::dimensionless())
        .to_checked_type(make_src())
        .unwrap();
    assert_eq!(dt, CheckedType::Quantity(Dimension::dimensionless()));
}

#[test]
fn convert_bool() {
    let dt = ResolvedValueType::Bool.to_checked_type(make_src()).unwrap();
    assert_eq!(dt, CheckedType::Bool);
}

#[test]
fn convert_int() {
    let dt = ResolvedValueType::Int.to_checked_type(make_src()).unwrap();
    assert_eq!(dt, CheckedType::Int);
}

#[test]
fn convert_quantity() {
    let dim = Dimension::base(BaseDimId::Prelude(
        crate::dimension::PreludeBaseDimension::Length,
    ));
    let dt = concrete_quantity(dim.clone())
        .to_checked_type(make_src())
        .unwrap();
    assert_eq!(dt, CheckedType::Quantity(dim));
}

#[test]
fn convert_struct() {
    let owner = crate::dag_id::DagId::root_in_package("test", "test");
    let resolved = ResolvedStructTypeName::for_test(owner, StructTypeName::expect_valid("Foo"));
    let dt = ResolvedValueType::Struct {
        name: resolved.clone(),
        generic_args: Vec::new(),
        span: Span::new(0, 0),
    }
    .to_checked_type(make_src())
    .unwrap();
    assert_eq!(
        dt,
        CheckedType::Struct(StructTypeRef::from_resolved(resolved), vec![])
    );
}

#[test]
fn convert_indexed() {
    let owner = crate::dag_id::DagId::root_in_package("test", "test");
    let resolved_index = ResolvedIndexName::for_test(owner, IndexName::expect_valid("M"));
    let dt = ResolvedDeclType::Indexed {
        element: concrete_quantity(Dimension::base(BaseDimId::Prelude(
            crate::dimension::PreludeBaseDimension::Length,
        ))),
        indexes: NonEmpty::singleton(ResolvedIndex::Concrete(
            resolved_index.clone(),
            Span::new(0, 0),
        )),
    }
    .to_checked_type(make_src())
    .unwrap();
    assert_eq!(
        dt,
        CheckedType::Indexed {
            element: Box::new(CheckedType::Quantity(Dimension::base(BaseDimId::Prelude(
                crate::dimension::PreludeBaseDimension::Length,
            )))),
            index: IndexTypeRef::from_resolved(resolved_index),
        }
    );
}

#[test]
fn convert_generic_dim_param_fails() {
    let span = Span::new(0, 0);
    let dimension = |power: i16| ResolvedDim::Symbolic {
        terms: vec![ResolvedDimTerm::GenericParam {
            name: type_param("D"),
            power: Rational::from(power),
            op: crate::desugar::desugared_ast::MulDivOp::Mul,
            span,
        }],
        span,
    };
    let err = ResolvedValueType::Quantity(dimension(1))
        .to_checked_type(make_src())
        .unwrap_err();
    assert!(matches!(
        err,
        SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Struct(kind @ StructError::UnboundGenericInConcreteType { .. }), .. })
            if kind.to_string() == "cannot use generic dimension parameter `D` as a concrete type"
    ));
    let err = ResolvedValueType::Quantity(dimension(2))
        .to_checked_type(make_src())
        .unwrap_err();
    assert!(matches!(
        err,
        SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Struct(kind @ StructError::UnboundGenericInConcreteType { .. }), .. })
            if kind.to_string() == "cannot use generic dimension expression as a concrete type"
    ));
}

#[test]
fn convert_generic_index_fails() {
    let err = ResolvedDeclType::Indexed {
        element: concrete_quantity(Dimension::dimensionless()),
        indexes: NonEmpty::singleton(ResolvedIndex::GenericParam(
            type_param("I"),
            Span::new(0, 0),
        )),
    }
    .to_checked_type(make_src())
    .unwrap_err();
    assert!(matches!(
        err,
        SemanticError::Located(crate::diagnostic::Diagnostic {
            kind: SemanticErrorKind::Struct(StructError::UnboundGenericInConcreteType { .. }),
            ..
        })
    ));
}

// --- Datetime type resolution tests ---

#[test]
fn resolve_bare_datetime() {
    let resolved = resolve_source_type("Datetime", &[], &[], &[]).unwrap();
    assert_eq!(
        scalar(resolved),
        ResolvedValueType::Datetime(TimeScale::UTC)
    );
}

#[test]
fn resolve_datetime_utc() {
    let resolved = resolve_source_type("Datetime<UTC>", &[], &[], &[]).unwrap();
    assert_eq!(
        scalar(resolved),
        ResolvedValueType::Datetime(TimeScale::UTC)
    );
}

#[test]
fn resolve_datetime_tt() {
    let resolved = resolve_source_type("Datetime<TT>", &[], &[], &[]).unwrap();
    assert_eq!(scalar(resolved), ResolvedValueType::Datetime(TimeScale::TT));
}

#[test]
fn resolve_datetime_tai() {
    let resolved = resolve_source_type("Datetime<TAI>", &[], &[], &[]).unwrap();
    assert_eq!(
        scalar(resolved),
        ResolvedValueType::Datetime(TimeScale::TAI)
    );
}

#[test]
fn resolve_datetime_gpst() {
    let resolved = resolve_source_type("Datetime<GPST>", &[], &[], &[]).unwrap();
    assert_eq!(
        scalar(resolved),
        ResolvedValueType::Datetime(TimeScale::GPST)
    );
}

#[test]
fn resolve_datetime_unknown_scale_error() {
    let err = resolve_source_type("Datetime<XYZ>", &[], &[], &[]).unwrap_err();
    assert!(matches!(
        err,
        SemanticError::Located(crate::diagnostic::Diagnostic {
            kind: SemanticErrorKind::Dimension(DimensionError::UnknownTimeScale { .. }),
            ..
        })
    ));
}

#[test]
fn convert_datetime_utc() {
    let dt = ResolvedValueType::Datetime(TimeScale::UTC)
        .to_checked_type(make_src())
        .unwrap();
    assert_eq!(dt, CheckedType::Datetime(TimeScale::UTC));
}

#[test]
fn convert_datetime_tt() {
    let dt = ResolvedValueType::Datetime(TimeScale::TT)
        .to_checked_type(make_src())
        .unwrap();
    assert_eq!(dt, CheckedType::Datetime(TimeScale::TT));
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
    let a = NatPolyForm::from_var(type_param("N"));
    let b = NatPolyForm::from_var(type_param("N"));
    assert!(a.is_leq(&b));
}

#[test]
fn nat_leq_var_plus_constant() {
    // N <= N + 1
    let a = NatPolyForm::from_var(type_param("N"));
    let b = NatPolyForm::from_var(type_param("N"))
        .add(&NatPolyForm::from_constant(1))
        .unwrap();
    assert!(a.is_leq(&b));
}

#[test]
fn nat_leq_var_plus_constant_reverse() {
    // N + 1 <= N → false
    let a = NatPolyForm::from_var(type_param("N"))
        .add(&NatPolyForm::from_constant(1))
        .unwrap();
    let b = NatPolyForm::from_var(type_param("N"));
    assert!(!a.is_leq(&b));
}

#[test]
fn nat_leq_different_vars() {
    // N <= M → false (N could be larger)
    let a = NatPolyForm::from_var(type_param("N"));
    let b = NatPolyForm::from_var(type_param("M"));
    assert!(!a.is_leq(&b));
}

#[test]
fn nat_leq_zero_leq_anything() {
    // 0 <= N
    let a = NatPolyForm::from_constant(0);
    let b = NatPolyForm::from_var(type_param("N"));
    assert!(a.is_leq(&b));
}

// -----------------------------------------------------------------------
// Finite structural index typed-reference tests
// -----------------------------------------------------------------------

#[test]
fn finite_index_concrete_form_to_index_type_ref() -> Result<(), Box<dyn std::error::Error>> {
    let reference = crate::semantic::checked_type::IndexTypeRef::from_finite_index_form(
        NatPolyForm::from_constant(3),
    )?;
    assert_eq!(
        reference
            .finite_index()
            .map(crate::semantic::index_def::FiniteIndex::size_u64),
        Some(3)
    );
    assert_eq!(reference.display_name().to_string(), "Fin(3)");
    Ok(())
}

#[test]
fn finite_index_symbolic_form_to_display_only_index_type_ref()
-> Result<(), Box<dyn std::error::Error>> {
    let reference = crate::semantic::checked_type::IndexTypeRef::from_finite_index_form(
        NatPolyForm::from_var(type_param("N"))
            .add(&NatPolyForm::from_constant(1))
            .unwrap(),
    )?;
    assert_eq!(reference.finite_index(), None);
    assert_eq!(reference.display_name().to_string(), "Fin(N + 1)");
    Ok(())
}

#[test]
fn resolved_index_display_renders_source_spelling() {
    let span = Span::new(0, 0);
    let owner =
        crate::dag_id::DagId::from_virtual_relative_path(std::path::Path::new("test.gcl")).unwrap();
    let concrete = ResolvedIndex::Concrete(
        ResolvedIndexName::for_test(
            owner,
            crate::syntax::index_name::IndexName::expect_valid("Phase"),
        ),
        span,
    );
    let generic = ResolvedIndex::GenericParam(type_param("I"), span);
    let finite = ResolvedIndex::Finite(
        NatPolyForm::from_var(type_param("N"))
            .add(&NatPolyForm::from_constant(1))
            .unwrap(),
        span,
    );
    assert_eq!(concrete.to_string(), "Phase");
    assert_eq!(generic.to_string(), "I");
    assert_eq!(finite.to_string(), "Fin(N + 1)");
    assert_eq!(
        ResolvedValueType::Key {
            index: finite,
            span
        }
        .format(&make_registry()),
        "Key<Fin(N + 1)>"
    );
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
    let n = NatPolyForm::from_var(type_param("N"));
    let three = NatPolyForm::from_constant(3);
    let result = n.mul(&three).unwrap();
    // Should format as "3 * N"
    assert_eq!(result.format(), "3 * N");
    // Evaluate with N=5 → 15
    let mut bindings = HashMap::new();
    bindings.insert(type_param("N"), 5);
    assert_eq!(result.evaluate(&bindings), Some(15));
}

#[test]
fn nat_mul_two_vars() {
    // M * N
    let m = NatPolyForm::from_var(type_param("M"));
    let n = NatPolyForm::from_var(type_param("N"));
    let result = m.mul(&n).unwrap();
    assert_eq!(result.format(), "M * N");
    let mut bindings = HashMap::new();
    bindings.insert(type_param("M"), 3);
    bindings.insert(type_param("N"), 4);
    assert_eq!(result.evaluate(&bindings), Some(12));
}

#[test]
fn nat_mul_distributive() {
    // (M + 1) * N = M * N + N
    let m = NatPolyForm::from_var(type_param("M"));
    let n = NatPolyForm::from_var(type_param("N"));
    let m_plus_1 = m.add(&NatPolyForm::from_constant(1)).unwrap();
    let result = m_plus_1.mul(&n).unwrap();
    // Evaluate with M=2, N=3 → (2+1)*3 = 9
    let mut bindings = HashMap::new();
    bindings.insert(type_param("M"), 2);
    bindings.insert(type_param("N"), 3);
    assert_eq!(result.evaluate(&bindings), Some(9));
}

#[test]
fn nat_mul_mixed_add() {
    // M * N + 1
    let m = NatPolyForm::from_var(type_param("M"));
    let n = NatPolyForm::from_var(type_param("N"));
    let result = m
        .mul(&n)
        .unwrap()
        .add(&NatPolyForm::from_constant(1))
        .unwrap();
    assert_eq!(result.format(), "M * N + 1");
    let mut bindings = HashMap::new();
    bindings.insert(type_param("M"), 2);
    bindings.insert(type_param("N"), 3);
    assert_eq!(result.evaluate(&bindings), Some(7));
}

#[test]
fn nat_poly_is_constant() {
    let c = NatPolyForm::from_constant(5);
    assert!(c.is_constant());

    let n = NatPolyForm::from_var(type_param("N"));
    assert!(!n.is_constant());

    let mn = NatPolyForm::from_var(type_param("M"))
        .mul(&NatPolyForm::from_var(type_param("N")))
        .unwrap();
    assert!(!mn.is_constant());
}

#[test]
fn nat_poly_leq_with_mul() {
    // M * N <= M * N + 1
    let mn = NatPolyForm::from_var(type_param("M"))
        .mul(&NatPolyForm::from_var(type_param("N")))
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
fn nat_poly_format_zero() {
    let z = NatPolyForm::from_constant(0);
    assert_eq!(z.format(), "0");
}

#[test]
fn rigid_views_keep_bound_defaulted_ports_opaque_and_recompute_derived_dimensions() {
    use crate::dimension::PreludeBaseDimension;
    use crate::ir::static_substitution::StaticSubstitution;
    use crate::syntax::dimension::DimName;

    let owner = crate::dag_id::DagId::root_in_package("test", "lib");
    let dim = |name: &str| ResolvedDimName::for_test(owner.clone(), DimName::expect_valid(name));
    let opaque =
        |identity: &ResolvedDimName| Dimension::base(BaseDimId::UserDefined(identity.clone()));
    let prelude = |base| Dimension::base(BaseDimId::Prelude(base));
    let (q, p, qp, required) = (dim("Q"), dim("P"), dim("QP"), dim("R"));
    let length = prelude(PreludeBaseDimension::Length);
    let mass = prelude(PreludeBaseDimension::Mass);

    // `pub(bind) dim Q = Length; pub(bind) dim P = Mass; dim QP = Q * P;
    //  pub(bind) dim R;`
    let mut statics = crate::ir::module_definitions::StaticDefinitions::new(owner);
    for (identity, default, generic) in [
        (&q, length.clone(), Some(opaque(&q))),
        (&p, mass.clone(), Some(opaque(&p))),
        (
            &qp,
            (&length * &mass).unwrap(),
            Some((&opaque(&q) * &opaque(&p)).unwrap()),
        ),
        (&required, opaque(&required), None),
    ] {
        statics.insert_dimension(identity.clone(), default).unwrap();
        if let Some(generic) = generic {
            statics
                .insert_port_generic_dimension(identity.clone(), generic)
                .unwrap();
        }
    }
    let mut store = ProjectTypeStore::default();
    store
        .insert_module(
            &crate::ir::module_definitions::ModuleDefinitions::try_new(
                statics,
                crate::hir::nominal::NominalTypeRegistry::default(),
            )
            .unwrap(),
        )
        .unwrap();

    // Only defaulted ports a substitution binds are reported.
    let substitution = StaticSubstitution {
        dimensions: [(q.clone(), p.clone()), (required, p.clone())]
            .into_iter()
            .collect(),
        ..StaticSubstitution::default()
    };
    assert_eq!(
        store.bound_defaulted_dimension_ports(&substitution),
        vec![q.clone()]
    );

    let rigid_q = store
        .with_rigid_dimensions(std::slice::from_ref(&q))
        .unwrap();
    assert_eq!(rigid_q.get_dimension(&q), Some(&opaque(&q)));
    assert_eq!(rigid_q.get_dimension(&p), Some(&mass));
    assert_eq!(
        rigid_q.get_dimension(&qp),
        Some(&(&opaque(&q) * &mass).unwrap())
    );
    // Views compose: a view of a rigid view keeps both ports rigid.
    let rigid_both = rigid_q
        .with_rigid_dimensions(std::slice::from_ref(&p))
        .unwrap();
    assert_eq!(
        rigid_both.get_dimension(&qp),
        Some(&(&opaque(&q) * &opaque(&p)).unwrap())
    );
    // The canonical store is untouched.
    assert_eq!(store.get_dimension(&qp), Some(&(&length * &mass).unwrap()));
}

#[test]
fn checked_tir_pairs_each_local_body_with_everything_its_check_published() {
    let source = "param p: Dimensionless = 2.0; node x: Dimensionless = @p * 1.0;";
    let src = crate::source_registry::SourceRegistry::new()
        .register("test.gcl", Arc::new(source.to_string()));
    let tir = check_draft(parse_and_type_resolve_builder(source).unwrap(), src).unwrap();
    let root = tir.root();
    assert!(root.bodies().cover(root.body().owned_expression_roots()));
    assert_eq!(
        root.runtime_schedule().execution_dags(),
        std::slice::from_ref(tir.root_dag_id())
    );
}

#[test]
fn instance_graph_splits_canonical_bodies_from_materialized_instances() {
    use super::instance_graph::{InstanceGraph, TemplateBody};
    use crate::syntax::module_name::{ModuleAliasName, ScopeSegment};

    let mut tir = draft_with_local_children().finish();
    let root_id = tir.root_dag_id().clone();
    let a_id = root_id.inline_dag_child(crate::syntax::decl_name::DeclName::expect_valid("a"));
    let mut graph = InstanceGraph::new(&tir.dags);
    assert!(
        graph
            .template(
                &tir.dags,
                &root_id.instance_child(ScopeSegment::Named(ModuleAliasName::expect_valid("nope")))
            )
            .is_none()
    );
    let (a_position, template) = graph.template(&tir.dags, &a_id).unwrap();
    assert!(matches!(template, TemplateBody::Local(_)));

    // Materialize one instance of `a` under the root.
    let instance_id =
        root_id.instance_child(ScopeSegment::Named(ModuleAliasName::expect_valid("i")));
    let mut instance = tir.dags.at(a_position).clone();
    instance.dag_id = instance_id.clone();
    let specialization = crate::ir::static_substitution::StaticSpecializationId::new(
        a_id.clone(),
        crate::ir::static_substitution::StaticSubstitution::default(),
    );
    let instance_position = graph
        .push_instance(
            &mut tir.dags,
            instance,
            (a_position, template),
            specialization.clone(),
        )
        .unwrap();
    graph.record_edge(super::dag_position::DagPosition::ROOT, instance_position);
    assert_eq!(
        graph.instances_of(super::dag_position::DagPosition::ROOT),
        [instance_position]
    );
    assert!(graph.instances_of(a_position).is_empty());
    // An instance is never a template.
    assert!(graph.template(&tir.dags, &instance_id).is_none());

    let canonical = graph
        .map_canonical(&tir.dags, |position, dag| {
            assert_eq!(tir.dags.position(dag.dag_id()), Some(position));
            Ok::<_, ()>(dag.dag_id().clone())
        })
        .unwrap();
    assert_eq!(canonical.iter().count(), 3);
    assert!(canonical.iter().all(|dag_id| dag_id != &instance_id));
    let instances = graph
        .map_instances(&tir.dags, |position, dag, origin| {
            assert_eq!(position, instance_position);
            assert_eq!(origin.template_position(), a_position);
            assert_eq!(origin.specialization(), &specialization);
            match canonical.template(origin.template()) {
                super::instance_graph::TemplateFact::Local(template) => {
                    assert_eq!(template, &a_id);
                }
                super::instance_graph::TemplateFact::Shared(_) => panic!("local template"),
            }
            Ok::<_, ()>(dag.dag_id().clone())
        })
        .unwrap();
    // The joined table pairs every local body with its own fact.
    let joined = graph.join(canonical, instances);
    assert_eq!(tir.dags.with_local_facts(&joined).count(), 4);
    assert!(
        tir.dags
            .with_local_facts(&joined)
            .all(|(dag, fact)| dag.dag_id() == fact)
    );
}

/// A draft whose root has two other local bodies, `b` added before `a`.
fn draft_with_local_children() -> TirDraft {
    let mut draft = parse_and_type_resolve_builder("node x: Dimensionless = 1.0;").unwrap();
    let root_id = draft.root().dag_id().clone();
    for name in ["b", "a"] {
        let mut child = draft.root().clone();
        child.dag_id =
            root_id.inline_dag_child(crate::syntax::decl_name::DeclName::expect_valid(name));
        draft.insert_dag(child).unwrap();
    }
    draft
}

#[test]
fn draft_positions_follow_insertion_while_visits_follow_identity() {
    let tir = draft_with_local_children().finish();
    let position = |name: &str| {
        let dag_id = tir
            .root_dag_id()
            .inline_dag_child(crate::syntax::decl_name::DeclName::expect_valid(name));
        tir.dags.position(&dag_id).unwrap().index()
    };
    // `b` joined before `a`.
    assert_eq!((position("b"), position("a")), (1, 2));
    assert_eq!(tir.dags.positioned().count(), 3);
    let visited = tir
        .dags
        .local_iter()
        .map(|(dag_id, _)| dag_id.to_string())
        .collect::<Vec<_>>();
    assert_eq!(visited[0], tir.root_dag_id().to_string());
    assert!(visited[1].ends_with('a') && visited[2].ends_with('b'));
    assert!(tir.dags.positioned().all(|(position, dag)| {
        tir.dags.position(dag.dag_id()) == Some(position)
            && std::ptr::eq(tir.dags.at(position), dag)
    }));
}

#[test]
fn imported_slots_keep_their_position_when_localized() {
    let leaf = importer_tir("leaf.gcl", &[])
        .freeze_local_dag_store()
        .unwrap();
    let (leaf_id, _) = leaf.iter().next().unwrap();
    let mut draft =
        parse_and_type_resolve_builder_named("node x: Dimensionless = 1.0;", "root.gcl").unwrap();
    draft.install_shared_dag_stores([&leaf]).unwrap();
    let mut tir = draft.finish();
    let imported = tir.dags.position(leaf_id).unwrap();
    assert_eq!(imported.index(), 1);
    assert!(std::ptr::eq(
        tir.dags.shared_at(imported).unwrap(),
        leaf.get(leaf_id).unwrap()
    ));
    assert!(
        tir.dags
            .shared_at(super::dag_position::DagPosition::ROOT)
            .is_none()
    );
    assert_eq!(tir.dags[leaf_id].dag_id(), leaf_id);
    let facts = tir
        .dags
        .map_local(|_, dag| Ok::<_, ()>(dag.dag_id().clone()))
        .unwrap();
    assert!(tir.dags.local_fact_at(&facts, imported).is_none());
    assert!(tir.dags.local_with_fact(&facts, leaf_id).is_none());
    assert_eq!(
        tir.dags
            .local_fact_at(&facts, super::dag_position::DagPosition::ROOT),
        Some(tir.root_dag_id())
    );

    // Localizing the import copies its body into a local slot at the same
    // position; it is then visited with the local bodies.
    let localized = tir.dags.localized_mut(imported).dag_id().clone();
    assert_eq!(&localized, leaf_id);
    assert_eq!(tir.dags.position(leaf_id), Some(imported));
    assert!(tir.dags.shared_at(imported).is_none());
    assert!(tir.dags.shared(leaf_id).is_none());
    assert_eq!(tir.dags.local_iter().count(), 2);
    assert_eq!(tir.dags.len(), 2);
}

#[test]
fn local_dag_facts_pair_each_local_body_with_the_fact_mapped_from_it() {
    let tir = draft_with_local_children().finish();
    let facts = tir
        .dags
        .map_local(|position, dag| {
            assert_eq!(tir.dags.position(dag.dag_id()), Some(position));
            Ok::<_, ()>(dag.dag_id().clone())
        })
        .unwrap();
    // Visited in identity order, paired by body.
    let visited = tir
        .dags
        .with_local_facts(&facts)
        .map(|(dag, fact)| {
            assert_eq!(dag.dag_id(), fact);
            fact.to_string()
        })
        .collect::<Vec<_>>();
    assert_eq!(visited.len(), 3);
    assert_eq!(visited[0], tir.root_dag_id().to_string());
    assert!(visited[1].ends_with('a') && visited[2].ends_with('b'));
    assert_eq!(
        tir.dags.local_fact(&facts, tir.root_dag_id()),
        Some(tir.root_dag_id())
    );

    // Derived tables keep the alignment, so their facts still pair by body.
    let rendered = facts
        .try_map_ref(|owner| Ok::<_, ()>(owner.to_string()))
        .unwrap();
    let (owners, rendered) = facts.zip(rendered).unzip();
    let lengths = tir
        .dags
        .map_local_facts(rendered.map(|text| text.len()), |dag, length| {
            assert_eq!(length, dag.dag_id().to_string().len());
            length
        });
    let paired = tir
        .dags
        .zip_locals(owners.zip(lengths), |dag, (owner, length)| {
            assert_eq!(dag.dag_id(), &owner);
            assert_eq!(length, owner.to_string().len());
            dag
        });
    assert_eq!(paired.len(), 3);
}

#[test]
fn local_dag_facts_stop_at_the_first_failure_in_visiting_order() {
    let tir = draft_with_local_children().finish();
    let mut visited = Vec::new();
    let failure = tir.dags.map_local(|_, dag| {
        visited.push(dag.dag_id().clone());
        if visited.len() == 2 {
            Err(dag.dag_id().clone())
        } else {
            Ok(())
        }
    });
    let Err(failed) = failure else {
        panic!("expected the second body to fail");
    };
    assert_eq!(visited.len(), 2);
    assert!(failed.to_string().ends_with('a'));
    let facts = tir
        .dags
        .map_local(|_, dag| Ok::<_, ()>(dag.dag_id().clone()))
        .unwrap();
    assert_eq!(
        facts
            .try_map(|owner| if owner == *tir.root_dag_id() {
                Ok(())
            } else {
                Err(owner)
            })
            .map(|_| ()),
        Err(tir
            .root_dag_id()
            .inline_dag_child(crate::syntax::decl_name::DeclName::expect_valid("b")))
    );
}

#[test]
fn module_type_context_carries_its_owners_symbol_table() {
    let source = "type T { T(x: Dimensionless) }\n";
    let raw_file = Parser::new(source).parse_file().unwrap();
    let file = crate::desugar::desugared_ast::File::from(raw_file);
    let src = crate::source_registry::SourceRegistry::new()
        .register("test.gcl", Arc::new(source.to_string()));
    let lowered =
        crate::ir::lower::lower_file_with_inline_dags_for_test(&file, "test.gcl", src).unwrap();
    let types = ProjectTypeStore::default();
    let owner = lowered.root.dag_id().clone();
    let ctx = ModuleTypeContext::new(lowered.root.module(), &lowered.resolver, &types);
    assert_eq!(ctx.owner(), &owner);
    assert!(
        ctx.symbols()
            .struct_types()
            .contains_key(&StructTypeName::expect_valid("T"))
    );
}
