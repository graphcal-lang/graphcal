//! Behavioral tests for `#[derive(PhaseLift)]` on a miniature phased AST.

use graphcal_ast_derive::PhaseLift;

trait Phase {
    type Sugar: core::fmt::Debug + Eq;
}

#[derive(Debug, PartialEq, Eq)]
enum Surface {}

#[derive(Debug, PartialEq, Eq)]
enum Core {}

impl Phase for Surface {
    type Sugar = Negate;
}

impl Phase for Core {
    type Sugar = core::convert::Infallible;
}

/// Surface-only sugar: `-x`, lowered to `0 - x`.
#[derive(Debug, PartialEq, Eq)]
struct Negate(Box<Expr<Surface>>);

#[derive(Debug, PartialEq, Eq, PhaseLift)]
#[phase_lift(from = Surface, to = Core)]
enum Expr<P: Phase> {
    Number(i64),
    // `Self` names the phase-parameterized type, so it is lifted too.
    Sub(Box<Self>, Box<Self>),
    Call {
        name: String,
        args: Vec<Self>,
        named: Option<Vec<Arg<P>>>,
    },
    Pair(#[phase_lift(map)] Pair<Arg<P>>),
    #[phase_lift(from_payload)]
    Sugar(P::Sugar),
}

impl From<Negate> for Expr<Core> {
    fn from(Negate(operand): Negate) -> Self {
        Self::Sub(Box::new(Self::Number(0)), Box::new((*operand).into()))
    }
}

#[derive(Debug, PartialEq, Eq, PhaseLift)]
#[phase_lift(from = Surface, to = Core)]
struct Arg<P: Phase> {
    label: &'static str,
    value: Expr<P>,
}

/// A container with its own `map`, standing in for `NonEmpty<T>`.
#[derive(Debug, PartialEq, Eq)]
struct Pair<T>(Vec<T>);

impl<T> Pair<T> {
    fn new(first: T, second: T) -> Self {
        Self(vec![first, second])
    }

    fn map<U>(self, f: impl FnMut(T) -> U) -> Pair<U> {
        Pair(self.0.into_iter().map(f).collect())
    }
}

#[derive(Debug, PartialEq, Eq, PhaseLift)]
#[phase_lift(from = Surface, to = Core)]
struct Tuple<P: Phase>(u8, Expr<P>);

const fn number<P: Phase>(n: i64) -> Expr<P> {
    Expr::Number(n)
}

fn sub<P: Phase>(lhs: Expr<P>, rhs: Expr<P>) -> Expr<P> {
    Expr::Sub(Box::new(lhs), Box::new(rhs))
}

const fn arg<P: Phase>(label: &'static str, value: Expr<P>) -> Arg<P> {
    Arg { label, value }
}

fn negate(operand: Expr<Surface>) -> Expr<Surface> {
    Expr::Sugar(Negate(Box::new(operand)))
}

#[test]
fn structural_variants_are_rebuilt_and_sugar_is_lowered() {
    let surface: Expr<Surface> = Expr::Call {
        name: "f".to_owned(),
        args: vec![
            number(1),
            negate(number(2)),
            Expr::Pair(Pair::new(arg("a", number(3)), arg("b", negate(number(4))))),
        ],
        named: Some(vec![Arg {
            label: "x",
            value: sub(number(5), number(6)),
        }]),
    };

    let core: Expr<Core> = surface.into();

    assert_eq!(
        core,
        Expr::Call {
            name: "f".to_owned(),
            args: vec![
                number(1),
                sub(number(0), number(2)),
                Expr::Pair(Pair::new(
                    arg("a", number(3)),
                    arg("b", sub(number(0), number(4)))
                )),
            ],
            named: Some(vec![Arg {
                label: "x",
                value: sub(number(5), number(6)),
            }]),
        }
    );
}

#[test]
fn absent_optional_fields_stay_absent() {
    let surface: Expr<Surface> = Expr::Call {
        name: "g".to_owned(),
        args: vec![],
        named: None,
    };
    let core: Expr<Core> = surface.into();
    assert_eq!(
        core,
        Expr::Call {
            name: "g".to_owned(),
            args: vec![],
            named: None,
        }
    );
}

#[test]
fn tuple_structs_lift_positionally() {
    let surface: Tuple<Surface> = Tuple(7, negate(number(8)));
    let core: Tuple<Core> = surface.into();
    assert_eq!(core, Tuple(7, sub(number(0), number(8))));
}
