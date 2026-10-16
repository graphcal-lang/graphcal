use std::path::{Path, PathBuf};

use graphcal_io::RealFileSystem;

use super::*;
use crate::loader::load_project;
use crate::project_compiler::ProjectCompiler;

/// A sample multi-file project: a power budget that includes an EPS sheet and imports an index.
fn power_project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/view_model/power_project")
        .canonicalize()
        .unwrap()
}

fn view_of(root: &Path, file: &str, evaluate: bool) -> ViewModel {
    let fs = RealFileSystem::rooted(root).unwrap();
    let entry = root.join(file);
    let project = load_project(&entry, Some(root), &fs).unwrap();
    let host_fns = graphcal_eval::host_fns::demo_registry();
    let checked = ProjectCompiler::new(&project)
        .host_fns(&host_fns)
        .check()
        .unwrap();
    let mut projection = project_view(&project, &checked).unwrap();
    if !evaluate {
        return projection.into_model();
    }
    let prepared = checked.prepare_with_host_fns(&host_fns).unwrap();
    let evaluated = prepared
        .evaluate(&prepared.binding_builder().finish().unwrap())
        .unwrap();
    projection.attach_evaluation(&evaluated).unwrap();
    projection.into_model()
}

/// The node the module spells `label`.
fn node<'a>(view: &'a ViewModel, label: &str) -> &'a ViewNode {
    view.nodes
        .iter()
        .find(|node| node.label == label)
        .unwrap_or_else(|| panic!("no node `{label}`"))
}

fn id_of(view: &ViewModel, label: &str) -> ViewNodeId {
    node(view, label).id.clone()
}

fn labels(view: &ViewModel) -> Vec<&str> {
    view.nodes.iter().map(|node| node.label.as_str()).collect()
}

fn index<'a>(view: &'a ViewModel, id: &ViewIndexId) -> &'a ViewIndex {
    view.indexes
        .iter()
        .find(|index| index.id == *id)
        .unwrap_or_else(|| panic!("no index `{id}`"))
}

#[test]
fn power_max_structure_and_values() {
    let root = power_project_root();
    let view = view_of(&root, "src/power_project/power_max.gcl", true);

    assert_eq!(view.module.id, "power_project.power_max");
    assert_eq!(view.module.file, "src/power_project/power_max.gcl");
    assert_eq!(
        node(&view, "margin").doc.as_deref(),
        Some("Design margin on the required power.\nApplied on top of the summed bus power.")
    );
    assert_eq!(node(&view, "eta_dcdc").doc, None);
    assert_eq!(view.module.package, "power_project");
    // Source order: the include alias sits where the `include` is written.
    assert_eq!(
        &labels(&view)[..4],
        ["margin", "p_budget", "eta_dcdc", "p_unit"]
    );

    let indexes: Vec<(&str, Option<&str>, Vec<&str>)> = view
        .indexes
        .iter()
        .map(|index| {
            (
                index.name.as_str(),
                index.module.as_deref(),
                index
                    .entries
                    .iter()
                    .map(|entry| entry.display.as_str())
                    .collect(),
            )
        })
        .collect();
    assert_eq!(
        indexes,
        vec![
            (
                "Component",
                Some("power_project.components"),
                vec!["Obc", "Radio", "Heater", "Payload"]
            ),
            (
                "Mode",
                Some("power_project.components"),
                vec!["Safe", "Nominal", "Science"]
            ),
        ]
    );

    let duty = node(&view, "duty");
    assert_eq!(duty.kind, ViewNodeKind::Param);
    assert_eq!(duty.origin, ViewOrigin::Local);
    let axis_names: Vec<&str> = duty
        .axes
        .iter()
        .map(|axis| index(&view, axis).name.as_str())
        .collect();
    assert_eq!(axis_names, ["Component", "Mode"]);
    assert_eq!(duty.dimension.as_deref(), Some("Dimensionless"));
    assert_eq!(duty.unit, None);
    assert!(
        duty.expr
            .as_deref()
            .unwrap()
            .starts_with("table[Component, Mode] {")
    );
    let source = duty.source.as_ref().unwrap();
    assert_eq!(
        source.file.as_deref(),
        Some("src/power_project/power_max.gcl")
    );
    // The two `///` lines above `margin` push the table down by two.
    assert_eq!(source.line, 16);

    let p_required = node(&view, "p_required");
    assert_eq!(p_required.kind, ViewNodeKind::Node);
    assert_eq!(p_required.origin, ViewOrigin::Local);
    assert!(p_required.is_public);
    assert_eq!(
        p_required.deps,
        [id_of(&view, "margin"), id_of(&view, "p_total")]
    );
    assert_eq!(
        p_required.expr.as_deref(),
        Some("@p_total * (1.0 + @margin)")
    );
    assert_eq!(p_required.unit.as_deref(), Some("W"));
    let Some(ViewValue::Quantity { unit, .. }) = &p_required.value else {
        panic!("p_required has no quantity value");
    };
    assert_eq!(unit.as_deref(), Some("W"));
}

#[test]
fn power_max_reads_other_modules() {
    let root = power_project_root();
    let view = view_of(&root, "src/power_project/power_max.gcl", true);
    let p_mean = node(&view, "p_mean");
    assert_eq!(p_mean.deps, [id_of(&view, "num"), id_of(&view, "p_total")]);
    let num = node(&view, "num");
    assert_eq!(num.kind, ViewNodeKind::Const);
    assert_eq!(num.origin, ViewOrigin::Imported);
    assert_eq!(num.module, "power_project.components");
    assert!(num.is_public);
    assert!(matches!(num.value, Some(ViewValue::Int { .. })));

    let budget = node(&view, "p_budget");
    assert_eq!(budget.kind, ViewNodeKind::Node);
    assert_eq!(budget.module, "power_project.eps");
    assert_eq!(budget.deps, Vec::<ViewNodeId>::new());
    // `include power_project.eps()` binds nothing.
    assert_eq!(
        budget.origin,
        ViewOrigin::Included {
            instance: ViewInstance {
                name: "eps".to_string(),
                bound: false
            }
        }
    );
    assert_eq!(budget.source.as_ref().unwrap().line, 23);
    assert!(
        budget
            .expr
            .as_deref()
            .unwrap()
            .starts_with("if @p_sun_avail")
    );
    assert_eq!(
        budget.source.as_ref().unwrap().file.as_deref(),
        Some("src/power_project/eps.gcl")
    );
    assert!(matches!(budget.value, Some(ViewValue::Quantity { .. })));

    let within_budget = node(&view, "within_budget");
    assert_eq!(within_budget.kind, ViewNodeKind::Assert);
    assert_eq!(within_budget.dimension.as_deref(), Some("Bool"));
    assert_eq!(
        within_budget.deps,
        [id_of(&view, "p_budget"), id_of(&view, "p_required")]
    );
    assert_eq!(
        within_budget.expr.as_deref(),
        Some("@p_required <= @p_budget")
    );
    assert!(matches!(
        within_budget.value,
        Some(ViewValue::Bool { value: true, .. })
    ));
}

#[test]
fn indexed_values_are_keyed_by_their_axis_entries() {
    let root = power_project_root();
    let view = view_of(&root, "src/power_project/power_max.gcl", true);
    let p_unit = node(&view, "p_unit");
    let axis = index(&view, &p_unit.axes[0]);
    let Some(ViewValue::Indexed { entries, .. }) = &p_unit.value else {
        panic!("p_unit is not indexed: {:?}", p_unit.value);
    };
    let keys: Vec<&str> = entries.iter().map(|entry| entry.key.as_str()).collect();
    let axis_keys: Vec<&str> = axis
        .entries
        .iter()
        .map(|entry| entry.key.as_str())
        .collect();
    assert_eq!(keys, axis_keys);
}

#[test]
fn structure_only_without_evaluation() {
    let root = power_project_root();
    let view = view_of(&root, "src/power_project/power_max.gcl", false);
    assert!(view.nodes.iter().all(|node| node.value.is_none()));
    assert!(!node(&view, "margin").is_public);
    assert_eq!(node(&view, "p_total").unit.as_deref(), Some("W"));
}

#[test]
fn a_module_opened_alone_keeps_the_id_its_readers_use() {
    let root = power_project_root();
    let main = view_of(&root, "src/power_project/power_max.gcl", false);
    let eps = view_of(&root, "src/power_project/eps.gcl", false);
    assert_eq!(eps.module.id, node(&main, "p_budget").module);
    assert_eq!(eps.module.id, "power_project.eps");
}

fn single_file_project(source: &str) -> (crate::loader::LoadedProject, CheckedProjectFixture) {
    let mut fs = graphcal_io::InMemoryFileSystem::new();
    fs.add_file(
        graphcal_io::VirtualAbsolutePath::new("/proj/view.gcl").unwrap(),
        source.to_string(),
    )
    .unwrap();
    let project = load_project(Path::new("/proj/view.gcl"), Some(Path::new("/proj")), &fs).unwrap();
    let host_fns = graphcal_eval::host_fns::demo_registry();
    let checked = ProjectCompiler::new(&project)
        .host_fns(&host_fns)
        .check()
        .unwrap();
    (project, CheckedProjectFixture { checked, host_fns })
}

struct CheckedProjectFixture {
    checked: crate::project_compiler::CheckedProject,
    host_fns: graphcal_eval::host_fns::HostFunctionRegistry,
}

#[test]
fn single_file_project_projects() {
    let (project, fixture) = single_file_project(
        "node thrust: Dimensionless = 1.0;\nnode doubled: Dimensionless = @thrust * 2.0;\n",
    );
    let view = project_view(&project, &fixture.checked)
        .unwrap()
        .into_model();
    assert_eq!(view.module.id, "view");
    assert_eq!(view.module.file, "view.gcl");
    assert_eq!(view.nodes.len(), 2);
    assert_eq!(node(&view, "doubled").deps, [id_of(&view, "thrust")]);
}

/// The evaluated view of a one-file project.
fn evaluated_view_of_source(source: &str) -> ViewModel {
    let (project, fixture) = single_file_project(source);
    let mut projection = project_view(&project, &fixture.checked).unwrap();
    let prepared = fixture
        .checked
        .prepare_with_host_fns(&fixture.host_fns)
        .unwrap();
    let evaluated = prepared
        .evaluate(&prepared.binding_builder().finish().unwrap())
        .unwrap();
    projection.attach_evaluation(&evaluated).unwrap();
    projection.into_model()
}

fn display_of(view: &ViewModel, label: &str) -> Option<String> {
    match &node(view, label).value {
        Some(ViewValue::Quantity { display, .. }) => Some(display.clone()),
        _ => None,
    }
}

#[test]
fn inline_dag_templates_stay_out_of_the_sheet() {
    // Both templates declare `x` and `out`; only their instances are read.
    let view = evaluated_view_of_source(
        "\
dag one {
    param x: Dimensionless = 1.0;
    pub node out: Dimensionless = @x;
}
dag two {
    param x: Dimensionless = 2.0;
    pub node out: Dimensionless = @x;
}
include one() as a;
include two() as b;
node y: Dimensionless = @a::out + @b::out;
",
    );
    let mut sorted = labels(&view);
    sorted.sort_unstable();
    assert_eq!(sorted, ["a::out", "b::out", "y"]);
    assert_eq!(
        node(&view, "y").deps,
        [id_of(&view, "a::out"), id_of(&view, "b::out")]
    );
    assert_eq!(node(&view, "a::out").kind, ViewNodeKind::Node);
    assert!(matches!(
        node(&view, "a::out").origin,
        ViewOrigin::Included { .. }
    ));
    assert_eq!(node(&view, "a::out").module, "view.one");
    assert_eq!(display_of(&view, "a::out").as_deref(), Some("1"));
    assert_eq!(display_of(&view, "b::out").as_deref(), Some("2"));
    assert_eq!(display_of(&view, "y").as_deref(), Some("3"));
}

#[test]
fn a_root_declaration_named_like_a_template_member_keeps_its_own_node() {
    let view = evaluated_view_of_source(
        "\
dag checked {
    param v: Dimensionless;
    pub node out: Dimensionless = @v * 2.0;
}
node out: Dimensionless = @checked(v: 3.0)::out;
include checked(v: 4.0) as also_fine;
node z: Dimensionless = @out + @also_fine::out;
",
    );
    assert_eq!(labels(&view), ["out", "z", "also_fine::out"]);
    assert_eq!(
        node(&view, "z").deps,
        [id_of(&view, "out"), id_of(&view, "also_fine::out")]
    );
    assert_ne!(id_of(&view, "out"), id_of(&view, "also_fine::out"));
    assert_eq!(display_of(&view, "out").as_deref(), Some("6"));
    assert_eq!(display_of(&view, "also_fine::out").as_deref(), Some("8"));
}

#[test]
fn a_coordinate_axis_lists_its_entries_under_the_keys_of_its_values() {
    // The reproduction of #2060: every cell over a coordinate index showed
    // `—` because the axis and the values named their entries differently.
    let view = evaluated_view_of_source(
        "\
index TimeStep = range(0.0 s, 1.0 s, step: 0.5 s);
param x0: Dimensionless = 1.0;
node x: Dimensionless[TimeStep] = for t: TimeStep { @x0 * 2.0 };
",
    );
    let x = node(&view, "x");
    let axis = index(&view, &x.axes[0]);
    assert_eq!(axis.name, "TimeStep");
    let displays: Vec<&str> = axis.entries.iter().map(|e| e.display.as_str()).collect();
    assert_eq!(displays, ["0 s", "0.5 s", "1 s"]);
    let Some(ViewValue::Indexed { entries, .. }) = &x.value else {
        panic!("x is not indexed: {:?}", x.value);
    };
    let keys: Vec<&str> = entries.iter().map(|entry| entry.key.as_str()).collect();
    let axis_keys: Vec<&str> = axis
        .entries
        .iter()
        .map(|entry| entry.key.as_str())
        .collect();
    assert_eq!(keys, axis_keys);
}

#[test]
fn a_fin_axis_is_listed_with_its_positions() {
    let view = evaluated_view_of_source("node v: Dimensionless[Fin(3)] = for i: Fin(3) { 1.0 };\n");
    let v = node(&view, "v");
    let axis = index(&view, &v.axes[0]);
    assert_eq!(axis.name, "Fin(3)");
    assert_eq!(axis.module, None);
    assert_eq!(axis.entries.len(), 3);
    let Some(ViewValue::Indexed { entries, .. }) = &v.value else {
        panic!("v is not indexed: {:?}", v.value);
    };
    let keys: Vec<&str> = entries.iter().map(|entry| entry.key.as_str()).collect();
    let axis_keys: Vec<&str> = axis
        .entries
        .iter()
        .map(|entry| entry.key.as_str())
        .collect();
    assert_eq!(keys, axis_keys);
}

fn bare_node(id: &ViewNodeId, deps: &[&ViewNodeId]) -> ViewNode {
    ViewNode {
        id: id.clone(),
        label: id.to_string(),
        name: id.to_string(),
        module: "m".to_string(),
        kind: ViewNodeKind::Node,
        origin: ViewOrigin::Local,
        axes: Vec::new(),
        dimension: None,
        unit: None,
        is_public: false,
        deps: deps.iter().map(|dep| (*dep).clone()).collect(),
        expr: None,
        reduce: None,
        doc: None,
        source: None,
        value: None,
    }
}

#[test]
fn node_references_reject_a_shared_id_and_a_dangling_dependency() {
    use super::project::check_node_references;

    let view =
        evaluated_view_of_source("node a: Dimensionless = 1.0;\nnode b: Dimensionless = @a;\n");
    let (a, b) = (id_of(&view, "a"), id_of(&view, "b"));
    assert!(check_node_references(&[bare_node(&a, &[]), bare_node(&b, &[&a])]).is_ok());
    assert!(matches!(
        check_node_references(&[bare_node(&a, &[]), bare_node(&a, &[])]),
        Err(ViewProjectionError::DuplicateNodeId { id }) if id == a
    ));
    assert!(matches!(
        check_node_references(&[bare_node(&b, &[&a])]),
        Err(ViewProjectionError::UnknownDependency { node, dependency })
            if node == b && dependency == a
    ));
}

#[test]
fn an_include_with_arguments_is_marked_bound() {
    let view = evaluated_view_of_source(
        "\
dag lib {
    param gain: Dimensionless = 1.0;
    pub node out: Dimensionless = @gain;
}
include lib(gain: 2.0) as p;
include lib() as q;
node y: Dimensionless = @p::out + @q::out;
",
    );
    let instance = |label: &str| match &node(&view, label).origin {
        ViewOrigin::Included { instance } => instance.clone(),
        other => panic!("`{label}` is no include member: {other:?}"),
    };
    assert_eq!(instance("p::out").name, "p");
    assert!(instance("p::out").bound);
    assert_eq!(instance("q::out").name, "q");
    assert!(!instance("q::out").bound);
    assert_eq!(node(&view, "y").origin, ViewOrigin::Local);
    assert_eq!(display_of(&view, "p::out").as_deref(), Some("2"));
}

/// The text the view would replace for `label`: its `expr`, and the source
/// sliced by its `expr_range` (lines and UTF-16 columns).
fn expr_and_range_text(view: &ViewModel, source: &str, label: &str) -> (String, String) {
    let node = node(view, label);
    let range = node
        .source
        .as_ref()
        .and_then(|source| source.expr_range)
        .unwrap_or_else(|| panic!("`{label}` has no expression range"));
    let offset = |position: ViewPosition| -> usize {
        let line_start: usize = source
            .split_inclusive('\n')
            .take(position.line as usize)
            .map(str::len)
            .sum();
        let line = &source[line_start..];
        let mut units: usize = 0;
        let mut bytes: usize = 0;
        for ch in line.chars() {
            if units >= position.character as usize {
                break;
            }
            units = units.saturating_add(ch.len_utf16());
            bytes = bytes.saturating_add(ch.len_utf8());
        }
        line_start.saturating_add(bytes)
    };
    let text = source[offset(range.start)..offset(range.end)].to_string();
    (node.expr.clone().unwrap(), text)
}

#[test]
fn the_expression_range_covers_the_whole_text_after_the_equals_sign() {
    // The reproduction of #2062: a range that starts after an opening `(`
    // turned an edit of `z` into `(5.0;`.
    let source = "\
node a: Dimensionless = 1.0;
node y: Dimensionless = (@a + 2.0) * 3.0;
node z: Dimensionless = (@a);
node nested: Dimensionless = ((@a) + (2.0));
param p: Dimensionless = (1.5);
const node c: Dimensionless = (2.5);
node multi: Dimensionless = (@a
    // a comment inside
    + 1.0) // a comment after
    ;
assert ok = (@y) > 0.0;
";
    let view = evaluated_view_of_source(source);
    for (label, expected) in [
        ("y", "(@a + 2.0) * 3.0"),
        ("z", "(@a)"),
        ("nested", "((@a) + (2.0))"),
        ("p", "(1.5)"),
        ("c", "(2.5)"),
        ("multi", "(@a\n    // a comment inside\n    + 1.0)"),
        ("ok", "(@y) > 0.0"),
    ] {
        let (expr, text) = expr_and_range_text(&view, source, label);
        assert_eq!(expr, expected, "expr of `{label}`");
        assert_eq!(text, expected, "range of `{label}`");
    }
}

#[test]
fn the_expression_range_counts_utf16_columns() {
    // `π` is one UTF-16 unit and two bytes; `𝑥` is two units and four bytes.
    let source = "\
/// π and 𝑥 before the declaration do not shift it.
node a: Dimensionless = 1.0;
node b: Dimensionless = (@a) * 2.0; // π𝑥
";
    let view = evaluated_view_of_source(source);
    let (expr, text) = expr_and_range_text(&view, source, "b");
    assert_eq!(expr, "(@a) * 2.0");
    assert_eq!(text, expr);
}
