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
        assert_eq!(identifier.atom().as_str(), valid);
        assert_eq!(format!("{identifier:?}"), format!("{valid:?}"));
        assert_eq!(identifier.to_string(), valid);
        assert_eq!(
            SourceIdentifier::new_unchecked_for_parser(valid.to_owned()),
            identifier
        );
        assert_eq!(identifier.into_atom().as_str(), valid);
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
    let tokens = lex_tokens("type TransferResult { TransferResult(dv1: Velocity, dv2: Velocity) }");
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
