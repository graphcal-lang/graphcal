//! Differential conformance between the reviewed Lean external-surface model
//! and Rust's shared Static semantic core.

use std::collections::HashSet;
use std::env;
use std::process::Command;

use serde::Deserialize;

use super::static_dependencies::{StaticImportRejection, static_import_rejection};
use super::static_interface::{
    StaticInputKind, StaticInterface, StaticProjectionError, StaticProjectionIdentity, StaticRole,
    project_static_identity, static_binding_valid,
};
use crate::syntax::ast::ImportItemNamespace;
use crate::syntax::module_resolve::{DeclSymbolKind, ExportedImportItemKind, include_projection};
use crate::syntax::names::NameAtom;
use crate::syntax::parser::Parser;

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
    const fn production(self) -> StaticRole {
        match self {
            Self::Fixed => StaticRole::Fixed,
            Self::OptionalInput => StaticRole::OptionalInput,
            Self::RequiredInput => StaticRole::RequiredInput,
        }
    }

    const fn index_declaration(self) -> &'static str {
        match self {
            Self::Fixed => "index Dependency = { Only };",
            Self::OptionalInput => "pub(bind) index Dependency = { Only };",
            Self::RequiredInput => "pub(bind) index Dependency;",
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
    let oracle_path = env::var_os(ORACLE_ENV)
        .ok_or_else(|| format!("{ORACLE_ENV} must name the built Lean oracle executable"))?;
    let output = Command::new(&oracle_path)
        .output()
        .map_err(|error| format!("failed to execute Lean oracle {oracle_path:?}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Lean oracle {oracle_path:?} failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("Lean oracle emitted invalid JSON: {error}"))
}

fn import_source(source: OracleRole, dependency: Option<OracleRole>) -> String {
    let dependency_declaration = dependency.map_or(String::new(), |role| {
        format!("{}\n", role.index_declaration())
    });
    let source_declaration = match source {
        OracleRole::Fixed => {
            let field = dependency.map_or("Int", |_| "Dimensionless[Dependency]");
            format!("pub type Subject {{ Subject(value: {field}) }}")
        }
        OracleRole::OptionalInput => {
            let field = dependency.map_or("Int", |_| "Dimensionless[Dependency]");
            format!("pub(bind) type Subject {{ Subject(value: {field}) }}")
        }
        OracleRole::RequiredInput => "pub(bind) type Subject;".to_string(),
    };
    format!("{dependency_declaration}{source_declaration}")
}

fn run_import(source: OracleRole, dependency: Option<OracleRole>) -> OracleDecision {
    let source_text = import_source(source, dependency);
    let parsed = Parser::new(&source_text)
        .parse_file()
        .unwrap_or_else(|error| panic!("oracle scenario rendered invalid Graphcal: {error}"));
    let file = crate::syntax::desugar::desugar_multi_decls_in_file(parsed);
    let subject = NameAtom::parse("Subject").expect("valid test identifier");
    match static_import_rejection(&file.declarations, &subject, ImportItemNamespace::Type) {
        None => OracleDecision::Accepted,
        Some(StaticImportRejection::RequiredInput { .. }) => OracleDecision::Rejected {
            rule: OracleRule::DeclarationNotImportable,
        },
        Some(StaticImportRejection::UnresolvedDependency(_)) => OracleDecision::Rejected {
            rule: OracleRule::UnresolvedStaticDependency,
        },
    }
}

const fn interface(kind: StaticInputKind, role: OracleRole) -> StaticInterface {
    StaticInterface::new(kind, role.production())
}

fn run_binding(input: OracleRole, target: OracleRole, same_kind: bool) -> OracleDecision {
    let input = interface(StaticInputKind::Type, input);
    let target_kind = if same_kind {
        StaticInputKind::Type
    } else {
        StaticInputKind::Dimension
    };
    if static_binding_valid(input, interface(target_kind, target)) {
        OracleDecision::Accepted
    } else {
        OracleDecision::Rejected {
            rule: OracleRule::InvalidStaticBinding,
        }
    }
}

fn run_projection(source: OracleRole, target: OracleTarget) -> OracleDecision {
    let source = interface(StaticInputKind::Type, source);
    let target = match target {
        OracleTarget::None => None,
        OracleTarget::Some { role, same_kind } => Some(interface(
            if same_kind {
                StaticInputKind::Type
            } else {
                StaticInputKind::Dimension
            },
            role,
        )),
    };
    match project_static_identity(source, target) {
        Ok(StaticProjectionIdentity::SourceDeclaration) => OracleDecision::ProjectedSource,
        Ok(StaticProjectionIdentity::BindingTarget) => OracleDecision::ProjectedTarget,
        Err(StaticProjectionError::MissingBinding) => OracleDecision::Rejected {
            rule: OracleRule::MissingStaticBinding,
        },
        Err(StaticProjectionError::InvalidBinding) => OracleDecision::Rejected {
            rule: OracleRule::InvalidStaticBinding,
        },
    }
}

fn run_entity_projection(entity: OracleProjectionEntity, owner_rebound: bool) -> OracleDecision {
    let kind = match entity {
        OracleProjectionEntity::Constructor => ExportedImportItemKind::Constructor,
        OracleProjectionEntity::Dag => ExportedImportItemKind::Decl(DeclSymbolKind::Dag),
    };
    if include_projection(kind).is_none() {
        return OracleDecision::Rejected {
            rule: OracleRule::DeclarationNotProjectable,
        };
    }
    if entity == OracleProjectionEntity::Constructor && owner_rebound {
        return OracleDecision::Rejected {
            rule: OracleRule::ConstructorOwnerRebound,
        };
    }
    OracleDecision::ProjectedSource
}

fn run_scenario(scenario: OracleScenario) -> OracleDecision {
    match scenario {
        OracleScenario::ImportStatic { source, dependency } => run_import(source, dependency),
        OracleScenario::StaticBinding {
            input,
            target,
            same_kind,
        } => run_binding(input, target, same_kind),
        OracleScenario::ProjectStatic { source, target } => run_projection(source, target),
        OracleScenario::ProjectEntity {
            entity,
            owner_rebound,
        } => run_entity_projection(entity, owner_rebound),
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
        .filter_map(|case| {
            let production = run_scenario(case.scenario);
            (production != case.decision).then(|| {
                format!(
                    "Lean returned {:?} for {:?}, but Rust returned {production:?}",
                    case.decision, case.scenario
                )
            })
        })
        .collect::<Vec<_>>();
    assert!(
        mismatches.is_empty(),
        "Lean/Rust external-surface mismatches:\n{}",
        mismatches.join("\n")
    );
}
