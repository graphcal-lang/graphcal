//! Browser-specific report metadata and rendering adapter.

use std::collections::BTreeMap;
use std::fmt::Write;

use graphcal_eval::eval::{EvalOutputView, EvalResult};
use graphcal_eval::loader::LoadedProject;
use graphcal_report::report_html::render_host_report_html;
use graphcal_report::report_ir::{
    Provenance, ReportInputs, SourceDigest, build_report, collect_doc_captions,
};
use sha2::{Digest, Sha256};

use crate::bindings::BindingRequest;
use crate::project::VirtualProject;

pub struct ReportMetadata {
    title: String,
    docs: BTreeMap<String, String>,
    sources: Vec<SourceDigest>,
    entry: String,
}

impl ReportMetadata {
    pub(crate) fn from_loaded(project: &LoadedProject, entry: String) -> Self {
        let title = std::path::Path::new(&entry).file_stem().map_or_else(
            || "report".to_string(),
            |stem| stem.to_string_lossy().into_owned(),
        );
        let docs = collect_doc_captions(project.root_file().ast());
        let mut sources = project
            .files()
            .values()
            .map(|file| {
                let name = VirtualProject::relative_source_name(&file.path().to_string_lossy())
                    .unwrap_or_else(|| file.path().display().to_string());
                SourceDigest {
                    name,
                    sha256: graphcal_package::Sha256Digest::from_bytes(
                        Sha256::digest(file.source().as_bytes()).into(),
                    )
                    .to_string(),
                }
            })
            .collect::<Vec<_>>();
        sources.sort_by(|a, b| a.name.cmp(&b.name));
        Self {
            title,
            docs,
            sources,
            entry,
        }
    }

    pub(crate) fn render(
        &self,
        result: &EvalResult,
        bindings: &[BindingRequest],
    ) -> Result<String, String> {
        let provenance = Provenance {
            compiler_version: env!("CARGO_PKG_VERSION").to_string(),
            sources: self
                .sources
                .iter()
                .map(|source| SourceDigest {
                    name: source.name.clone(),
                    sha256: source.sha256.clone(),
                })
                .collect(),
            baseline_params: baseline_params(result),
            repro_command: repro_command(&self.entry, bindings),
        };
        let document = build_report(ReportInputs {
            title: &self.title,
            result,
            docs: &self.docs,
            provenance,
        })
        .map_err(|error| error.to_string())?;
        Ok(render_host_report_html(&document))
    }
}

fn baseline_params(result: &EvalResult) -> Vec<(String, String)> {
    result
        .output_params(EvalOutputView::Surface)
        .map(|(name, outcome)| {
            let value = outcome.as_ref().map_or_else(
                |error| format!("ERROR: {error}"),
                |value| {
                    graphcal_report::value_display::scalar_display(value, &result.base_dim_symbols)
                        .unwrap_or_else(|error| format!("ERROR: {error}"))
                },
            );
            (name.to_string(), value)
        })
        .collect()
}

fn repro_command(entry: &str, bindings: &[BindingRequest]) -> String {
    let mut command = format!("graphcal eval {}", shell_quote(entry));
    for binding in bindings {
        let _ = write!(
            command,
            " --param {}",
            shell_quote(&format!("{}={}", binding.name, binding.expr))
        );
    }
    command
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
