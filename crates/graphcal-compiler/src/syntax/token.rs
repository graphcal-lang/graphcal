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
    #[token("base")]
    Base,
    #[token("dimension")]
    Dimension,
    #[token("unit")]
    Unit,
    #[token("type")]
    Type,
    #[token("let")]
    Let,
    #[token("fn")]
    Fn,
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
    #[token("::")]
    ColonColon,
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
            Self::Dimension => write!(f, "dimension"),
            Self::Unit => write!(f, "unit"),
            Self::Type => write!(f, "type"),
            Self::Let => write!(f, "let"),
            Self::Fn => write!(f, "fn"),
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
            Self::ColonColon => write!(f, "::"),
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
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        reason = "test code"
    )]

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
        let tokens = lex_tokens("const node G0 = 9.80665;");
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
        let mut lexer = Token::lexer("PI");
        assert_eq!(lexer.next(), Some(Ok(Token::Ident)));
        assert_eq!(lexer.slice(), "PI");
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
            "param node const if else base dimension unit type let fn index for import include dag match as assert table plot figure scan unfold linspace step",
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
                Token::Let,
                Token::Fn,
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
            ]
        );
    }

    #[test]
    fn lex_identifier_starting_with_base() {
        // "baseline" should be Ident, not Base + "line"
        let mut lexer = Token::lexer("baseline");
        assert_eq!(lexer.next(), Some(Ok(Token::Ident)));
        assert_eq!(lexer.slice(), "baseline");
    }

    #[test]
    fn lex_identifier_starting_with_new_keywords() {
        // "scanner" should be Ident, not Scan + "ner"
        for word in [
            "scanner",
            "unfolder",
            "stepped",
            "indexed",
            "indexing",
            "linspaced",
        ] {
            let mut lexer = Token::lexer(word);
            assert_eq!(lexer.next(), Some(Ok(Token::Ident)));
            assert_eq!(lexer.slice(), word);
        }
    }

    #[test]
    fn lex_identifier_starting_with_table() {
        // "tableau" should be Ident, not Table + "au"
        let mut lexer = Token::lexer("tableau");
        assert_eq!(lexer.next(), Some(Ok(Token::Ident)));
        assert_eq!(lexer.slice(), "tableau");
    }

    #[test]
    fn lex_identifier_starting_with_keyword() {
        // "parameter" should be Ident, not Param + "eter"
        let mut lexer = Token::lexer("parameter");
        assert_eq!(lexer.next(), Some(Ok(Token::Ident)));
        assert_eq!(lexer.slice(), "parameter");
    }

    // Phase 1 specific tests

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
        let tokens = lex_tokens("dimension Velocity = Length / Time;");
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

    // Phase 2 specific tests

    #[test]
    fn lex_type_decl() {
        let tokens = lex_tokens("type TransferResult { dv1: Velocity, dv2: Velocity }");
        assert_eq!(
            tokens,
            vec![
                Token::Type,
                Token::Ident,
                Token::LBrace,
                Token::Ident,
                Token::Colon,
                Token::Ident,
                Token::Comma,
                Token::Ident,
                Token::Colon,
                Token::Ident,
                Token::RBrace,
            ]
        );
    }

    #[test]
    fn lex_let_binding() {
        let tokens = lex_tokens("let r1 = @x + @y;");
        assert_eq!(
            tokens,
            vec![
                Token::Let,
                Token::Ident,
                Token::Eq,
                Token::At,
                Token::Ident,
                Token::Plus,
                Token::At,
                Token::Ident,
                Token::Semicolon,
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
    fn lex_identifier_starting_with_type() {
        // "typedef" should be Ident, not Type + "def"
        let mut lexer = Token::lexer("typedef");
        assert_eq!(lexer.next(), Some(Ok(Token::Ident)));
        assert_eq!(lexer.slice(), "typedef");
    }

    #[test]
    fn lex_identifier_starting_with_let() {
        // "letter" should be Ident, not Let + "ter"
        let mut lexer = Token::lexer("letter");
        assert_eq!(lexer.next(), Some(Ok(Token::Ident)));
        assert_eq!(lexer.slice(), "letter");
    }

    // Phase 3 specific tests

    #[test]
    fn lex_fn_decl_short() {
        let tokens = lex_tokens("fn lerp(a: D, b: D, t: Dimensionless) -> D = a + (b - a) * t;");
        assert_eq!(
            tokens,
            vec![
                Token::Fn,
                Token::Ident, // lerp
                Token::LParen,
                Token::Ident, // a
                Token::Colon,
                Token::Ident, // D
                Token::Comma,
                Token::Ident, // b
                Token::Colon,
                Token::Ident, // D
                Token::Comma,
                Token::Ident, // t
                Token::Colon,
                Token::Ident, // Dimensionless
                Token::RParen,
                Token::Arrow,
                Token::Ident, // D
                Token::Eq,
                Token::Ident, // a
                Token::Plus,
                Token::LParen,
                Token::Ident, // b
                Token::Minus,
                Token::Ident, // a
                Token::RParen,
                Token::Star,
                Token::Ident, // t
                Token::Semicolon,
            ]
        );
    }

    #[test]
    fn lex_fn_with_generics() {
        let tokens = lex_tokens("fn abs<D: Dim>(x: D) -> D");
        assert_eq!(
            tokens,
            vec![
                Token::Fn,
                Token::Ident, // abs
                Token::Lt,    // <
                Token::Ident, // D
                Token::Colon,
                Token::Ident, // Dim
                Token::Gt,    // >
                Token::LParen,
                Token::Ident, // x
                Token::Colon,
                Token::Ident, // D
                Token::RParen,
                Token::Arrow,
                Token::Ident, // D
            ]
        );
    }

    #[test]
    fn lex_identifier_starting_with_fn() {
        // "fnord" should be Ident, not Fn + "ord"
        let mut lexer = Token::lexer("fnord");
        assert_eq!(lexer.next(), Some(Ok(Token::Ident)));
        assert_eq!(lexer.slice(), "fnord");
    }

    // Phase 4 specific tests

    #[test]
    fn lex_import_statement() {
        let tokens = lex_tokens(r#"import "./helper.gcl" { G0, isp };"#);
        assert_eq!(
            tokens,
            vec![
                Token::Import,
                Token::StringLiteral,
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
        let mut lexer = Token::lexer(r#""./orbit/transfer.gcl""#);
        assert_eq!(lexer.next(), Some(Ok(Token::StringLiteral)));
        assert_eq!(lexer.slice(), r#""./orbit/transfer.gcl""#);
    }

    #[test]
    fn lex_use_statement_with_alias() {
        let tokens = lex_tokens(r#"import "./f.gcl" { x as y };"#);
        assert_eq!(
            tokens,
            vec![
                Token::Import,
                Token::StringLiteral,
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
    fn lex_identifier_starting_with_import() {
        // "importable" should be Ident, not Import + "able"
        let mut lexer = Token::lexer("importable");
        assert_eq!(lexer.next(), Some(Ok(Token::Ident)));
        assert_eq!(lexer.slice(), "importable");
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
    fn lex_identifier_starting_with_dag() {
        // "dagger" should be Ident, not Dag + "ger"
        let mut lexer = Token::lexer("dagger");
        assert_eq!(lexer.next(), Some(Ok(Token::Ident)));
        assert_eq!(lexer.slice(), "dagger");
    }

    #[test]
    fn lex_identifier_starting_with_include() {
        // "included" should be Ident, not Include + "d"
        let mut lexer = Token::lexer("included");
        assert_eq!(lexer.next(), Some(Ok(Token::Ident)));
        assert_eq!(lexer.slice(), "included");
    }
}
