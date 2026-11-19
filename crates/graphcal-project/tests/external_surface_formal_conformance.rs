//! Differential conformance between the reviewed Lean external-surface model
//! and Graphcal's production import/include pipeline.
//!
//! Every scenario is rendered as a two-file project (`lib.gcl` declares the
//! `Subject`, `main.gcl` imports or includes it) and compiled by the project
//! pipeline. A rejection is compared by its diagnostic code. An accepted
//! projection is classified by the identity the production resolver
//! ([`ModuleResolver::build`]) gives the importer's `Subject`.
#![cfg(test)]

use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::resolve::ModuleResolver;
use graphcal_compiler::syntax::names::{NameAtom, NamePath};
use graphcal_io::RealFileSystem;
use graphcal_project::compile_error::CompileError;
use graphcal_project::prepare::compile_and_eval_project;
use serde::Deserialize;

const ORACLE_ENV: &str = "GRAPHCAL_EXTERNAL_SURFACE_ORACLE";
const EXPECTED_CASE_COUNT: usize = 55;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "camelCase")]
enum OracleRole {
    Fixed,
    OptionalInput,
    RequiredInput,
}

impl OracleRole {
    /// A `type Name` declaration with this role and a nullary constructor
    /// of the same name when it has a definition.
    fn type_declaration(self, name: &str) -> String {
        self.type_declaration_with(name, name)
    }

    /// A `type Name` declaration with this role and the single constructor
    /// `constructor` when it has a definition.
    fn type_declaration_with(self, name: &str, constructor: &str) -> String {
        match self {
            Self::Fixed => format!("pub type {name} {{ {constructor} }}\n"),
            Self::OptionalInput => format!("pub(bind) type {name} {{ {constructor} }}\n"),
            Self::RequiredInput => format!("pub(bind) type {name};\n"),
        }
    }

    /// A `dim Name` declaration with this role.
    fn dimension_declaration(self, name: &str) -> String {
        match self {
            Self::Fixed => format!("pub dim {name} = Length;\n"),
            Self::OptionalInput => format!("pub(bind) dim {name} = Length;\n"),
            Self::RequiredInput => format!("pub(bind) dim {name};\n"),
        }
    }

    const fn dependency_declaration(self) -> &'static str {
        match self {
            Self::Fixed => "pub index Dependency = { Only };\n",
            Self::OptionalInput => "pub(bind) index Dependency = { Only };\n",
            Self::RequiredInput => "pub(bind) index Dependency;\n",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "camelCase")]
enum OracleTarget {
    None,
    Some {
        role: OracleRole,
        #[serde(rename = "sameKind")]
        same_kind: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "camelCase")]
enum OracleProjectionEntity {
    Constructor,
    Dag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "camelCase")]
enum OracleScenario {
    ImportStatic {
        source: OracleRole,
        dependency: Option<OracleRole>,
    },
    StaticBinding {
        input: OracleRole,
        target: OracleRole,
        #[serde(rename = "sameKind")]
        same_kind: bool,
    },
    ProjectStatic {
        source: OracleRole,
        target: OracleTarget,
    },
    ProjectEntity {
        entity: OracleProjectionEntity,
        #[serde(rename = "ownerRebound")]
        owner_rebound: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum OracleRule {
    DeclarationNotImportable,
    UnresolvedStaticDependency,
    MissingStaticBinding,
    InvalidStaticBinding,
    DeclarationNotProjectable,
    ConstructorOwnerRebound,
    UnexpectedProjectionCategory,
}

impl OracleRule {
    /// The production diagnostic codes that report this rule.
    const fn codes(self) -> &'static [&'static str] {
        match self {
            Self::DeclarationNotImportable => &["graphcal::M028"],
            Self::UnresolvedStaticDependency => &["graphcal::M029"],
            Self::MissingStaticBinding => &["graphcal::I010"],
            // A fixed input is not bindable (M011), a target of another
            // category does not resolve as a type (M036), and a target
            // without a definition is not concrete (M030).
            Self::InvalidStaticBinding => &["graphcal::M011", "graphcal::M030", "graphcal::M036"],
            Self::DeclarationNotProjectable => &["graphcal::M031"],
            Self::ConstructorOwnerRebound => &["graphcal::M032"],
            Self::UnexpectedProjectionCategory => &[],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum OracleDecision {
    Accepted,
    ProjectedSource,
    ProjectedTarget,
    Rejected { rule: OracleRule },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
struct OracleCase {
    scenario: OracleScenario,
    decision: OracleDecision,
}

fn load_oracle_cases() -> Result<Vec<OracleCase>, String> {
    let oracle_path = PathBuf::from(
        env::var_os(ORACLE_ENV)
            .ok_or_else(|| format!("{ORACLE_ENV} must name the built Lean oracle executable"))?,
    );
    let output = Command::new(&oracle_path).output().map_err(|error| {
        format!(
            "failed to execute Lean oracle {}: {error}",
            oracle_path.display()
        )
    })?;
    if !output.status.success() {
        return Err(format!(
            "Lean oracle {} failed with {}: {}",
            oracle_path.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("Lean oracle emitted invalid JSON: {error}"))
}

/// One scenario rendered as Graphcal source.
struct Project {
    lib: String,
    main: String,
}

/// What production decided for one scenario.
#[derive(Debug)]
enum Production {
    Accepted,
    /// The importer's `Subject` denotes the library's declaration (or its
    /// include specialization).
    ProjectedSource,
    /// The importer's `Subject` denotes the importer's binding target.
    ProjectedTarget,
    /// The importer's `Subject` denotes something else.
    ProjectedElsewhere(#[expect(dead_code, reason = "rendered by `Debug` in mismatches")] DagId),
    Rejected(Box<CompileError>),
}

/// The importer-side target of a `Subject` binding, when the scenario has
/// one.
fn target_declaration(role: OracleRole, same_kind: bool) -> String {
    if same_kind {
        role.type_declaration("Target")
    } else {
        role.dimension_declaration("Target")
    }
}

fn import_project(source: OracleRole, dependency: Option<OracleRole>) -> Project {
    let field = dependency.map_or("Int", |_| "Dimensionless[Dependency]");
    let subject = source.type_declaration_with("Subject", &format!("Subject(value: {field})"));
    Project {
        lib: format!(
            "{}{subject}",
            dependency.map_or("", OracleRole::dependency_declaration)
        ),
        main: "import oracle.lib::{ type Subject };\n".to_owned(),
    }
}

fn binding_project(input: OracleRole, target: OracleRole, same_kind: bool) -> Project {
    Project {
        lib: format!(
            "{}pub node out: Dimensionless = 1.0;\n",
            input.type_declaration("Subject")
        ),
        main: format!(
            "{}include oracle.lib(type Subject: Target) as configured;\n",
            target_declaration(target, same_kind)
        ),
    }
}

/// A fixed declaration is not an input, so no binding names it: the Lean
/// environment's entry for it is never consulted.
fn projection_project(source: OracleRole, target: OracleTarget) -> Project {
    let (target_declaration, binding) = match (source, target) {
        (_, OracleTarget::None) => (String::new(), ""),
        (OracleRole::Fixed, OracleTarget::Some { role, same_kind }) => {
            (target_declaration(role, same_kind), "")
        }
        (_, OracleTarget::Some { role, same_kind }) => {
            (target_declaration(role, same_kind), "type Subject: Target")
        }
    };
    Project {
        lib: source.type_declaration("Subject"),
        main: format!("{target_declaration}include oracle.lib({binding})::{{ type Subject }};\n"),
    }
}

/// The constructor's owner is rebound by binding it as a type input; a DAG
/// has no owner, so a rebound sibling type input stands in for one.
fn entity_project(entity: OracleProjectionEntity, owner_rebound: bool) -> Project {
    let lib = match entity {
        OracleProjectionEntity::Constructor if owner_rebound => {
            "pub(bind) type Owner { Subject }\n"
        }
        OracleProjectionEntity::Constructor => "pub type Owner { Subject }\n",
        OracleProjectionEntity::Dag if owner_rebound => {
            "pub(bind) type Owner { Value }\npub dag Subject {}\n"
        }
        OracleProjectionEntity::Dag => "pub dag Subject {}\n",
    };
    let binding = if owner_rebound {
        "type Owner: Target"
    } else {
        ""
    };
    Project {
        lib: lib.to_owned(),
        main: format!(
            "pub type Target {{ Target }}\n\
             include oracle.lib({binding})::{{ Subject }};\n"
        ),
    }
}

fn project(scenario: OracleScenario) -> Project {
    match scenario {
        OracleScenario::ImportStatic { source, dependency } => import_project(source, dependency),
        OracleScenario::StaticBinding {
            input,
            target,
            same_kind,
        } => binding_project(input, target, same_kind),
        OracleScenario::ProjectStatic { source, target } => projection_project(source, target),
        OracleScenario::ProjectEntity {
            entity,
            owner_rebound,
        } => entity_project(entity, owner_rebound),
    }
}

/// Which namespace a projection scenario's `Subject` lives in.
#[derive(Debug, Clone, Copy)]
enum SubjectNamespace {
    Type,
    Constructor,
}

const fn projected_subject(scenario: OracleScenario) -> Option<SubjectNamespace> {
    match scenario {
        OracleScenario::ProjectStatic { .. } => Some(SubjectNamespace::Type),
        OracleScenario::ProjectEntity { .. } => Some(SubjectNamespace::Constructor),
        OracleScenario::ImportStatic { .. } | OracleScenario::StaticBinding { .. } => None,
    }
}

fn parse(source: &str) -> graphcal_compiler::desugar::desugared_ast::File {
    let parsed = graphcal_compiler::syntax::parser::Parser::new(source)
        .parse_file()
        .unwrap_or_else(|error| panic!("oracle scenario rendered invalid Graphcal: {error}"));
    graphcal_compiler::desugar::desugared_ast::File::from(parsed)
}

/// Classify the canonical identity the production resolver gives the
/// importer's `Subject`.
fn projected_identity(project: &Project, namespace: SubjectNamespace) -> Production {
    let lib = parse(&project.lib);
    let main = parse(&project.main);
    let lib_id = DagId::root_in_package("oracle", "lib");
    let main_id = DagId::root_in_package("oracle", "main");
    let resolver = ModuleResolver::build(
        [
            (lib_id.clone(), lib.declarations.as_slice()),
            (main_id.clone(), main.declarations.as_slice()),
        ],
        &|_: &DagId, _: &graphcal_compiler::syntax::ast::ModulePath| Some(lib_id.clone()),
    )
    .unwrap_or_else(|error| panic!("production accepted a scenario the resolver rejects: {error}"));
    let subject = NamePath::local(NameAtom::parse("Subject").expect("valid identifier"));
    let owner = match namespace {
        SubjectNamespace::Type => resolver
            .resolve_struct_type_path(&main_id, &subject)
            .map(|symbol| symbol.resolved().owner().clone()),
        SubjectNamespace::Constructor => resolver
            .resolve_constructor_path(&main_id, &subject)
            .map(|symbol| symbol.resolved().owner().clone()),
    }
    .unwrap_or_else(|error| panic!("the importer's `Subject` does not resolve: {error}"));
    if owner == main_id {
        Production::ProjectedTarget
    } else if owner == lib_id || owner.is_instance_child_of(&main_id) {
        Production::ProjectedSource
    } else {
        Production::ProjectedElsewhere(owner)
    }
}

fn compile(project: &Project) -> Result<(), Box<CompileError>> {
    let directory = tempfile::tempdir().expect("temporary project directory");
    let source_root = directory.path().join("src/oracle");
    let write = |path: &Path, contents: &str| {
        std::fs::write(path, contents).expect("writable temporary project");
    };
    std::fs::create_dir_all(&source_root).expect("writable temporary project");
    write(
        &directory.path().join("graphcal.toml"),
        "[package]\nname = \"oracle\"\n",
    );
    write(&source_root.join("lib.gcl"), &project.lib);
    let root = source_root.join("main.gcl");
    write(&root, &project.main);
    compile_and_eval_project(&root, &HashMap::new(), None, &RealFileSystem::default())
        .map(drop)
        .map_err(Box::new)
}

fn run_scenario(scenario: OracleScenario) -> Production {
    let project = project(scenario);
    match (compile(&project), projected_subject(scenario)) {
        (Err(error), _) => Production::Rejected(error),
        (Ok(()), None) => Production::Accepted,
        (Ok(()), Some(namespace)) => projected_identity(&project, namespace),
    }
}

fn compare_case(case: OracleCase) -> Result<(), String> {
    let production = run_scenario(case.scenario);
    match (case.decision, &production) {
        (OracleDecision::Accepted, Production::Accepted)
        | (OracleDecision::ProjectedSource, Production::ProjectedSource)
        | (OracleDecision::ProjectedTarget, Production::ProjectedTarget) => Ok(()),
        (OracleDecision::Rejected { rule }, Production::Rejected(error)) => {
            let code = miette::Diagnostic::code(error.as_ref()).map(|code| code.to_string());
            if code
                .as_deref()
                .is_some_and(|code| rule.codes().contains(&code))
            {
                Ok(())
            } else {
                Err(format!(
                    "Lean rejected {:?} by {rule:?}, but Rust rejected it with {code:?}: {error}",
                    case.scenario
                ))
            }
        }
        (decision, production) => Err(format!(
            "Lean returned {decision:?} for {:?}, but Rust returned {production:?}",
            case.scenario
        )),
    }
}

#[test]
#[ignore = "requires the Lean oracle; run `just formal-conformance`"]
fn external_surface_matches_lean_oracle() {
    let cases = load_oracle_cases().unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(cases.len(), EXPECTED_CASE_COUNT);
    assert_eq!(
        cases
            .iter()
            .map(|case| case.scenario)
            .collect::<HashSet<_>>()
            .len(),
        EXPECTED_CASE_COUNT,
        "the oracle matrix must not contain duplicate scenarios"
    );

    let mismatches = cases
        .iter()
        .filter_map(|case| compare_case(*case).err())
        .collect::<Vec<_>>();
    assert!(
        mismatches.is_empty(),
        "Lean/Rust external-surface mismatches:\n{}",
        mismatches.join("\n")
    );
}
