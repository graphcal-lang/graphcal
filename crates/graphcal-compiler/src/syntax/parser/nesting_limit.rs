//! The parser's recursion bound.

/// Maximum nesting depth shared by every recursive grammar production:
/// declarations, attributes, expressions, and dimension, unit, and type
/// expressions.
///
/// The recursive-descent parser consumes one or more stack frames per
/// nesting level; without a bound, pathological input like 100k nested
/// delimiters overflows the stack and aborts the process (including the LSP
/// server). The limit is far above any realistic engineering program — note
/// that left-nested operator *chains* (`1.0 + 1.0 + …`) are parsed iteratively
/// and are not limited by this bound.
pub(super) const MAX_NESTING_DEPTH: usize = 256;
