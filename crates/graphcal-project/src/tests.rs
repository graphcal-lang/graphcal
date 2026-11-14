mod checked_expressions;
mod dag_calls;
mod declaration_rules;
mod definition_site_scope;
mod derived_dimensions;
mod domain_constraints;
mod evaluation;
mod exec_plan;
mod generics;
mod include_bindings;
mod index_injection;
mod inline_dags;
mod instances;
mod instantiated_imports;
mod model_sessions;
mod overrides;
mod plans;
mod prepared_bindings;
mod presentation_evidence;
mod same_leaf_owners;
mod sealed_program;
mod type_store;
mod work_bounds;

use crate::binding_error::BindingError;
use graphcal_eval::eval::EvalEntry;

use std::collections::HashSet;

use std::collections::HashMap;

use crate::compile_error::CompileError;
use crate::prepare::*;
use crate::project_compiler::{ProjectCompiler, compile_to_tir, compile_to_tir_from_project};
use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::semantic_error::SemanticErrorKind;
use graphcal_compiler::semantic_error::attribute::AttributeError;
use graphcal_compiler::semantic_error::dimension::DimensionError;
use graphcal_compiler::semantic_error::domain::DomainError;
use graphcal_compiler::semantic_error::graph::GraphError;
use graphcal_compiler::semantic_error::index::IndexError;
use graphcal_compiler::semantic_error::module::ModuleError;
use graphcal_compiler::semantic_error::name::NameError;
use graphcal_compiler::semantic_error::rendered::RenderedSemanticError;
use graphcal_compiler::semantic_error::structure::StructError;
use graphcal_compiler::semantic_error::visibility::VisibilityError;
use graphcal_compiler::syntax::attribute::AttributeName;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::index_name::IndexVariantName;
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_eval::eval::{AssertResult, EvalResult, NodeUnavailable, UnitLabel, Value};
use graphcal_io::RealFileSystem;

/// Test-only convenience projection for a loaded multi-file project.
pub fn compile_to_tir_project<F: graphcal_io::FileSystemReader>(
    root_path: &std::path::Path,
    project_root: Option<&std::path::Path>,
    fs: &F,
) -> Result<
    (
        graphcal_compiler::tir::typed::CheckedTir,
        crate::loader::LoadedProject,
    ),
    CompileError,
> {
    let project = crate::loader::load_project(root_path, project_root, fs)?;
    let tir = compile_to_tir_from_project(&project)?;
    Ok((tir, project))
}

fn fs() -> RealFileSystem {
    RealFileSystem::default()
}

fn scoped_name(name: &str) -> ScopedName {
    ScopedName::local(graphcal_compiler::syntax::decl_name::DeclName::expect_valid(name))
}

/// The checked type of the root declaration written as `name`.
fn root_decl_type<'a>(
    tir: &'a graphcal_compiler::tir::typed::CheckedTir,
    name: &str,
) -> &'a graphcal_compiler::tir::typed::CheckedDeclType {
    let identity = tir
        .root()
        .body_for_test()
        .bound_decl_identity(&scoped_name(name))
        .unwrap_or_else(|| panic!("`{name}` is not declared in the root"));
    tir.decl_type(identity)
        .unwrap_or_else(|| panic!("`{name}` has no checked type"))
}

fn member_name(owner: &[&str], leaf: &str) -> ScopedName {
    let owner = owner
        .iter()
        .map(|segment| {
            graphcal_compiler::syntax::module_name::ModuleAliasName::expect_valid(*segment).into()
        })
        .collect();
    ScopedName::qualified(
        graphcal_compiler::syntax::non_empty::NonEmpty::try_from_vec(owner).unwrap(),
        graphcal_compiler::syntax::decl_name::DeclName::expect_valid(leaf),
    )
}

/// Find the SI value of a named quantity declaration.
fn find_value(result: &EvalResult, name: &str) -> f64 {
    // Check consts first
    if let Some((_, val)) = result.consts().find(|(n, _)| n.to_string() == name) {
        return val.as_ref().unwrap().si_value().unwrap().get();
    }
    // Check params and nodes (wrapped in Result)
    result
        .params()
        .chain(result.nodes())
        .find(|(n, _)| n.to_string() == name)
        .unwrap_or_else(|| panic!("value `{name}` not found"))
        .1
        .as_ref()
        .unwrap_or_else(|e| panic!("value `{name}` has error: {e}"))
        .si_value()
        .unwrap()
        .get()
}

fn assert_quantity_value(result: &EvalResult, name: &str, expected: f64) {
    let actual = find_value(result, name);
    assert!(
        (actual - expected).abs() < f64::EPSILON,
        "expected `{name}` to equal {expected}, got {actual}"
    );
}

/// Write a small package-shaped project for pipeline regression tests.
fn write_pipeline_project(
    files: &[(&str, &str)],
    root: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let source_root = directory.path().join("src/pipeline");
    std::fs::create_dir_all(&source_root).unwrap();
    std::fs::write(
        directory.path().join("graphcal.toml"),
        "[package]\nname = \"pipeline\"\n",
    )
    .unwrap();
    for (name, source) in files {
        std::fs::write(source_root.join(name), source).unwrap();
    }
    (directory, source_root.join(root))
}

/// One node of a checked tree: its span, and its concrete type when the tree
/// is executable and the node is a value.
type CheckedNodeSummary = (
    graphcal_compiler::syntax::span::Span,
    Option<graphcal_compiler::semantic::checked_type::CheckedType>,
    bool,
);

/// Every node of every checked tree of `dag`, by occurrence.
fn checked_nodes(
    dag: &graphcal_compiler::tir::typed::CheckedDag,
) -> HashMap<graphcal_compiler::expression_id::ExprId, CheckedNodeSummary> {
    use graphcal_compiler::tir::texpr::{CheckedBody, TNodeRef, visit_tnodes};
    let mut nodes = HashMap::new();
    for (_, body) in dag.bodies_for_test().roots() {
        match body {
            CheckedBody::Executable(body) => visit_tnodes(body.as_node(), &mut |node| {
                let summary = match node {
                    TNodeRef::Value(expr) => (expr.span(), Some(expr.ty().clone()), false),
                    TNodeRef::Contextual(literal) => (literal.span(), None, true),
                };
                nodes.insert(node.id().clone(), summary);
            }),
            CheckedBody::Deferred(body) | CheckedBody::Unfinished(body) => {
                visit_tnodes(body.as_node(), &mut |node| {
                    let summary = match node {
                        TNodeRef::Value(expr) => (expr.span(), None, false),
                        TNodeRef::Contextual(literal) => (literal.span(), None, true),
                    };
                    nodes.insert(node.id().clone(), summary);
                });
            }
        }
    }
    nodes
}

/// Register a test host function `name(params: Dimensionless, ...) -> result`
/// of the host plugin `plugin`, with the signature the pipeline verifies
/// the test's declaration against.
fn register_test_host_fn(
    registry: &mut graphcal_eval::host_fns::HostFunctionRegistry,
    plugin: &str,
    name: &str,
    params: &[&str],
    result: graphcal_compiler::function_signature::NamedParamKind,
    function: impl Fn(
        &[graphcal_eval::host_abi::argument::HostArgument],
    ) -> Result<
        graphcal_eval::host_fns::HostFnValue,
        graphcal_eval::host_fns::HostFnError,
    > + Send
    + Sync
    + 'static,
) {
    use graphcal_compiler::function_signature::{
        FunctionSignature, NamedFunctionParam, NamedParamKind,
    };
    let signature = FunctionSignature::try_new(
        Vec::new(),
        Vec::new(),
        params
            .iter()
            .map(|param| NamedFunctionParam {
                name: graphcal_compiler::syntax::function_name::FnParamName::expect_valid(*param),
                kind: NamedParamKind::dimensionless(),
            })
            .collect(),
        result.into(),
    )
    .unwrap();
    registry.register_with_signature(
        graphcal_compiler::plugin_identity::PluginIdentity::Host(
            graphcal_compiler::syntax::plugin::PluginPath::new(plugin),
        ),
        graphcal_compiler::syntax::function_name::FnName::expect_valid(name),
        signature,
        function,
    );
}

/// Helper: find a named value and return it (for indexed value tests).
fn find_entry(result: &EvalResult, name: &str) -> Value {
    result
        .entries
        .iter()
        .find(|EvalEntry { name: n, .. }| n.to_string() == name)
        .unwrap_or_else(|| panic!("value `{name}` not found"))
        .outcome
        .as_ref()
        .unwrap_or_else(|e| panic!("value `{name}` has error: {e}"))
        .clone()
}

/// A closed value parsed from its own text, as a `--param` value is.
fn parse_expr(s: &str) -> ExternalValue {
    ExternalValue::parse("<--param>", s).unwrap()
}

fn session_over(
    prepared: &PreparedProject,
    inputs: &[&str],
    outputs: &[&str],
) -> std::sync::Arc<graphcal_model_protocol::SessionSchema> {
    let names = |names: &[&str]| {
        names
            .iter()
            .map(|name| graphcal_model_protocol::PortName::new(*name).unwrap())
            .collect()
    };
    std::sync::Arc::new(
        prepared
            .session_schema(&graphcal_model_protocol::SessionSpec::new(
                names(inputs),
                names(outputs),
            ))
            .unwrap(),
    )
}

/// Helper: find a named Int value.
fn find_int_value(result: &EvalResult, name: &str) -> i64 {
    let val = result
        .entries
        .iter()
        .find(|EvalEntry { name: n, .. }| n.to_string() == name)
        .unwrap_or_else(|| panic!("value `{name}` not found"))
        .outcome
        .as_ref()
        .unwrap_or_else(|e| panic!("value `{name}` has error: {e}"));
    match val {
        Value::Int(i) => *i,
        other => panic!("expected Int for `{name}`, got {other:?}"),
    }
}

fn write_same_leaf_constructor_project(
    main_source: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub type Action { Pick(distance: Length), Idle }\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub type Command { Pick(duration: Time), Idle }\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

fn loaded_file_dag_id(
    project: &crate::loader::LoadedProject,
    file_name: &str,
) -> graphcal_compiler::dag_id::DagId {
    project
        .files()
        .iter()
        .find(|file| file.path().file_name().and_then(|name| name.to_str()) == Some(file_name))
        .map_or_else(
            || panic!("loaded file `{file_name}` not found"),
            |file| file.dag_id().clone(),
        )
}
