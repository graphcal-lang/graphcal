use std::path::{Path, PathBuf};

use graphcal_io::RealFileSystem;

use super::*;
use crate::loader::load_project;
use crate::project_compiler::ProjectCompiler;

/// The sample multi-file project the web view's fixtures are built from.
fn power_project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../web/view/fixtures/power_project")
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
    let mut projection = project_view(&project, &checked, Some(root)).unwrap();
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

fn node<'a>(view: &'a ViewModel, id: &str) -> &'a ViewNode {
    view.nodes
        .iter()
        .find(|node| node.id == id)
        .unwrap_or_else(|| panic!("no node `{id}`"))
}

#[test]
fn power_max_structure_and_values() {
    let root = power_project_root();
    let view = view_of(&root, "src/power_project/power_max.gcl", true);
    eprintln!("{}", serde_json::to_string_pretty(&view).unwrap());

    assert_eq!(view.module.id, "power_project.power_max");
    assert_eq!(view.module.file, "src/power_project/power_max.gcl");
    assert_eq!(view.module.package, "power_project");
    // Source order: the include alias sits where the `include` is written.
    let order: Vec<&str> = view.nodes.iter().map(|node| node.id.as_str()).collect();
    assert_eq!(&order[..4], ["margin", "p_budget", "eta_dcdc", "p_unit"]);

    let indexes: Vec<(&str, Vec<&str>)> = view
        .indexes
        .iter()
        .map(|index| {
            (
                index.name.as_str(),
                index.labels.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    assert_eq!(
        indexes,
        vec![
            ("Component", vec!["Obc", "Radio", "Heater", "Payload"]),
            ("Mode", vec!["Safe", "Nominal", "Science"]),
        ]
    );

    let duty = node(&view, "duty");
    assert_eq!(duty.kind, ViewNodeKind::Param);
    assert_eq!(duty.axes, ["Component", "Mode"]);
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
    assert_eq!(source.line, 14);

    let p_required = node(&view, "p_required");
    assert_eq!(p_required.kind, ViewNodeKind::Node);
    assert_eq!(p_required.instance, None);
    assert!(p_required.is_public);
    assert_eq!(p_required.deps, ["margin", "p_total"]);
    assert_eq!(
        p_required.expr.as_deref(),
        Some("@p_total * (1.0 + @margin)")
    );
    assert_eq!(p_required.unit.as_deref(), Some("W"));
    let Some(ViewValue::Quantity { unit, .. }) = &p_required.value else {
        panic!("p_required has no quantity value");
    };
    assert_eq!(unit.as_deref(), Some("W"));

    let p_mean = node(&view, "p_mean");
    assert_eq!(p_mean.deps, ["num", "p_total"]);
    let num = node(&view, "num");
    assert_eq!(num.kind, ViewNodeKind::External);
    assert_eq!(num.module, "power_project.components");
    assert!(num.is_public);
    assert_eq!(num.instance, None);
    assert!(matches!(num.value, Some(ViewValue::Int { .. })));

    let budget = node(&view, "p_budget");
    assert_eq!(budget.kind, ViewNodeKind::External);
    assert_eq!(budget.module, "power_project.eps");
    assert_eq!(budget.deps, Vec::<String>::new());
    assert_eq!(budget.instance.as_deref(), Some("eps"));
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
    assert_eq!(within_budget.deps, ["p_budget", "p_required"]);
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
fn structure_only_without_evaluation() {
    let root = power_project_root();
    let view = view_of(&root, "src/power_project/power_max.gcl", false);
    assert!(view.nodes.iter().all(|node| node.value.is_none()));
    assert!(!node(&view, "margin").is_public);
    assert_eq!(node(&view, "p_total").unit.as_deref(), Some("W"));
}

#[test]
fn single_file_project_projects() {
    let mut fs = graphcal_io::InMemoryFileSystem::new();
    fs.add_file(
        graphcal_io::VirtualAbsolutePath::new("/proj/view.gcl").unwrap(),
        "node thrust: Dimensionless = 1.0;\nnode doubled: Dimensionless = @thrust * 2.0;\n"
            .to_string(),
    )
    .unwrap();
    let project = load_project(Path::new("/proj/view.gcl"), Some(Path::new("/proj")), &fs).unwrap();
    let host_fns = graphcal_eval::host_fns::demo_registry();
    let checked = ProjectCompiler::new(&project)
        .host_fns(&host_fns)
        .check()
        .unwrap();
    let view = project_view(&project, &checked, None).unwrap().into_model();
    assert_eq!(view.module.id, "view");
    assert_eq!(view.nodes.len(), 2);
    assert_eq!(node(&view, "doubled").deps, ["thrust"]);
}
