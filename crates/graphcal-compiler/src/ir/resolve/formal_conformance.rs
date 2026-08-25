//! Differential conformance test between the Lean V002 oracle and the
//! production declaration-shell validator.

use std::collections::HashSet;
use std::env;
use std::process::Command;
use std::sync::Arc;

use miette::NamedSource;
use serde::Deserialize;

use super::{CollectedFile, GraphcalError, resolve};
use crate::syntax::parser::Parser;

const ORACLE_ENV: &str = "GRAPHCAL_REQUIRED_BINDABILITY_ORACLE";
const EXPECTED_CASE_COUNT: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "camelCase")]
enum OracleNominalKind {
    Index,
    Type,
    Dimension,
}

impl OracleNominalKind {
    const fn diagnostic_name(self) -> &'static str {
        match self {
            Self::Index => "index",
            Self::Type => "type",
            Self::Dimension => "dim",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "camelCase")]
enum OracleVisibility {
    Local,
    Exported,
    ExportedBindable,
}

impl OracleVisibility {
    const fn source_prefix(self) -> &'static str {
        match self {
            Self::Local => "",
            Self::Exported => "pub ",
            Self::ExportedBindable => "pub(bind) ",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "camelCase")]
enum OracleRequirement {
    Defaulted,
    Required,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "camelCase")]
enum OracleDeclaration {
    Param {
        requirement: OracleRequirement,
    },
    Nominal {
        kind: OracleNominalKind,
        visibility: OracleVisibility,
        requirement: OracleRequirement,
    },
}

impl OracleDeclaration {
    /// Render a source declaration independently of the production parser. The
    /// Lean decision is computed from this typed value before Rust sees the
    /// rendered text, so a parser-side visibility mistake cannot taint both
    /// sides of the comparison.
    fn render_source(&self) -> String {
        match self {
            Self::Param {
                requirement: OracleRequirement::Defaulted,
            } => "param input: Dimensionless = 1.0;".to_string(),
            Self::Param {
                requirement: OracleRequirement::Required,
            } => "param input: Dimensionless;".to_string(),
            Self::Nominal {
                kind,
                visibility,
                requirement,
            } => {
                let declaration = match (kind, requirement) {
                    (OracleNominalKind::Index, OracleRequirement::Defaulted) => "index I = { A };",
                    (OracleNominalKind::Index, OracleRequirement::Required) => "index I;",
                    (OracleNominalKind::Type, OracleRequirement::Defaulted) => "type T { T }",
                    (OracleNominalKind::Type, OracleRequirement::Required) => "type T;",
                    (OracleNominalKind::Dimension, OracleRequirement::Defaulted) => {
                        "dim D = Length;"
                    }
                    (OracleNominalKind::Dimension, OracleRequirement::Required) => "dim D;",
                };
                format!("{}{declaration}", visibility.source_prefix())
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum OracleRule {
    RequiredMustBeBindable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum OracleDecision {
    Accepted,
    Rejected {
        rule: OracleRule,
        kind: OracleNominalKind,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct OracleCase {
    declaration: OracleDeclaration,
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

fn parse_and_resolve_case(source: &str) -> Result<CollectedFile, GraphcalError> {
    let raw_file = Parser::new(source)
        .parse_file()
        .unwrap_or_else(|error| panic!("oracle rendered invalid Graphcal `{source}`: {error}"));
    let file = crate::syntax::desugar::desugar_multi_decls_in_file(raw_file);
    let src = NamedSource::new("lean-oracle-case.gcl", Arc::new(source.to_string()));
    resolve(&file, &src)
}

fn compare_case(case: &OracleCase) -> Result<(), String> {
    let source = case.declaration.render_source();
    let production = parse_and_resolve_case(&source);

    match (case.decision, production) {
        (OracleDecision::Accepted, Ok(_)) => Ok(()),
        (
            OracleDecision::Rejected {
                rule: OracleRule::RequiredMustBeBindable,
                kind: expected_kind,
            },
            Err(GraphcalError::RequiredItemMustBeBindable { kind, .. }),
        ) if kind == expected_kind.diagnostic_name() => Ok(()),
        (OracleDecision::Accepted, Err(error)) => Err(format!(
            "Lean accepted `{source}`, but Rust rejected it with {error:?}"
        )),
        (OracleDecision::Rejected { .. }, Ok(_)) => {
            Err(format!("Lean rejected `{source}`, but Rust accepted it"))
        }
        (OracleDecision::Rejected { kind, .. }, Err(error)) => Err(format!(
            "Lean rejected `{source}` as V002 for {kind:?}, but Rust returned {error:?}"
        )),
    }
}

#[test]
#[ignore = "requires the Lean oracle; run `just formal-conformance`"]
fn required_bindability_matches_lean_oracle() {
    let cases = load_oracle_cases().unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        cases.len(),
        EXPECTED_CASE_COUNT,
        "the oracle must enumerate the complete V002 state matrix"
    );
    assert_eq!(
        cases
            .iter()
            .map(|case| &case.declaration)
            .collect::<HashSet<_>>()
            .len(),
        EXPECTED_CASE_COUNT,
        "the oracle state matrix must not contain duplicate declarations"
    );

    let mismatches = cases
        .iter()
        .filter_map(|case| compare_case(case).err())
        .collect::<Vec<_>>();
    assert!(
        mismatches.is_empty(),
        "Lean/Rust V002 mismatches:\n{}",
        mismatches.join("\n")
    );
}
