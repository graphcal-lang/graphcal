//! Import-item diagnostics shared by the pure project compiler and inline-DAG
//! self-import preprocessing.
//!
//! Classification of what a module declares and exposes lives in the
//! compiler's [`ModuleInterface`]; this module only turns those typed answers
//! into source diagnostics. Keeping it outside `project_compiler` avoids a
//! circular module dependency between the two consumers.

use graphcal_compiler::ir::module_interface::ModuleInterface;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::semantic_error::graph::DagReference;
use graphcal_compiler::semantic_error::module::ModuleError;
use graphcal_compiler::source_id::SourceId;
use graphcal_compiler::syntax::ast::ImportItemNamespace;
use graphcal_compiler::syntax::import_category::ImportItemCategoryMismatch;
use graphcal_compiler::syntax::names::NameAtom;
use graphcal_compiler::syntax::span::Span;

/// Diagnose a selective item that names nothing importable in `expected`:
/// a category mismatch when `name` exists in another namespace, otherwise
/// an unknown name.
pub fn import_item_not_found_error(
    interface: &ModuleInterface,
    name: &NameAtom,
    expected: ImportItemNamespace,
    file_path: &DagReference,
    src: SourceId,
    span: Span,
) -> SemanticError {
    interface.namespaces_of(name).map_or_else(
        || {
            SemanticError::located(
                src,
                span,
                ModuleError::ImportNameNotFound {
                    name: name.clone(),
                    file_path: file_path.clone(),
                },
            )
        },
        |alternatives| {
            SemanticError::located(
                src,
                span,
                ModuleError::ImportCategoryMismatch {
                    file_path: file_path.clone(),
                    mismatch: ImportItemCategoryMismatch::new(name.clone(), expected, alternatives),
                },
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphcal_compiler::semantic_error::SemanticErrorKind;
    use graphcal_compiler::syntax::parser::Parser;

    fn interface(source: &str) -> ModuleInterface {
        let parsed = Parser::new(source).parse_file().expect("source parses");
        ModuleInterface::new(
            &graphcal_compiler::desugar::desugared_ast::File::from(parsed).declarations,
        )
    }

    fn src() -> SourceId {
        graphcal_compiler::source_registry::SourceRegistry::new()
            .register("main.gcl", std::sync::Arc::new(String::new()))
    }

    #[test]
    fn missing_items_report_category_mismatch_or_unknown_name() {
        let interface = interface("pub base unit JPY: Dimensionless;\n");
        let jpy = NameAtom::parse("JPY").unwrap();
        match import_item_not_found_error(
            &interface,
            &jpy,
            ImportItemNamespace::Type,
            &DagReference::Dag(graphcal_compiler::dag_id::DagId::root_in_package(
                "pkg", "lib",
            )),
            src(),
            Span::new(0, 3),
        ) {
            SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind:
                    SemanticErrorKind::Module(ModuleError::ImportCategoryMismatch { mismatch, .. }),
                ..
            }) => assert_eq!(
                mismatch,
                ImportItemCategoryMismatch::new(
                    jpy,
                    ImportItemNamespace::Type,
                    graphcal_compiler::syntax::non_empty::NonEmpty::try_from_vec(vec![
                        ImportItemNamespace::Unit
                    ])
                    .unwrap(),
                )
            ),
            other => panic!("expected category mismatch, got {other:?}"),
        }
        assert!(matches!(
            import_item_not_found_error(
                &interface,
                &NameAtom::parse("missing").unwrap(),
                ImportItemNamespace::Term,
                &DagReference::Dag(graphcal_compiler::dag_id::DagId::root_in_package(
                    "pkg", "lib"
                )),
                src(),
                Span::new(0, 7),
            ),
            SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Module(ModuleError::ImportNameNotFound { .. }),
                ..
            })
        ));
    }
}
