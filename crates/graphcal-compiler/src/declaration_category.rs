//! Categories of declarations retained in evaluation source order.

/// A value, assertion, or visualization declaration in source order.
#[derive(Debug, Clone, Copy)]
pub enum DeclCategory {
    Const,
    Param,
    Node,
    Assert,
    Plot,
    Figure,
    Layer,
}

impl std::fmt::Display for DeclCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Const => "const",
            Self::Param => "param",
            Self::Node => "node",
            Self::Assert => "assert",
            Self::Plot => "plot",
            Self::Figure => "figure",
            Self::Layer => "layer",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::DeclCategory;

    #[test]
    fn diagnostic_names_preserve_source_categories() {
        for (category, spelling) in [
            (DeclCategory::Const, "const"),
            (DeclCategory::Param, "param"),
            (DeclCategory::Node, "node"),
            (DeclCategory::Assert, "assert"),
            (DeclCategory::Plot, "plot"),
            (DeclCategory::Figure, "figure"),
            (DeclCategory::Layer, "layer"),
        ] {
            assert_eq!(category.to_string(), spelling);
        }
    }
}
