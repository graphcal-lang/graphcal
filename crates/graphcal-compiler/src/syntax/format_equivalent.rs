//! Format-insensitive structural equality for the [`Raw`] syntax tree.
//!
//! The formatter rewrites source text and must then guarantee that it changed
//! only *formatting* — never the program. [`FormatEquivalent`] is the single
//! relation that defines what "same program" means at the syntax level: it
//! compares two [`File<Raw>`](crate::syntax::ast::File) trees while ignoring
//! source spans (and the parser-attached `///` doc block of a declaration,
//! which the formatter preserves through comments instead).
//!
//! # Why a dedicated trait instead of `PartialEq`
//!
//! Spans are load-bearing for diagnostics, so the AST's normal equality (where
//! present) considers them — two `Spanned` values that differ only in span are
//! deliberately *not* `PartialEq`-equal. The formatter needs the opposite
//! relation: trees that differ *only* in formatting are equivalent. Giving that
//! relation its own trait keeps the "ignore spans" decision explicit instead
//! of scattering span normalization or ad-hoc `==` overrides through the
//! codebase.
//!
//! # Where the impls live
//!
//! - This module: the trait, the leaves (span-free values compared by their
//!   normal equality), and the generic containers.
//! - AST node types: `#[derive(FormatEquivalent)]` at the type definition,
//!   with `#[fe(skip)]` on every span or other formatting-only field. The
//!   derive destructures exhaustively, so a new field is compared by default;
//!   a new span field that is not skipped fails to compile, because [`Span`]
//!   deliberately has no impl.
//! - `syntax/ast/format_equivalent.rs`: the hand-written impls whose
//!   comparison is not structural (the stack-guarded `Expr`, and table
//!   literals, whose entries have multiset semantics). A future formatter
//!   transformation that reorders nodes changes the relevant impl there.
//!
//! The trait lives in the compiler crate, next to the AST, although
//! `graphcal-fmt` is its only consumer: the derive has to run at each AST type
//! definition, and `graphcal-fmt` depends on this crate, so defining the trait
//! in `graphcal-fmt` would need a dependency cycle.
//!
//! [`Raw`]: crate::syntax::phase::Raw
//! [`Span`]: crate::syntax::span::Span

pub use graphcal_ast_derive::FormatEquivalent;

use crate::dimension::Rational;
use crate::exact_rational::ExactRational;
use crate::syntax::fin_position::FinPosition;
use crate::syntax::import_category::ImportItemNamespace;
use crate::syntax::index_name::IndexEntryKey;
use crate::syntax::module_name::ScopeSegment;
use crate::syntax::names::{NameAtom, NameDef, NameNamespace, Qualified};
use crate::syntax::non_empty::{AtLeastTwo, NonEmpty};
use crate::syntax::plugin::PluginPath;
use crate::syntax::span::Spanned;
use crate::syntax::token::SourceIdentifier;

/// Structural equality of two [`Raw`](crate::syntax::phase::Raw) syntax trees
/// modulo formatting — currently, modulo source spans.
///
/// Returns `true` when `self` and `other` denote the same program. This is the
/// invariant the formatter must uphold: formatting changes spans (and, in the
/// future, possibly node order) but never meaning.
pub trait FormatEquivalent {
    /// Returns `true` if `self` and `other` are equivalent up to formatting.
    fn format_equivalent(&self, other: &Self) -> bool;
}

// ---------------------------------------------------------------------------
// Leaves: types that carry no spans, compared by their normal equality.
// ---------------------------------------------------------------------------

/// Implements [`FormatEquivalent`] via `PartialEq` for span-free leaf types.
///
/// These types contain no source positions, so format-equivalence collapses to
/// ordinary equality. Restricting this shortcut to a hand-listed set keeps
/// span-bearing types from accidentally opting into a span-sensitive compare.
macro_rules! format_equivalent_via_eq {
    ($($t:ty),+ $(,)?) => {
        $(impl FormatEquivalent for $t {
            fn format_equivalent(&self, other: &Self) -> bool {
                self == other
            }
        })+
    };
}

format_equivalent_via_eq!(
    bool,
    i64,
    u64,
    usize,
    String,
    Rational,
    ExactRational,
    // Identifiers and paths — written identity only, never a span.
    NameAtom,
    SourceIdentifier,
    ScopeSegment,
    IndexEntryKey,
    FinPosition,
    PluginPath,
    ImportItemNamespace,
);

/// Every definition-site name newtype (`DeclName`, `FieldName`, …) is a bare
/// validated atom tagged with its namespace — written identity only.
impl<Ns: NameNamespace> FormatEquivalent for NameDef<Ns>
where
    Self: PartialEq,
{
    fn format_equivalent(&self, other: &Self) -> bool {
        self == other
    }
}

/// Numeric literals compare bit-for-bit.
///
/// Bit comparison (rather than `==`) makes the relation reflexive even for the
/// degenerate `NaN` case and sidesteps the `clippy::float_cmp` lint — two
/// literals are format-equivalent exactly when they are the same literal.
impl FormatEquivalent for f64 {
    fn format_equivalent(&self, other: &Self) -> bool {
        self.to_bits() == other.to_bits()
    }
}

// ---------------------------------------------------------------------------
// Generic containers.
// ---------------------------------------------------------------------------

impl<T: FormatEquivalent> FormatEquivalent for Box<T> {
    fn format_equivalent(&self, other: &Self) -> bool {
        (**self).format_equivalent(&**other)
    }
}

impl<T: FormatEquivalent> FormatEquivalent for Option<T> {
    fn format_equivalent(&self, other: &Self) -> bool {
        match (self, other) {
            (Some(a), Some(b)) => a.format_equivalent(b),
            (None, None) => true,
            (Some(_), None) | (None, Some(_)) => false,
        }
    }
}

impl<T: FormatEquivalent> FormatEquivalent for [T] {
    fn format_equivalent(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self
                .iter()
                .zip(other.iter())
                .all(|(a, b)| a.format_equivalent(b))
    }
}

impl<T: FormatEquivalent> FormatEquivalent for Vec<T> {
    fn format_equivalent(&self, other: &Self) -> bool {
        self.as_slice().format_equivalent(other.as_slice())
    }
}

impl<T: FormatEquivalent> FormatEquivalent for NonEmpty<T> {
    fn format_equivalent(&self, other: &Self) -> bool {
        self.as_slice().format_equivalent(other.as_slice())
    }
}

/// A source path is equivalent when its owner segments and leaf are.
impl<Seg: FormatEquivalent, Leaf: FormatEquivalent> FormatEquivalent for Qualified<Seg, Leaf> {
    fn format_equivalent(&self, other: &Self) -> bool {
        let owners_match = match (self.owner(), other.owner()) {
            (None, None) => true,
            (Some(owner), Some(other_owner)) => owner.format_equivalent(other_owner),
            (None, Some(_)) | (Some(_), None) => false,
        };
        owners_match && self.leaf().format_equivalent(other.leaf())
    }
}

impl<T: FormatEquivalent> FormatEquivalent for AtLeastTwo<T> {
    fn format_equivalent(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self
                .iter()
                .zip(other.iter())
                .all(|(a, b)| a.format_equivalent(b))
    }
}

/// A spanned value is equivalent to another when their payloads are — the span
/// is the formatting difference this whole trait exists to ignore.
impl<T: FormatEquivalent> FormatEquivalent for Spanned<T> {
    fn format_equivalent(&self, other: &Self) -> bool {
        let Self { value, span: _ } = self;
        let Self {
            value: other_value,
            span: _,
        } = other;
        value.format_equivalent(other_value)
    }
}

#[cfg(test)]
mod tests {
    use super::FormatEquivalent;
    use crate::syntax::non_empty::{AtLeastTwo, NonEmpty};
    use crate::syntax::span::{Span, Spanned};

    #[test]
    fn floats_compare_bit_for_bit() {
        assert!(f64::NAN.format_equivalent(&f64::NAN));
        assert!(1.5_f64.format_equivalent(&1.5));
        assert!(!0.0_f64.format_equivalent(&-0.0));
    }

    #[test]
    fn options_require_the_same_presence_and_payload() {
        assert!(None::<u64>.format_equivalent(&None));
        assert!(Some(1_u64).format_equivalent(&Some(1)));
        assert!(!Some(1_u64).format_equivalent(&Some(2)));
        assert!(!Some(1_u64).format_equivalent(&None));
        assert!(!None.format_equivalent(&Some(1_u64)));
    }

    #[test]
    fn sequences_compare_length_and_elements_in_order() {
        assert!(vec![1_u64, 2].format_equivalent(&vec![1, 2]));
        assert!(!vec![1_u64, 2].format_equivalent(&vec![2, 1]));
        assert!(!vec![1_u64].format_equivalent(&vec![1, 1]));
        let pair = |a, b| AtLeastTwo::new(a, b);
        assert!(pair(1_u64, 2).format_equivalent(&pair(1, 2)));
        assert!(!pair(1_u64, 2).format_equivalent(&pair(1, 3)));
        let mut longer = pair(1_u64, 2);
        longer.push(3);
        assert!(!pair(1_u64, 2).format_equivalent(&longer));
        assert!(!longer.format_equivalent(&pair(1, 2)));
        let one = NonEmpty::from(1_u64);
        assert!(one.format_equivalent(&NonEmpty::from(1)));
        assert!(!one.format_equivalent(&NonEmpty::from(2)));
    }

    #[test]
    fn spans_and_boxes_are_transparent() {
        let at = |value, start| Spanned::new(value, Span::new(start, 1));
        assert!(at(1_u64, 0).format_equivalent(&at(1, 9)));
        assert!(!at(1_u64, 0).format_equivalent(&at(2, 0)));
        assert!(Box::new(1_u64).format_equivalent(&Box::new(1)));
        assert!(!Box::new(1_u64).format_equivalent(&Box::new(2)));
    }
}
