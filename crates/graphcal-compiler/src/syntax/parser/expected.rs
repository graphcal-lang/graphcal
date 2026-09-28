//! What a parse error expected and what it found instead.
//!
//! [`Expected`] names the grammar production the parser was looking for and
//! [`Found`] names the source item that stopped it. Both are closed
//! vocabularies: parser code chooses a variant, and only their `Display`
//! implementations turn them into diagnostic text.

use std::fmt;

use crate::syntax::ast::{EncodingChannel, GenericConstraint, MarkType};
use crate::syntax::local_name::LocalName;
use crate::syntax::names::NamePath;
use crate::syntax::token::{SourceIdentifier, Token};

/// The grammar production a parser position accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expected {
    /// Exactly this token.
    Token(Token),
    /// The end of a standalone expression, unit, or dimension input.
    EndOfInput,
    /// Any further token.
    AnyToken,
    /// Any identifier.
    Identifier,
    /// Any expression.
    Expression,
    /// An integer literal.
    Integer,
    /// The scale of a unit definition: a number or a parenthesized expression.
    UnitScale,
    /// A term of a dimension expression.
    DimensionTerm,
    /// The separator after a `match` arm.
    MatchArmSeparator,
    /// The tuple names after a `for` binding list, one per binding.
    ForTupleEntries { binding_count: usize },
    /// The tuple name matching the `for` binding at the same position.
    ForBindingName(LocalName),
    /// A Nat expression atom.
    NatAtom,
    /// The `}` closing a `dag` block.
    DagBlockClose,
    /// A `param` declaration, which takes no visibility.
    ParamWithoutVisibility,
    /// An `include` declaration, which takes no leading visibility.
    IncludeWithoutVisibility,
    /// The only visibility a `node` accepts.
    NodeVisibility,
    /// The only visibility a non-bindable declaration accepts.
    NonBindableVisibility,
    /// The keyword after `const`.
    AfterConst,
    /// The keyword after `base`.
    AfterBase,
    /// The start of a declaration.
    Declaration,
    /// The only declaration a leading `pub` may prefix among imports.
    PubWholeDagImport,
    /// A multi-decl, which takes no attributes.
    MultiDeclWithoutAttributes,
    /// The tail of an `import` path, where parameter bindings are not allowed.
    ImportTailWithoutBindings,
    /// The tail of an `import` declaration after its path.
    ImportTail,
    /// The tail of an `include` declaration after its parameter bindings.
    IncludeTail,
    /// An extern function declaration or the end of a plugin block.
    ExternFunctionOrBlockClose,
    /// The constraint of an extern generic binder.
    ExternBinderConstraint,
    /// The `(` opening `include` parameter bindings.
    ParamBindingsOpen,
    /// The `{` opening a selector after `::`.
    SelectorBrace,
    /// The name of an import or include item.
    ImportItemName,
    /// The body of a non-base unit declaration.
    UnitDefinition,
    /// A domain constraint inside `( … )`.
    DomainConstraint,
    /// An atom of an index expression.
    IndexExprAtom,
    /// A bare Nat parameter name inside index arithmetic.
    BareNatParameter,
    /// A generic parameter constraint.
    GenericConstraint,
    /// The `::<output>` projection of an inline DAG call.
    InlineDagProjection,
    /// The `:` after the first label of a map literal.
    MapLiteralColon,
    /// A map literal after `{`.
    MapLiteral,
    /// The `mark` field of a plot declaration.
    MarkField,
    /// A mark type.
    MarkType,
    /// A plot encoding channel.
    EncodingChannel,
    /// The body of a `type` declaration.
    TypeBody,
    /// A constructor in a non-empty `type` body.
    TypeConstructor,
    /// A constructor rather than a record field.
    ConstructorNotField,
    /// What may follow a constructor name.
    ConstructorTail,
    /// The separator between constructor fields.
    FieldSeparator,
    /// A constructor in a body that started as a tagged union.
    UnionConstructor,
    /// A shared axis of a multi-decl table.
    SharedTableAxis,
    /// The kind keyword of a multi-decl slot.
    MultiDeclSlotKind,
    /// A table axis.
    TableAxis,
    /// An entry of a multi-decl slot tuple.
    SlotTupleEntry,
    /// A cell of a multi-decl header row.
    HeaderCell,
    /// A table data row.
    TableDataRow,
    /// A single-key entry of a scalar-key map literal.
    SingleKeyMapEntry,
    /// A tuple-key entry of a tuple-key map literal.
    TupleKeyMapEntry,
    /// The body of an `index` declaration.
    IndexBody,
}

/// Write `` `a`, `b`, or `c` `` (or `` `a` or `b` `` for two alternatives).
fn write_alternatives<'a>(
    f: &mut fmt::Formatter<'_>,
    alternatives: impl ExactSizeIterator<Item = &'a str>,
) -> fmt::Result {
    let count = alternatives.len();
    for (position, alternative) in alternatives.enumerate() {
        if position > 0 {
            f.write_str(if count > 2 { ", " } else { " " })?;
            if position + 1 == count {
                f.write_str("or ")?;
            }
        }
        write!(f, "`{alternative}`")?;
    }
    Ok(())
}

impl fmt::Display for Expected {
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per expected production keeps the vocabulary in one table"
    )]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Token(token) => write!(f, "`{token}`"),
            Self::EndOfInput => f.write_str("end of input"),
            Self::AnyToken => f.write_str("token"),
            Self::Identifier => f.write_str("identifier"),
            Self::Expression => f.write_str("expression"),
            Self::Integer => f.write_str("integer"),
            Self::UnitScale => f.write_str("number or `(`"),
            Self::DimensionTerm => f.write_str("dimension term"),
            Self::MatchArmSeparator => {
                f.write_str("`,` between match arms or `}` after the final arm")
            }
            Self::ForTupleEntries { binding_count } => {
                write!(
                    f,
                    "one tuple entry per `for` binding ({binding_count} expected)"
                )
            }
            Self::ForBindingName(name) => write!(f, "the `for` binding name `{name}`"),
            Self::NatAtom => f.write_str("integer literal or Nat parameter name"),
            Self::DagBlockClose => f.write_str("`}` to close `dag` block"),
            Self::ParamWithoutVisibility => {
                f.write_str("no visibility annotation (`param` declares a named input port)")
            }
            Self::IncludeWithoutVisibility => f.write_str(
                "an include without leading visibility; put `pub` on selected outcomes",
            ),
            Self::NodeVisibility => f.write_str(
                "`pub` (nodes are computed values — `pub(bind)` is not meaningful; use `param` to declare a named input port)",
            ),
            Self::NonBindableVisibility => f.write_str(
                "`pub` (`pub(bind)` is only valid on bindable declaration kinds: `dim`, `type`, and `index`)",
            ),
            Self::AfterConst => f.write_str("`node` or `unit` after `const`"),
            Self::AfterBase => f.write_str("`dim` or `unit` after `base`"),
            Self::Declaration => f.write_str(
                "`param`, `node`, `const node`, `base dim`, `dim`, `unit`, `const unit`, `type`, `dag`, `index`, `import`, `include`, `assert`, `plot`, `figure`, or `layer`",
            ),
            Self::PubWholeDagImport => f.write_str(
                "a whole-DAG import after `pub`; selective re-exports use per-item `pub` and plugin aliases remain private",
            ),
            Self::MultiDeclWithoutAttributes => f.write_str(
                "no attributes on multi-decl (attributes are forbidden on multi-decl surface forms in v1)",
            ),
            Self::ImportTailWithoutBindings => f.write_str(
                "`{`, `as`, or `;` after path (`import` cannot have param bindings; use `include` for DAG instantiation)",
            ),
            Self::ImportTail => f.write_str("`::{`, `as`, or `;` after path"),
            Self::IncludeTail => f.write_str("`::{`, `as`, or `;` after param bindings"),
            Self::ExternFunctionOrBlockClose => f.write_str(
                "`fn` to declare an extern function, or `}` to close the plugin block",
            ),
            Self::ExternBinderConstraint => {
                write_alternatives(
                    f,
                    [GenericConstraint::Dim, GenericConstraint::Index]
                        .map(GenericConstraint::as_str)
                        .into_iter(),
                )?;
                f.write_str(" as the binder constraint")
            }
            Self::ParamBindingsOpen => f.write_str("`(` to begin param bindings"),
            Self::SelectorBrace => f.write_str("`{` to begin a brace-list selector after `::`"),
            Self::ImportItemName => f.write_str(
                "an identifier (`pub(bind)` is not allowed on import/include items — use `pub`)",
            ),
            Self::UnitDefinition => f.write_str(
                "`=` followed by a unit definition (use `base unit` for a no-body declaration)",
            ),
            Self::DomainConstraint => {
                f.write_str("at least one domain constraint (e.g., `min: 0`)")
            }
            Self::IndexExprAtom => f.write_str("integer literal or type-level name path"),
            Self::BareNatParameter => {
                f.write_str("bare Nat parameter name in arithmetic index expression")
            }
            Self::GenericConstraint => write_alternatives(
                f,
                GenericConstraint::ALL
                    .map(GenericConstraint::as_str)
                    .into_iter(),
            ),
            Self::InlineDagProjection => f.write_str("`::<output>` projection"),
            Self::MapLiteralColon => f.write_str("`:` after label in map literal"),
            Self::MapLiteral => f.write_str("map literal (`{ Index#Variant: expr, ... }`)"),
            Self::MarkField => f.write_str("`mark` field in plot declaration"),
            Self::MarkType => {
                write_alternatives(f, MarkType::ALL.map(MarkType::as_str).into_iter())
            }
            Self::EncodingChannel => {
                f.write_str("encoding channel (")?;
                for (position, channel) in EncodingChannel::ALL.into_iter().enumerate() {
                    if position > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "`{channel}`")?;
                }
                f.write_str(")")
            }
            Self::TypeBody => f.write_str("'{' or ';'"),
            Self::TypeConstructor => f.write_str(
                "at least one constructor (`type T { T }` for a unit marker, `type T;` for a required type)",
            ),
            Self::ConstructorNotField => f.write_str(
                "a constructor — write `type T { T(x: U, ...) }` instead of a field list",
            ),
            Self::ConstructorTail => {
                f.write_str("'(' (constructor payload), or ',' / '}' (unit constructor)")
            }
            Self::FieldSeparator => f.write_str("',' or terminator"),
            Self::UnionConstructor => f.write_str(
                "constructor (this body started as a tagged union, every entry must be a constructor)",
            ),
            Self::SharedTableAxis => f.write_str("at least one shared table axis"),
            Self::MultiDeclSlotKind => {
                f.write_str("`param`, `node`, or `const node` for next multi-decl slot")
            }
            Self::TableAxis => f.write_str("index name or `Fin(N)`"),
            Self::SlotTupleEntry => f.write_str("`_` or an axis identifier path in slot tuple"),
            Self::HeaderCell => {
                f.write_str("`_` or a bare label in the axis-determined header row")
            }
            Self::TableDataRow => f.write_str("at least one table data row"),
            Self::SingleKeyMapEntry => {
                f.write_str("single-key map entry (`Index#Variant: value`)")
            }
            Self::TupleKeyMapEntry => {
                f.write_str("tuple-key map entry (`(Index#Variant, ...): value`)")
            }
            Self::IndexBody => f.write_str("`{`, `range`, or `linspace`"),
        }
    }
}

/// The source item a parser position rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    /// A token, rendered by its kind (`identifier`, `number`, `(`, …).
    Token(Token),
    /// The end of the source.
    EndOfFile,
    /// An identifier, rendered by its spelling.
    Name(SourceIdentifier),
    /// A qualified or bare name path, rendered by its spelling.
    Path(NamePath),
    /// A `pub` visibility prefix.
    Pub,
    /// A `pub(bind)` visibility prefix.
    PubBind,
    /// A number of `for` tuple entries.
    EntryCount(usize),
    /// A `type` body with no entries.
    EmptyBody,
    /// A record-style `name: Type` field.
    RecordStyleField,
    /// A non-DAG import (selective or plugin).
    NonDagImport,
    /// A leading attribute list.
    Attributes,
    /// An empty shared-axis list.
    EmptyAxisList,
}

impl Found {
    /// The next token, or the end of the source when there is none.
    pub(super) fn token_or_end(token: Option<Token>) -> Self {
        token.map_or(Self::EndOfFile, Self::Token)
    }
}

impl fmt::Display for Found {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Token(token) => token.fmt(f),
            Self::EndOfFile => f.write_str("end of file"),
            Self::Name(name) => name.fmt(f),
            Self::Path(path) => path.fmt(f),
            Self::Pub => f.write_str("`pub`"),
            Self::PubBind => f.write_str("`pub(bind)`"),
            Self::EntryCount(count) => write!(f, "{count} entries"),
            Self::EmptyBody => f.write_str("empty body"),
            Self::RecordStyleField => f.write_str("record-style field"),
            Self::NonDagImport => f.write_str("non-DAG import"),
            Self::Attributes => f.write_str("`#[...]`"),
            Self::EmptyAxisList => f.write_str("empty axis list"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_vocabularies_render_from_their_tables() {
        assert_eq!(
            Expected::MarkType.to_string(),
            "`point`, `line`, `bar`, `area`, `rect`, or `tick`"
        );
        assert_eq!(
            Expected::EncodingChannel.to_string(),
            "encoding channel (`x`, `y`, `color`, `size`, `shape`, `opacity`, `detail`, `text`, `tooltip`)"
        );
        assert_eq!(
            Expected::GenericConstraint.to_string(),
            "`Dim`, `Index`, `Nat`, or `Type`"
        );
        assert_eq!(
            Expected::ExternBinderConstraint.to_string(),
            "`Dim` or `Index` as the binder constraint"
        );
    }

    #[test]
    fn tokens_render_quoted_when_expected_and_bare_when_found() {
        assert_eq!(Expected::Token(Token::Comma).to_string(), "`,`");
        assert_eq!(Found::Token(Token::LParen).to_string(), "(");
        assert_eq!(Found::Token(Token::Ident).to_string(), "identifier");
    }

    #[test]
    fn found_end_of_source_is_the_absence_of_a_token() {
        assert_eq!(Found::token_or_end(None), Found::EndOfFile);
        assert_eq!(
            Found::token_or_end(Some(Token::Semicolon)),
            Found::Token(Token::Semicolon)
        );
        assert_eq!(Found::EndOfFile.to_string(), "end of file");
    }

    #[test]
    fn counted_and_parameterized_items_render_their_payload() {
        assert_eq!(Found::EntryCount(3).to_string(), "3 entries");
        assert_eq!(
            Expected::ForTupleEntries { binding_count: 2 }.to_string(),
            "one tuple entry per `for` binding (2 expected)"
        );
        assert_eq!(Found::Pub.to_string(), "`pub`");
        assert_eq!(Found::PubBind.to_string(), "`pub(bind)`");
    }

    /// Every expected production renders exactly the text the former
    /// string-typed parse errors carried.
    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one row per expected production keeps the rendering table complete"
    )]
    fn every_expected_production_renders_its_legacy_text() {
        let cases = [
            (Expected::Token(Token::Comma), "`,`"),
            (Expected::EndOfInput, "end of input"),
            (Expected::AnyToken, "token"),
            (Expected::Identifier, "identifier"),
            (Expected::Expression, "expression"),
            (Expected::Integer, "integer"),
            (Expected::UnitScale, "number or `(`"),
            (Expected::DimensionTerm, "dimension term"),
            (
                Expected::MatchArmSeparator,
                "`,` between match arms or `}` after the final arm",
            ),
            (
                Expected::ForBindingName(LocalName::try_new("i").expect("valid local name")),
                "the `for` binding name `i`",
            ),
            (Expected::NatAtom, "integer literal or Nat parameter name"),
            (Expected::DagBlockClose, "`}` to close `dag` block"),
            (
                Expected::ParamWithoutVisibility,
                "no visibility annotation (`param` declares a named input port)",
            ),
            (
                Expected::IncludeWithoutVisibility,
                "an include without leading visibility; put `pub` on selected outcomes",
            ),
            (
                Expected::NodeVisibility,
                "`pub` (nodes are computed values — `pub(bind)` is not meaningful; use `param` to declare a named input port)",
            ),
            (
                Expected::NonBindableVisibility,
                "`pub` (`pub(bind)` is only valid on bindable declaration kinds: `dim`, `type`, and `index`)",
            ),
            (Expected::AfterConst, "`node` or `unit` after `const`"),
            (Expected::AfterBase, "`dim` or `unit` after `base`"),
            (
                Expected::Declaration,
                "`param`, `node`, `const node`, `base dim`, `dim`, `unit`, `const unit`, `type`, `dag`, `index`, `import`, `include`, `assert`, `plot`, `figure`, or `layer`",
            ),
            (
                Expected::PubWholeDagImport,
                "a whole-DAG import after `pub`; selective re-exports use per-item `pub` and plugin aliases remain private",
            ),
            (
                Expected::MultiDeclWithoutAttributes,
                "no attributes on multi-decl (attributes are forbidden on multi-decl surface forms in v1)",
            ),
            (
                Expected::ImportTailWithoutBindings,
                "`{`, `as`, or `;` after path (`import` cannot have param bindings; use `include` for DAG instantiation)",
            ),
            (Expected::ImportTail, "`::{`, `as`, or `;` after path"),
            (
                Expected::IncludeTail,
                "`::{`, `as`, or `;` after param bindings",
            ),
            (
                Expected::ExternFunctionOrBlockClose,
                "`fn` to declare an extern function, or `}` to close the plugin block",
            ),
            (Expected::ParamBindingsOpen, "`(` to begin param bindings"),
            (
                Expected::SelectorBrace,
                "`{` to begin a brace-list selector after `::`",
            ),
            (
                Expected::ImportItemName,
                "an identifier (`pub(bind)` is not allowed on import/include items — use `pub`)",
            ),
            (
                Expected::UnitDefinition,
                "`=` followed by a unit definition (use `base unit` for a no-body declaration)",
            ),
            (
                Expected::DomainConstraint,
                "at least one domain constraint (e.g., `min: 0`)",
            ),
            (
                Expected::IndexExprAtom,
                "integer literal or type-level name path",
            ),
            (
                Expected::BareNatParameter,
                "bare Nat parameter name in arithmetic index expression",
            ),
            (Expected::InlineDagProjection, "`::<output>` projection"),
            (Expected::MapLiteralColon, "`:` after label in map literal"),
            (
                Expected::MapLiteral,
                "map literal (`{ Index#Variant: expr, ... }`)",
            ),
            (Expected::MarkField, "`mark` field in plot declaration"),
            (Expected::TypeBody, "'{' or ';'"),
            (
                Expected::TypeConstructor,
                "at least one constructor (`type T { T }` for a unit marker, `type T;` for a required type)",
            ),
            (
                Expected::ConstructorNotField,
                "a constructor — write `type T { T(x: U, ...) }` instead of a field list",
            ),
            (
                Expected::ConstructorTail,
                "'(' (constructor payload), or ',' / '}' (unit constructor)",
            ),
            (Expected::FieldSeparator, "',' or terminator"),
            (
                Expected::UnionConstructor,
                "constructor (this body started as a tagged union, every entry must be a constructor)",
            ),
            (Expected::SharedTableAxis, "at least one shared table axis"),
            (
                Expected::MultiDeclSlotKind,
                "`param`, `node`, or `const node` for next multi-decl slot",
            ),
            (Expected::TableAxis, "index name or `Fin(N)`"),
            (
                Expected::SlotTupleEntry,
                "`_` or an axis identifier path in slot tuple",
            ),
            (
                Expected::HeaderCell,
                "`_` or a bare label in the axis-determined header row",
            ),
            (Expected::TableDataRow, "at least one table data row"),
            (
                Expected::SingleKeyMapEntry,
                "single-key map entry (`Index#Variant: value`)",
            ),
            (
                Expected::TupleKeyMapEntry,
                "tuple-key map entry (`(Index#Variant, ...): value`)",
            ),
            (Expected::IndexBody, "`{`, `range`, or `linspace`"),
        ];
        for (expected, text) in cases {
            assert_eq!(expected.to_string(), text, "{expected:?}");
        }
    }

    #[test]
    fn every_found_item_renders_its_legacy_text() {
        let name = SourceIdentifier::parse("circle").expect("valid identifier");
        let path = NamePath::local(
            crate::syntax::names::NameAtom::parse("Axis").expect("valid name atom"),
        );
        let cases = [
            (Found::Name(name), "circle"),
            (Found::Path(path), "Axis"),
            (Found::EmptyBody, "empty body"),
            (Found::RecordStyleField, "record-style field"),
            (Found::NonDagImport, "non-DAG import"),
            (Found::Attributes, "`#[...]`"),
            (Found::EmptyAxisList, "empty axis list"),
        ];
        for (found, text) in cases {
            assert_eq!(found.to_string(), text, "{found:?}");
        }
    }
}
