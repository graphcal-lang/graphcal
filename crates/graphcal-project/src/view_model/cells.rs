//! The literal cells of a declaration's definition: where each entry of a
//! table is written, keyed the way its value is keyed.
//!
//! A `table[…] { … }` literal and each column of a multi-declaration table
//! desugar to a map literal whose entries keep the source span of the cell
//! they came from, so a cell can be edited by replacing exactly that span.

use graphcal_compiler::desugar::desugared_ast::{DeclKind, Declaration, Expr, ExprKind};
use graphcal_compiler::syntax::ast::MapEntryKey;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::index_name::IndexEntryKey;
use graphcal_compiler::syntax::span::Span;

/// One written cell of a map-literal definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DefinitionCell {
    /// The entry's key on each axis, outermost first.
    pub(super) keys: Vec<IndexEntryKey>,
    /// The span of the cell's expression.
    pub(super) span: Span,
}

/// The cells of the definition of `name` among `declarations`: one per
/// entry of a map literal. Empty when `name` is not declared there or its
/// definition is not a map literal.
pub(super) fn definition_cells(
    declarations: &[Declaration],
    name: &DeclName,
) -> Vec<DefinitionCell> {
    declarations
        .iter()
        .find_map(|declaration| defining_expr(&declaration.kind, name))
        .map_or_else(Vec::new, map_cells)
}

/// The defining expression of the value declaration `name`, if `kind` is it.
fn defining_expr<'a>(kind: &'a DeclKind, name: &DeclName) -> Option<&'a Expr> {
    match kind {
        DeclKind::Param(param) if param.name.value == *name => param.value.as_ref(),
        DeclKind::ConstNode(constant) if constant.name.value == *name => Some(&constant.value),
        DeclKind::Node(node) if node.name.value == *name => Some(&node.value),
        _ => None,
    }
}

fn map_cells(expr: &Expr) -> Vec<DefinitionCell> {
    let ExprKind::MapLiteral { entries } = &expr.kind else {
        return Vec::new();
    };
    entries
        .iter()
        .map(|entry| DefinitionCell {
            keys: entry.keys.iter().map(MapEntryKey::entry_key).collect(),
            span: entry.value.span,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use graphcal_compiler::desugar::desugared_ast::File;
    use graphcal_compiler::syntax::parser::Parser;

    use super::*;

    fn cells_of(source: &str, name: &str) -> Vec<(Vec<String>, String)> {
        let file = File::from(Parser::new(source).parse_file().unwrap());
        definition_cells(&file.declarations, &DeclName::expect_valid(name))
            .into_iter()
            .map(|cell| {
                (
                    cell.keys.iter().map(ToString::to_string).collect(),
                    source[cell.span.offset()..cell.span.offset().saturating_add(cell.span.len())]
                        .to_string(),
                )
            })
            .collect()
    }

    fn cell(keys: &[&str], text: &str) -> (Vec<String>, String) {
        (
            keys.iter().map(ToString::to_string).collect(),
            text.to_string(),
        )
    }

    #[test]
    fn a_table_lists_its_cells_with_their_spans() {
        let source = "\
index I = { A, B };
param p: Dimensionless[I] = table[I] {
    A: atan2(1.0, 2.0);
    B: 2.0;
};
node n: Dimensionless = 1.0;
";
        assert_eq!(
            cells_of(source, "p"),
            [cell(&["A"], "atan2(1.0, 2.0)"), cell(&["B"], "2.0")]
        );
        assert_eq!(cells_of(source, "n"), []);
        assert_eq!(cells_of(source, "missing"), []);
    }

    #[test]
    fn each_column_of_a_multi_declaration_has_its_own_cells() {
        let source = "\
index I = { A, B };
param x: Dimensionless[I],
node y: Dimensionless[I]
  = table[I, (_, _)] {
      : _, _;
      A: 1.0, max(2.0, 3.0);
      B: 4.0, 5.0;
  };
";
        assert_eq!(
            cells_of(source, "x"),
            [cell(&["A"], "1.0"), cell(&["B"], "4.0")]
        );
        assert_eq!(
            cells_of(source, "y"),
            [cell(&["A"], "max(2.0, 3.0)"), cell(&["B"], "5.0")]
        );
    }

    #[test]
    fn a_fin_table_keys_its_cells_by_position() {
        let source = "param v: Dimensionless[Fin(2)] = table[Fin(2)] { 1.0; 2.0; };\n";
        assert_eq!(
            cells_of(source, "v"),
            [cell(&["#0"], "1.0"), cell(&["#1"], "2.0")]
        );
    }
}
