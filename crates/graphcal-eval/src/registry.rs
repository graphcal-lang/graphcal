use std::collections::HashMap;

use graphcal_syntax::ast::{DimExpr, FnBody, MulDivOp, TypeExpr, TypeExprKind, UnitExpr};
use graphcal_syntax::dimension::{Dimension, Rational};
use graphcal_syntax::names::{
    DimName, FieldName, FnName, GenericParamName, IndexName, StructTypeName, UnitName, VariantName,
};
use graphcal_syntax::span::Span;

/// Information about a registered unit.
#[derive(Debug, Clone)]
pub struct UnitInfo {
    /// The dimension this unit measures.
    pub dimension: Dimension,
    /// Scale factor to convert 1 of this unit to base SI units.
    /// e.g., km -> 1000.0 (1 km = 1000 m)
    pub scale: f64,
}

/// A field in a struct type definition.
#[derive(Debug, Clone)]
pub struct StructField {
    pub name: FieldName,
    pub dimension: Dimension,
}

/// A struct type definition with its name and fields.
#[derive(Debug, Clone)]
pub struct StructDef {
    pub name: StructTypeName,
    pub fields: Vec<StructField>,
}

/// A user-defined function parameter.
#[derive(Debug, Clone)]
pub struct FnParamDef {
    pub name: String,
    pub type_expr: TypeExpr,
}

/// A generic parameter on a user-defined function, with its constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FnGenericConstraint {
    /// `D: Dim` — the generic stands for a dimension.
    Dim,
    /// `I: Index` — the generic stands for an index.
    Index,
}

/// A generic parameter with name and constraint.
#[derive(Debug, Clone)]
pub struct FnGenericParam {
    pub name: GenericParamName,
    pub constraint: FnGenericConstraint,
}

/// A user-defined function stored in the registry.
#[derive(Debug, Clone)]
pub struct FnDef {
    pub name: FnName,
    pub generic_params: Vec<FnGenericParam>,
    pub params: Vec<FnParamDef>,
    pub return_type_expr: TypeExpr,
    pub body: FnBody,
    pub span: Span,
}

/// A declared index with its ordered variants.
#[derive(Debug, Clone)]
pub struct IndexDef {
    pub name: IndexName,
    pub variants: Vec<VariantName>,
}

/// Maps dimension names to `Dimension` values and unit names to `UnitInfo`.
#[derive(Debug, Default)]
pub struct Registry {
    dimensions: HashMap<DimName, Dimension>,
    units: HashMap<UnitName, UnitInfo>,
    structs: HashMap<StructTypeName, StructDef>,
    functions: HashMap<FnName, FnDef>,
    indexes: HashMap<IndexName, IndexDef>,
}

impl Registry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a named dimension.
    pub fn register_dimension(&mut self, name: DimName, dim: Dimension) {
        self.dimensions.insert(name, dim);
    }

    /// Register a named unit with its dimension and SI scale factor.
    pub fn register_unit(&mut self, name: UnitName, dimension: Dimension, scale: f64) {
        self.units.insert(name, UnitInfo { dimension, scale });
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

    /// Register a struct type definition.
    pub fn register_struct(&mut self, def: StructDef) {
        self.structs.insert(def.name.clone(), def);
    }

    /// Look up a struct type definition by name.
    #[must_use]
    pub fn get_struct(&self, name: &str) -> Option<&StructDef> {
        self.structs.get(name)
    }

    /// Register a user-defined function.
    pub fn register_function(&mut self, def: FnDef) {
        self.functions.insert(def.name.clone(), def);
    }

    /// Look up a user-defined function by name.
    #[must_use]
    pub fn get_function(&self, name: &str) -> Option<&FnDef> {
        self.functions.get(name)
    }

    /// Iterate over all user-defined functions.
    pub fn all_functions(&self) -> impl Iterator<Item = &FnDef> {
        self.functions.values()
    }

    /// Register an index definition.
    pub fn register_index(&mut self, def: IndexDef) {
        self.indexes.insert(def.name.clone(), def);
    }

    /// Look up an index definition by name.
    #[must_use]
    pub fn get_index(&self, name: &str) -> Option<&IndexDef> {
        self.indexes.get(name)
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
            TypeExprKind::Bool | TypeExprKind::Int => None, // Bool/Int are not dimension types
            TypeExprKind::DimExpr(dim_expr) => self.resolve_dim_expr(dim_expr),
            TypeExprKind::Indexed { base, .. } => self.resolve_type_expr(base),
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
            let info = self.units.get(item.name.value.as_str())?;
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
    #![allow(clippy::unwrap_used, reason = "test code")]
    use super::*;
    use crate::prelude::load_prelude;
    use graphcal_syntax::ast::{DimExprItem, DimTerm, Ident, UnitExprItem};
    use graphcal_syntax::dimension::BaseDim;
    use graphcal_syntax::names::Spanned;
    use graphcal_syntax::span::Span;

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

    fn make_unit_name(name: &str) -> Spanned<UnitName> {
        Spanned::new(UnitName::new(name), Span::new(0, 0))
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
                    name: make_unit_name("m"),
                    power: None,
                },
                UnitExprItem {
                    op: MulDivOp::Div,
                    name: make_unit_name("s"),
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
                    name: make_unit_name("km"),
                    power: None,
                },
                UnitExprItem {
                    op: MulDivOp::Div,
                    name: make_unit_name("hour"),
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

    #[test]
    fn registry_struct_register_and_lookup() {
        let mut r = make_registry();
        let velocity_dim = Dimension::base(BaseDim::Length) / Dimension::base(BaseDim::Time);
        r.register_struct(StructDef {
            name: StructTypeName::new("TransferResult"),
            fields: vec![
                StructField {
                    name: FieldName::new("dv1"),
                    dimension: velocity_dim,
                },
                StructField {
                    name: FieldName::new("dv2"),
                    dimension: velocity_dim,
                },
            ],
        });
        let def = r.get_struct("TransferResult").unwrap();
        assert_eq!(def.name.as_str(), "TransferResult");
        assert_eq!(def.fields.len(), 2);
        assert_eq!(def.fields[0].name.as_str(), "dv1");
        assert_eq!(def.fields[0].dimension, velocity_dim);
        assert!(r.get_struct("NonExistent").is_none());
    }

    #[test]
    fn registry_index_register_and_lookup() {
        let mut r = make_registry();
        r.register_index(IndexDef {
            name: IndexName::new("Maneuver"),
            variants: vec![
                VariantName::new("Departure"),
                VariantName::new("Correction"),
                VariantName::new("Insertion"),
            ],
        });
        let def = r.get_index("Maneuver").unwrap();
        assert_eq!(def.name.as_str(), "Maneuver");
        let variant_strs: Vec<&str> = def.variants.iter().map(VariantName::as_str).collect();
        assert_eq!(variant_strs, vec!["Departure", "Correction", "Insertion"]);
        assert!(r.get_index("NonExistent").is_none());
    }
}
