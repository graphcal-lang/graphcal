//! Typed document model for auto-reports, derived from one evaluated project.
//!
//! The auto-report needs no authoring: the model is the report. Controls come
//! from `param` declarations, value cards from the evaluated surface in
//! declaration order, figures from plot/figure/layer declarations, checks
//! from assertions, and captions from attached `///` doc comments. Renderers
//! (`report_html`, `report_markdown`) are pure functions over this model.

use std::collections::BTreeMap;

use graphcal_eval::eval::{AssertResult, DeclType, EvalOutputView, EvalResult};
use thiserror::Error;

use crate::value_display::{ValueBody, project_value_body};
use crate::vega::{RenderedFigure, UnknownPlotReference, build_figures};

/// One complete auto-report at baseline values.
pub struct ReportDocument {
    /// Page title (normally the entry model name).
    pub(crate) title: String,
    /// Entry parameters in declaration order.
    pub params: Vec<ValueCard>,
    /// Constants and nodes in declaration order.
    pub values: Vec<ValueCard>,
    /// Renderable figures in declaration order, with optional captions.
    pub figures: Vec<FigureCard>,
    /// Plot declarations that failed to evaluate.
    pub(crate) plot_errors: Vec<CheckMessage>,
    /// Assertion results in declaration order.
    pub checks: Vec<CheckRow>,
    /// Build provenance rendered in the footer.
    pub(crate) provenance: Provenance,
}

/// One evaluated declaration card.
pub struct ValueCard {
    pub name: String,
    pub(crate) kind: DeclType,
    /// Caption from the declaration's `///` doc block.
    pub doc: Option<String>,
    pub(crate) body: CardBody,
}

/// Display body of one card.
pub enum CardBody {
    Value(ValueBody),
    /// The declaration failed to evaluate (or its display projection failed);
    /// per-node fault isolation keeps the rest of the report intact.
    Error {
        message: String,
    },
}

/// One renderable figure with optional caption.
pub struct FigureCard {
    pub(crate) figure: RenderedFigure,
    pub(crate) doc: Option<String>,
}

/// One assertion result row.
pub struct CheckRow {
    pub(crate) name: String,
    pub(crate) status: CheckStatus,
    pub(crate) message: Option<String>,
    /// Declarations this check is declared to cover (`#[assumes]`).
    pub(crate) affected: Vec<String>,
}

/// Assertion outcome family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Pass,
    Fail,
    Error,
}

/// A named non-fatal message (currently: plot evaluation failures).
pub struct CheckMessage {
    pub(crate) name: String,
    pub(crate) message: String,
}

/// Deterministic build provenance. Deliberately excludes wall-clock
/// timestamps so the artifact is byte-reproducible from identical sources
/// (#1153's CI-diffability requirement).
pub struct Provenance {
    /// Compiler/CLI version that built the report.
    pub compiler_version: String,
    /// Every source file with its SHA-256 content digest, in load order.
    pub sources: Vec<SourceDigest>,
    /// Baseline parameter values as displayed (after `--set` overrides).
    pub baseline_params: Vec<(String, String)>,
    /// Copy-pasteable command reproducing the baseline evaluation.
    pub repro_command: String,
}

/// One source file digest.
pub struct SourceDigest {
    pub name: String,
    /// Lowercase hex SHA-256 of the file content.
    pub sha256: String,
}

/// Everything needed to derive one auto-report.
pub struct ReportInputs<'a> {
    pub title: &'a str,
    pub result: &'a EvalResult,
    /// Doc captions keyed by declaration display name.
    pub docs: &'a BTreeMap<String, String>,
    pub provenance: Provenance,
}

/// Why a report could not be assembled.
#[derive(Debug, Error)]
pub enum ReportBuildError {
    #[error(transparent)]
    UnknownPlotReference(#[from] UnknownPlotReference),
}

/// Collect `///` doc captions from one entry file, keyed by name.
///
/// Nested `dag` declarations are intentionally skipped: their members
/// surface under instance-qualified names, not bare ones.
#[must_use]
pub fn collect_doc_captions(
    file: &graphcal_compiler::desugar::desugared_ast::File,
) -> BTreeMap<String, String> {
    file.declarations
        .iter()
        .filter_map(|declaration| {
            let doc = declaration.doc.as_ref()?;
            let (name, _) = declaration.kind.name_and_span()?;
            Some((name.to_string(), doc.text().to_string()))
        })
        .collect()
}

/// Derive the auto-report document from one evaluated result.
///
/// Value display failures are isolated into per-card errors, mirroring the
/// evaluator's per-node fault isolation; only compiler-invariant failures
/// (dangling figure references) abort the build.
pub fn build_report(inputs: ReportInputs<'_>) -> Result<ReportDocument, ReportBuildError> {
    let result = inputs.result;
    let mut params = Vec::new();
    let mut values = Vec::new();
    for (name, outcome, kind) in result.output_values(EvalOutputView::Surface) {
        let name = name.to_string();
        let body = match outcome {
            Ok(value) => match project_value_body(value, &result.base_dim_symbols) {
                Ok(body) => CardBody::Value(body),
                Err(error) => CardBody::Error {
                    message: error.to_string(),
                },
            },
            Err(error) => CardBody::Error {
                message: error.to_string(),
            },
        };
        let card = ValueCard {
            doc: inputs.docs.get(&name).cloned(),
            name,
            kind: *kind,
            body,
        };
        match kind {
            DeclType::Param => params.push(card),
            DeclType::Const | DeclType::Node => values.push(card),
        }
    }

    let figures = build_figures(&result.plots, &result.figures, &result.layers)?
        .into_iter()
        .map(|figure| FigureCard {
            doc: inputs.docs.get(&figure.name).cloned(),
            figure,
        })
        .collect();

    let plot_errors = result
        .plot_errors
        .iter()
        .map(|error| CheckMessage {
            name: error.name.to_string(),
            message: error.message.clone(),
        })
        .collect();

    let checks = result
        .assertions
        .iter()
        .map(|(name, outcome, _)| {
            // An assertion without `#[assumes]` legally covers nothing.
            let affected = result
                .assumes_map
                .get(name)
                .map_or_else(Vec::new, |affected| {
                    affected.iter().map(ToString::to_string).collect()
                });
            let (status, message) = match outcome {
                AssertResult::Pass => (CheckStatus::Pass, None),
                AssertResult::Fail { message } => (CheckStatus::Fail, Some(message.clone())),
                AssertResult::Error { message } => (CheckStatus::Error, Some(message.clone())),
            };
            CheckRow {
                name: name.to_string(),
                status,
                message,
                affected,
            }
        })
        .collect();

    Ok(ReportDocument {
        title: inputs.title.to_string(),
        params,
        values,
        figures,
        plot_errors,
        checks,
        provenance: inputs.provenance,
    })
}
