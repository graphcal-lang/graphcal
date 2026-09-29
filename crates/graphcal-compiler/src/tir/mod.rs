//! Graphcal TIR: typed intermediate representation and dimension checking.

#[warn(clippy::arithmetic_side_effects)]
pub mod dim_check;
pub mod expression_facts;
pub mod materialized_shape;
pub mod presentation;
pub mod schedule;
pub(crate) mod template_closure;
pub mod texpr;
pub mod typed;
