use super::*;
use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::desugar::desugared_ast::{DeclKind, File, ImportKind};
use graphcal_compiler::resolve::ModuleResolver;

const LIB: &str = "\
pub const node fixed: Dimensionless = 1.0;
param input: Dimensionless = 1.0;
pub node computed: Dimensionless = 2.0;
node hidden: Dimensionless = 3.0;
pub assert check = true;
pub plot chart = { mark: point, encode: { x: 1.0 } };
pub figure fig = { plots: [chart] };
pub type Reading { Reading(value: Dimensionless) }
pub dim Speed = Length / Time;
pub index Axis = { a, b };
pub const unit ft: Length = 0.3048 m;
param rate: Dimensionless = 2.0;
pub unit coin: Length = (@rate) m;
pub dag blueprint { pub node out: Dimensionless = 1.0; }
";

fn parse(source: &str) -> File {
    File::from(
        graphcal_compiler::syntax::parser::Parser::new(source)
            .parse_file()
            .unwrap(),
    )
}

fn lib_id() -> DagId {
    DagId::root_in_package("test", "lib")
}

fn exports() -> Vec<ExportedBinding> {
    let lib = parse(LIB);
    let owner = lib_id();
    ModuleResolver::without_edges([(owner.clone(), lib.declarations.as_slice())])
        .unwrap()
        .exported_bindings(&owner)
        .unwrap()
}

/// The items of `include lib()::{ <items> };`.
fn items(items: &str) -> Vec<ImportItem> {
    let file = parse(&format!("include lib()::{{ {items} }};"));
    let Some(DeclKind::Include(include)) = file.declarations.into_iter().next().map(|d| d.kind)
    else {
        panic!("an include");
    };
    let ImportKind::Selective(items) = include.kind else {
        panic!("a selective include");
    };
    items
}

#[test]
fn every_selectable_item_is_classified_by_its_export() {
    let exports = exports();
    let cases = [
        (
            "fixed",
            Some(SelectedItemKind::Value(SelectedValueKind::Const)),
        ),
        (
            "input",
            Some(SelectedItemKind::Value(SelectedValueKind::Runtime)),
        ),
        (
            "computed",
            Some(SelectedItemKind::Value(SelectedValueKind::Runtime)),
        ),
        ("check", Some(SelectedItemKind::Assertion)),
        ("chart", Some(SelectedItemKind::Plot)),
        ("fig", Some(SelectedItemKind::Composition)),
        ("Reading", Some(SelectedItemKind::Constructor)),
        ("type Reading", Some(SelectedItemKind::Type)),
        ("dim Speed", Some(SelectedItemKind::Static)),
        ("index Axis", Some(SelectedItemKind::Static)),
        (
            "unit ft",
            Some(SelectedItemKind::Unit(UnitConstness::Const)),
        ),
        (
            "unit coin",
            Some(SelectedItemKind::Unit(UnitConstness::Dynamic)),
        ),
        // Not exported, or not an instance member.
        ("hidden", None),
        ("blueprint", None),
        // Exported, but in another namespace.
        ("Speed", None),
        ("type fixed", None),
    ];
    for (item, expected) in cases {
        let [item_ast] = items(item).try_into().expect("one item");
        assert_eq!(
            SelectedItemKind::classify(&exports, &lib_id(), &item_ast),
            expected,
            "{item}"
        );
    }
}

#[test]
fn exported_values_are_the_graph_values_of_the_surface() {
    let exports = exports();
    let lib = lib_id();
    let mut values = exported_values(&exports, &lib)
        .map(|(name, kind)| (name.to_string(), kind))
        .collect::<Vec<_>>();
    values.sort_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(
        values,
        [
            ("computed".to_owned(), SelectedValueKind::Runtime),
            ("fixed".to_owned(), SelectedValueKind::Const),
            ("input".to_owned(), SelectedValueKind::Runtime),
            ("rate".to_owned(), SelectedValueKind::Runtime),
        ]
    );
}
