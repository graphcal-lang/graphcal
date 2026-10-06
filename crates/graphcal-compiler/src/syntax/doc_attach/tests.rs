use crate::syntax::parser::Parser;

fn parse(source: &str) -> crate::syntax::ast::File {
    Parser::new(source).parse_file().unwrap()
}

fn doc_of(file: &crate::syntax::ast::File, index: usize) -> Option<&str> {
    file.declarations[index]
        .doc
        .as_ref()
        .map(crate::syntax::comments::DocComment::text)
}

#[test]
fn doc_comment_attaches_to_following_declaration() {
    let file = parse("/// The mass budget.\nparam x: Dimensionless = 1.0;");
    assert_eq!(doc_of(&file, 0), Some("The mass budget."));
}

#[test]
fn multi_line_doc_block_joins_lines() {
    let file = parse("/// First line.\n/// Second line.\nnode y: Dimensionless = 1.0;");
    assert_eq!(doc_of(&file, 0), Some("First line.\nSecond line."));
}

#[test]
fn blank_line_detaches_doc_comment() {
    let file = parse("/// Dangling.\n\nparam x: Dimensionless = 1.0;");
    assert_eq!(doc_of(&file, 0), None);
}

#[test]
fn line_comment_is_not_documentation() {
    let file = parse("// plain comment\nparam x: Dimensionless = 1.0;");
    assert_eq!(doc_of(&file, 0), None);
}

#[test]
fn four_slashes_is_not_documentation() {
    let file = parse("//// separator\nparam x: Dimensionless = 1.0;");
    assert_eq!(doc_of(&file, 0), None);
}

#[test]
fn line_comment_breaks_a_doc_run() {
    let file = parse("/// Lost.\n// interrupt\nparam x: Dimensionless = 1.0;");
    assert_eq!(doc_of(&file, 0), None);
}

#[test]
fn trailing_comment_of_previous_declaration_does_not_attach() {
    let file = parse("param x: Dimensionless = 1.0; /// trailing\nparam y: Dimensionless = 2.0;");
    assert_eq!(doc_of(&file, 0), None);
    assert_eq!(doc_of(&file, 1), None);
}

#[test]
fn doc_attaches_through_attributes() {
    let file = parse(
        "/// Hidden helper plot.\n#[hidden]\nplot p = { mark: line, encode: { x: 1.0, y: 1.0 } };",
    );
    assert_eq!(doc_of(&file, 0), Some("Hidden helper plot."));
}

#[test]
fn each_declaration_gets_its_own_doc() {
    let file = parse(
        "/// First.\nparam x: Dimensionless = 1.0;\n/// Second.\nparam y: Dimensionless = 2.0;",
    );
    assert_eq!(doc_of(&file, 0), Some("First."));
    assert_eq!(doc_of(&file, 1), Some("Second."));
}

#[test]
fn docs_attach_inside_dag_bodies() {
    let file = parse("dag d {\n    /// Inner doc.\n    param x: Dimensionless = 1.0;\n}\n");
    let crate::syntax::ast::DeclKind::Dag(dag) = &file.declarations[0].kind else {
        panic!("expected dag declaration");
    };
    assert_eq!(
        dag.body[0]
            .doc
            .as_ref()
            .map(crate::syntax::comments::DocComment::text),
        Some("Inner doc.")
    );
}

#[test]
fn multi_decl_slots_keep_individual_docs_and_spans_through_desugaring() {
    use crate::syntax::ast::{DeclKind, File, RawDeclSugar};
    use crate::syntax::phase::Desugared;

    let source = "\
/// First slot.
pub node first: Int[Fin(1)],
/// Second slot.
/// Another line.
const node SECOND: Int[Fin(1)],
param third: Int[Fin(1)]
= table[Fin(1), (_, _, _)] { : _, _, _; 1, 2, 3; };
";
    for source in [source.to_owned(), format!("dag d {{\n{source}\n}}")] {
        let file = parse(&source);
        let declarations = match &file.declarations[0].kind {
            DeclKind::Dag(dag) => &dag.body,
            _ => &file.declarations,
        };
        // There is no whole-multi-declaration doc to accidentally inherit.
        assert!(declarations[0].doc.is_none());
        let DeclKind::Sugar(RawDeclSugar::Multi(multi)) = &declarations[0].kind else {
            panic!("expected multi-declaration");
        };
        let expected = [
            Some("First slot."),
            Some("Second slot.\nAnother line."),
            None,
        ];
        for (slot, expected) in multi.slots().iter().zip(expected) {
            assert_eq!(slot.doc.as_ref().map(|doc| doc.text()), expected);
            if let Some(doc) = &slot.doc {
                let text = &source[doc.span().offset()..doc.span().offset() + doc.span().len()];
                assert!(text.starts_with("/// "));
                assert_eq!(text.lines().count(), doc.text().lines().count());
            }
        }

        let desugared: File<Desugared> = file.into();
        let declarations = match &desugared.declarations[0].kind {
            DeclKind::Dag(dag) => &dag.body,
            _ => &desugared.declarations,
        };
        assert_eq!(
            declarations
                .iter()
                .map(|decl| decl.doc.as_ref().map(|doc| doc.text()))
                .collect::<Vec<_>>(),
            expected
        );
    }
}

#[test]
fn later_slot_docs_follow_the_same_adjacency_rules_as_ordinary_declarations() {
    use crate::syntax::ast::{DeclKind, RawDeclSugar};

    for (comment, expected) in [
        ("/// Later.\n", Some("Later.")),
        ("/// Later.\r\n", Some("Later.")),
        ("/// Detached.\n\n", None),
        ("/// Interrupted.\n// ordinary\n", None),
        (
            "/// First line.\n/// Second line.\n",
            Some("First line.\nSecond line."),
        ),
        ("/// trailing\n", None),
    ] {
        let gap = if comment == "/// trailing\n" {
            " "
        } else {
            "\n"
        };
        let source = format!(
            "param first: Int[Fin(1)],{gap}{comment}pub node later: Int[Fin(1)] = table[Fin(1), (_, _)] {{ : _, _; 1, 2; }};"
        );
        let file = parse(&source);
        let DeclKind::Sugar(RawDeclSugar::Multi(multi)) = &file.declarations[0].kind else {
            panic!("expected multi-declaration");
        };
        assert_eq!(
            multi.slots()[1].doc.as_ref().map(|doc| doc.text()),
            expected
        );
    }
}

#[test]
fn doc_span_covers_the_whole_block() {
    let source = "/// a\n/// b\nparam x: Dimensionless = 1.0;";
    let file = parse(source);
    let doc = file.declarations[0].doc.as_ref().unwrap();
    assert_eq!(doc.span().offset(), 0);
    assert_eq!(doc.span().len(), "/// a\n/// b".len());
}

#[test]
fn indented_doc_comment_still_attaches() {
    let file = parse("    /// Indented.\n    param x: Dimensionless = 1.0;");
    assert_eq!(doc_of(&file, 0), Some("Indented."));
}

#[test]
fn crlf_line_endings_attach() {
    let file = parse("/// Windows.\r\nparam x: Dimensionless = 1.0;\r\n");
    assert_eq!(doc_of(&file, 0), Some("Windows."));
}
