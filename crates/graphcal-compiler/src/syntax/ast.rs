use core::convert::Infallible;

use crate::syntax::phase::{Desugared, Phase, Raw};

mod common;
mod decl;
mod format_equivalent;
mod value;

pub use common::*;
pub use decl::*;
pub use format_equivalent::FormatEquivalent;
pub use value::*;

impl Phase for Raw {
    type DeclSugar = RawDeclSugar;
    type ExprSugar = RawExprSugar;
}

impl Phase for Desugared {
    type DeclSugar = Infallible;
    type ExprSugar = Infallible;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::names::DeclName;
    use crate::syntax::span::{Span, Spanned};

    #[test]
    fn construct_ast_by_hand() {
        let file: File<crate::syntax::phase::Desugared> = File {
            declarations: vec![Declaration {
                attributes: vec![],
                kind: DeclKind::Param(ParamDecl {
                    name: Spanned::new(DeclName::new("x"), Span::new(6, 1)),
                    type_ann: TypeExpr {
                        kind: TypeExprKind::Dimensionless,
                        constraints: vec![],
                        span: Span::new(9, 15),
                    },
                    value: Some(Expr::new(ExprKind::Number(1.0), Span::new(27, 3))),
                }),
                span: Span::new(0, 31),
            }],
        };
        assert_eq!(file.declarations.len(), 1);
    }
}
