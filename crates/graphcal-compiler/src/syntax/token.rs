//! Lexical token model.
//!
//! This module intentionally separates the tokens recognized by the concrete
//! lexer from the tokens exposed to the parser.
//!
//! - `LexicalToken` is the private Logos-facing token enum. It includes both
//!   syntax and trivia because Logos scans the full source text, including
//!   whitespace and comments.
//! - `TriviaToken` classifies non-syntax source regions. Trivia never reaches
//!   the parser; `lexer.rs` consumes it and records typed formatter metadata
//!   such as comments and blank lines.
//! - `LexicalItem` is the typed boundary between the raw Logos token and the
//!   parser-facing lexer. It forces every raw token to be classified as either
//!   syntax or trivia before the lexer decides whether to yield or record it.
//! - [`Token`] is the parser-facing syntax token enum. It deliberately cannot
//!   represent whitespace or comments, making trivia unrepresentable in parser
//!   code.

use logos::Logos;

#[derive(Logos, Debug, Clone, PartialEq)]
pub(crate) enum LexicalToken {
    // Trivia. The parser-facing lexer consumes these and exposes them through
    // typed source metadata instead of yielding them as syntax tokens.
    #[regex(r"[ \t\r\n]+")]
    Whitespace,
    #[regex(r"//[^\n\r]*", allow_greedy = true)]
    Comment,

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
    #[token("base")]
    Base,
    #[token("dim")]
    Dimension,
    #[token("unit")]
    Unit,
    #[token("type")]
    Type,
    #[token("index")]
    Index,
    #[token("for")]
    For,
    #[token("import")]
    Import,
    #[token("include")]
    Include,
    #[token("dag")]
    Dag,
    #[token("match")]
    Match,
    #[token("as")]
    As,
    #[token("assert")]
    Assert,
    #[token("table")]
    Table,
    #[token("plot")]
    Plot,
    #[token("figure")]
    Figure,
    #[token("layer")]
    Layer,
    #[token("scan")]
    Scan,
    #[token("unfold")]
    Unfold,
    #[token("linspace")]
    Linspace,
    #[token("step")]
    Step,
    #[token("pub")]
    Pub,

    // Literals
    #[regex(r#""[^"]*""#)]
    StringLiteral,

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
    #[token("%")]
    Percent,
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
    #[token("->")]
    Arrow,
    #[token("|")]
    Pipe,
    #[token("=>")]
    FatArrow,
    #[token("~=")]
    TildeEq,
    #[token("+/-")]
    PlusMinus,

    // Attribute prefix
    #[token("#")]
    Hash,

    // Delimiters
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token(";")]
    Semicolon,
    #[token(",")]
    Comma,
    #[token("@")]
    At,
    #[token(":")]
    Colon,
    #[token(".")]
    Dot,

    // Wildcard pattern
    #[token("_")]
    Underscore,

    // General identifier: covers lower_snake_case, UPPER_SNAKE_CASE, PascalCase, and mixed
    #[regex(r"[a-zA-Z][a-zA-Z0-9_]*")]
    Ident,

    // Numeric literal (with _ separators and scientific notation)
    #[regex(r"[0-9][0-9_]*(\.[0-9][0-9_]*)?([eE][+-]?[0-9]+)?")]
    Number,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TriviaToken {
    Whitespace,
    Comment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LexicalItem {
    Trivia(TriviaToken),
    Syntax(Token),
}

impl LexicalToken {
    #[must_use]
    pub(crate) const fn classify(self) -> LexicalItem {
        match self {
            Self::Whitespace => LexicalItem::Trivia(TriviaToken::Whitespace),
            Self::Comment => LexicalItem::Trivia(TriviaToken::Comment),
            Self::Param => LexicalItem::Syntax(Token::Param),
            Self::Node => LexicalItem::Syntax(Token::Node),
            Self::Const => LexicalItem::Syntax(Token::Const),
            Self::If => LexicalItem::Syntax(Token::If),
            Self::Else => LexicalItem::Syntax(Token::Else),
            Self::True => LexicalItem::Syntax(Token::True),
            Self::False => LexicalItem::Syntax(Token::False),
            Self::Base => LexicalItem::Syntax(Token::Base),
            Self::Dimension => LexicalItem::Syntax(Token::Dimension),
            Self::Unit => LexicalItem::Syntax(Token::Unit),
            Self::Type => LexicalItem::Syntax(Token::Type),
            Self::Index => LexicalItem::Syntax(Token::Index),
            Self::For => LexicalItem::Syntax(Token::For),
            Self::Import => LexicalItem::Syntax(Token::Import),
            Self::Include => LexicalItem::Syntax(Token::Include),
            Self::Dag => LexicalItem::Syntax(Token::Dag),
            Self::Match => LexicalItem::Syntax(Token::Match),
            Self::As => LexicalItem::Syntax(Token::As),
            Self::Assert => LexicalItem::Syntax(Token::Assert),
            Self::Table => LexicalItem::Syntax(Token::Table),
            Self::Plot => LexicalItem::Syntax(Token::Plot),
            Self::Figure => LexicalItem::Syntax(Token::Figure),
            Self::Layer => LexicalItem::Syntax(Token::Layer),
            Self::Scan => LexicalItem::Syntax(Token::Scan),
            Self::Unfold => LexicalItem::Syntax(Token::Unfold),
            Self::Linspace => LexicalItem::Syntax(Token::Linspace),
            Self::Step => LexicalItem::Syntax(Token::Step),
            Self::Pub => LexicalItem::Syntax(Token::Pub),
            Self::StringLiteral => LexicalItem::Syntax(Token::StringLiteral),
            Self::Plus => LexicalItem::Syntax(Token::Plus),
            Self::Minus => LexicalItem::Syntax(Token::Minus),
            Self::Star => LexicalItem::Syntax(Token::Star),
            Self::Slash => LexicalItem::Syntax(Token::Slash),
            Self::Caret => LexicalItem::Syntax(Token::Caret),
            Self::Percent => LexicalItem::Syntax(Token::Percent),
            Self::Eq => LexicalItem::Syntax(Token::Eq),
            Self::EqEq => LexicalItem::Syntax(Token::EqEq),
            Self::BangEq => LexicalItem::Syntax(Token::BangEq),
            Self::Lt => LexicalItem::Syntax(Token::Lt),
            Self::Gt => LexicalItem::Syntax(Token::Gt),
            Self::LtEq => LexicalItem::Syntax(Token::LtEq),
            Self::GtEq => LexicalItem::Syntax(Token::GtEq),
            Self::AmpAmp => LexicalItem::Syntax(Token::AmpAmp),
            Self::PipePipe => LexicalItem::Syntax(Token::PipePipe),
            Self::Bang => LexicalItem::Syntax(Token::Bang),
            Self::Arrow => LexicalItem::Syntax(Token::Arrow),
            Self::Pipe => LexicalItem::Syntax(Token::Pipe),
            Self::FatArrow => LexicalItem::Syntax(Token::FatArrow),
            Self::TildeEq => LexicalItem::Syntax(Token::TildeEq),
            Self::PlusMinus => LexicalItem::Syntax(Token::PlusMinus),
            Self::Hash => LexicalItem::Syntax(Token::Hash),
            Self::LParen => LexicalItem::Syntax(Token::LParen),
            Self::RParen => LexicalItem::Syntax(Token::RParen),
            Self::LBrace => LexicalItem::Syntax(Token::LBrace),
            Self::RBrace => LexicalItem::Syntax(Token::RBrace),
            Self::LBracket => LexicalItem::Syntax(Token::LBracket),
            Self::RBracket => LexicalItem::Syntax(Token::RBracket),
            Self::Semicolon => LexicalItem::Syntax(Token::Semicolon),
            Self::Comma => LexicalItem::Syntax(Token::Comma),
            Self::At => LexicalItem::Syntax(Token::At),
            Self::Colon => LexicalItem::Syntax(Token::Colon),
            Self::Dot => LexicalItem::Syntax(Token::Dot),
            Self::Underscore => LexicalItem::Syntax(Token::Underscore),
            Self::Ident => LexicalItem::Syntax(Token::Ident),
            Self::Number => LexicalItem::Syntax(Token::Number),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    // Keywords
    Param,
    Node,
    Const,
    If,
    Else,
    True,
    False,
    Base,
    Dimension,
    Unit,
    Type,
    Index,
    For,
    Import,
    Include,
    Dag,
    Match,
    As,
    Assert,
    Table,
    Plot,
    Figure,
    Layer,
    Scan,
    Unfold,
    Linspace,
    Step,
    Pub,

    // Literals
    StringLiteral,

    // Operators
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    Percent,
    Eq,
    EqEq,
    BangEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
    AmpAmp,
    PipePipe,
    Bang,
    Arrow,
    Pipe,
    FatArrow,
    TildeEq,
    PlusMinus,

    // Attribute prefix
    Hash,

    // Delimiters
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Semicolon,
    Comma,
    At,
    Colon,
    Dot,

    // Wildcard pattern
    Underscore,

    // General identifier: covers lower_snake_case, UPPER_SNAKE_CASE, PascalCase, and mixed
    Ident,

    // Numeric literal (with _ separators and scientific notation)
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
            Self::Base => write!(f, "base"),
            Self::Dimension => write!(f, "dim"),
            Self::Unit => write!(f, "unit"),
            Self::Type => write!(f, "type"),
            Self::Index => write!(f, "index"),
            Self::For => write!(f, "for"),
            Self::Import => write!(f, "import"),
            Self::Include => write!(f, "include"),
            Self::Dag => write!(f, "dag"),
            Self::Match => write!(f, "match"),
            Self::As => write!(f, "as"),
            Self::Assert => write!(f, "assert"),
            Self::Table => write!(f, "table"),
            Self::Plot => write!(f, "plot"),
            Self::Figure => write!(f, "figure"),
            Self::Layer => write!(f, "layer"),
            Self::Scan => write!(f, "scan"),
            Self::Unfold => write!(f, "unfold"),
            Self::Linspace => write!(f, "linspace"),
            Self::Step => write!(f, "step"),
            Self::Pub => write!(f, "pub"),
            Self::StringLiteral => write!(f, "string"),
            Self::Plus => write!(f, "+"),
            Self::Minus => write!(f, "-"),
            Self::Star => write!(f, "*"),
            Self::Slash => write!(f, "/"),
            Self::Caret => write!(f, "^"),
            Self::Percent => write!(f, "%"),
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
            Self::Arrow => write!(f, "->"),
            Self::Pipe => write!(f, "|"),
            Self::FatArrow => write!(f, "=>"),
            Self::TildeEq => write!(f, "~="),
            Self::PlusMinus => write!(f, "+/-"),
            Self::Hash => write!(f, "#"),
            Self::LParen => write!(f, "("),
            Self::RParen => write!(f, ")"),
            Self::LBrace => write!(f, "{{"),
            Self::RBrace => write!(f, "}}"),
            Self::LBracket => write!(f, "["),
            Self::RBracket => write!(f, "]"),
            Self::Semicolon => write!(f, ";"),
            Self::Comma => write!(f, ","),
            Self::At => write!(f, "@"),
            Self::Colon => write!(f, ":"),
            Self::Dot => write!(f, "."),
            Self::Underscore => write!(f, "_"),
            Self::Ident => write!(f, "identifier"),
            Self::Number => write!(f, "number"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex_tokens(input: &str) -> Vec<Token> {
        let mut lexer = crate::syntax::lexer::Lexer::new(input);
        let mut tokens = Vec::new();
        while let Some((token, _)) = lexer.next_token() {
            tokens.push(token);
        }
        tokens
    }

    fn assert_single_token(input: &str, expected: Token) {
        let mut lexer = crate::syntax::lexer::Lexer::new(input);
        let Some((token, span)) = lexer.next_token() else {
            panic!("expected one token");
        };
        assert_eq!(token, expected);
        assert_eq!(lexer.slice_at(span), input);
        assert_eq!(lexer.next_token(), None);
    }

    #[test]
    fn lex_param_decl() {
        let tokens = lex_tokens("param dry_mass = 1200.0;");
        assert_eq!(
            tokens,
            vec![
                Token::Param,
                Token::Ident,
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
                Token::Ident,
                Token::Eq,
                Token::At,
                Token::Ident,
                Token::Star,
                Token::Ident,
                Token::Semicolon,
            ]
        );
    }

    #[test]
    fn lex_const_decl() {
        let tokens = lex_tokens("const node g0 = 9.80665;");
        assert_eq!(
            tokens,
            vec![
                Token::Const,
                Token::Node,
                Token::Ident,
                Token::Eq,
                Token::Number,
                Token::Semicolon,
            ]
        );
    }

    #[test]
    fn lex_scientific_notation() {
        assert_single_token("3.98e5", Token::Number);
    }

    #[test]
    fn lex_scientific_notation_negative_exponent() {
        assert_single_token("1e-3", Token::Number);
    }

    #[test]
    fn lex_underscore_separator() {
        assert_single_token("200_000", Token::Number);
    }

    #[test]
    fn lex_underscore_separator_with_decimal() {
        assert_single_token("1_000.5", Token::Number);
    }

    #[test]
    fn lex_integer() {
        assert_single_token("42", Token::Number);
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
                Token::Ident,
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
    fn lex_attribute() {
        let tokens = lex_tokens("#[lazy]");
        assert_eq!(
            tokens,
            vec![Token::Hash, Token::LBracket, Token::Ident, Token::RBracket,]
        );
    }

    #[test]
    fn lex_attribute_with_args() {
        let tokens = lex_tokens("#[assumes(x, y)]");
        assert_eq!(
            tokens,
            vec![
                Token::Hash,
                Token::LBracket,
                Token::Ident,
                Token::LParen,
                Token::Ident,
                Token::Comma,
                Token::Ident,
                Token::RParen,
                Token::RBracket,
            ]
        );
    }

    #[test]
    fn lex_function_call() {
        let tokens = lex_tokens("sqrt(@x)");
        assert_eq!(
            tokens,
            vec![
                Token::Ident,
                Token::LParen,
                Token::At,
                Token::Ident,
                Token::RParen,
            ]
        );
    }

    #[test]
    fn lex_upper_ident_pi() {
        assert_single_token("PI", Token::Ident);
    }

    #[test]
    fn lex_booleans() {
        let tokens = lex_tokens("true false");
        assert_eq!(tokens, vec![Token::True, Token::False]);
    }

    #[test]
    fn lex_keywords_not_identifiers() {
        // "param" should be Token::Param, not Ident
        let tokens = lex_tokens(
            "param node const if else base dim unit type index for import include dag match as assert table plot figure scan unfold linspace step pub",
        );
        assert_eq!(
            tokens,
            vec![
                Token::Param,
                Token::Node,
                Token::Const,
                Token::If,
                Token::Else,
                Token::Base,
                Token::Dimension,
                Token::Unit,
                Token::Type,
                Token::Index,
                Token::For,
                Token::Import,
                Token::Include,
                Token::Dag,
                Token::Match,
                Token::As,
                Token::Assert,
                Token::Table,
                Token::Plot,
                Token::Figure,
                Token::Scan,
                Token::Unfold,
                Token::Linspace,
                Token::Step,
                Token::Pub,
            ]
        );
    }

    #[test]
    fn lex_identifier_starting_with_new_keywords() {
        // "scanner" should be Ident, not Scan + "ner"
        for word in [
            "baseline",
            "scanner",
            "unfolder",
            "stepped",
            "indexed",
            "indexing",
            "linspaced",
            "tableau",
            "parameter",
            "typedef",
            "importable",
            "dagger",
            "public",
            "included",
        ] {
            assert_single_token(word, Token::Ident);
        }
    }

    #[test]
    fn lex_pascal_case_identifiers() {
        let tokens = lex_tokens("Length Time Mass Velocity Dimensionless");
        assert_eq!(
            tokens,
            vec![
                Token::Ident,
                Token::Ident,
                Token::Ident,
                Token::Ident,
                Token::Ident,
            ]
        );
    }

    #[test]
    fn lex_mixed_case_unit_identifiers() {
        // Pa, Hz, kN, kPa, MPa -- all should lex as single Ident tokens
        let tokens = lex_tokens("Pa Hz kN kPa MPa");
        assert_eq!(
            tokens,
            vec![
                Token::Ident,
                Token::Ident,
                Token::Ident,
                Token::Ident,
                Token::Ident,
            ]
        );
    }

    #[test]
    fn lex_colon() {
        let tokens = lex_tokens("param alt: Length = 400 km;");
        assert_eq!(
            tokens,
            vec![
                Token::Param,
                Token::Ident,
                Token::Colon,
                Token::Ident,
                Token::Eq,
                Token::Number,
                Token::Ident,
                Token::Semicolon,
            ]
        );
    }

    #[test]
    fn lex_arrow() {
        let tokens = lex_tokens("@speed -> km");
        assert_eq!(
            tokens,
            vec![Token::At, Token::Ident, Token::Arrow, Token::Ident,]
        );
    }

    #[test]
    fn lex_dimension_decl() {
        let tokens = lex_tokens("dim Velocity = Length / Time;");
        assert_eq!(
            tokens,
            vec![
                Token::Dimension,
                Token::Ident,
                Token::Eq,
                Token::Ident,
                Token::Slash,
                Token::Ident,
                Token::Semicolon,
            ]
        );
    }

    #[test]
    fn lex_unit_decl() {
        let tokens = lex_tokens("unit km: Length = 1000 m;");
        assert_eq!(
            tokens,
            vec![
                Token::Unit,
                Token::Ident,
                Token::Colon,
                Token::Ident,
                Token::Eq,
                Token::Number,
                Token::Ident,
                Token::Semicolon,
            ]
        );
    }

    #[test]
    fn lex_type_decl() {
        let tokens =
            lex_tokens("type TransferResult { TransferResult(dv1: Velocity, dv2: Velocity) }");
        assert_eq!(
            tokens,
            vec![
                Token::Type,   // type
                Token::Ident,  // TransferResult
                Token::LBrace, // {
                Token::Ident,  // TransferResult
                Token::LParen, // (
                Token::Ident,  // dv1
                Token::Colon,  // :
                Token::Ident,  // Velocity
                Token::Comma,  // ,
                Token::Ident,  // dv2
                Token::Colon,  // :
                Token::Ident,  // Velocity
                Token::RParen, // )
                Token::RBrace, // }
            ]
        );
    }

    #[test]
    fn lex_dot_field_access() {
        let tokens = lex_tokens("@transfer.dv1");
        assert_eq!(
            tokens,
            vec![Token::At, Token::Ident, Token::Dot, Token::Ident,]
        );
    }

    #[test]
    fn lex_import_statement() {
        let tokens = lex_tokens("import helper.{G0, isp};");
        assert_eq!(
            tokens,
            vec![
                Token::Import,
                Token::Ident, // helper
                Token::Dot,
                Token::LBrace,
                Token::Ident, // G0
                Token::Comma,
                Token::Ident, // isp
                Token::RBrace,
                Token::Semicolon,
            ]
        );
    }

    #[test]
    fn lex_string_literal() {
        // String literals survive in non-import contexts (e.g., timezone names).
        assert_single_token(r#""UTC""#, Token::StringLiteral);
    }

    #[test]
    fn lex_use_statement_with_alias() {
        let tokens = lex_tokens("import f.{x as y};");
        assert_eq!(
            tokens,
            vec![
                Token::Import,
                Token::Ident, // f
                Token::Dot,
                Token::LBrace,
                Token::Ident, // x
                Token::As,
                Token::Ident, // y
                Token::RBrace,
                Token::Semicolon,
            ]
        );
    }

    #[test]
    fn lex_dag_keyword() {
        let tokens = lex_tokens("dag my_pipeline {}");
        assert_eq!(
            tokens,
            vec![Token::Dag, Token::Ident, Token::LBrace, Token::RBrace,]
        );
    }

    #[test]
    fn lex_import_type() {
        let tokens = lex_tokens("import f.{type T, T};");
        assert_eq!(
            tokens,
            vec![
                Token::Import,
                Token::Ident, // f
                Token::Dot,
                Token::LBrace,
                Token::Type,
                Token::Ident, // T
                Token::Comma,
                Token::Ident,
                Token::RBrace,
                Token::Semicolon,
            ]
        );
    }
}
