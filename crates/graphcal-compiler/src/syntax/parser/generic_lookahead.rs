//! Token-level lookahead that tells a generic-argument list `<…>` apart from
//! a `<` comparison operator.
//!
//! `callee<T>(args)` and `todo<T> { … }` begin exactly like the comparison
//! `callee < T`. The parser commits to the generic reading only when the
//! tokens after `<` form a balanced generic-argument list whose closing `>` is
//! directly followed by the expected token. The scan never backtracks into
//! the parser, and it is bounded by [`MAX_NESTING_DEPTH`] open delimiters, so
//! a long run of comparisons costs time linear in its length.

use super::nesting_limit::MAX_NESTING_DEPTH;
use crate::syntax::token::Token;

/// A delimiter opened inside a generic-argument list.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Delimiter {
    /// `<…>`: generic arguments.
    Angle,
    /// `(…)`: a group, `Fin(…)`, or domain constraints.
    Paren,
    /// `[…]`: an index list.
    Bracket,
}

impl Delimiter {
    const fn opened_by(token: Token) -> Option<Self> {
        match token {
            Token::Lt => Some(Self::Angle),
            Token::LParen => Some(Self::Paren),
            Token::LBracket => Some(Self::Bracket),
            _ => None,
        }
    }

    const fn closed_by(token: Token) -> Option<Self> {
        match token {
            Token::Gt => Some(Self::Angle),
            Token::RParen => Some(Self::Paren),
            Token::RBracket => Some(Self::Bracket),
            _ => None,
        }
    }
}

/// Whether `token` may appear directly inside `<…>` or `[…]` of a type or Nat
/// expression. A `:` is excluded, so `low: a < b, high: a > (c)` is never
/// read as generic arguments.
const fn is_generic_arg_token(token: Token) -> bool {
    token.is_identifier()
        || matches!(
            token,
            Token::Number
                | Token::Dot
                | Token::DoubleColon
                | Token::Comma
                | Token::Star
                | Token::Slash
                | Token::Caret
                | Token::Plus
                | Token::Minus
                | Token::Hash
        )
}

/// Whether a token may appear inside `(…)` of a generic argument. Parentheses
/// can hold domain-constraint expressions (`Length(min: 0.0 m)`), so only
/// tokens that end a statement or open a block rule the generic reading out.
const fn is_parenthesized_arg_token(token: Token) -> bool {
    !matches!(token, Token::Semicolon | Token::LBrace | Token::RBrace)
}

/// Whether `after_lt`, the tokens following an opening `<`, form a balanced
/// generic-argument list whose closing `>` is directly followed by
/// `follower`.
pub(super) fn generic_args_close_before(
    mut after_lt: impl Iterator<Item = Token>,
    follower: Token,
) -> bool {
    let mut open = vec![Delimiter::Angle];
    while let Some(token) = after_lt.next() {
        if let Some(delimiter) = Delimiter::opened_by(token) {
            if open.len() >= MAX_NESTING_DEPTH {
                return false;
            }
            open.push(delimiter);
        } else if let Some(delimiter) = Delimiter::closed_by(token) {
            if open.pop() != Some(delimiter) {
                return false;
            }
            if open.is_empty() {
                return after_lt.next() == Some(follower);
            }
        } else {
            let allowed = match open.last() {
                Some(Delimiter::Paren) => is_parenthesized_arg_token(token),
                Some(Delimiter::Angle | Delimiter::Bracket) | None => is_generic_arg_token(token),
            };
            if !allowed {
                return false;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::lexer::tokenize;

    /// Run the lookahead on `source`, which must start with `<`.
    fn closes_before(source: &str, follower: Token) -> bool {
        let tokenized = tokenize(source);
        let mut tokens = tokenized.tokens().iter().map(|(token, _)| *token);
        assert_eq!(tokens.next(), Some(Token::Lt));
        generic_args_close_before(tokens, follower)
    }

    #[test]
    fn generic_argument_lists_are_recognized() {
        for source in [
            "<3>()",
            "<Length, 3>(x)",
            "<Vec<Length, Eci>>(x)",
            "<Length / Time^-2>(x)",
            "<Fin(N + 1)>(x)",
            "<Length[Phase, Fin(2)]>(x)",
            "<module.Index#Label>(x)",
            "<Length(min: 0.0 m)>(x)",
            "<m::Vec<Length>>(x)",
        ] {
            assert!(closes_before(source, Token::LParen), "{source}");
        }
        assert!(closes_before("<Length> { @a }", Token::LBrace));
    }

    #[test]
    fn comparisons_are_not_generic_arguments() {
        for source in [
            // FE-6 (#2250): the `:` of the next named argument ends the scan.
            "< b, high: a > (b - 0.5 m))",
            "< b && c > (d)",
            "< b; node c: Length = (d);",
            "< b",
            "< b > c",
            "< b) > (c)",
            "<b[c)>(d)",
        ] {
            assert!(!closes_before(source, Token::LParen), "{source}");
        }
        assert!(!closes_before("<Length>(x)", Token::LBrace));
    }

    #[test]
    fn comparison_runs_are_scanned_a_bounded_distance() {
        // FE-6 (#2250): each `<` of `f(PI < E, PI < E, …)` scanned to the end
        // of the line, so n comparisons cost O(n^2). The scan now stops once
        // the would-be generic nesting exceeds the parser's depth limit.
        let source = format!("<{});", vec![" E, PI <"; 10 * MAX_NESTING_DEPTH].join(""));
        let tokenized = tokenize(&source);
        let mut consumed = 0_usize;
        let tokens = tokenized.tokens()[1..].iter().map(|(token, _)| {
            consumed += 1;
            *token
        });
        assert!(!generic_args_close_before(tokens, Token::LParen));
        assert!(
            consumed <= 4 * MAX_NESTING_DEPTH,
            "scanned {consumed} of {} tokens",
            tokenized.tokens().len()
        );
    }

    #[test]
    fn nesting_beyond_the_parser_limit_is_not_generic() {
        let deep = format!(
            "{}{}(x)",
            "<A".repeat(MAX_NESTING_DEPTH + 1),
            ">".repeat(MAX_NESTING_DEPTH + 1)
        );
        assert!(!closes_before(&deep, Token::LParen));
        let shallow = format!(
            "{}{}(x)",
            "<A".repeat(MAX_NESTING_DEPTH),
            ">".repeat(MAX_NESTING_DEPTH)
        );
        assert!(closes_before(&shallow, Token::LParen));
    }
}
