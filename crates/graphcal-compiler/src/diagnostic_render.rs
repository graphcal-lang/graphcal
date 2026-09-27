//! Shell-side `miette` adapter for core [`Diagnostic`]s.
//!
//! [`RenderableDiagnostic`] resolves a diagnostic's [`SourceId`] through a
//! [`SourceRegistry`] and exposes the result as a [`miette::Diagnostic`], so
//! CLI, LSP, and snapshot renderers keep using `miette` while the core stays
//! free of it. The adapter reproduces what `#[derive(miette::Diagnostic)]`
//! yields for an equivalent hand-written error: the same code, message, help,
//! and non-primary labels in declaration order (primary first).
//!
//! [`SourceId`]: crate::source_id::SourceId

use std::fmt;
use std::sync::Arc;

use miette::{LabeledSpan, NamedSource, SourceCode};

use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::source_registry::{ForeignSourceId, SourceRegistry};

/// A core [`Diagnostic`] with its source attached, ready for `miette`.
#[derive(Debug)]
pub struct RenderableDiagnostic<K> {
    diagnostic: Diagnostic<K>,
    source: NamedSource<Arc<String>>,
}

impl<K> RenderableDiagnostic<K> {
    /// Attach the source `diagnostic` refers to.
    ///
    /// # Errors
    ///
    /// Returns [`ForeignSourceId`] if `registry` did not issue the
    /// diagnostic's source id.
    pub fn new(
        diagnostic: Diagnostic<K>,
        registry: &SourceRegistry,
    ) -> Result<Self, ForeignSourceId> {
        let source = registry.named_source(diagnostic.src)?.clone();
        Ok(Self { diagnostic, source })
    }

    /// The underlying core diagnostic.
    pub const fn diagnostic(&self) -> &Diagnostic<K> {
        &self.diagnostic
    }

    /// The named source the diagnostic's spans index into.
    pub const fn named_source(&self) -> &NamedSource<Arc<String>> {
        &self.source
    }
}

impl<K: DiagnosticKind> fmt::Display for RenderableDiagnostic<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.diagnostic.kind.fmt(f)
    }
}

impl<K: DiagnosticKind + fmt::Debug> std::error::Error for RenderableDiagnostic<K> {}

impl<K: DiagnosticKind + fmt::Debug> miette::Diagnostic for RenderableDiagnostic<K> {
    fn code<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        Some(Box::new(self.diagnostic.kind.code()))
    }

    fn help<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        self.diagnostic
            .kind
            .help()
            .map(|help| Box::new(help) as Box<dyn fmt::Display + 'a>)
    }

    fn source_code(&self) -> Option<&dyn SourceCode> {
        Some(&self.source)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        let primary = LabeledSpan::new_with_span(
            self.diagnostic.kind.primary_label(),
            self.diagnostic.primary,
        );
        let secondary = self
            .diagnostic
            .kind
            .secondary_labels()
            .into_iter()
            .map(|label| LabeledSpan::new_with_span(Some(label.text), label.span));
        Some(Box::new(std::iter::once(primary).chain(secondary)))
    }
}

#[cfg(test)]
mod tests {
    use miette::NarratableReportHandler;

    use super::*;
    use crate::diagnostic::SecondaryLabel;
    use crate::syntax::names::NameAtom;
    use crate::syntax::parser::ParseError;
    use crate::syntax::span::Span;

    /// Typed mirrors of two derived `ParseError` variants, used to prove the
    /// adapter renders exactly like `#[derive(miette::Diagnostic)]`.
    #[derive(Debug)]
    enum MirrorKind {
        UnexpectedToken { expected: String, found: String },
        DuplicateDagBinding { name: NameAtom, first: Span },
    }

    impl fmt::Display for MirrorKind {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::UnexpectedToken { found, .. } => write!(f, "unexpected token `{found}`"),
                Self::DuplicateDagBinding { name, .. } => {
                    write!(f, "duplicate DAG binding `{name}`")
                }
            }
        }
    }

    impl DiagnosticKind for MirrorKind {
        fn code(&self) -> &'static str {
            match self {
                Self::UnexpectedToken { .. } => "graphcal::P001",
                Self::DuplicateDagBinding { .. } => "graphcal::P025",
            }
        }

        fn primary_label(&self) -> Option<String> {
            Some(
                match self {
                    Self::UnexpectedToken { .. } => "here",
                    Self::DuplicateDagBinding { .. } => "duplicate binding",
                }
                .to_string(),
            )
        }

        fn help(&self) -> Option<String> {
            Some(match self {
                Self::UnexpectedToken { expected, .. } => format!("expected {expected}"),
                Self::DuplicateDagBinding { .. } => {
                    "each name may appear at most once in a DAG binding list".to_string()
                }
            })
        }

        fn secondary_labels(&self) -> Vec<SecondaryLabel> {
            match self {
                Self::UnexpectedToken { .. } => Vec::new(),
                Self::DuplicateDagBinding { first, .. } => vec![SecondaryLabel {
                    span: *first,
                    text: "first bound here".to_string(),
                }],
            }
        }
    }

    fn render(diagnostic: &dyn miette::Diagnostic) -> String {
        let mut rendered = String::new();
        NarratableReportHandler::new()
            .render_report(&mut rendered, diagnostic)
            .expect("rendering into a String cannot fail");
        rendered
    }

    #[test]
    fn renders_identically_to_the_derived_diagnostic() {
        let text = Arc::new("dag d(a = 1, a = 2) {}\nnode x = ;\n".to_string());
        let mut registry = SourceRegistry::new();
        let id = registry.register("main.gcl", Arc::clone(&text));
        let named = || NamedSource::new("main.gcl", Arc::clone(&text));
        let name = NameAtom::parse("a").expect("valid name atom");
        let token_span = Span::new(32, 1);
        let duplicate = Span::new(13, 1);
        let first = Span::new(6, 1);

        let cases = [
            (
                Diagnostic::new(
                    id,
                    token_span,
                    MirrorKind::UnexpectedToken {
                        expected: "expression".to_string(),
                        found: ";".to_string(),
                    },
                ),
                ParseError::UnexpectedToken {
                    expected: "expression".to_string(),
                    found: ";".to_string(),
                    src: named(),
                    span: token_span.into(),
                },
            ),
            (
                Diagnostic::new(
                    id,
                    duplicate,
                    MirrorKind::DuplicateDagBinding {
                        name: name.clone(),
                        first,
                    },
                ),
                ParseError::DuplicateDagBinding {
                    name,
                    src: named(),
                    duplicate: duplicate.into(),
                    first: first.into(),
                },
            ),
        ];

        for (diagnostic, derived) in cases {
            let adapted = RenderableDiagnostic::new(diagnostic, &registry).expect("own source id");
            assert_eq!(render(&adapted), render(&derived));
        }
    }

    #[test]
    fn foreign_source_ids_are_rejected() {
        let mut issuing = SourceRegistry::new();
        let id = issuing.register("main.gcl", Arc::new(String::new()));
        let diagnostic = Diagnostic::new(
            id,
            Span::new(0, 0),
            MirrorKind::UnexpectedToken {
                expected: "expression".to_string(),
                found: ";".to_string(),
            },
        );

        assert!(matches!(
            RenderableDiagnostic::new(diagnostic, &SourceRegistry::new()),
            Err(ForeignSourceId)
        ));
    }
}
