use graphcal_compiler::text_position::{Utf16LineIndex, Utf16Position, Utf16Range};
use graphcal_project::compile_error::CompileError;
use serde::Serialize;

use crate::project::VirtualProject;

/// A browser-renderable compiler diagnostic with UTF-16 source ranges.
#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticView {
    pub severity: DiagnosticSeverity,
    pub code: Option<String>,
    pub message: String,
    pub help: Option<String>,
    pub file: String,
    pub labels: Vec<DiagnosticLabelView>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticLabelView {
    pub message: Option<String>,
    pub range: TextRange,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct TextRange {
    pub start: TextPosition,
    pub end: TextPosition,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct TextPosition {
    pub line: u32,
    pub character: u32,
}

impl From<Utf16Position> for TextPosition {
    fn from(position: Utf16Position) -> Self {
        Self {
            line: position.line,
            character: position.character,
        }
    }
}

impl From<Utf16Range> for TextRange {
    fn from(range: Utf16Range) -> Self {
        Self {
            start: TextPosition::from(range.start),
            end: TextPosition::from(range.end),
        }
    }
}

pub fn compile_error_view(error: &CompileError, project: &VirtualProject) -> DiagnosticView {
    let diagnostic: &dyn miette::Diagnostic = error;

    let source = error.registered_source();
    // A detached text (a single in-memory source) is the entry itself.
    let file = source
        .and_then(|source| source.origin().file_path())
        .and_then(VirtualProject::relative_source_name)
        .unwrap_or_else(|| project.entry_display());
    let labels = source.map_or_else(Vec::new, |source| {
        let index = Utf16LineIndex::new(source.text().as_str());
        diagnostic
            .labels()
            .into_iter()
            .flatten()
            .map(|label| DiagnosticLabelView {
                message: label.label().map(ToString::to_string),
                range: TextRange::from(index.range(label.offset(), label.len())),
            })
            .collect()
    });

    DiagnosticView {
        severity: DiagnosticSeverity::Error,
        code: diagnostic.code().map(|code| code.to_string()),
        message: diagnostic.to_string(),
        help: diagnostic.help().map(|help| help.to_string()),
        file,
        labels,
    }
}

#[cfg(test)]
mod tests {
    use graphcal_project::prepare::compile_and_eval_named;

    use crate::project::{PlaygroundFile, PlaygroundRequest};

    use super::*;

    #[test]
    fn utf16_positions_count_astral_characters_as_two_code_units() {
        let source = "// 🙂\nnode x: Dimensionless = true + 1.0;";
        let project = VirtualProject::try_from(PlaygroundRequest {
            entry: "main.gcl".to_string(),
            files: vec![PlaygroundFile {
                path: "main.gcl".to_string(),
                content: source.to_string(),
            }],
        })
        .unwrap();
        let error = compile_and_eval_named(source, "/playground/main.gcl").unwrap_err();
        let diagnostic = compile_error_view(&error, &project);

        assert_eq!(diagnostic.file, "main.gcl");
        assert!(!diagnostic.labels.is_empty());
        assert!(diagnostic.labels[0].range.start.line >= 1);
    }

    #[test]
    fn a_file_is_named_by_its_origin_not_by_its_display_name() {
        use std::path::PathBuf;
        use std::sync::Arc;

        use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
        use graphcal_compiler::semantic_error::SemanticError;
        use graphcal_compiler::source_registry::{SourceOrigin, SourceRegistry};

        let project = VirtualProject::try_from(PlaygroundRequest {
            entry: "main.gcl".to_string(),
            files: vec![PlaygroundFile {
                path: "main.gcl".to_string(),
                content: String::new(),
            }],
        })
        .unwrap();
        let mut sources = SourceRegistry::new();
        // In package mode a file is displayed as `<package>:<path>`.
        let source = sources.register_with_origin(
            SourceOrigin::File(PathBuf::from("/playground/my_app/lib.gcl")),
            "pkg-my_app:/playground/my_app/lib.gcl",
            Arc::new("node x".to_string()),
        );
        let error = CompileError::semantic(
            SemanticError::internal_error("failure", source, DiagnosticAnchor::WholeFile),
            &sources,
        );

        assert_eq!(compile_error_view(&error, &project).file, "my_app/lib.gcl");
    }

    #[test]
    fn line_index_converts_utf8_to_utf16() {
        let source = "a🙂b\nnext";
        let index = Utf16LineIndex::new(source);
        assert_eq!(
            TextPosition::from(index.position("a🙂".len())),
            TextPosition {
                line: 0,
                character: 3,
            }
        );
        assert_eq!(
            TextPosition::from(index.position("a🙂b\n".len())),
            TextPosition {
                line: 1,
                character: 0,
            }
        );
    }
}
