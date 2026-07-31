//! Cartesian runtime representation of a complex numeric value.

/// A complex number stored as Cartesian binary64 components.
///
/// The evaluator enforces finiteness whenever values enter or leave an
/// operation. This type preserves the two-component structure across compiler,
/// evaluator, project-import, and display boundaries without encoding it in a
/// field map or string convention.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ComplexValue {
    re: f64,
    im: f64,
}

impl ComplexValue {
    #[must_use]
    pub const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    #[must_use]
    pub const fn re(self) -> f64 {
        self.re
    }

    #[must_use]
    pub const fn im(self) -> f64 {
        self.im
    }
}
