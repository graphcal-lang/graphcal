//! Behavioral tests for `#[derive(FormatEquivalent)]` on a miniature AST.
#![expect(
    dead_code,
    reason = "skipped fields exist only to show that the derive ignores them"
)]

use std::marker::PhantomData;

use graphcal_ast_derive::FormatEquivalent;

/// Local stand-in for the compiler's trait; the derive names it by its bare
/// name at the derive site.
trait FormatEquivalent {
    fn format_equivalent(&self, other: &Self) -> bool;
}

impl FormatEquivalent for i64 {
    fn format_equivalent(&self, other: &Self) -> bool {
        self == other
    }
}

impl FormatEquivalent for &'static str {
    fn format_equivalent(&self, other: &Self) -> bool {
        self == other
    }
}

impl<T: FormatEquivalent> FormatEquivalent for Box<T> {
    fn format_equivalent(&self, other: &Self) -> bool {
        (**self).format_equivalent(&**other)
    }
}

impl<T: FormatEquivalent> FormatEquivalent for Vec<T> {
    fn format_equivalent(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().zip(other).all(|(a, b)| a.format_equivalent(b))
    }
}

/// A source position: deliberately not `FormatEquivalent`.
#[derive(Clone, Copy)]
struct Span(u32);

trait Phase {
    type Sugar: FormatEquivalent;
}

struct Surface;

impl Phase for Surface {
    type Sugar = Negate;
}

#[derive(FormatEquivalent)]
enum Negate {
    Of(Box<Expr<Surface>>),
}

#[derive(FormatEquivalent)]
#[fe(phase = Surface)]
enum Expr<P: Phase> {
    Hole,
    Number(i64, #[fe(skip)] Span),
    Sub(Box<Self>, Box<Self>),
    Call {
        name: &'static str,
        args: Vec<Self>,
        #[fe(skip)]
        span: Span,
    },
    Sugar(P::Sugar),
}

#[derive(FormatEquivalent)]
#[fe(phase = Surface)]
struct Decl<P: Phase> {
    name: Spanned<&'static str>,
    value: Expr<P>,
    #[fe(skip)]
    doc: Option<String>,
}

#[derive(FormatEquivalent)]
struct Spanned<T> {
    value: T,
    #[fe(skip)]
    span: Span,
}

#[derive(FormatEquivalent)]
struct Both<A, B>(A, B, #[fe(skip)] PhantomData<fn() -> u8>);

#[derive(FormatEquivalent)]
struct OnlySpan {
    #[fe(skip)]
    span: Span,
}

#[derive(FormatEquivalent)]
enum Never {}

const fn number(value: i64, at: u32) -> Expr<Surface> {
    Expr::Number(value, Span(at))
}

fn sub(lhs: Expr<Surface>, rhs: Expr<Surface>) -> Expr<Surface> {
    Expr::Sub(Box::new(lhs), Box::new(rhs))
}

const fn call(name: &'static str, args: Vec<Expr<Surface>>, at: u32) -> Expr<Surface> {
    Expr::Call {
        name,
        args,
        span: Span(at),
    }
}

fn decl(name: &'static str, value: Expr<Surface>, at: u32, doc: Option<&str>) -> Decl<Surface> {
    Decl {
        name: Spanned {
            value: name,
            span: Span(at),
        },
        value,
        doc: doc.map(str::to_owned),
    }
}

#[test]
fn skipped_fields_do_not_affect_equivalence() {
    let a = decl("x", call("f", vec![number(1, 10)], 5), 0, Some("docs"));
    let b = decl("x", call("f", vec![number(1, 99)], 50), 7, None);
    assert!(a.format_equivalent(&b));
}

#[test]
fn every_compared_field_must_match() {
    let base = || decl("x", sub(number(1, 0), number(2, 0)), 0, None);
    assert!(base().format_equivalent(&base()));
    assert!(!base().format_equivalent(&decl("y", sub(number(1, 0), number(2, 0)), 0, None)));
    assert!(!base().format_equivalent(&decl("x", sub(number(1, 0), number(3, 0)), 0, None)));
    assert!(!base().format_equivalent(&decl("x", sub(number(3, 0), number(2, 0)), 0, None)));
    assert!(!base().format_equivalent(&decl("x", sub(number(2, 0), number(1, 0)), 0, None)));
}

#[test]
fn named_variant_fields_are_compared() {
    let f = call("f", vec![number(1, 0)], 0);
    assert!(!f.format_equivalent(&call("g", vec![number(1, 0)], 0)));
    assert!(!f.format_equivalent(&call("f", vec![number(2, 0)], 0)));
    assert!(!f.format_equivalent(&call("f", vec![], 0)));
}

#[test]
fn different_variants_are_not_equivalent() {
    let variants = [
        Expr::Hole,
        number(1, 0),
        sub(number(1, 0), number(1, 0)),
        call("f", vec![], 0),
        Expr::Sugar(Negate::Of(Box::new(number(1, 0)))),
    ];
    for (i, lhs) in variants.iter().enumerate() {
        for (j, rhs) in variants.iter().enumerate() {
            assert_eq!(lhs.format_equivalent(rhs), i == j, "variants {i} and {j}");
        }
    }
}

#[test]
fn phase_payloads_and_single_variant_enums_compare_structurally() {
    let negate = |value| Expr::Sugar(Negate::Of(Box::new(number(value, 0))));
    assert!(negate(1).format_equivalent(&negate(1)));
    assert!(!negate(1).format_equivalent(&negate(2)));
}

#[test]
fn generic_impls_compare_each_type_parameter() {
    let both = |a, b| Both(a, b, PhantomData);
    assert!(both(1_i64, "a").format_equivalent(&both(1, "a")));
    assert!(!both(1_i64, "a").format_equivalent(&both(2, "a")));
    assert!(!both(1_i64, "a").format_equivalent(&both(1, "b")));
}

#[test]
fn types_with_only_skipped_fields_are_always_equivalent() {
    let (lhs, rhs) = (OnlySpan { span: Span(1) }, OnlySpan { span: Span(2) });
    assert_ne!(lhs.span.0, rhs.span.0);
    assert!(lhs.format_equivalent(&rhs));
}

#[test]
fn empty_enums_have_an_impl() {
    fn assert_impl<T: FormatEquivalent>() {}
    assert_impl::<Never>();
}
