//! Browser WebAssembly adapter for Graphcal compilation and evaluation.
//!
//! The compiler and evaluator remain platform-neutral functional cores. This
//! crate validates an in-memory browser project, invokes those cores, and
//! converts their typed results into a JavaScript-facing transport model.

mod diagnostics;
mod output;
mod project;

use std::collections::HashMap;

use graphcal_compiler::desugar::desugared_ast::{DeclKind, Declaration};
use graphcal_eval::eval::{CompileError, compile_and_eval_from_project};
use graphcal_eval::loader::load_project;
use serde::Serialize;

pub use diagnostics::{
    DiagnosticLabelView, DiagnosticSeverity, DiagnosticView, TextPosition, TextRange,
};
pub use output::{
    AssertionOutcomeView, AssertionView, DeclarationKindView, DeclarationOutcomeView,
    DeclarationView, EvaluationView, IndexEntryKeyView, IndexedEntryView, NodeErrorView,
    NoticeView, StructFieldView, UnsupportedCapability, ValueView,
};
pub use project::{
    PlaygroundFile, PlaygroundRequest, ProjectValidationError, RequestErrorKind, RequestErrorView,
};

use crate::diagnostics::compile_error_view;
use crate::project::VirtualProject;

/// Complete outcome of one browser playground request.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PlaygroundOutcome {
    Rejected { error: RequestErrorView },
    CompileError { diagnostics: Vec<DiagnosticView> },
    Evaluated { evaluation: EvaluationView },
}

/// Validate, compile, and evaluate an in-memory browser project.
#[must_use]
pub fn evaluate(request: PlaygroundRequest) -> PlaygroundOutcome {
    let project = match VirtualProject::try_from(request) {
        Ok(project) => project,
        Err(error) => {
            return PlaygroundOutcome::Rejected {
                error: RequestErrorView::from(&error),
            };
        }
    };

    let filesystem = project.filesystem();
    let loaded = match load_project(
        &project.entry_path(),
        Some(VirtualProject::root_path()),
        &filesystem,
    ) {
        Ok(loaded) => loaded,
        Err(error) => return compile_error_outcome(&error, &project),
    };

    if loaded
        .files()
        .values()
        .any(|file| declarations_use_plugins(&file.ast().declarations))
    {
        let error = ProjectValidationError::PluginsUnsupported;
        return PlaygroundOutcome::Rejected {
            error: RequestErrorView::from(&error),
        };
    }

    match compile_and_eval_from_project(&loaded, &HashMap::new()) {
        Ok(result) => PlaygroundOutcome::Evaluated {
            evaluation: EvaluationView::from(&result),
        },
        Err(error) => compile_error_outcome(&error, &project),
    }
}

fn declarations_use_plugins(declarations: &[Declaration]) -> bool {
    declarations
        .iter()
        .any(|declaration| match &declaration.kind {
            DeclKind::PluginImport(_) => true,
            DeclKind::Dag(dag) => declarations_use_plugins(&dag.body),
            _ => false,
        })
}

fn compile_error_outcome(error: &CompileError, project: &VirtualProject) -> PlaygroundOutcome {
    PlaygroundOutcome::CompileError {
        diagnostics: vec![compile_error_view(error, project)],
    }
}

/// JavaScript boundary used by the generated `wasm-bindgen` module.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(js_name = evaluateProject)]
pub fn evaluate_project_js(
    request: wasm_bindgen::JsValue,
) -> Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue> {
    let request = serde_wasm_bindgen::from_value(request).map_err(|error| {
        wasm_bindgen::JsValue::from_str(&format!("invalid playground request: {error}"))
    })?;
    serde_wasm_bindgen::to_value(&evaluate(request)).map_err(|error| {
        wasm_bindgen::JsValue::from_str(&format!("could not serialize playground result: {error}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn single_file(source: &str) -> PlaygroundRequest {
        PlaygroundRequest {
            entry: "main.gcl".to_string(),
            files: vec![PlaygroundFile {
                path: "main.gcl".to_string(),
                content: source.to_string(),
            }],
        }
    }

    #[test]
    fn evaluates_single_file() {
        let outcome = evaluate(single_file(
            "param x: Dimensionless = 2.0;\nnode doubled: Dimensionless = @x * 2.0;",
        ));
        let PlaygroundOutcome::Evaluated { evaluation } = outcome else {
            panic!("expected evaluation, got {outcome:?}");
        };
        assert_eq!(evaluation.values.len(), 2);
        assert!(!evaluation.has_errors);
    }

    #[test]
    fn returns_structured_compile_diagnostic() {
        let outcome = evaluate(single_file("node bad: Dimensionless = 1.0 kg + 1.0 s;"));
        let PlaygroundOutcome::CompileError { diagnostics } = outcome else {
            panic!("expected compile error, got {outcome:?}");
        };
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].file, "main.gcl");
        assert!(!diagnostics[0].labels.is_empty());
    }

    #[test]
    fn serializes_the_browser_transport_shape() {
        let evaluated =
            serde_json::to_value(evaluate(single_file("node distance: Length = 2.0 m;"))).unwrap();
        assert_eq!(evaluated["status"], "evaluated");
        assert_eq!(evaluated["evaluation"]["has_errors"], false);
        assert_eq!(
            evaluated["evaluation"]["values"][0]["declaration_kind"],
            "node"
        );
        assert_eq!(
            evaluated["evaluation"]["values"][0]["outcome"]["status"],
            "value"
        );
        assert_eq!(
            evaluated["evaluation"]["values"][0]["outcome"]["value"]["kind"],
            "quantity"
        );

        let rejected = serde_json::to_value(evaluate(PlaygroundRequest {
            entry: "main.gcl".to_string(),
            files: Vec::new(),
        }))
        .unwrap();
        assert_eq!(rejected["status"], "rejected");
        assert_eq!(rejected["error"]["kind"], "no_files");
    }

    #[test]
    fn rejects_plugin_imports_explicitly() {
        let outcome = evaluate(single_file(
            "import plugin \"graphcal:demo\" as demo { fn add(x: Dimensionless, y: Dimensionless) -> Dimensionless; }",
        ));
        let PlaygroundOutcome::Rejected { error } = outcome else {
            panic!("expected rejection, got {outcome:?}");
        };
        assert_eq!(error.kind, RequestErrorKind::PluginsUnsupported);
    }

    #[test]
    fn rejects_plugin_imports_inside_dag_bodies() {
        let outcome = evaluate(single_file(
            "dag nested { import plugin \"graphcal:demo\" as demo { fn add(x: Dimensionless, y: Dimensionless) -> Dimensionless; } param x: Dimensionless = 1.0; }",
        ));
        let PlaygroundOutcome::Rejected { error } = outcome else {
            panic!("expected rejection, got {outcome:?}");
        };
        assert_eq!(error.kind, RequestErrorKind::PluginsUnsupported);
    }
}
