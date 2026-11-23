//! Choosing the one figure a single-image plot output (`--plot FILE.svg`)
//! writes.
//!
//! A single-image format cannot hold several figures, so the choice is never
//! implicit: a model with exactly one displayed figure writes that figure,
//! and any other model must name one with `--figure`.

use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_compiler::syntax::token::{SourceIdentifier, SourceIdentifierError};
use graphcal_report::vega::RenderedFigure;
use thiserror::Error;

/// Parse a `--figure` argument into the declaration name it denotes.
///
/// Displayed figures are named in the root's namespace (an included plot is
/// displayed under its local alias), so the argument is one source
/// identifier.
///
/// # Errors
///
/// Returns [`SourceIdentifierError`] when the argument is not a Graphcal
/// identifier.
pub fn parse_figure_name(spelling: &str) -> Result<ScopedName, SourceIdentifierError> {
    SourceIdentifier::parse(spelling)
        .map(|identifier| ScopedName::local(DeclName::classify(identifier.into_atom())))
}

/// Why no single figure could be chosen.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FigureSelectionError {
    /// The model displays no figure (none declared, or all `#[hidden]`).
    #[error("no displayed figure to write")]
    NoFigures,
    /// Several figures are displayed and none was named.
    #[error(
        "{} figures are displayed; choose one with `--figure <NAME>` (available: {})",
        available.len(),
        list_names(available)
    )]
    Ambiguous {
        /// The displayed figure names, in output order.
        available: Vec<ScopedName>,
    },
    /// The named figure is not among the displayed figures.
    #[error(
        "no displayed figure named `{requested}` (available: {})",
        list_names(available)
    )]
    NotFound {
        /// The name given with `--figure`.
        requested: ScopedName,
        /// The displayed figure names, in output order.
        available: Vec<ScopedName>,
    },
}

fn list_names(names: &[ScopedName]) -> String {
    if names.is_empty() {
        return "none".to_owned();
    }
    names
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Choose the figure to write: the named one, or the only one when no name
/// is given.
///
/// # Errors
///
/// Returns [`FigureSelectionError`] when there is no figure, when several
/// figures are displayed and none is named, or when the named figure is not
/// displayed.
pub fn select_figure<'f>(
    figures: &'f [RenderedFigure],
    requested: Option<&ScopedName>,
) -> Result<&'f RenderedFigure, FigureSelectionError> {
    let available = || figures.iter().map(|figure| figure.name.clone()).collect();
    match (requested, figures) {
        (Some(requested), _) => figures
            .iter()
            .find(|figure| figure.name == *requested)
            .ok_or_else(|| FigureSelectionError::NotFound {
                requested: requested.clone(),
                available: available(),
            }),
        (None, []) => Err(FigureSelectionError::NoFigures),
        (None, [only]) => Ok(only),
        (None, _) => Err(FigureSelectionError::Ambiguous {
            available: available(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn figures(source: &str) -> Vec<RenderedFigure> {
        let result = graphcal_project::prepare::compile_and_eval(source).unwrap();
        assert!(!result.has_errors(), "{result:?}");
        graphcal_report::vega::build_figures(&result.plots, &result.figures, &result.layers)
            .unwrap()
    }

    const INDEX: &str = "pub index Step = { First, Second };\n\
        param v: Dimensionless[Step] = { Step#First: 1.0, Step#Second: 2.0 };\n";

    fn plot(name: &str) -> String {
        format!(
            "plot {name} = {{ mark: point, encode: {{ x: for s: Step {{ @v[s] }}, y: for s: Step {{ @v[s] }} }} }};\n"
        )
    }

    fn two_plots() -> String {
        format!("{INDEX}{}{}", plot("alpha"), plot("beta"))
    }

    fn name(spelling: &str) -> ScopedName {
        parse_figure_name(spelling).unwrap()
    }

    #[test]
    fn parses_an_identifier_as_a_root_local_name() {
        assert_eq!(
            name("curve_a"),
            ScopedName::local(DeclName::expect_valid("curve_a"))
        );
    }

    #[test]
    fn rejects_a_non_identifier_name() {
        assert_eq!(
            parse_figure_name("mod.curve"),
            Err(SourceIdentifierError::InvalidCharacters)
        );
        assert_eq!(
            parse_figure_name(""),
            Err(SourceIdentifierError::InvalidCharacters)
        );
    }

    #[test]
    fn the_only_figure_is_chosen_without_a_name() {
        let figures = figures(&format!("{INDEX}{}", plot("only")));
        assert_eq!(select_figure(&figures, None).unwrap().name, name("only"));
    }

    #[test]
    fn several_figures_without_a_name_are_ambiguous() {
        let figures = figures(&two_plots());
        assert_eq!(
            select_figure(&figures, None).err(),
            Some(FigureSelectionError::Ambiguous {
                available: vec![name("alpha"), name("beta")],
            })
        );
    }

    #[test]
    fn a_named_figure_is_chosen_among_several() {
        let figures = figures(&two_plots());
        assert_eq!(
            select_figure(&figures, Some(&name("beta"))).unwrap().name,
            name("beta")
        );
    }

    #[test]
    fn an_unknown_name_lists_the_available_figures() {
        let figures = figures(&two_plots());
        let error = select_figure(&figures, Some(&name("gamma"))).err().unwrap();
        assert_eq!(
            error.to_string(),
            "no displayed figure named `gamma` (available: `alpha`, `beta`)"
        );
    }

    #[test]
    fn a_hidden_plot_is_not_selectable() {
        let figures = figures(&format!(
            "{INDEX}#[hidden]\n{}figure combined = {{ plots: [inner] }};\n",
            plot("inner")
        ));
        assert_eq!(
            select_figure(&figures, None).unwrap().name,
            name("combined")
        );
        assert!(matches!(
            select_figure(&figures, Some(&name("inner"))),
            Err(FigureSelectionError::NotFound { .. })
        ));
    }

    #[test]
    fn no_figures_is_an_error() {
        let figures = figures("param a: Dimensionless = 1.0;");
        assert_eq!(
            select_figure(&figures, None).err(),
            Some(FigureSelectionError::NoFigures)
        );
    }
}
