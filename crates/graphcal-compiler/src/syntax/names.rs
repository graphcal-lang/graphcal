//! Typed newtype wrappers for string identifiers.
//!
//! Each newtype represents a distinct semantic category of name in the graphcal
//! language, preventing accidental mixing at compile time.

/// Macro to define a newtype wrapper around `String` with standard trait impls.
macro_rules! define_name_type {
    (
        $(#[$meta:meta])*
        $vis:vis struct $Name:ident;
    ) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        $vis struct $Name(String);

        impl std::fmt::Debug for $Name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                // Delegate to the inner String's Debug so that Vec<$Name> formats
                // as ["foo", "bar"] rather than [TypeName("foo"), TypeName("bar")].
                std::fmt::Debug::fmt(&self.0, f)
            }
        }

        impl $Name {
            /// Create a new name from a string.
            #[must_use]
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }

            /// Get the underlying string slice.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Consume and return the inner `String`.
            #[must_use]
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl std::fmt::Display for $Name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl PartialEq<str> for $Name {
            fn eq(&self, other: &str) -> bool {
                self.0 == other
            }
        }

        impl PartialEq<&str> for $Name {
            fn eq(&self, other: &&str) -> bool {
                self.0 == *other
            }
        }

        impl AsRef<str> for $Name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl std::borrow::Borrow<str> for $Name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl From<String> for $Name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<&str> for $Name {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }
    };
}

define_name_type! {
    /// Name of a const, param, or node declaration.
    pub struct DeclName;
}

define_name_type! {
    /// Name of a dimension (e.g., `"Length"`, `"Velocity"`).
    pub struct DimName;
}

define_name_type! {
    /// Name of a unit (e.g., `"m"`, `"km"`, `"hour"`).
    pub struct UnitName;
}

define_name_type! {
    /// Name of a struct type (e.g., `"TransferResult"`).
    pub struct StructTypeName;
}

define_name_type! {
    /// Name of an index type (e.g., `"Maneuver"`).
    pub struct IndexName;
}

define_name_type! {
    /// Name of a function (e.g., `"sqrt"`, `"lerp"`).
    pub struct FnName;
}

define_name_type! {
    /// Name of a struct field (e.g., `"dv1"`, `"altitude"`).
    pub struct FieldName;
}

define_name_type! {
    /// Name of an index variant (e.g., `"Departure"`, `"Correction"`).
    pub struct VariantName;
}

define_name_type! {
    /// Name of a tagged-union constructor (e.g., `"LowThrust"`, `"Coast"`).
    ///
    /// Constructors live in a *separate namespace* from types: a single
    /// lexeme can name both a type and a constructor (and will, once the
    /// single-variant sugar lands). Keeping these as distinct newtypes
    /// enforces the namespace boundary at the type level.
    pub struct ConstructorName;
}

define_name_type! {
    /// Name of a generic type parameter (e.g., `"D"`, `"I"`).
    pub struct GenericParamName;
}

// --- Module-scoped names ---

/// A declaration name that may optionally be module-qualified.
///
/// Selective imports produce `Local` names (`x`); whole-module imports and
/// alias-rewritten qualified references produce `Qualified` names
/// (`module::x`). The variant carries the qualification structurally — no
/// flat string parsing is needed to recover it.
///
/// The `Display` impl renders `Qualified { module: "m", member: "x" }` as
/// `m::x`. That serialized form is for *boundary* use only (debug output,
/// `HashMap` keys that haven't yet been re-typed). The functional core
/// should pattern-match on the variant directly.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ScopedName {
    /// A bare local name: `x`, `G0`, etc.
    Local(String),
    /// A module-qualified name: `module::x`, `constants::G0`, etc.
    Qualified { module: String, member: String },
}

impl ScopedName {
    /// Create a `Local` name.
    #[must_use]
    pub fn local(name: impl Into<String>) -> Self {
        Self::Local(name.into())
    }

    /// Create a `Qualified` name.
    #[must_use]
    pub fn qualified(module: impl Into<String>, member: impl Into<String>) -> Self {
        Self::Qualified {
            module: module.into(),
            member: member.into(),
        }
    }

    /// Returns the member (leaf) part of the name.
    ///
    /// For `Local("x")` this returns `"x"`.
    /// For `Qualified { module: "m", member: "x" }` this also returns `"x"`.
    #[must_use]
    pub fn member(&self) -> &str {
        match self {
            Self::Local(name) => name,
            Self::Qualified { member, .. } => member,
        }
    }

    /// Returns the module part, if qualified.
    #[must_use]
    pub fn module(&self) -> Option<&str> {
        match self {
            Self::Qualified { module, .. } => Some(module),
            Self::Local(_) => None,
        }
    }

    /// Returns whether this is a qualified name.
    #[must_use]
    pub const fn is_qualified(&self) -> bool {
        matches!(self, Self::Qualified { .. })
    }

    /// Qualify a name with a prefix.
    ///
    /// `Local("x").with_prefix("p")` → `Qualified { module: "p", member: "x" }`.
    /// `Qualified { module: "m", member: "x" }.with_prefix("p")` → `Qualified { module: "p", member: "x" }`.
    #[must_use]
    pub fn with_prefix(&self, prefix: &str) -> Self {
        Self::Qualified {
            module: prefix.to_string(),
            member: self.member().to_string(),
        }
    }
}

impl std::fmt::Display for ScopedName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local(name) => write!(f, "{name}"),
            Self::Qualified { module, member } => write!(f, "{module}::{member}"),
        }
    }
}

impl From<String> for ScopedName {
    /// Wrap a bare string as `ScopedName::Local`. This is what
    /// [`crate::syntax::ast::Ident::into_spanned`] uses to lift parser
    /// identifiers into the typed name; qualified forms are constructed
    /// explicitly via [`ScopedName::qualified`].
    fn from(s: String) -> Self {
        Self::Local(s)
    }
}

impl From<DeclName> for ScopedName {
    /// Wrap a `DeclName` as a `ScopedName::Local`. Use this at the resolver →
    /// IR boundary where resolver keys (local `DeclName`s) become IR keys
    /// (`ScopedName`s).
    fn from(name: DeclName) -> Self {
        Self::Local(name.into_inner())
    }
}

// --- Qualified-variant rendering ---

/// Render a qualified index variant `Index.Variant` in surface syntax.
///
/// Centralizes the separator (`.`, since the alpha-4 module-system redesign
/// removed `::` from the language) so diagnostics, table headers, error
/// messages, and value descriptions all stay consistent. Use anywhere a
/// qualified variant needs to appear in user-visible output — never roll
/// your own `format!("{idx}.{var}")` inline; if the surface separator
/// ever changes again, this single call site is the only thing that must
/// move.
///
/// Accepts any `Display` types so the helper works whether the caller
/// holds typed [`IndexName`] / [`VariantName`] values, raw `&str`s
/// extracted from the registry, or anything else printable.
pub fn fmt_qualified_variant(
    index: impl std::fmt::Display,
    variant: impl std::fmt::Display,
) -> String {
    format!("{index}.{variant}")
}

// --- Naming convention helpers ---

/// Check if `s` is a valid `lower_snake_case` identifier
/// (starts with a lowercase letter, contains only lowercase letters, digits, and underscores).
#[must_use]
pub fn is_lower_snake_case(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_lowercase())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

// --- Spanned wrapper ---

use crate::syntax::span::Span;

/// A value paired with its source span.
///
/// `PartialEq`/`Eq`/`Hash` delegate to `value` only, so two occurrences
/// of the same name at different source positions are considered equal.
#[derive(Debug, Clone)]
pub struct Spanned<T> {
    pub value: T,
    pub span: Span,
}

impl<T> Spanned<T> {
    /// Create a new spanned value.
    pub const fn new(value: T, span: Span) -> Self {
        Self { value, span }
    }
}

impl<T: PartialEq> PartialEq for Spanned<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<T: Eq> Eq for Spanned<T> {}

impl<T: std::hash::Hash> std::hash::Hash for Spanned<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}

impl<T: std::fmt::Display> std::fmt::Display for Spanned<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.value.fmt(f)
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
    use std::collections::HashMap;

    #[test]
    fn newtype_display() {
        let name = DeclName::new("dry_mass");
        assert_eq!(format!("{name}"), "dry_mass");
    }

    #[test]
    fn newtype_as_str() {
        let name = DimName::new("Length");
        assert_eq!(name.as_str(), "Length");
    }

    #[test]
    fn newtype_into_inner() {
        let name = UnitName::new("km");
        assert_eq!(name.into_inner(), "km");
    }

    #[test]
    fn newtype_hash_map_borrow_lookup() {
        let mut map = HashMap::new();
        map.insert(DeclName::new("x"), 42);
        // Lookup with &str via Borrow<str>
        assert_eq!(map.get("x"), Some(&42));
    }

    #[test]
    fn newtype_from_string() {
        let name: FieldName = "dv1".to_string().into();
        assert_eq!(name.as_str(), "dv1");
    }

    #[test]
    fn newtype_from_str() {
        let name: VariantName = "Departure".into();
        assert_eq!(name.as_str(), "Departure");
    }

    #[test]
    fn newtype_equality() {
        assert_eq!(IndexName::new("Maneuver"), IndexName::new("Maneuver"));
        assert_ne!(IndexName::new("Maneuver"), IndexName::new("Phase"));
    }

    #[test]
    fn newtype_ord() {
        let a = FnName::new("alpha");
        let b = FnName::new("beta");
        assert!(a < b);
    }

    #[test]
    fn spanned_eq_ignores_span() {
        let a = Spanned::new(DeclName::new("x"), Span::new(0, 1));
        let b = Spanned::new(DeclName::new("x"), Span::new(10, 11));
        assert_eq!(a, b);
    }

    #[test]
    fn spanned_ne_different_value() {
        let a = Spanned::new(DeclName::new("x"), Span::new(0, 1));
        let b = Spanned::new(DeclName::new("y"), Span::new(0, 1));
        assert_ne!(a, b);
    }

    #[test]
    fn spanned_hash_ignores_span() {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let a = Spanned::new(DeclName::new("x"), Span::new(0, 1));
        let b = Spanned::new(DeclName::new("x"), Span::new(10, 11));
        let mut ha = DefaultHasher::new();
        a.hash(&mut ha);
        let mut hb = DefaultHasher::new();
        b.hash(&mut hb);
        assert_eq!(ha.finish(), hb.finish());
    }
}
