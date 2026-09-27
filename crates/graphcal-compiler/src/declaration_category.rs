//! Categories of declarations retained in evaluation source order.

/// A declaration that produces an evaluated value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueDeclCategory {
    Const,
    Param,
    Node,
}

/// A value, assertion, or visualization declaration in source order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclCategory {
    Value(ValueDeclCategory),
    Assert,
    Plot,
    Figure,
    Layer,
}

impl std::fmt::Display for ValueDeclCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Const => "const",
            Self::Param => "param",
            Self::Node => "node",
        })
    }
}

impl std::fmt::Display for DeclCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Value(category) => category.fmt(f),
            Self::Assert => f.write_str("assert"),
            Self::Plot => f.write_str("plot"),
            Self::Figure => f.write_str("figure"),
            Self::Layer => f.write_str("layer"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DeclCategory, ValueDeclCategory};

    #[test]
    fn diagnostic_names_preserve_source_categories() {
        for (category, spelling) in [
            (DeclCategory::Value(ValueDeclCategory::Const), "const"),
            (DeclCategory::Value(ValueDeclCategory::Param), "param"),
            (DeclCategory::Value(ValueDeclCategory::Node), "node"),
            (DeclCategory::Assert, "assert"),
            (DeclCategory::Plot, "plot"),
            (DeclCategory::Figure, "figure"),
            (DeclCategory::Layer, "layer"),
        ] {
            assert_eq!(category.to_string(), spelling);
        }
    }
}
