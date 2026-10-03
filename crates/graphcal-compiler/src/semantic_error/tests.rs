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

#[test]
fn structure_payloads_render_their_names_as_before() {
    use super::structure::{FieldlessOperand, StructError, UnknownLocal, UnknownStructTypeName};
    use crate::dag_id::DagId;
    use crate::resolved_name::ResolvedStructTypeName;
    use crate::semantic::checked_type::StructTypeRef;
    use crate::syntax::type_name::StructTypeName;

    let resolved = ResolvedStructTypeName::for_test(
        DagId::root_in_package("pkg", "lib"),
        StructTypeName::expect_valid("Pair"),
    );
    let checked = StructTypeRef::from_resolved(resolved.clone());
    assert_eq!(
        UnknownStructTypeName::Resolved(resolved.clone()).to_string(),
        resolved.to_string()
    );
    assert_eq!(
        UnknownStructTypeName::Checked(checked.clone()).to_string(),
        "Pair"
    );
    assert_eq!(
        StructError::NotAStruct {
            name: FieldlessOperand::RequiredType(checked.clone()),
        }
        .to_string(),
        "cannot access field of non-struct value `required type `Pair` has no fields`"
    );
    assert_eq!(
        FieldlessOperand::Union(checked).to_string(),
        "union type `Pair` (use `match` to access fields)"
    );
    assert_eq!(
        UnknownLocal::Unbound(crate::syntax::names::NameAtom::parse("x").unwrap()).to_string(),
        "x"
    );
}

#[test]
fn visibility_payloads_render_their_mentions_as_before() {
    use super::visibility::{OverrideMention, ReexportedDeclarationKind};
    use crate::syntax::ast::ImportItemNamespace;
    use crate::syntax::index_name::{IndexName, IndexVariantName};
    use crate::syntax::names::{NameAtom, NamePath};
    use crate::syntax::type_name::{ConstructorName, FieldName, StructTypeName};

    let owner = StructTypeName::expect_valid("Record");
    let index = IndexName::expect_valid("Phase");
    let variant = IndexVariantName::expect_valid("Launch");
    let constructor = ConstructorName::expect_valid("Pick");
    let cases = [
        (
            OverrideMention::Field {
                field: FieldName::expect_valid("mass"),
                owner: owner.clone(),
            },
            "field `mass` of type `Record`",
        ),
        (
            OverrideMention::Constructor {
                constructor: constructor.clone(),
                owner: owner.clone(),
            },
            "constructor `Pick` of type `Record`",
        ),
        (OverrideMention::TypeArgument(owner), "type `Record`"),
        (
            OverrideMention::IndexLabel {
                index: index.clone(),
                variant: variant.clone(),
            },
            "index label `Phase#Launch`",
        ),
        (OverrideMention::IndexArgument(index), "index `Phase`"),
        (
            OverrideMention::WrittenLabel {
                index: NamePath::local(NameAtom::parse("Phase").unwrap()),
                variant,
            },
            "`Phase#Launch`",
        ),
        (
            OverrideMention::WrittenConstructor(constructor.clone()),
            "constructor `Pick`",
        ),
        (
            OverrideMention::WrittenConstructorCall(constructor.clone()),
            "constructor `Pick(...)`",
        ),
        (
            OverrideMention::MatchConstructor(constructor),
            "match constructor `Pick`",
        ),
    ];
    for (mention, expected) in cases {
        assert_eq!(mention.to_string(), expected);
    }
    let kinds = [
        (ReexportedDeclarationKind::Param, "param"),
        (ReexportedDeclarationKind::Node, "node"),
        (ReexportedDeclarationKind::ConstNode, "const node"),
        (ReexportedDeclarationKind::Dimension, "dim"),
        (ReexportedDeclarationKind::Unit, "unit"),
        (ReexportedDeclarationKind::Index, "index"),
        (ReexportedDeclarationKind::Type, "type"),
    ];
    for (kind, expected) in kinds {
        assert_eq!(kind.to_string(), expected);
    }
    assert_eq!(
        crate::declaration_kind::DeclarationKind::Index.to_string(),
        "index"
    );
    assert_eq!(ImportItemNamespace::Term.noun(), "term");
    assert_eq!(ImportItemNamespace::Dimension.noun(), "dim");
}

#[test]
fn domain_payloads_render_their_subjects_as_before() {
    use super::domain::{
        DomainSubject, NominalFieldPath, UnconstrainableType, ValuePath, ValuePathStep,
    };
    use crate::syntax::decl_name::DeclName;
    use crate::syntax::index_name::{IndexEntryKey, IndexVariantName};
    use crate::syntax::type_name::{ConstructorName, FieldName, StructTypeName};

    let field = FieldName::expect_valid("mass");
    let root = ValuePath::new(DeclName::expect_valid("sat"));
    let nested = root
        .child(ValuePathStep::Field(field.clone()))
        .child(ValuePathStep::Entry(IndexEntryKey::Named(
            IndexVariantName::expect_valid("Launch"),
        )));
    let cases = [
        (
            DomainSubject::Declaration(DeclName::expect_valid("sat")),
            "sat",
        ),
        (
            DomainSubject::NominalField(Box::new(NominalFieldPath {
                type_name: StructTypeName::expect_valid("Sat"),
                constructor: None,
                field: field.clone(),
            })),
            "Sat.mass",
        ),
        (
            DomainSubject::NominalField(Box::new(NominalFieldPath {
                type_name: StructTypeName::expect_valid("Craft"),
                constructor: Some(ConstructorName::expect_valid("Sat")),
                field: field.clone(),
            })),
            "Craft.Sat.mass",
        ),
        (
            DomainSubject::ConstructorField(Box::new((
                ConstructorName::expect_valid("Sat"),
                field,
            ))),
            "Sat.mass",
        ),
        (DomainSubject::Value(Box::new(root)), "sat"),
    ];
    for (subject, expected) in cases {
        assert_eq!(subject.to_string(), expected);
    }
    assert_eq!(
        DomainSubject::Value(Box::new(nested)).to_string(),
        format!(
            "sat.mass.{}",
            IndexEntryKey::Named(IndexVariantName::expect_valid("Launch"))
        )
    );
    let targets = [
        (UnconstrainableType::Bool, "Bool"),
        (UnconstrainableType::Complex, "Complex"),
        (UnconstrainableType::Key, "Key"),
        (
            UnconstrainableType::Struct(StructTypeName::expect_valid("Sat")),
            "struct `Sat`",
        ),
    ];
    for (target, expected) in targets {
        assert_eq!(target.to_string(), expected);
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "lists every fixed dimension-mismatch rule and expectation"
)]
fn dimension_mismatch_payloads_render_distinct_texts() {
    use super::dimension_mismatch::{
        ExternScalar, FinKeyArithmeticRule, MismatchOperand, MismatchRule, NonQuantityValue,
        OperandExpectation,
    };
    use std::collections::HashSet;

    let rules = [
        MismatchRule::ExpectedQuantity(NonQuantityValue::Complex),
        MismatchRule::ExpectedQuantity(NonQuantityValue::Bool),
        MismatchRule::ExpectedQuantity(NonQuantityValue::Int),
        MismatchRule::ExpectedQuantity(NonQuantityValue::Datetime),
        MismatchRule::ExpectedQuantity(NonQuantityValue::Key),
        MismatchRule::ExpectedQuantity(NonQuantityValue::Struct),
        MismatchRule::ExpectedQuantity(NonQuantityValue::Indexed),
        MismatchRule::ComplexComponentsSameDimension,
        MismatchRule::PolarPhaseAngle,
        MismatchRule::ExpDimensionless,
        MismatchRule::ToFloatInt,
        MismatchRule::ToIntFiniteKeys,
        MismatchRule::ToIntDimensionless,
        MismatchRule::CoordExtractsCoordinate,
        MismatchRule::CoordCoordinateKeysOnly,
        MismatchRule::DatetimeStringLiteral,
        MismatchRule::DatetimeTimezoneLiteral,
        MismatchRule::EpochCivilLiteral,
        MismatchRule::StringLiteralContext,
        MismatchRule::KeyStaticPosition,
        MismatchRule::FinKeyIntPosition,
        MismatchRule::ScanBody,
        MismatchRule::UnfoldBody,
        MismatchRule::FinKeyArithmetic(FinKeyArithmeticRule::NamedOrCoordinateKey),
        MismatchRule::FinKeyArithmetic(FinKeyArithmeticRule::Subtraction),
        MismatchRule::FinKeyArithmetic(FinKeyArithmeticRule::IntegerAddend),
        MismatchRule::FinKeyArithmetic(FinKeyArithmeticRule::RuntimeOffset),
        MismatchRule::FinKeyArithmetic(FinKeyArithmeticRule::NegativeAddend),
        MismatchRule::BooleanOperands,
        MismatchRule::EqualitySameType,
        MismatchRule::ComplexUnordered,
        MismatchRule::ComparisonSameType,
        MismatchRule::DatetimeComparisonScales,
        MismatchRule::ComparisonSameDimension,
        MismatchRule::FinKeyArithmeticKeyFirst,
        MismatchRule::ComplexAdditionSameDimension,
        MismatchRule::NoImplicitComplexPromotion,
        MismatchRule::DatetimeSubtractionScales,
        MismatchRule::DatetimeAddition,
        MismatchRule::DurationAddSubtract,
        MismatchRule::DurationAdd,
        MismatchRule::DatetimeFromQuantity,
        MismatchRule::AdditionSameDimension,
        MismatchRule::ModuloInt,
        MismatchRule::NonNegativeExponent,
        MismatchRule::ExactIntegerExponent,
        MismatchRule::DimensionlessExponent,
        MismatchRule::LogicalNot,
        MismatchRule::Negation,
        MismatchRule::IfConditionBool,
        MismatchRule::IfBranchesSameDimension,
        MismatchRule::MatchArmsSameType,
        MismatchRule::ToleranceSameDimension,
        MismatchRule::AbsoluteToleranceDimension,
    ];
    let texts: HashSet<String> = rules.iter().map(ToString::to_string).collect();
    assert_eq!(texts.len(), rules.len());
    assert!(texts.iter().all(|text| !text.is_empty()));
    assert_eq!(
        MismatchRule::ExpectedQuantity(NonQuantityValue::Key).to_string(),
        "expected a quantity value, not an index-key value"
    );

    let expectations = [
        OperandExpectation::QuantityType,
        OperandExpectation::RankIndexedQuantity { rank: 2 },
        OperandExpectation::IndexedCollection,
        OperandExpectation::IndexedQuantityCollection,
        OperandExpectation::DimensionlessOrInt,
        OperandExpectation::ComplexQuantity,
        OperandExpectation::RealOrComplexQuantity,
        OperandExpectation::Angle,
        OperandExpectation::DimensionlessOrComplexDimensionless,
        OperandExpectation::Int,
        OperandExpectation::FiniteKey,
        OperandExpectation::Dimensionless,
        OperandExpectation::CoordinateKey,
        OperandExpectation::Datetime,
        OperandExpectation::DatetimeLiteral,
        OperandExpectation::TimezoneLiteral,
        OperandExpectation::ScaleFreeDatetimeLiteral,
        OperandExpectation::NumericOrBooleanExpression,
        OperandExpectation::Bool,
        OperandExpectation::ExternIndexedCollection { rank: 2 },
        OperandExpectation::ExternIndexedQuantityCollection { rank: 2 },
        OperandExpectation::ExternScalarAxes {
            scalar: ExternScalar::Bool,
            rank: 2,
        },
        OperandExpectation::ExternScalarAxes {
            scalar: ExternScalar::Int,
            rank: 2,
        },
        OperandExpectation::StaticNatPosition,
        OperandExpectation::StaticNatConstant,
        OperandExpectation::OrderedQuantity,
        OperandExpectation::TimeQuantity,
        OperandExpectation::Time,
        OperandExpectation::NonNegativeIntExponent,
        OperandExpectation::NonNegativeExactIntExponent,
        OperandExpectation::DimensionlessExponent,
        OperandExpectation::IntOrQuantity,
    ];
    let texts: HashSet<String> = expectations.iter().map(ToString::to_string).collect();
    assert_eq!(texts.len(), expectations.len());
    assert_eq!(
        OperandExpectation::ExternScalarAxes {
            scalar: ExternScalar::Int,
            rank: 2,
        }
        .to_string(),
        "Int with exactly 2 indexed axes"
    );
    assert_eq!(MismatchOperand::IntExponent(-2).to_string(), "-2");
    assert_eq!(
        MismatchOperand::ContextualStringLiteral.to_string(),
        "contextual string literal"
    );
    // A found operand that is not a dimension is not labeled as one.
    assert_eq!(
        MismatchOperand::IntExponent(-2).found_label(),
        "exponent is -2"
    );
    assert_eq!(
        MismatchOperand::Expected(OperandExpectation::Int).found_label(),
        "is Int"
    );
}

#[test]
fn dimension_payloads_render_their_text_as_before() {
    use super::dimension::{
        BaseUnitRejection, DimensionError, LinearAlgebraAxisMismatch, PlotPropertyValue,
        ShapeContext,
    };
    use crate::builtin::LinearAlgebraFn;
    use crate::diagnostic::DiagnosticKind;
    use crate::syntax::dimension::UnitName;

    assert_eq!(
        PlotPropertyValue::NotStringLiteral.to_string(),
        "not a string literal"
    );
    assert_eq!(
        PlotPropertyValue::StringLiteral.to_string(),
        "a string literal"
    );
    assert_eq!(
        ShapeContext::ToleranceAssertion.to_string(),
        "tolerance assertion"
    );

    let cross = LinearAlgebraFn::Cross;
    let cardinality = DimensionError::LinearAlgebraShapeMismatch {
        function: cross,
        mismatch: LinearAlgebraAxisMismatch::Cardinality {
            expected: 3,
            found: Some(2),
        },
    };
    assert_eq!(
        cardinality.to_string(),
        "incompatible indexed shape for `cross()`: expected an axis with exactly 3 entries, found an axis with 2 entries"
    );
    assert_eq!(
        cardinality.help().as_deref(),
        Some("cross() is defined only for three-component vectors")
    );
    let unknown = DimensionError::LinearAlgebraShapeMismatch {
        function: cross,
        mismatch: LinearAlgebraAxisMismatch::Cardinality {
            expected: 3,
            found: None,
        },
    };
    assert_eq!(
        unknown.primary_label().as_deref(),
        Some("found an axis whose cardinality is not concrete")
    );
    let generic = DimensionError::LinearAlgebraShapeMismatch {
        function: cross,
        mismatch: LinearAlgebraAxisMismatch::ConcreteCardinalityRequired,
    };
    assert_eq!(
        generic.to_string(),
        "incompatible indexed shape for `cross()`: expected an axis with a concrete cardinality, found an axis whose cardinality is still generic"
    );
    assert!(
        generic
            .help()
            .is_some_and(|help| help.starts_with("cross() needs a concrete matrix size"))
    );

    let taken = BaseUnitRejection::CanonicalUnitTaken {
        existing: UnitName::expect_valid("USD"),
    };
    assert_ne!(taken, BaseUnitRejection::NotBaseDimension);
    assert!(
        DimensionError::FloatPowerExponent { replacement: None }
            .help()
            .is_some_and(|help| help.contains("exact integer"))
    );
    assert_eq!(
        DimensionError::FloatPowerExponent {
            replacement: Some("(1/2)".to_owned())
        }
        .help()
        .as_deref(),
        Some("replace the float exponent with `(1/2)`")
    );
}

#[test]
fn index_payloads_render_their_text_as_before() {
    use super::index::CoordinateArgumentDimensions;
    use crate::dimension::Dimension;
    use crate::semantic::dimension_table::DimensionFormattingRegistry;

    let registry = DimensionFormattingRegistry::new(std::collections::BTreeMap::new(), []);
    let dimensionless = || registry.dimension_spelling(&Dimension::dimensionless());
    let rendered = dimensionless().to_string();
    assert_eq!(
        CoordinateArgumentDimensions::Range {
            start: dimensionless(),
            end: dimensionless(),
            step: dimensionless(),
        }
        .to_string(),
        format!(
            "range start, end, and step have dimensions {rendered}, {rendered}, and {rendered}"
        )
    );
    assert_eq!(
        CoordinateArgumentDimensions::Linspace {
            start: dimensionless(),
            end: dimensionless(),
        }
        .to_string(),
        format!("linspace start and end have dimensions {rendered} and {rendered}")
    );
}

#[test]
fn name_payloads_render_their_text_as_before() {
    use super::name::{DuplicateDeclaration, NameError, PlotPropertyContext};
    use crate::diagnostic::DiagnosticKind;
    use crate::syntax::index_name::{IndexName, IndexVariantName};
    use crate::syntax::names::NameAtom;

    assert_eq!(
        DuplicateDeclaration::Name(NameAtom::parse("speed").unwrap()).to_string(),
        "speed"
    );
    let variant =
        IndexVariantName::expect_valid("Launch").qualified_by(&IndexName::expect_valid("Phase"));
    assert_eq!(
        DuplicateDeclaration::IndexVariant(variant.clone()).to_string(),
        variant.to_string()
    );
    let contexts = [
        (PlotPropertyContext::MarkBlock, "a mark block"),
        (PlotPropertyContext::PlotDeclaration, "a plot declaration"),
        (
            PlotPropertyContext::FigureDeclaration,
            "a figure declaration",
        ),
        (PlotPropertyContext::LayerDeclaration, "a layer declaration"),
    ];
    for (context, expected) in contexts {
        assert_eq!(context.to_string(), expected);
        let error = NameError::InvalidPlotProperty {
            property: crate::syntax::ast::PlotPropertyName::expect_valid("bogus"),
            context,
        };
        assert_eq!(
            error.to_string(),
            format!("property `bogus` is not valid in {expected}")
        );
        assert!(
            error
                .help()
                .is_some_and(|help| help.starts_with("valid properties are: "))
        );
    }
    assert!(
        NameError::InvalidPlotProperty {
            property: crate::syntax::ast::PlotPropertyName::expect_valid("bogus"),
            context: PlotPropertyContext::FigureDeclaration,
        }
        .help()
        .is_some_and(|help| help.ends_with("so sizes belong on the constituent plots or layers"))
    );
}

#[test]
fn attribute_payloads_render_their_text_as_before() {
    use super::attribute::AttributeError;
    use crate::diagnostic::DiagnosticKind;

    assert_eq!(
        AttributeError::NegativeTolerance { value: -0.0 }
            .primary_label()
            .as_deref(),
        Some("tolerance is -0")
    );
    assert_eq!(
        AttributeError::NegativeTolerance { value: -1.5 }
            .primary_label()
            .as_deref(),
        Some("tolerance is -1.5")
    );
    assert_eq!(
        AttributeError::UnknownAttribute {
            name: crate::syntax::token::SourceIdentifier::parse("bogus").unwrap(),
        }
        .to_string(),
        "unknown attribute `bogus`"
    );
}

#[test]
fn plugin_payloads_render_their_text_as_before() {
    use super::plugin::{ExternCallContext, ExternSignatureError};
    use crate::syntax::names::NameAtom;
    use crate::syntax::type_name::{FieldName, StructTypeName};
    use std::collections::HashSet;

    let errors = [
        ExternSignatureError::DuplicateGenericBinder(NameAtom::parse("D").unwrap()),
        ExternSignatureError::DomainConstraint,
        ExternSignatureError::UnsupportedParameterType,
        ExternSignatureError::GenericStructReturn,
        ExternSignatureError::UndeclaredRecordType(StructTypeName::expect_valid("Pair")),
        ExternSignatureError::GenericRecordType(StructTypeName::expect_valid("Pair")),
        ExternSignatureError::NotARecordType(StructTypeName::expect_valid("Pair")),
        ExternSignatureError::UnsupportedStructField(FieldName::expect_valid("x")),
        ExternSignatureError::ArrayAxesMustBeBinders,
        ExternSignatureError::ArrayElementKind,
    ];
    let texts: HashSet<String> = errors.iter().map(ToString::to_string).collect();
    assert_eq!(texts.len(), errors.len());
    assert_eq!(
        ExternSignatureError::DuplicateGenericBinder(NameAtom::parse("D").unwrap()).to_string(),
        "generic binder `D` is declared more than once"
    );
    assert_eq!(
        ExternSignatureError::NotARecordType(StructTypeName::expect_valid("Pair")).to_string(),
        "`Pair` is not a record type; extern struct returns need a single constructor named after the type"
    );
    let contexts = [
        (ExternCallContext::DomainBound, "domain bound"),
        (
            ExternCallContext::UnitScaleExpression,
            "unit scale expression",
        ),
        (ExternCallContext::ConstExpression, "const expression"),
    ];
    for (context, expected) in contexts {
        assert_eq!(context.to_string(), expected);
    }
}

#[test]
fn inexact_int_domain_bound_has_its_own_domain_code() {
    use super::domain::DomainError;
    use crate::diagnostic::DiagnosticKind;

    let error = DomainError::InexactIntDomainBound { value: i64::MAX };
    assert_eq!(error.code(), "graphcal::C008");
    assert_eq!(
        error.to_string(),
        format!(
            "domain bound integer {} is too large for exact quantity comparison",
            i64::MAX
        )
    );
    assert_eq!(error.primary_label().as_deref(), Some("error here"));
    assert_eq!(error.help(), None);
}

#[test]
fn plugin_load_failures_render_their_cause() {
    let module = crate::semantic_error::plugin::PluginLoadFailure::Module {
        reason: "invalid magic number".to_owned(),
    };
    assert_eq!(module.to_string(), "invalid magic number");
    let artifact = crate::semantic_error::plugin::PluginLoadFailure::Artifact(std::sync::Arc::new(
        std::io::Error::other("cannot read `x.wasm`"),
    ));
    assert_eq!(artifact.to_string(), "cannot read `x.wasm`");
}
