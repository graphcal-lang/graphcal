//! Derive macros that generate the mechanical traversals of the Graphcal
//! syntax AST.
//!
//! The AST in `graphcal-compiler` is parameterized over a phase marker
//! (`Raw` / `Desugared`). Code that only rebuilds a tree node-by-node —
//! without any per-node decision — is generated here instead of being written
//! (and reviewed) by hand. Each derive lives in its own module; a derive's
//! attribute namespace is named after the derive (`#[phase_lift(..)]` for
//! [`PhaseLift`], `#[fe(..)]` for [`FormatEquivalent`]) so several derives can
//! annotate the same AST type without ambiguity.

mod common;
mod format_equivalent;
mod phase_lift;

/// Derive `From<T<Source>> for T<Target>` for a phase-parameterized AST type.
///
/// The type must have exactly one type parameter (the phase) and no lifetime
/// or const parameters. The container attribute names the two phases
/// explicitly:
///
/// ```text
/// #[derive(PhaseLift)]
/// #[phase_lift(from = Raw, to = Desugared)]
/// pub struct ParamDecl<P: Phase = Raw> {
///     pub name: Spanned<DeclName>,     // moved unchanged
///     pub type_ann: TypeExpr<P>,       // From::from
///     pub value: Option<Expr<P>>,      // Option::map(From::from)
/// }
/// ```
///
/// # Field conversion
///
/// A field is converted if and only if its type syntactically mentions the
/// phase parameter or `Self`; every other field is moved unchanged (its type is
/// identical in both phases). A converted field is rebuilt by its shape:
///
/// - `Box<T>` → `Box::new(lift(*field))`
/// - `Vec<T>` → `field.into_iter().map(lift).collect()`
/// - `Option<T>` → `field.map(lift)`
/// - any other type → `From::from(field)`
///
/// where `lift` applies the same rules recursively to `T`. A misclassified
/// field is a compile error in the generated impl, never a silent change.
///
/// # Attributes
///
/// - Container: `#[phase_lift(from = <source phase>, to = <target phase>)]`
///   (required).
/// - Field: `#[phase_lift(map)]` / `#[phase_lift(map = <method>)]` converts a
///   phase-dependent field with a container of its own, as
///   `field.<method>(From::from)` (the method defaults to `map`), e.g.
///   `NonEmpty<T>` or `NodeDefinition<Expr<P>, _>::map_formula`.
/// - Enum variant: `#[phase_lift(from_payload)]` on a single-field tuple
///   variant replaces the whole value with `From::from(payload)`. This is the
///   hook for sugar variants whose lowering produces a different variant of
///   the target type; the `From<Payload> for T<Target>` impl carrying that
///   lowering is written by hand where the lowering belongs.
/// - Enum variant: `#[phase_lift(residual = <payload type>)]` on a
///   single-field tuple variant (at most one per enum) derives
///   `TryFrom<T<Source>> for T<Target>` instead of `From`, with the payload
///   type as the error: every other variant converts, and this one is handed
///   back to the caller. This is the hook for sugar whose lowering does not
///   produce a single target value (one declaration expanding into many).
///
/// Types whose conversion needs real logic (one-to-many expansion, stack
/// growth guards, private constructors) keep a hand-written `From` impl.
#[proc_macro_derive(PhaseLift, attributes(phase_lift))]
pub fn derive_phase_lift(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    phase_lift::expand(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Derive the formatter's `FormatEquivalent` relation (structural equality
/// modulo formatting) for an AST type.
///
/// Two values are equivalent when they are the same variant and every
/// compared field is pairwise `FormatEquivalent`; fields marked
/// `#[fe(skip)]` (source spans and other formatting-only data) are ignored.
/// The generated impl destructures both operands exhaustively, so every
/// field is either compared or explicitly skipped. A span field left
/// unmarked is a compile error (`Span` does not implement the trait), not a
/// silent span-sensitive comparison.
///
/// ```text
/// #[derive(FormatEquivalent)]
/// #[fe(phase = Raw)]
/// pub struct TypeExpr<P: Phase = Raw> {
///     pub kind: TypeExprKind<P>,          // compared
///     pub constraints: Vec<DomainBound<P>>, // compared
///     #[fe(skip)]
///     pub span: Span,                     // ignored
/// }
/// ```
///
/// # Trait path
///
/// The generated code names the trait as `FormatEquivalent`, resolved at the
/// derive site. The trait and this derive share one name in different
/// namespaces, so the single `use ...::FormatEquivalent;` that brings the
/// derive into scope also brings the trait; the derive is only usable where
/// the trait is.
///
/// # Attributes
///
/// - Container: `#[fe(phase = <phase>)]` implements the trait only for the
///   type instantiated at that phase (`impl FormatEquivalent for T<Raw>`).
///   The type must have exactly one type parameter. Without it, the impl is
///   generic and every type parameter is bounded by `FormatEquivalent`.
/// - Field: `#[fe(skip)]` excludes the field from the comparison.
///
/// Types whose equivalence is not structural (multiset comparisons,
/// stack-growth guards) keep a hand-written impl.
#[proc_macro_derive(FormatEquivalent, attributes(fe))]
pub fn derive_format_equivalent(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    format_equivalent::expand(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
