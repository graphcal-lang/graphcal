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
//!
//! Keyword vocabularies have one table each: hard keywords are the
//! `hard_keywords` section of [`Token`] (listed by [`Token::HARD_KEYWORDS`]) and
//! contextual keywords are [`ContextualKeyword`] (listed by
//! [`ContextualKeyword::ALL`]). Tests lex every table entry, so a spelling
//! missing from the Logos attributes cannot go unnoticed.

use logos::Logos;

#[derive(Logos, Debug, Clone, PartialEq)]
pub(crate) enum LexicalToken {
    // Trivia. The parser-facing lexer consumes these and exposes them through
    // typed source metadata instead of yielding them as syntax tokens.
    #[regex(r"[ \t\r\n]+")]
    Whitespace,
    #[regex(r"//[^\n\r]*", allow_greedy = true)]
    Comment,

    // Hard keywords: reserved spellings that never lex as identifiers. Each
    // spelling yields an entry of `Token::HARD_KEYWORDS`.
    #[token("param", |_| Token::Param)]
    #[token("node", |_| Token::Node)]
    #[token("const", |_| Token::Const)]
    #[token("if", |_| Token::If)]
    #[token("else", |_| Token::Else)]
    #[token("true", |_| Token::True)]
    #[token("false", |_| Token::False)]
    #[token("base", |_| Token::Base)]
    #[token("dim", |_| Token::Dimension)]
    #[token("unit", |_| Token::Unit)]
    #[token("type", |_| Token::Type)]
    #[token("index", |_| Token::Index)]
    #[token("for", |_| Token::For)]
    #[token("import", |_| Token::Import)]
    #[token("include", |_| Token::Include)]
    #[token("dag", |_| Token::Dag)]
    #[token("match", |_| Token::Match)]
    #[token("as", |_| Token::As)]
    #[token("assert", |_| Token::Assert)]
    #[token("table", |_| Token::Table)]
    #[token("plot", |_| Token::Plot)]
    #[token("figure", |_| Token::Figure)]
    #[token("layer", |_| Token::Layer)]
    #[token("pub", |_| Token::Pub)]
    HardKeyword(Token),

    // Contextual keywords: identifier spellings with a special meaning only in
    // selected productions.
    #[token("todo", |_| ContextualKeyword::Todo)]
    #[token("scan", |_| ContextualKeyword::Scan)]
    #[token("unfold", |_| ContextualKeyword::Unfold)]
    #[token("range", |_| ContextualKeyword::Range)]
    #[token("linspace", |_| ContextualKeyword::Linspace)]
    #[token("step", |_| ContextualKeyword::Step)]
    #[token("points", |_| ContextualKeyword::Points)]
    #[token("Fin", |_| ContextualKeyword::Fin)]
    #[token("key", |_| ContextualKeyword::Key)]
    #[token("fin_key", |_| ContextualKeyword::FinKey)]
    #[token("floor_key", |_| ContextualKeyword::FloorKey)]
    #[token("ceil_key", |_| ContextualKeyword::CeilKey)]
    #[token("nearest_key", |_| ContextualKeyword::NearestKey)]
    #[token("plugin", |_| ContextualKeyword::Plugin)]
    #[token("fn", |_| ContextualKeyword::Fn)]
    #[token("bind", |_| ContextualKeyword::Bind)]
    #[token("mark", |_| ContextualKeyword::Mark)]
    #[token("encode", |_| ContextualKeyword::Encode)]
    #[token("plots", |_| ContextualKeyword::Plots)]
    ContextualKeyword(ContextualKeyword),

    // Literals
    #[regex(r#""[^"\r\n]*""#)]
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
    #[token("::")]
    DoubleColon,
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
    #[regex(r"[0-9][0-9_]*(\.[0-9][0-9_]*)?([eE][+-]?[0-9][0-9_]*)?")]
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
            Self::HardKeyword(token) => LexicalItem::Syntax(token),
            Self::ContextualKeyword(keyword) => {
                LexicalItem::Syntax(Token::ContextualKeyword(keyword))
            }
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
            Self::DoubleColon => LexicalItem::Syntax(Token::DoubleColon),
            Self::Colon => LexicalItem::Syntax(Token::Colon),
            Self::Dot => LexicalItem::Syntax(Token::Dot),
            Self::Underscore => LexicalItem::Syntax(Token::Underscore),
            Self::Ident => LexicalItem::Syntax(Token::Ident),
            Self::Number => LexicalItem::Syntax(Token::Number),
        }
    }
}

/// Define [`ContextualKeyword`], its canonical spellings, and its `ALL` listing
/// from a single table so tests can iterate every entry.
macro_rules! define_contextual_keywords {
    ($($variant:ident => $spelling:literal),+ $(,)?) => {
        /// An identifier spelling that has keyword meaning only in a precise parser context.
        ///
        /// These spellings remain ordinary identifiers everywhere else. Keeping their
        /// lexical classification typed lets parser code select a special production
        /// without recovering semantics from source strings or repeating unions of
        /// otherwise unrelated token variants.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum ContextualKeyword {
            $($variant),+
        }

        impl ContextualKeyword {
            /// Every contextual keyword, in table order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// Canonical source spelling recognized by the lexer.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $spelling),+
                }
            }
        }
    };
}

define_contextual_keywords! {
    Todo => "todo",
    Scan => "scan",
    Unfold => "unfold",
    Range => "range",
    Linspace => "linspace",
    Step => "step",
    Points => "points",
    Fin => "Fin",
    Key => "key",
    FinKey => "fin_key",
    FloorKey => "floor_key",
    CeilKey => "ceil_key",
    NearestKey => "nearest_key",
    Plugin => "plugin",
    Fn => "fn",
    Bind => "bind",
    Mark => "mark",
    Encode => "encode",
    Plots => "plots",
}

impl ContextualKeyword {
    /// Classify an AST identifier spelling at a syntax boundary.
    #[must_use]
    pub fn matches(self, spelling: &str) -> bool {
        spelling == self.as_str()
    }
}

impl std::fmt::Display for ContextualKeyword {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when text is not one source-level Graphcal identifier.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourceIdentifierError {
    /// The spelling does not match the source `IDENT` lexical production.
    #[error("must start with an ASCII letter and contain only ASCII letters, digits, or `_`")]
    InvalidCharacters,
    /// Hard keywords are separate lexer tokens and cannot occupy identifier positions.
    #[error("is a reserved Graphcal keyword")]
    ReservedKeyword,
}

/// A spelling proven safe to render in a Graphcal `IDENT` position.
///
/// This is deliberately narrower than
/// [`NameAtom`](crate::syntax::names::NameAtom), because wire and generated
/// names may contain characters that the source lexer cannot accept. Source
/// generators should construct this type before writing identifier text.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceIdentifier(String);

impl SourceIdentifier {
    /// Validate one source identifier spelling.
    ///
    /// Asks the lexer instead of a hand-kept rule, so the accepted set is
    /// exactly the spellings that lex as one identifier token: hard keywords
    /// are rejected while contextual keywords remain valid identifiers.
    ///
    /// # Errors
    ///
    /// Returns [`SourceIdentifierError`] for non-`IDENT` text or a hard keyword.
    pub fn parse(spelling: impl Into<String>) -> Result<Self, SourceIdentifierError> {
        let spelling = spelling.into();
        let whole_token = {
            let mut lexer = LexicalToken::lexer(&spelling);
            match lexer.next() {
                Some(Ok(token)) if lexer.span() == (0..spelling.len()) => Some(token.classify()),
                _ => None,
            }
        };
        match whole_token {
            Some(LexicalItem::Syntax(token)) if token.is_identifier() => Ok(Self(spelling)),
            Some(LexicalItem::Syntax(token)) if Token::HARD_KEYWORDS.contains(&token) => {
                Err(SourceIdentifierError::ReservedKeyword)
            }
            Some(LexicalItem::Syntax(_) | LexicalItem::Trivia(_)) | None => {
                Err(SourceIdentifierError::InvalidCharacters)
            }
        }
    }

    /// Return the validated source spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SourceIdentifier {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Define [`Token`], its `Display` rendering, and the [`Token::HARD_KEYWORDS`]
/// listing from a single table. Every variant in the `hard_keywords` section
/// is a reserved spelling; tests lex each one to prove the lexer agrees.
macro_rules! define_tokens {
    (
        hard_keywords { $($keyword:ident => $keyword_text:literal),+ $(,)? }
        others { $($variant:ident => $text:literal),+ $(,)? }
    ) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Token {
            $($keyword,)+

            /// Identifier spellings with a special meaning in selected productions.
            ContextualKeyword(ContextualKeyword),

            $($variant,)+
        }

        impl Token {
            /// Every hard keyword, in table order.
            pub const HARD_KEYWORDS: &'static [Self] = &[$(Self::$keyword),+];
        }

        impl std::fmt::Display for Token {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    $(Self::$keyword => f.write_str($keyword_text),)+
                    Self::ContextualKeyword(keyword) => keyword.fmt(f),
                    $(Self::$variant => f.write_str($text),)+
                }
            }
        }
    };
}

define_tokens! {
    hard_keywords {
        Param => "param",
        Node => "node",
        Const => "const",
        If => "if",
        Else => "else",
        True => "true",
        False => "false",
        Base => "base",
        Dimension => "dim",
        Unit => "unit",
        Type => "type",
        Index => "index",
        For => "for",
        Import => "import",
        Include => "include",
        Dag => "dag",
        Match => "match",
        As => "as",
        Assert => "assert",
        Table => "table",
        Plot => "plot",
        Figure => "figure",
        Layer => "layer",
        Pub => "pub",
    }
    others {
        // Literals
        StringLiteral => "string",

        // Operators
        Plus => "+",
        Minus => "-",
        Star => "*",
        Slash => "/",
        Caret => "^",
        Percent => "%",
        Eq => "=",
        EqEq => "==",
        BangEq => "!=",
        Lt => "<",
        Gt => ">",
        LtEq => "<=",
        GtEq => ">=",
        AmpAmp => "&&",
        PipePipe => "||",
        Bang => "!",
        Arrow => "->",
        Pipe => "|",
        FatArrow => "=>",
        TildeEq => "~=",
        PlusMinus => "+/-",

        // Attribute prefix
        Hash => "#",

        // Delimiters
        LParen => "(",
        RParen => ")",
        LBrace => "{",
        RBrace => "}",
        LBracket => "[",
        RBracket => "]",
        Semicolon => ";",
        Comma => ",",
        At => "@",
        DoubleColon => "::",
        Colon => ":",
        Dot => ".",

        // Wildcard pattern
        Underscore => "_",

        // General identifier: covers lower_snake_case, UPPER_SNAKE_CASE, PascalCase, and mixed
        Ident => "identifier",

        // Numeric literal (with _ separators and scientific notation)
        Number => "number",
    }
}

impl Token {
    /// Keywords that introduce a top-level declaration (optionally after a
    /// `pub` / `pub(bind)` prefix), in editor-completion order.
    ///
    /// The parser test `declaration_keywords_match_parser_dispatch` ties this
    /// list to the declaration dispatch in `parser/decl/mod.rs`.
    pub const DECLARATION_KEYWORDS: &'static [Self] = &[
        Self::Param,
        Self::Node,
        Self::Const,
        Self::Base,
        Self::Type,
        Self::Dimension,
        Self::Unit,
        Self::Index,
        Self::Assert,
        Self::Dag,
        Self::Plot,
        Self::Figure,
        Self::Layer,
        Self::Import,
        Self::Include,
    ];

    /// Whether this token is accepted anywhere the grammar expects an identifier.
    #[must_use]
    pub const fn is_identifier(self) -> bool {
        matches!(self, Self::Ident | Self::ContextualKeyword(_))
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
    fn source_identifier_matches_identifier_token_policy() {
        let contextual = ContextualKeyword::ALL
            .iter()
            .map(|keyword| keyword.as_str());
        for valid in ["x", "SolveOrbitResult2", "nodes", "x_1"]
            .into_iter()
            .chain(contextual)
        {
            let identifier = SourceIdentifier::parse(valid).unwrap();
            assert_eq!(identifier.as_str(), valid);
            assert!(lex_tokens(valid)[0].is_identifier(), "{valid}");
        }
        for keyword in Token::HARD_KEYWORDS {
            let spelling = keyword.to_string();
            assert_eq!(
                SourceIdentifier::parse(spelling.as_str()).unwrap_err(),
                SourceIdentifierError::ReservedKeyword,
                "{spelling}"
            );
        }
        for invalid in [
            "",
            "_",
            "3d",
            "a-b",
            "a.b",
            "x;\nnode injected",
            " x",
            "x ",
            "x//c",
            "é",
            "node x",
        ] {
            assert_eq!(
                SourceIdentifier::parse(invalid).unwrap_err(),
                SourceIdentifierError::InvalidCharacters,
                "{invalid:?}"
            );
        }
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
    fn hard_keywords_are_not_identifiers() {
        for &keyword in Token::HARD_KEYWORDS {
            let spelling = keyword.to_string();
            assert_single_token(&spelling, keyword);
            assert!(!keyword.is_identifier(), "{spelling}");
            // Keyword spellings are whole-word matches.
            assert_single_token(&format!("{spelling}_x"), Token::Ident);
        }
    }

    #[test]
    fn contextual_keywords_are_identifiers() {
        for &keyword in ContextualKeyword::ALL {
            let token = Token::ContextualKeyword(keyword);
            assert_single_token(keyword.as_str(), token);
            assert!(token.is_identifier(), "{keyword}");
            // Contextual spellings are whole-word matches.
            assert_single_token(&format!("{keyword}_x"), Token::Ident);
        }
    }

    #[test]
    fn lex_identifier_starting_with_new_keywords() {
        // Contextual spellings remain whole-word matches.
        for word in [
            "baseline",
            "scanner",
            "unfolder",
            "ranged",
            "stepped",
            "pointset",
            "Finite",
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
        let tokens = lex_tokens("import helper::{G0, isp};");
        assert_eq!(
            tokens,
            vec![
                Token::Import,
                Token::Ident, // helper
                Token::DoubleColon,
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
    fn lex_same_line_unicode_string_literal() {
        // String literals survive in non-import contexts (e.g., plot labels).
        assert_single_token(r#""Δv 🚀""#, Token::StringLiteral);
    }

    #[test]
    fn physical_line_breaks_are_not_part_of_string_literals() {
        for line_ending in ["\n", "\r", "\r\n"] {
            let input = format!("\"first{line_ending}second\"");
            let mut lexer = crate::syntax::lexer::Lexer::new(&input);
            let mut tokens = Vec::new();
            while let Some((token, _)) = lexer.next_token() {
                tokens.push(token);
            }

            assert!(
                lexer.first_error_span().is_some(),
                "line ending {line_ending:?} should produce a lexical error"
            );
            assert!(
                !tokens.contains(&Token::StringLiteral),
                "line ending {line_ending:?} was swallowed by a string token"
            );
        }
    }

    #[test]
    fn lex_use_statement_with_alias() {
        let tokens = lex_tokens("import f::{x as y};");
        assert_eq!(
            tokens,
            vec![
                Token::Import,
                Token::Ident, // f
                Token::DoubleColon,
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
        let tokens = lex_tokens("import f::{type T, T};");
        assert_eq!(
            tokens,
            vec![
                Token::Import,
                Token::Ident, // f
                Token::DoubleColon,
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
