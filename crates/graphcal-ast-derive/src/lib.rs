//! Derive macros that generate the mechanical traversals of the Graphcal
//! syntax AST.
//!
//! The AST in `graphcal-compiler` is parameterized over a phase marker
//! (`Raw` / `Desugared`). Code that only rebuilds a tree node-by-node —
//! without any per-node decision — is generated here instead of being written
//! (and reviewed) by hand. Each derive lives in its own module; a derive's
//! attribute namespace is named after the derive (`#[phase_lift(..)]` for
//! [`PhaseLift`]) so several derives can annotate the same AST type without
//! ambiguity.

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
