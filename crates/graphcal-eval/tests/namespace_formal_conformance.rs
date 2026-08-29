//! Differential conformance between Graphcal's production namespace pipeline
//! and the reviewed Lean namespace-resolution oracle.

use std::collections::{HashMap, HashSet};
use std::env;
use std::path::PathBuf;
use std::process::Command;

use graphcal_eval::eval::{CompileError, compile_and_eval, compile_and_eval_project};
use graphcal_io::RealFileSystem;
use serde::Deserialize;

const ORACLE_ENV: &str = "GRAPHCAL_NAMESPACE_RESOLUTION_ORACLE";
const EXPECTED_CASE_COUNT: usize = 72;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "camelCase")]
enum OracleNamespace {
    Static,
    Term,
    Unit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "camelCase")]
enum OracleInputCategory {
    Unmarked,
    NominalType,
    Dimension,
    Index,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "camelCase")]
enum OracleScenario {
    VisibleLookup {
        occupied: Vec<OracleNamespace>,
        query: OracleNamespace,
    },
    MemberLookup {
        occupied: Vec<OracleNamespace>,
        query: OracleNamespace,
    },
    DuplicateSlot {
        space: OracleNamespace,
    },
    InputBinding {
        target: OracleInputCategory,
        selector: OracleInputCategory,
    },
    Label {
        owner: String,
        label: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum OracleRule {
    Unknown,
    Ambiguous,
    NotNamespaceOwner,
    NonDagPathSegment,
    CannotTraverse,
    WrongNamespace,
    InvalidStaticUse,
    InvalidTermUse,
    InvalidInputTarget,
    LabelOwnerNotIndex,
    LabelOwnerMismatch,
    DuplicateSlot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum OracleDecision {
    Accepted,
    Rejected { rule: OracleRule },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct OracleCase {
    scenario: OracleScenario,
    decision: OracleDecision,
}

#[derive(Debug)]
enum ScenarioError {
    Production(Box<CompileError>),
    Harness(std::io::Error),
}

impl From<CompileError> for ScenarioError {
    fn from(error: CompileError) -> Self {
        Self::Production(Box::new(error))
    }
}

impl From<std::io::Error> for ScenarioError {
    fn from(error: std::io::Error) -> Self {
        Self::Harness(error)
    }
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

fn contains(occupied: &[OracleNamespace], namespace: OracleNamespace) -> bool {
    occupied.contains(&namespace)
}

fn subject_declarations(occupied: &[OracleNamespace]) -> String {
    let mut source = String::new();
    if contains(occupied, OracleNamespace::Static) {
        source.push_str("pub dim Subject = Length;\n");
    }
    if contains(occupied, OracleNamespace::Term) {
        source.push_str("pub type Carrier { Subject }\n");
    } else {
        source.push_str("pub type Carrier { CarrierValue }\n");
    }
    if contains(occupied, OracleNamespace::Unit) {
        source.push_str("pub const unit Subject: Length = 1.0 m;\n");
    }
    source
}

const fn visible_query(query: OracleNamespace) -> &'static str {
    match query {
        OracleNamespace::Static => "param observed: Subject = 1.0 m;\n",
        OracleNamespace::Term => "param observed: Carrier = Subject;\n",
        OracleNamespace::Unit => "param observed: Length = 1.0 Subject;\n",
    }
}

const fn member_query(query: OracleNamespace) -> &'static str {
    match query {
        OracleNamespace::Static => "param observed: lib::Subject = 1.0 m;\n",
        OracleNamespace::Term => "param observed: lib::Carrier = lib::Subject;\n",
        OracleNamespace::Unit => "param observed: Length = 1.0 lib::Subject;\n",
    }
}

fn run_visible_lookup(
    occupied: &[OracleNamespace],
    query: OracleNamespace,
) -> Result<(), ScenarioError> {
    let source = format!("{}{}", subject_declarations(occupied), visible_query(query));
    compile_and_eval(&source)?;
    Ok(())
}

fn run_member_lookup(
    occupied: &[OracleNamespace],
    query: OracleNamespace,
) -> Result<(), ScenarioError> {
    let directory = tempfile::tempdir()?;
    let source_root = directory.path().join("src/oracle");
    std::fs::create_dir_all(&source_root)?;
    std::fs::write(
        directory.path().join("graphcal.toml"),
        "[package]\nname = \"oracle\"\n",
    )?;
    std::fs::write(
        source_root.join("library.gcl"),
        subject_declarations(occupied),
    )?;
    let main = format!("import oracle.library as lib;\n{}", member_query(query));
    let root = source_root.join("main.gcl");
    std::fs::write(&root, main)?;
    compile_and_eval_project(&root, &HashMap::new(), None, &RealFileSystem::default())?;
    Ok(())
}

fn run_duplicate(space: OracleNamespace) -> Result<(), ScenarioError> {
    let source = match space {
        OracleNamespace::Static => "dim Subject = Length;\nindex Subject = { Only };\n",
        OracleNamespace::Term => "type Left { Subject }\ntype Right { Subject }\n",
        OracleNamespace::Unit => {
            "const unit Subject: Length = 1.0 m;\nconst unit Subject: Length = 2.0 m;\n"
        }
    };
    compile_and_eval(source)?;
    Ok(())
}

const fn input_target_source(target: OracleInputCategory) -> &'static str {
    match target {
        OracleInputCategory::Unmarked => {
            "param Slot: Dimensionless;\npub node output: Dimensionless = @Slot;"
        }
        OracleInputCategory::NominalType => "pub(bind) type Slot;",
        OracleInputCategory::Dimension => "pub(bind) dim Slot;",
        OracleInputCategory::Index => "pub(bind) index Slot;",
    }
}

const fn input_selector_source(selector: OracleInputCategory) -> &'static str {
    match selector {
        OracleInputCategory::Unmarked => "Slot: 1.0",
        OracleInputCategory::NominalType => "type Slot: Replacement",
        OracleInputCategory::Dimension => "dim Slot: Length",
        OracleInputCategory::Index => "index Slot: ReplacementIndex",
    }
}

fn run_input_binding(
    target: OracleInputCategory,
    selector: OracleInputCategory,
) -> Result<(), ScenarioError> {
    let source = format!(
        "type Replacement {{ Replacement }}\n\
         index ReplacementIndex = {{ Only }};\n\
         dag target {{\n{}\n}}\n\
         include target({}) as configured;\n",
        input_target_source(target),
        input_selector_source(selector),
    );
    compile_and_eval(&source)?;
    Ok(())
}

fn run_label(owner: &str, label: &str) -> Result<(), ScenarioError> {
    let expected_index = if owner == "B" { "B" } else { "A" };
    let source = format!(
        "index A = {{ Same }};\n\
         index B = {{ Same }};\n\
         dim NotIndex = Length;\n\
         node observed: Key<{expected_index}> = {owner}#{label};\n"
    );
    compile_and_eval(&source)?;
    Ok(())
}

fn run_scenario(scenario: &OracleScenario) -> Result<(), ScenarioError> {
    match scenario {
        OracleScenario::VisibleLookup { occupied, query } => run_visible_lookup(occupied, *query),
        OracleScenario::MemberLookup { occupied, query } => run_member_lookup(occupied, *query),
        OracleScenario::DuplicateSlot { space } => run_duplicate(*space),
        OracleScenario::InputBinding { target, selector } => run_input_binding(*target, *selector),
        OracleScenario::Label { owner, label } => run_label(owner, label),
    }
}

fn compare_case(case: &OracleCase) -> Result<(), String> {
    let production = run_scenario(&case.scenario);
    match (case.decision, production) {
        (OracleDecision::Accepted, Ok(()))
        | (OracleDecision::Rejected { .. }, Err(ScenarioError::Production(_))) => Ok(()),
        (OracleDecision::Accepted, Err(ScenarioError::Production(error))) => Err(format!(
            "Lean accepted {:?}, but Rust rejected it with {error:?}",
            case.scenario
        )),
        (OracleDecision::Accepted, Err(ScenarioError::Harness(error))) => Err(format!(
            "Lean accepted {:?}, but the Rust harness failed: {error}",
            case.scenario
        )),
        (OracleDecision::Rejected { rule }, Ok(())) => Err(format!(
            "Lean rejected {:?} by {rule:?}, but Rust accepted it",
            case.scenario
        )),
        (OracleDecision::Rejected { rule }, Err(ScenarioError::Harness(error))) => Err(format!(
            "Lean rejected {:?} by {rule:?}, but the Rust harness failed: {error}",
            case.scenario
        )),
    }
}

#[test]
#[ignore = "requires the Lean oracle; run `just formal-conformance`"]
fn namespace_resolution_matches_lean_oracle() {
    let cases = load_oracle_cases().unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        cases.len(),
        EXPECTED_CASE_COUNT,
        "the oracle must enumerate the reviewed namespace state matrix"
    );
    assert_eq!(
        cases
            .iter()
            .map(|case| &case.scenario)
            .collect::<HashSet<_>>()
            .len(),
        EXPECTED_CASE_COUNT,
        "the oracle state matrix must not contain duplicate scenarios"
    );

    let mismatches = cases
        .iter()
        .filter_map(|case| compare_case(case).err())
        .collect::<Vec<_>>();
    assert!(
        mismatches.is_empty(),
        "Lean/Rust namespace-resolution mismatches:\n{}",
        mismatches.join("\n")
    );
}
