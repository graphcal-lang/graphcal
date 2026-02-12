use std::collections::HashMap;

use kasuri_syntax::ast::{DimExpr, MulDivOp, TypeExpr, TypeExprKind, UnitExpr};
use kasuri_syntax::dimension::{Dimension, Rational};

/// Information about a registered unit.
#[derive(Debug, Clone)]
pub struct UnitInfo {
    /// The dimension this unit measures.
    pub dimension: Dimension,
    /// Scale factor to convert 1 of this unit to base SI units.
    /// e.g., km -> 1000.0 (1 km = 1000 m)
    pub scale: f64,
}

/// Maps dimension names to `Dimension` values and unit names to `UnitInfo`.
#[derive(Debug, Default)]
pub struct Registry {
    dimensions: HashMap<String, Dimension>,
    units: HashMap<String, UnitInfo>,
}

impl Registry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a named dimension.
    pub fn register_dimension(&mut self, name: &str, dim: Dimension) {
        self.dimensions.insert(name.to_string(), dim);
    }

    /// Register a named unit with its dimension and SI scale factor.
    pub fn register_unit(&mut self, name: &str, dimension: Dimension, scale: f64) {
        self.units
            .insert(name.to_string(), UnitInfo { dimension, scale });
    }

    /// Look up a dimension by name.
    #[must_use]
    pub fn get_dimension(&self, name: &str) -> Option<&Dimension> {
        self.dimensions.get(name)
    }

    /// Look up a unit by name.
    #[must_use]
    pub fn get_unit(&self, name: &str) -> Option<&UnitInfo> {
        self.units.get(name)
    }

    /// Resolve a `DimExpr` AST node to a concrete `Dimension`.
    ///
    /// Returns `None` if any dimension name is unknown.
    #[must_use]
    pub fn resolve_dim_expr(&self, expr: &DimExpr) -> Option<Dimension> {
        let mut result = Dimension::DIMENSIONLESS;
        for item in &expr.terms {
            let base = self.dimensions.get(item.term.name.name.as_str())?;
            let exp = item.term.power.unwrap_or(1);
            let powered = base.pow(Rational::from_int(exp));
            result = match item.op {
                MulDivOp::Mul => result * powered,
                MulDivOp::Div => result / powered,
            };
        }
        Some(result)
    }

    /// Resolve a `TypeExpr` to a concrete `Dimension`.
    ///
    /// Returns `None` if the type references unknown dimensions.
    #[must_use]
    pub fn resolve_type_expr(&self, type_expr: &TypeExpr) -> Option<Dimension> {
        match &type_expr.kind {
            TypeExprKind::Dimensionless => Some(Dimension::DIMENSIONLESS),
            TypeExprKind::DimExpr(dim_expr) => self.resolve_dim_expr(dim_expr),
        }
    }

    /// Resolve a `UnitExpr` to its dimension and compound scale factor.
    ///
    /// Returns `None` if any unit name is unknown.
    #[must_use]
    pub fn resolve_unit_expr(&self, expr: &UnitExpr) -> Option<(Dimension, f64)> {
        let mut dim = Dimension::DIMENSIONLESS;
        let mut scale = 1.0;
        for item in &expr.terms {
            let info = self.units.get(item.name.name.as_str())?;
            let exp = item.power.unwrap_or(1);
            let powered_dim = info.dimension.pow(Rational::from_int(exp));
            let powered_scale = info.scale.powi(exp);
            match item.op {
                MulDivOp::Mul => {
                    dim = dim * powered_dim;
                    scale *= powered_scale;
                }
                MulDivOp::Div => {
                    dim = dim / powered_dim;
                    scale /= powered_scale;
                }
            }
        }
        Some((dim, scale))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::prelude::load_prelude;
    use kasuri_syntax::ast::{DimExprItem, DimTerm, Ident, UnitExprItem};
    use kasuri_syntax::dimension::BaseDim;
    use kasuri_syntax::span::Span;

    fn make_registry() -> Registry {
        let mut r = Registry::new();
        load_prelude(&mut r);
        r
    }

    fn make_ident(name: &str) -> Ident {
        Ident {
            name: name.to_string(),
            span: Span::new(0, 0),
        }
    }

    #[test]
    fn registry_base_dimensions() {
        let r = make_registry();
        assert_eq!(
            r.get_dimension("Length"),
            Some(&Dimension::base(BaseDim::Length))
        );
        assert_eq!(
            r.get_dimension("Time"),
            Some(&Dimension::base(BaseDim::Time))
        );
        assert_eq!(
            r.get_dimension("Mass"),
            Some(&Dimension::base(BaseDim::Mass))
        );
    }

    #[test]
    fn registry_derived_dimensions() {
        let r = make_registry();
        let velocity = r.get_dimension("Velocity").unwrap();
        let expected = Dimension::base(BaseDim::Length) / Dimension::base(BaseDim::Time);
        assert_eq!(*velocity, expected);
    }

    #[test]
    fn registry_base_units() {
        let r = make_registry();
        let m = r.get_unit("m").unwrap();
        assert_eq!(m.dimension, Dimension::base(BaseDim::Length));
        assert!((m.scale - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn registry_derived_units() {
        let r = make_registry();
        let km = r.get_unit("km").unwrap();
        assert_eq!(km.dimension, Dimension::base(BaseDim::Length));
        assert!((km.scale - 1000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn resolve_dim_expr_velocity() {
        let r = make_registry();
        // Length / Time
        let expr = DimExpr {
            terms: vec![
                DimExprItem {
                    op: MulDivOp::Mul,
                    term: DimTerm {
                        name: make_ident("Length"),
                        power: None,
                        span: Span::new(0, 0),
                    },
                },
                DimExprItem {
                    op: MulDivOp::Div,
                    term: DimTerm {
                        name: make_ident("Time"),
                        power: None,
                        span: Span::new(0, 0),
                    },
                },
            ],
            span: Span::new(0, 0),
        };
        let dim = r.resolve_dim_expr(&expr).unwrap();
        let expected = Dimension::base(BaseDim::Length) / Dimension::base(BaseDim::Time);
        assert_eq!(dim, expected);
    }

    #[test]
    fn resolve_unit_expr_m_per_s_squared() {
        let r = make_registry();
        // m / s^2
        let expr = UnitExpr {
            terms: vec![
                UnitExprItem {
                    op: MulDivOp::Mul,
                    name: make_ident("m"),
                    power: None,
                },
                UnitExprItem {
                    op: MulDivOp::Div,
                    name: make_ident("s"),
                    power: Some(2),
                },
            ],
            span: Span::new(0, 0),
        };
        let (dim, scale) = r.resolve_unit_expr(&expr).unwrap();
        let expected_dim =
            Dimension::base(BaseDim::Length) / Dimension::base(BaseDim::Time).pow_int(2);
        assert_eq!(dim, expected_dim);
        assert!((scale - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn resolve_unit_expr_km_per_hour() {
        let r = make_registry();
        // km / hour
        let expr = UnitExpr {
            terms: vec![
                UnitExprItem {
                    op: MulDivOp::Mul,
                    name: make_ident("km"),
                    power: None,
                },
                UnitExprItem {
                    op: MulDivOp::Div,
                    name: make_ident("hour"),
                    power: None,
                },
            ],
            span: Span::new(0, 0),
        };
        let (dim, scale) = r.resolve_unit_expr(&expr).unwrap();
        let expected_dim = Dimension::base(BaseDim::Length) / Dimension::base(BaseDim::Time);
        assert_eq!(dim, expected_dim);
        // km/hour = 1000 m / 3600 s ≈ 0.2778 m/s
        assert!((scale - 1000.0 / 3600.0).abs() < 1e-10);
    }
}
