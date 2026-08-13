//! Deterministic byte-to-model generation for coverage-guided fuzzers.

use crate::project::{DimensionlessExpr, GeneratedProject, GenerationLimits};

/// Convert bounded bytes into a typed project without embedding source conventions.
///
/// This is intentionally deterministic: libFuzzer mutates bytes, while this
/// adapter preserves a valid typed model that can reach checking and evaluation.
#[must_use]
pub fn project_from_bytes(data: &[u8], limits: GenerationLimits) -> GeneratedProject {
    let mut cursor = ByteCursor::new(data);
    let left = expression_from_cursor(&mut cursor, limits.expression_depth);
    let right = expression_from_cursor(&mut cursor, limits.expression_depth);
    match cursor.next() % 3 {
        0 => GeneratedProject::single_file(left),
        1 => GeneratedProject::multi_owner(left, right),
        _ => GeneratedProject::presented(left),
    }
}

fn expression_from_cursor(cursor: &mut ByteCursor<'_>, depth: u8) -> DimensionlessExpr {
    let tag = cursor.next();
    if depth == 0 || tag.is_multiple_of(3) {
        return DimensionlessExpr::Literal(i16::from(tag % 17) - 8);
    }
    let left = Box::new(expression_from_cursor(cursor, depth - 1));
    let right = Box::new(expression_from_cursor(cursor, depth - 1));
    match tag % 3 {
        1 => DimensionlessExpr::Add(left, right),
        _ => DimensionlessExpr::Multiply(left, right),
    }
}

struct ByteCursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> ByteCursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn next(&mut self) -> u8 {
        let byte = self.bytes.get(self.position).copied().unwrap_or(0);
        self.position = self.position.saturating_add(1);
        byte
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_adapter_is_deterministic_and_bounded() {
        let data = [1, 2, 3, 4, 5, 6, 7, 8];
        let first = project_from_bytes(&data, GenerationLimits::SMOKE);
        let second = project_from_bytes(&data, GenerationLimits::SMOKE);
        assert_eq!(first, second);
        assert!(first.render().root_source().len() < 16 * 1024);
    }
}
