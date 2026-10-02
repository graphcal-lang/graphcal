use std::collections::BTreeMap;

/// Every family module's source, scanned for its `code()` arms.
const FAMILY_SOURCES: &[(&str, &str)] = &[
    ("attribute", include_str!("attribute.rs")),
    ("domain", include_str!("domain.rs")),
    ("graph", include_str!("graph.rs")),
    ("structure", include_str!("structure.rs")),
    ("visibility", include_str!("visibility.rs")),
    ("name", include_str!("name.rs")),
    ("index", include_str!("index.rs")),
    ("plugin", include_str!("plugin.rs")),
    ("dimension", include_str!("dimension.rs")),
    ("module", include_str!("module.rs")),
    ("evaluation", include_str!("evaluation.rs")),
    // CATALOG
];

/// Variant name → stable code, read from each family's `code()` match.
fn family_code_catalog() -> BTreeMap<String, String> {
    let mut catalog = BTreeMap::new();
    for (family, source) in FAMILY_SOURCES {
        let Some((body, _)) = source
            .split_once("fn code(&self)")
            .and_then(|(_, rest)| rest.split_once("\n    }\n"))
        else {
            panic!("family `{family}` has no `code()` method");
        };
        for line in body.lines() {
            let Some((arm, code)) = line.trim().split_once(" => \"graphcal::") else {
                continue;
            };
            let variant = arm
                .strip_prefix("Self::")
                .and_then(|rest| rest.split([' ', '(', '{']).next())
                .unwrap_or_else(|| panic!("unexpected code arm in `{family}`: {line}"));
            let code = code.trim_end_matches([',', '"']);
            let previous = catalog.insert(variant.to_owned(), code.to_owned());
            assert!(
                previous.is_none(),
                "variant `{variant}` appears in more than one family"
            );
        }
    }
    catalog
}

#[test]
fn every_family_contributes_codes_with_its_own_prefix() {
    let catalog = family_code_catalog();
    assert!(!catalog.is_empty());
    for (variant, code) in &catalog {
        assert!(
            code.len() == 4 && code[1..].chars().all(|c| c.is_ascii_digit()),
            "malformed code `{code}` for `{variant}`"
        );
    }
}

#[test]
fn diagnostic_codes_are_unique_and_reassignments_are_pinned() {
    let mut catalog = family_code_catalog();
    let internal = crate::internal_error::InternalError::CODE;
    catalog.insert(
        "InternalError".to_owned(),
        internal.trim_start_matches("graphcal::").to_owned(),
    );
    assert!(catalog.len() > 100, "incomplete catalog: {catalog:?}");

    let mut variants_by_code = BTreeMap::new();
    for (variant, code) in &catalog {
        if let Some(previous) = variants_by_code.insert(code, variant) {
            panic!("diagnostic code `{code}` is shared by `{previous}` and `{variant}`");
        }
    }

    for (variant, expected) in [
        ("LinearAlgebraShapeMismatch", "D022"),
        ("AggregationCardinalityUnknown", "D027"),
        ("MaterializedShapeTooLarge", "D035"),
        ("InvalidDatetimeLiteral", "D028"),
        ("EpochTimeScaleArgumentCount", "D023"),
        ("InvalidEpochTimeScaleArgument", "D029"),
        ("UnsupportedEpochTimeScale", "D030"),
        ("ImportRuntimeItem", "M020"),
        ("InternalError", "X001"),
    ] {
        assert_eq!(catalog.get(variant).map(String::as_str), Some(expected));
    }
}

#[test]
fn found_nats_render_their_spelling_and_compare_by_expression() {
    use super::index::FoundNat;
    use crate::syntax::ast::NatExpr;
    use crate::syntax::names::NameAtom;
    use crate::syntax::span::Span;

    let three = FoundNat::Expression(NatExpr::Literal(3, Span::new(0, 1)));
    let moved_three = FoundNat::Expression(NatExpr::Literal(3, Span::new(7, 1)));
    let parameter = FoundNat::Parameter(NameAtom::parse("N").unwrap());
    assert_eq!(three.to_string(), "3");
    assert_eq!(parameter.to_string(), "N");
    assert_eq!(three, moved_three);
    assert_ne!(three, parameter);
}

#[test]
fn map_entry_coordinates_and_type_spellings_render_their_source_form() {
    use super::index::{IndexError, MapEntryCoordinate};
    use crate::nat::NatPolyForm;
    use crate::semantic::checked_type::{CheckedType, IndexDisplayName, Symbolic};

    let position = MapEntryCoordinate::Position {
        axis: IndexDisplayName::Finite(NatPolyForm::from_constant(3)),
        position: 2,
    };
    assert_eq!(position.to_string(), "Fin(3).#2");
    let missing = IndexError::NonExhaustiveMapLiteral {
        missing_count: std::num::NonZeroUsize::new(2).unwrap(),
        witness: vec![position.clone(), position],
    };
    assert_eq!(
        missing.to_string(),
        "non-exhaustive map literal: missing 2 entries; first missing entry is (Fin(3).#2, Fin(3).#2)"
    );

    let registry = crate::display::formatting_registry::FormattingRegistry::new(
        std::collections::BTreeMap::new(),
        Vec::new(),
    );
    let found = CheckedType::<Symbolic>::Bool.spelling(&registry.dimensions);
    assert_eq!(found.to_string(), "Bool");
    assert_eq!(
        IndexError::NonIntegerIndexExpression { found }.to_string(),
        "index expression must be an integer type, got Bool"
    );
}

#[test]
fn unbound_generics_and_symbolic_arguments_render_their_source_form() {
    use super::structure::{GenericSort, StructError, SymbolicGenericArgument, UnboundGeneric};
    use crate::nat::NatPolyForm;
    use crate::semantic::checked_type::IndexDisplayName;
    use crate::syntax::type_name::GenericParamName;

    let name = GenericParamName::expect_valid("N");
    for (sort, expected) in [
        (
            GenericSort::Type,
            "generic type parameter `N` is not concretely bound",
        ),
        (
            GenericSort::Index,
            "generic index parameter `N` is not concretely bound",
        ),
        (
            GenericSort::Dimension,
            "generic dimension parameter `N` is not concretely bound",
        ),
    ] {
        let generic = UnboundGeneric::NotConcretelyBound {
            sort,
            name: name.clone(),
        };
        assert_eq!(
            StructError::UnboundGenericInConcreteType {
                generic: Box::new(generic)
            }
            .to_string(),
            expected
        );
    }
    let form = NatPolyForm::from_constant(4);
    assert_eq!(
        UnboundGeneric::NatArgument(form.clone()).to_string(),
        "generic Nat argument `4` is not concrete"
    );
    assert_eq!(
        UnboundGeneric::NatAxis(form.clone()).to_string(),
        "cannot use generic nat expression `4` as a concrete type"
    );
    assert_eq!(SymbolicGenericArgument::Nat(form.clone()).to_string(), "4");
    assert_eq!(
        SymbolicGenericArgument::Index(IndexDisplayName::Finite(form)).to_string(),
        "Fin(4)"
    );
    assert_eq!(
        StructError::NonConcreteGenericArgument {
            parameter: name,
            argument: Box::new(SymbolicGenericArgument::Type(
                crate::semantic::checked_type::CheckedType::Bool
            )),
        }
        .to_string(),
        "generic argument `Bool` for `N` is not concrete"
    );
}

#[test]
fn module_resolution_keeps_ambiguous_paths_apart_and_cycles_name_their_templates() {
    use super::graph::GraphError;
    use super::module::ModuleError;
    use crate::dag_id::DagId;
    use crate::diagnostic::DiagnosticKind as _;
    use crate::resolve::error::ModuleResolveError;
    use crate::syntax::decl_name::DeclName;

    let first = DagId::root_in_package("test", "lib");
    let second = DagId::root_in_package("test", "main");
    let ambiguous = ModuleError::resolution(ModuleResolveError::AmbiguousModulePath {
        first,
        second: second.clone(),
    });
    assert!(matches!(ambiguous, ModuleError::AmbiguousModulePath { .. }));
    assert_eq!(ambiguous.code(), "graphcal::M034");
    let unknown = ModuleError::resolution(ModuleResolveError::UnknownModule {
        owner: second.clone(),
    });
    assert_eq!(unknown.code(), "graphcal::M033");
    assert_eq!(
        unknown.to_string(),
        ModuleResolveError::UnknownModule {
            owner: second.clone()
        }
        .to_string()
    );

    let inner = second.inline_dag_child(DeclName::expect_valid("inner"));
    let cycle = GraphError::RecursiveDagInstantiation {
        templates: vec![second.clone(), inner, second],
    };
    assert_eq!(
        cycle.to_string(),
        "recursive DAG instantiation: main -> inner -> main"
    );
}

#[test]
fn time_scale_and_unit_scale_diagnostics_keep_their_lowering_text() {
    use super::dimension::{DimensionError, UnitScaleSite};
    use crate::diagnostic::DiagnosticKind as _;

    assert_eq!(UnitScaleSite::Definition.to_string(), "unit scale");
    assert_eq!(UnitScaleSite::Compound.to_string(), "compound unit scale");
    assert_eq!(
        DimensionError::WrongDatetimeArgCount { got: 2 }.to_string(),
        "type `Datetime` expects 0 or 1 type argument(s), got 2"
    );
    assert_eq!(
        DimensionError::ExpectedTimeScale.to_string(),
        "expected a time scale name (e.g., UTC, TAI, TT, TDB, GPST)"
    );
    assert_eq!(DimensionError::ExpectedTimeScale.code(), "graphcal::D040");
}

#[test]
fn graph_payloads_render_their_names_as_before() {
    use super::graph::{CycleMember, DagReference, GraphError};
    use crate::dag_id::DagId;
    use crate::syntax::decl_name::DeclName;

    let dag = DagId::root_in_package("test", "lib");
    assert_eq!(CycleMember::Dag(dag.clone()).to_string(), dag.to_string());
    assert_eq!(
        CycleMember::Declaration(DeclName::expect_valid("a")).to_string(),
        "a"
    );
    assert_eq!(
        GraphError::MissingDagBindings {
            missing: vec![DeclName::expect_valid("x"), DeclName::expect_valid("y")],
            dag_name: DagReference::InlineDag(DeclName::expect_valid("lib")),
        }
        .to_string(),
        "missing required binding(s) [\"x\", \"y\"] when instantiating DAG `lib`"
    );
    assert_eq!(DagReference::Dag(dag.clone()).to_string(), dag.to_string());
    let ident = |name: &str| crate::syntax::ast::Ident {
        name: crate::syntax::token::SourceIdentifier::parse(name).unwrap(),
        span: crate::syntax::span::Span::new(0, 0),
    };
    assert_eq!(
        DagReference::Path(crate::syntax::ast::ModulePath {
            segments: crate::syntax::non_empty::NonEmpty::new(ident("pkg"), vec![ident("lib")]),
            span: crate::syntax::span::Span::new(0, 0),
        })
        .to_string(),
        "pkg.lib"
    );
    assert_eq!(
        GraphError::UnknownDagParam {
            name: DeclName::expect_valid("x"),
            dag_name: dag.clone(),
        }
        .to_string(),
        format!("unknown param `x` in DAG call to `{dag}`")
    );
}
