//! The leading `pub` / `pub(bind)` visibility prefix of a declaration.
//!
//! The prefix is parsed before the declaration keyword, so the parser does not
//! yet know which visibility the declaration accepts. [`VisibilityPrefix`]
//! holds it until the declaration kind is known; each declaration kind then
//! consumes it through exactly one `accept_*` method, which returns the
//! visibility type that kind stores (or the kind's diagnostic). Declarations
//! are built with their final visibility and never mutated afterwards.

use crate::syntax::ast::{BindableVisibility, Visibility};
use crate::syntax::span::Span;
use crate::syntax::token::{ContextualKeyword, Token};

use super::super::{Expected, Found, ParseError, Parser};

/// A leading visibility keyword sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisibilityKeyword {
    /// `pub`
    Pub,
    /// `pub(bind)`
    PubBind,
}

impl VisibilityKeyword {
    const fn found(self) -> Found {
        match self {
            Self::Pub => Found::Pub,
            Self::PubBind => Found::PubBind,
        }
    }
}

/// A parsed visibility prefix that the declaration it precedes has not yet
/// accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use = "the declaration after a visibility prefix must accept or reject it"]
pub(super) struct VisibilityPrefix {
    keyword: Option<(VisibilityKeyword, Span)>,
}

impl VisibilityPrefix {
    /// Source span of the prefix, if one was written.
    pub(super) fn span(self) -> Option<Span> {
        self.keyword.map(|(_, span)| span)
    }

    const fn rejected(keyword: VisibilityKeyword, span: Span, expected: Expected) -> ParseError {
        Parser::unexpected_token(expected, keyword.found(), span)
    }

    /// For declarations that take no visibility (`param`, `include`).
    pub(super) fn accept_none(self, expected: Expected) -> Result<(), ParseError> {
        match self.keyword {
            None => Ok(()),
            Some((keyword, span)) => Err(Self::rejected(keyword, span, expected)),
        }
    }

    /// For declarations that may be exported but not bound; `pub(bind)` is
    /// rejected with `pub_bind_expected`.
    pub(super) fn accept_public(
        self,
        pub_bind_expected: Expected,
    ) -> Result<Visibility, ParseError> {
        match self.keyword {
            None => Ok(Visibility::Private),
            Some((VisibilityKeyword::Pub, _)) => Ok(Visibility::Public),
            Some((keyword @ VisibilityKeyword::PubBind, span)) => {
                Err(Self::rejected(keyword, span, pub_bind_expected))
            }
        }
    }

    /// For bindable declarations (`dim`, `type`, `index`), which accept every
    /// prefix.
    pub(super) const fn accept_bindable(self) -> BindableVisibility {
        match self.keyword {
            None => BindableVisibility::Private,
            Some((VisibilityKeyword::Pub, _)) => BindableVisibility::Public,
            Some((VisibilityKeyword::PubBind, _)) => BindableVisibility::PublicBind,
        }
    }

    /// For selective and plugin imports, which are always private: selective
    /// re-exports mark each item, and plugin aliases never leave their module.
    pub(super) const fn accept_non_dag_import(self) -> Result<(), ParseError> {
        match self.keyword {
            None => Ok(()),
            Some((VisibilityKeyword::Pub, span)) => Err(Parser::unexpected_token(
                Expected::PubWholeDagImport,
                Found::NonDagImport,
                span,
            )),
            Some((keyword @ VisibilityKeyword::PubBind, span)) => Err(Self::rejected(
                keyword,
                span,
                Expected::NonBindableVisibility,
            )),
        }
    }
}

impl Parser<'_> {
    /// Parse an optional `pub` / `pub(bind)` visibility prefix.
    ///
    /// `bind` is a contextual keyword: it is recognized only inside the
    /// parentheses, not reserved elsewhere.
    pub(super) fn parse_visibility_prefix(&mut self) -> Result<VisibilityPrefix, ParseError> {
        if self.lexer.peek() != Some(&Token::Pub) {
            return Ok(VisibilityPrefix { keyword: None });
        }
        let (_, pub_span) = self.advance()?;
        if self.lexer.peek() != Some(&Token::LParen) {
            return Ok(VisibilityPrefix {
                keyword: Some((VisibilityKeyword::Pub, pub_span)),
            });
        }
        self.expect(Token::LParen)?;
        let (bind_tok, bind_span) = self.advance()?;
        if bind_tok != Token::ContextualKeyword(ContextualKeyword::Bind) {
            return Err(Self::unexpected(
                Expected::Token(Token::ContextualKeyword(ContextualKeyword::Bind)),
                bind_tok,
                bind_span,
            ));
        }
        let (_, rparen_span) = self.expect(Token::RParen)?;
        Ok(VisibilityPrefix {
            keyword: Some((VisibilityKeyword::PubBind, pub_span.merge(rparen_span))),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parser::ParseErrorKind;

    fn prefix(source: &str) -> VisibilityPrefix {
        Parser::new(source)
            .parse_visibility_prefix()
            .expect("valid visibility prefix")
    }

    fn rejection(error: &ParseError) -> (&Expected, &Found) {
        match &error.kind {
            ParseErrorKind::UnexpectedToken { expected, found } => (expected, found),
            other => panic!("expected an unexpected-token error, got {other:?}"),
        }
    }

    #[test]
    fn prefixes_record_their_keyword_and_span() {
        assert_eq!(prefix("node").span(), None);
        assert_eq!(prefix("pub node").span(), Some(Span::new(0, 3)));
        assert_eq!(prefix("pub(bind) dim").span(), Some(Span::new(0, 9)));
    }

    #[test]
    fn bindable_declarations_accept_every_prefix() {
        assert_eq!(prefix("dim").accept_bindable(), BindableVisibility::Private);
        assert_eq!(
            prefix("pub dim").accept_bindable(),
            BindableVisibility::Public
        );
        assert_eq!(
            prefix("pub(bind) dim").accept_bindable(),
            BindableVisibility::PublicBind
        );
    }

    #[test]
    fn exportable_declarations_reject_only_pub_bind() {
        assert_eq!(
            prefix("unit")
                .accept_public(Expected::NonBindableVisibility)
                .ok(),
            Some(Visibility::Private)
        );
        assert_eq!(
            prefix("pub unit")
                .accept_public(Expected::NonBindableVisibility)
                .ok(),
            Some(Visibility::Public)
        );
        let error = prefix("pub(bind) unit")
            .accept_public(Expected::NonBindableVisibility)
            .expect_err("pub(bind) is not exportable-only");
        assert_eq!(
            rejection(&error),
            (&Expected::NonBindableVisibility, &Found::PubBind)
        );
        assert_eq!(error.span, Span::new(0, 9));
    }

    #[test]
    fn prefix_free_declarations_reject_every_prefix() {
        assert!(
            prefix("param")
                .accept_none(Expected::ParamWithoutVisibility)
                .is_ok()
        );
        for (source, found) in [
            ("pub param", Found::Pub),
            ("pub(bind) param", Found::PubBind),
        ] {
            let error = prefix(source)
                .accept_none(Expected::ParamWithoutVisibility)
                .expect_err("param takes no visibility");
            assert_eq!(
                rejection(&error),
                (&Expected::ParamWithoutVisibility, &found)
            );
        }
    }

    #[test]
    fn non_dag_imports_stay_private() {
        assert!(prefix("import").accept_non_dag_import().is_ok());
        let public = prefix("pub import")
            .accept_non_dag_import()
            .expect_err("a selective import is never public");
        assert_eq!(
            rejection(&public),
            (&Expected::PubWholeDagImport, &Found::NonDagImport)
        );
        let bindable = prefix("pub(bind) import")
            .accept_non_dag_import()
            .expect_err("imports are never bindable");
        assert_eq!(
            rejection(&bindable),
            (&Expected::NonBindableVisibility, &Found::PubBind)
        );
    }

    #[test]
    fn only_bind_may_appear_inside_the_parentheses() {
        let error = Parser::new("pub(crate) node")
            .parse_visibility_prefix()
            .expect_err("only `bind` is accepted");
        assert_eq!(
            rejection(&error),
            (
                &Expected::Token(Token::ContextualKeyword(ContextualKeyword::Bind)),
                &Found::Token(Token::Ident)
            )
        );
    }
}
