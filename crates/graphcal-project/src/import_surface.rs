//! Import-item diagnostics shared by the pure project compiler and inline-DAG
//! self-import preprocessing.
//!
//! Classification of what a module declares and exposes lives in the
//! compiler's [`ModuleInterface`]; this module only turns those typed answers
//! into source diagnostics. Keeping it outside `project_compiler` avoids a
//! circular module dependency between the two consumers.

use graphcal_compiler::graphcal_error::GraphcalError;
use graphcal_compiler::ir::module_interface::ModuleInterface;
use graphcal_compiler::resolve::category::ExportedImportItemKind;
use graphcal_compiler::resolve::namespace::Namespace;
use graphcal_compiler::resolve::reserved_name::validate_reserved_name;
use graphcal_compiler::source_id::SourceId;
use graphcal_compiler::syntax::ast::{ImportItem, ImportItemNamespace};
use graphcal_compiler::syntax::import_category::ImportItemCategoryMismatch;
use graphcal_compiler::syntax::names::NameAtom;
use graphcal_compiler::syntax::span::Span;

/// Validate the source-visible local spelling introduced by one selective item.
pub fn validate_constructor_alias(
    kind: ExportedImportItemKind,
    import_item: &ImportItem,
    src: SourceId,
) -> Result<(), GraphcalError> {
    match kind {
        ExportedImportItemKind::Constructor => {
            validate_reserved_alias(Namespace::Term, import_item, src)
        }
        ExportedImportItemKind::Decl(_)
        | ExportedImportItemKind::Dimension
        | ExportedImportItemKind::Unit(_)
        | ExportedImportItemKind::Type
        | ExportedImportItemKind::Index => Ok(()),
    }
}

/// Validate one alias against the reserved vocabulary of its semantic namespace.
pub fn validate_reserved_alias(
    namespace: Namespace,
    import_item: &ImportItem,
    src: SourceId,
) -> Result<(), GraphcalError> {
    let local_name = import_item.local_name_atom();
    validate_reserved_name(namespace, local_name).map_err(|_| {
        let kind = match namespace {
            Namespace::Static => match import_item.namespace {
                ImportItemNamespace::Type => "type alias",
                ImportItemNamespace::Dimension => "dimension alias",
                ImportItemNamespace::Index => "index alias",
                ImportItemNamespace::Term | ImportItemNamespace::Unit => "Static alias",
            },
            Namespace::Unit => "unit alias",
            Namespace::Term => "Term alias",
        };
        GraphcalError::BuiltinNameShadowed {
            kind,
            name: local_name.to_string(),
            src,
            span: import_item.local_span().into(),
        }
    })
}

/// Diagnose a selective item that names nothing importable in `expected`:
/// a category mismatch when `name` exists in another namespace, otherwise
/// an unknown name.
pub fn import_item_not_found_error(
    interface: &ModuleInterface,
    name: &NameAtom,
    expected: ImportItemNamespace,
    file_path: &str,
    src: SourceId,
    span: Span,
) -> GraphcalError {
    interface.namespaces_of(name).map_or_else(
        || GraphcalError::ImportNameNotFound {
            name: name.to_string(),
            file_path: file_path.to_string(),
            src,
            span: span.into(),
        },
        |alternatives| GraphcalError::ImportCategoryMismatch {
            file_path: file_path.to_string(),
            mismatch: ImportItemCategoryMismatch::new(name.clone(), expected, alternatives),
            src,
            span: span.into(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
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
            "pkg.lib",
            src(),
            Span::new(0, 3),
        ) {
            GraphcalError::ImportCategoryMismatch { mismatch, .. } => assert_eq!(
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
                "pkg.lib",
                src(),
                Span::new(0, 7),
            ),
            GraphcalError::ImportNameNotFound { .. }
        ));
    }
}
