//! Browser-specific report metadata and rendering adapter.

use graphcal_eval::eval::EvalResult;
use graphcal_project::loader::LoadedProject;
use graphcal_report::report_html::render_host_report_html;
use graphcal_report::report_ir::{DocCaptions, ReportInputs, SourceDigest, build_report};
use graphcal_report::report_provenance::{ReproInvocation, build_provenance};
use sha2::{Digest, Sha256};

use crate::bindings::BindingRequest;
use crate::project::VirtualProject;

pub struct ReportMetadata {
    title: String,
    docs: DocCaptions,
    sources: Vec<SourceDigest>,
    entry: String,
}

impl ReportMetadata {
    pub(crate) fn new(project: &LoadedProject, entry: String, docs: DocCaptions) -> Self {
        let title = std::path::Path::new(&entry).file_stem().map_or_else(
            || "report".to_string(),
            |stem| stem.to_string_lossy().into_owned(),
        );
        let mut sources = project
            .files()
            .iter()
            .map(|file| {
                let name = VirtualProject::relative_source_name(file.path())
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
        let provenance = build_provenance(
            env!("CARGO_PKG_VERSION"),
            self.sources
                .iter()
                .map(|source| SourceDigest {
                    name: source.name.clone(),
                    sha256: source.sha256.clone(),
                })
                .collect(),
            result,
            &ReproInvocation {
                entry: &self.entry,
                root: None,
                parameters: bindings
                    .iter()
                    .map(|binding| (binding.name.as_str(), binding.expr.as_str()))
                    .collect(),
                parameter_json: None,
                parameter_json_max_bytes: None,
            },
        );
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
