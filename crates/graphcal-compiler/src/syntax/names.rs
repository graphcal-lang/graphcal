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
    /// Name of a const, param, or node declaration (e.g., `"G0"`, `"dry_mass"`, `"dv_total"`).
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
    pub struct IndexVariantName;
}

impl IndexVariantName {
    /// Build the variant name for the `n`-th step of a range index
    /// (`#0`, `#1`, …). Centralises the `"#"`-prefix format so registry,
    /// parser, and evaluator can't disagree on it.
    #[must_use]
    pub fn range_step(n: impl std::fmt::Display) -> Self {
        Self::new(format!("#{n}"))
    }

    /// Pair this variant with its index name for qualified rendering.
    #[must_use]
    pub fn qualified_by(&self, index: &IndexName) -> QualifiedIndexVariantName {
        QualifiedIndexVariantName::new(index.clone(), self.clone())
    }
}

/// A fully qualified index variant name, rendered as `Index.Variant`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct QualifiedIndexVariantName {
    index: IndexName,
    variant: IndexVariantName,
}

impl QualifiedIndexVariantName {
    /// Create a qualified index variant name from its index and variant parts.
    #[must_use]
    pub const fn new(index: IndexName, variant: IndexVariantName) -> Self {
        Self { index, variant }
    }

    /// The index/type part of the qualified variant.
    #[must_use]
    pub const fn index(&self) -> &IndexName {
        &self.index
    }

    /// The variant/constructor part of the qualified variant.
    #[must_use]
    pub const fn variant(&self) -> &IndexVariantName {
        &self.variant
    }
}

impl std::fmt::Display for QualifiedIndexVariantName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.index, self.variant)
    }
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

define_name_type! {
    /// Name of a dimension variable in a built-in function signature (e.g., `"D"`).
    ///
    /// Built-in signatures use these variables to relate argument and result
    /// dimensions, such as `sqrt: D -> D^(1/2)` or `min: (D, D) -> D`.
    pub struct DimVarName;
}

define_name_type! {
    /// Name of a local expression binding (e.g., `"x"`, `"stage_mass"`).
    pub struct LocalName;
}

define_name_type! {
    /// Name of a module alias introduced by an import/include declaration (e.g., `"constants"`, `"std"`).
    pub struct ModuleAliasName;
}

define_name_type! {
    /// Name of an open plot/figure/layer property (e.g., `"title"`, `"width"`, `"stroke_width"`).
    pub struct PlotPropertyName;
}

// --- Module-scoped names ---

use std::sync::Arc;

/// A declaration name that may optionally be qualified by a module path.
///
/// The qualifier is stored as structured path segments, not as a flat
/// dot-separated string. This allows arbitrary-depth qualification such as
/// `helpers.math.G0` while keeping the declaration member (`G0`) directly
/// accessible and distinct from the qualifier.
///
/// The `Display` impl renders `qualifier: ["helpers", "math"], member: "G0"`
/// as `helpers.math.G0`. That serialized form is for boundary use only
/// (diagnostics, debug output, third-party APIs); the compiler core should use
/// the typed accessors instead of splitting strings.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ScopedName {
    /// Module/path segments that qualify `member`. Empty for a local name.
    qualifier: Arc<[Arc<str>]>,
    /// The declaration/member name inside the qualifier scope.
    member: Arc<str>,
}

impl ScopedName {
    /// Create an unqualified local name.
    #[must_use]
    pub fn local(member: impl Into<Arc<str>>) -> Self {
        Self {
            qualifier: Arc::from([] as [Arc<str>; 0]),
            member: member.into(),
        }
    }

    /// Create a name qualified by a single module segment.
    #[must_use]
    pub fn qualified(module: impl Into<Arc<str>>, member: impl Into<Arc<str>>) -> Self {
        Self::qualified_path([module], member)
    }

    /// Create a name qualified by an arbitrary-depth module path.
    #[must_use]
    pub fn qualified_path(
        qualifier: impl IntoIterator<Item = impl Into<Arc<str>>>,
        member: impl Into<Arc<str>>,
    ) -> Self {
        Self {
            qualifier: qualifier.into_iter().map(Into::into).collect(),
            member: member.into(),
        }
    }

    /// Returns the member (leaf declaration) part of the name.
    ///
    /// For `x` this returns `"x"`; for `helpers.math.x` this also returns
    /// `"x"`.
    #[must_use]
    pub fn member(&self) -> &str {
        &self.member
    }

    /// Returns the qualifier path segments. Empty means this name is local.
    #[must_use]
    pub fn qualifier(&self) -> &[Arc<str>] {
        &self.qualifier
    }

    /// Returns whether this is a qualified name.
    #[must_use]
    pub fn is_qualified(&self) -> bool {
        !self.qualifier.is_empty()
    }

    /// Qualify a name with a single-segment prefix, replacing any existing
    /// qualifier while preserving the member.
    ///
    /// `x.with_prefix("p")` → `p.x`.
    /// `m.x.with_prefix("p")` → `p.x`.
    #[must_use]
    pub fn with_prefix(&self, prefix: &str) -> Self {
        Self::qualified(prefix, Arc::clone(&self.member))
    }
}

impl std::fmt::Display for ScopedName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for segment in self.qualifier.iter() {
            f.write_str(segment)?;
            f.write_str(".")?;
        }
        f.write_str(&self.member)
    }
}

impl From<String> for ScopedName {
    /// Wrap a bare string as a local `ScopedName`. This is what
    /// [`crate::syntax::ast::Ident::into_spanned`] uses to lift parser
    /// identifiers into the typed name; qualified forms are constructed
    /// explicitly via [`ScopedName::qualified`] or [`ScopedName::qualified_path`].
    fn from(s: String) -> Self {
        Self::local(s)
    }
}

impl From<DeclName> for ScopedName {
    /// Wrap a `DeclName` as a local `ScopedName`. Use this at the resolver →
    /// IR boundary where resolver keys (local `DeclName`s) become IR keys
    /// (`ScopedName`s).
    fn from(name: DeclName) -> Self {
        Self::local(name.into_inner())
    }
}

#[cfg(test)]
mod tests {
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
        let name: IndexVariantName = "Departure".into();
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
    fn scoped_name_qualified_display_uses_dot() {
        let name = ScopedName::qualified("module", "x");
        assert_eq!(format!("{name}"), "module.x");
        assert_eq!(name.member(), "x");
        assert_eq!(
            name.qualifier().iter().map(|s| &**s).collect::<Vec<_>>(),
            ["module"]
        );
    }

    #[test]
    fn scoped_name_supports_nested_qualifier_path() {
        let name = ScopedName::qualified_path(["helpers", "math"], "G0");
        assert_eq!(format!("{name}"), "helpers.math.G0");
        assert_eq!(name.member(), "G0");
        assert_eq!(
            name.qualifier().iter().map(|s| &**s).collect::<Vec<_>>(),
            ["helpers", "math"]
        );
    }
}
