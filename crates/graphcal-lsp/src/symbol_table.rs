//! Symbol table for LSP features: maps source locations to definitions and references.
//!
//! - [`index`] holds the table itself and its lookup structures.
//! - [`declarations`] builds a table from a file's AST declaration shells.
//! - [`hir_refs`] collects expression references from tolerantly lowered HIR.
//! - [`enrich`] adds hover types and details from a checked program.

mod declarations;
mod enrich;
mod hir_refs;
mod index;
#[cfg(test)]
mod tests;

pub use declarations::{build_for_buffer, build_from_ast, file_local_resolver};
pub use enrich::enrich_from_tir;
pub use index::{DefinitionInfo, ReferenceInfo, SymbolCategory, SymbolTable};
