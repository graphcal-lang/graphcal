//! Graphcal Compiler: syntax, semantic core, IR, and TIR.
#![expect(
    clippy::result_large_err,
    reason = "GraphcalError is inherently large and only constructed on the error path"
)]

pub mod assertion_expectation;
pub mod builtin;
pub mod cancellation;
pub mod complex_value;
pub mod dag_id;
pub mod datetime_literal;
pub mod declaration_category;
pub mod declaration_kind;
pub mod dependency_graph;
pub mod desugar;
pub mod diagnostic;
pub mod diagnostic_anchor;
pub mod diagnostic_render;
pub mod dimension;
pub mod display;
pub mod exact_rational;
pub mod expression_id;
pub mod expression_source;
pub mod extern_struct_result;
pub mod finite_value;
pub(crate) mod fresh_identity;
pub mod function_signature;
pub mod generic_param;
pub mod graphcal_error;
pub mod hir;
pub mod import_cycle;
pub mod ir;
pub mod nat;
pub mod node_definition;
pub mod node_unavailable;
pub mod outcome;
pub mod plot_props;
pub mod plot_shape;
pub mod plot_visibility;
pub mod plugin_identity;
pub mod ratio;
pub mod resolve;
pub mod resolved_name;
pub mod semantic;
pub mod semantic_error;
pub mod source_id;
pub(crate) mod source_line;
pub mod source_registry;
pub mod sparse_monomial;
pub mod stack;
pub mod static_interface;
pub mod syntax;
pub mod text_position;
pub mod tir;
