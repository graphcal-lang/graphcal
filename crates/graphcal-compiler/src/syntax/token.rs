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
//! [`ContextualKeyword::ALL`]). Both tables live in one `define_lexicon!`
//! invocation, which also generates the Logos `#[token]` attributes for every
//! keyword, so each spelling is written exactly once.

use logos::Logos;

use crate::syntax::names::NameAtom;

/// Define the lexer's [`LexicalToken`], the parser-facing [`Token`] (with its
/// `Display` rendering and [`Token::HARD_KEYWORDS`]), and [`ContextualKeyword`]
/// (with its spellings and `ALL` listing) from one table.
///
/// Each keyword spelling is written exactly once: the Logos `#[token]`
/// attributes for hard and contextual keywords are generated from the
/// `hard_keywords` and `contextual_keywords` sections. `lexical_only` holds the
/// remaining Logos variants verbatim. Tests lex every table entry.
macro_rules! define_lexicon {
    (
        hard_keywords { $($keyword:ident => $keyword_text:literal),+ $(,)? }
        contextual_keywords { $($contextual:ident => $contextual_text:literal),+ $(,)? }
        others { $($variant:ident => $text:literal),+ $(,)? }
        lexical_only { $($lexical:tt)* }
    ) => {
        #[derive(Logos, Debug, Clone, PartialEq)]
        pub(crate) enum LexicalToken {
            // Hard keywords: reserved spellings that never lex as identifiers.
            $(#[token($keyword_text, |_| Token::$keyword)])+
            HardKeyword(Token),

            // Contextual keywords: identifier spellings with a special meaning
            // only in selected productions.
            $(#[token($contextual_text, |_| ContextualKeyword::$contextual)])+
            ContextualKeyword(ContextualKeyword),

            $($lexical)*
        }

        /// An identifier spelling that has keyword meaning only in a precise parser context.
        ///
        /// These spellings remain ordinary identifiers everywhere else. Keeping their
        /// lexical classification typed lets parser code select a special production
        /// without recovering semantics from source strings or repeating unions of
        /// otherwise unrelated token variants.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum ContextualKeyword {
            $($contextual),+
        }

        impl ContextualKeyword {
            /// Every contextual keyword, in table order.
            pub const ALL: &'static [Self] = &[$(Self::$contextual),+];

            /// Canonical source spelling recognized by the lexer.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$contextual => $contextual_text),+
                }
            }
        }

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

define_lexicon! {
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
    contextual_keywords {
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
    lexical_only {
        // Trivia. The parser-facing lexer consumes these and exposes them through
        // typed source metadata instead of yielding them as syntax tokens.
        #[regex(r"[ \t\r\n]+")]
        Whitespace,
        #[regex(r"//[^\n\r]*", allow_greedy = true)]
        Comment,

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

/// A spelling proven by the lexer to occupy a Graphcal `IDENT` position.
///
/// This is deliberately narrower than [`NameAtom`], because wire and generated
/// names may contain characters that the source lexer cannot accept. Parsed
/// AST identifiers carry this type, and source generators should construct it
/// before writing identifier text.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SourceIdentifier(NameAtom);

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
            Some(LexicalItem::Syntax(token)) if token.is_identifier() => {
                Ok(Self(NameAtom::new_unchecked_for_parser(spelling)))
            }
            Some(LexicalItem::Syntax(token)) if Token::HARD_KEYWORDS.contains(&token) => {
                Err(SourceIdentifierError::ReservedKeyword)
            }
            Some(LexicalItem::Syntax(_) | LexicalItem::Trivia(_)) | None => {
                Err(SourceIdentifierError::InvalidCharacters)
            }
        }
    }

    /// Wrap the spelling of one lexer-produced identifier token.
    ///
    /// The parser has already tokenized `spelling` as a single `IDENT`, so the
    /// invariant is asserted here without making parser code handle an
    /// impossible error path.
    #[must_use]
    pub(crate) fn new_unchecked_for_parser(spelling: String) -> Self {
        debug_assert!(Self::parse(spelling.as_str()).is_ok());
        Self(NameAtom::new_unchecked_for_parser(spelling))
    }

    /// Return the validated source spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Borrow the identifier as a name atom (every source identifier is one).
    #[must_use]
    pub const fn atom(&self) -> &NameAtom {
        &self.0
    }

    /// Convert the identifier into its name atom.
    #[must_use]
    pub fn into_atom(self) -> NameAtom {
        self.0
    }
}

impl std::fmt::Display for SourceIdentifier {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::fmt::Debug for SourceIdentifier {
    /// Debug output matches [`NameAtom`]'s so AST dumps show one spelling.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.0, formatter)
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
mod tests;
