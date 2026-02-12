use logos::Logos;

#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(skip r"[ \t\r\n]+")]
#[logos(skip(r"//[^\n]*", allow_greedy = true))]
pub enum Token {
    // Keywords
    #[token("param")]
    Param,
    #[token("node")]
    Node,
    #[token("const")]
    Const,
    #[token("if")]
    If,
    #[token("else")]
    Else,
    #[token("true")]
    True,
    #[token("false")]
    False,

    // Operators
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("^")]
    Caret,
    #[token("=")]
    Eq,
    #[token("==")]
    EqEq,
    #[token("!=")]
    BangEq,
    #[token("<")]
    Lt,
    #[token(">")]
    Gt,
    #[token("<=")]
    LtEq,
    #[token(">=")]
    GtEq,
    #[token("&&")]
    AmpAmp,
    #[token("||")]
    PipePipe,
    #[token("!")]
    Bang,

    // Delimiters
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token(";")]
    Semicolon,
    #[token(",")]
    Comma,
    #[token("@")]
    At,

    // Identifiers
    #[regex(r"[a-z][a-z0-9_]*")]
    LowerIdent,
    #[regex(r"[A-Z][A-Z0-9_]*")]
    UpperIdent,

    // Numeric literal (with _ separators and scientific notation)
    #[regex(r"[0-9][0-9_]*(\.[0-9][0-9_]*)?([eE][+-]?[0-9]+)?")]
    Number,
}

impl std::fmt::Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Param => write!(f, "param"),
            Self::Node => write!(f, "node"),
            Self::Const => write!(f, "const"),
            Self::If => write!(f, "if"),
            Self::Else => write!(f, "else"),
            Self::True => write!(f, "true"),
            Self::False => write!(f, "false"),
            Self::Plus => write!(f, "+"),
            Self::Minus => write!(f, "-"),
            Self::Star => write!(f, "*"),
            Self::Slash => write!(f, "/"),
            Self::Caret => write!(f, "^"),
            Self::Eq => write!(f, "="),
            Self::EqEq => write!(f, "=="),
            Self::BangEq => write!(f, "!="),
            Self::Lt => write!(f, "<"),
            Self::Gt => write!(f, ">"),
            Self::LtEq => write!(f, "<="),
            Self::GtEq => write!(f, ">="),
            Self::AmpAmp => write!(f, "&&"),
            Self::PipePipe => write!(f, "||"),
            Self::Bang => write!(f, "!"),
            Self::LParen => write!(f, "("),
            Self::RParen => write!(f, ")"),
            Self::LBrace => write!(f, "{{"),
            Self::RBrace => write!(f, "}}"),
            Self::Semicolon => write!(f, ";"),
            Self::Comma => write!(f, ","),
            Self::At => write!(f, "@"),
            Self::LowerIdent => write!(f, "identifier"),
            Self::UpperIdent => write!(f, "IDENTIFIER"),
            Self::Number => write!(f, "number"),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn lex_tokens(input: &str) -> Vec<Token> {
        Token::lexer(input).map(|r| r.unwrap()).collect()
    }

    #[test]
    fn lex_param_decl() {
        let tokens = lex_tokens("param dry_mass = 1200.0;");
        assert_eq!(
            tokens,
            vec![
                Token::Param,
                Token::LowerIdent,
                Token::Eq,
                Token::Number,
                Token::Semicolon,
            ]
        );
    }

    #[test]
    fn lex_node_with_graph_ref() {
        let tokens = lex_tokens("node v_exhaust = @isp * G0;");
        assert_eq!(
            tokens,
            vec![
                Token::Node,
                Token::LowerIdent,
                Token::Eq,
                Token::At,
                Token::LowerIdent,
                Token::Star,
                Token::UpperIdent,
                Token::Semicolon,
            ]
        );
    }

    #[test]
    fn lex_const_decl() {
        let tokens = lex_tokens("const G0 = 9.80665;");
        assert_eq!(
            tokens,
            vec![
                Token::Const,
                Token::UpperIdent,
                Token::Eq,
                Token::Number,
                Token::Semicolon,
            ]
        );
    }

    #[test]
    fn lex_scientific_notation() {
        let mut lexer = Token::lexer("3.98e5");
        assert_eq!(lexer.next(), Some(Ok(Token::Number)));
        assert_eq!(lexer.slice(), "3.98e5");
    }

    #[test]
    fn lex_scientific_notation_negative_exponent() {
        let mut lexer = Token::lexer("1e-3");
        assert_eq!(lexer.next(), Some(Ok(Token::Number)));
        assert_eq!(lexer.slice(), "1e-3");
    }

    #[test]
    fn lex_underscore_separator() {
        let mut lexer = Token::lexer("200_000");
        assert_eq!(lexer.next(), Some(Ok(Token::Number)));
        assert_eq!(lexer.slice(), "200_000");
    }

    #[test]
    fn lex_underscore_separator_with_decimal() {
        let mut lexer = Token::lexer("1_000.5");
        assert_eq!(lexer.next(), Some(Ok(Token::Number)));
        assert_eq!(lexer.slice(), "1_000.5");
    }

    #[test]
    fn lex_integer() {
        let mut lexer = Token::lexer("42");
        assert_eq!(lexer.next(), Some(Ok(Token::Number)));
        assert_eq!(lexer.slice(), "42");
    }

    #[test]
    fn lex_line_comment_skipped() {
        let tokens = lex_tokens("// this is a comment\nparam x = 1.0;");
        assert_eq!(tokens[0], Token::Param);
    }

    #[test]
    fn lex_inline_comment_skipped() {
        let tokens = lex_tokens("param x = 1.0; // inline comment");
        assert_eq!(
            tokens,
            vec![
                Token::Param,
                Token::LowerIdent,
                Token::Eq,
                Token::Number,
                Token::Semicolon,
            ]
        );
    }

    #[test]
    fn lex_if_else() {
        let tokens = lex_tokens("if true { 1.0 } else { 2.0 }");
        assert_eq!(
            tokens,
            vec![
                Token::If,
                Token::True,
                Token::LBrace,
                Token::Number,
                Token::RBrace,
                Token::Else,
                Token::LBrace,
                Token::Number,
                Token::RBrace,
            ]
        );
    }

    #[test]
    fn lex_comparison_operators() {
        let tokens = lex_tokens("== != < > <= >=");
        assert_eq!(
            tokens,
            vec![
                Token::EqEq,
                Token::BangEq,
                Token::Lt,
                Token::Gt,
                Token::LtEq,
                Token::GtEq,
            ]
        );
    }

    #[test]
    fn lex_logical_operators() {
        let tokens = lex_tokens("&& || !");
        assert_eq!(tokens, vec![Token::AmpAmp, Token::PipePipe, Token::Bang,]);
    }

    #[test]
    fn lex_function_call() {
        let tokens = lex_tokens("sqrt(@x)");
        assert_eq!(
            tokens,
            vec![
                Token::LowerIdent,
                Token::LParen,
                Token::At,
                Token::LowerIdent,
                Token::RParen,
            ]
        );
    }

    #[test]
    fn lex_upper_ident_pi() {
        let mut lexer = Token::lexer("PI");
        assert_eq!(lexer.next(), Some(Ok(Token::UpperIdent)));
        assert_eq!(lexer.slice(), "PI");
    }

    #[test]
    fn lex_booleans() {
        let tokens = lex_tokens("true false");
        assert_eq!(tokens, vec![Token::True, Token::False]);
    }

    #[test]
    fn lex_keywords_not_identifiers() {
        // "param" should be Token::Param, not LowerIdent
        let tokens = lex_tokens("param node const if else");
        assert_eq!(
            tokens,
            vec![
                Token::Param,
                Token::Node,
                Token::Const,
                Token::If,
                Token::Else,
            ]
        );
    }

    #[test]
    fn lex_identifier_starting_with_keyword() {
        // "parameter" should be LowerIdent, not Param + "eter"
        let mut lexer = Token::lexer("parameter");
        assert_eq!(lexer.next(), Some(Ok(Token::LowerIdent)));
        assert_eq!(lexer.slice(), "parameter");
    }
}
