//! Differential conformance between the Lean Option A matrix and Rust's pure core.

use std::collections::HashSet;
use std::env;
use std::process::Command;

use serde::Deserialize;

use super::{
    StaticDependency, StaticUseContext, TemplateClosureCheck, TemplateClosureViolation, validate,
};
use crate::ir::static_interface::{StaticInputKind, StaticRole};

const ORACLE_ENV: &str = "GRAPHCAL_TEMPLATE_CLOSURE_ORACLE";
const EXPECTED_CASE_COUNT: usize = 36;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "camelCase")]
enum OracleKind {
    Type,
    Dimension,
    Index,
}

impl From<OracleKind> for StaticInputKind {
    fn from(value: OracleKind) -> Self {
        match value {
            OracleKind::Type => Self::Type,
            OracleKind::Dimension => Self::Dimension,
            OracleKind::Index => Self::Index,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "camelCase")]
enum OracleRole {
    Fixed,
    OptionalInput,
    RequiredInput,
}

impl From<OracleRole> for StaticRole {
    fn from(value: OracleRole) -> Self {
        match value {
            OracleRole::Fixed => Self::Fixed,
            OracleRole::OptionalInput => Self::OptionalInput,
            OracleRole::RequiredInput => Self::RequiredInput,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "camelCase")]
enum OracleContext {
    ParameterDefault,
    TemplateBody,
}

impl From<OracleContext> for StaticUseContext {
    fn from(value: OracleContext) -> Self {
        match value {
            OracleContext::ParameterDefault => Self::ParameterDefault,
            OracleContext::TemplateBody => Self::TemplateBody,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "camelCase")]
enum OracleDependency {
    Abstract,
    DefaultDefinition,
}

impl From<OracleDependency> for StaticDependency {
    fn from(value: OracleDependency) -> Self {
        match value {
            OracleDependency::Abstract => Self::Abstract,
            OracleDependency::DefaultDefinition => Self::DefaultDefinition,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
struct OracleCheck {
    kind: OracleKind,
    role: OracleRole,
    context: OracleContext,
    dependency: OracleDependency,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum OracleRule {
    TemplateBodyDependsOnDefault,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum OracleDecision {
    Accepted,
    Rejected { rule: OracleRule, kind: OracleKind },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
struct OracleCase {
    check: OracleCheck,
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

fn production_decision(check: OracleCheck) -> Result<(), TemplateClosureViolation> {
    validate(TemplateClosureCheck {
        kind: check.kind.into(),
        role: check.role.into(),
        context: check.context.into(),
        dependency: check.dependency.into(),
    })
}

#[test]
#[ignore = "requires the Lean oracle; run `just formal-conformance`"]
fn template_closure_matches_lean_oracle() {
    let cases = load_oracle_cases().unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(cases.len(), EXPECTED_CASE_COUNT);
    assert_eq!(
        cases
            .iter()
            .map(|case| case.check)
            .collect::<HashSet<_>>()
            .len(),
        EXPECTED_CASE_COUNT,
        "the oracle state matrix must not contain duplicate checks"
    );

    let mismatches = cases
        .iter()
        .filter_map(|case| {
            let production = production_decision(case.check);
            let agrees = match (case.decision, production) {
                (OracleDecision::Accepted, Ok(())) => true,
                (
                    OracleDecision::Rejected {
                        rule: OracleRule::TemplateBodyDependsOnDefault,
                        kind,
                    },
                    Err(violation),
                ) => violation.kind == StaticInputKind::from(kind),
                _ => false,
            };
            (!agrees).then(|| format!("Lean/Rust mismatch for {case:?}"))
        })
        .collect::<Vec<_>>();
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}
