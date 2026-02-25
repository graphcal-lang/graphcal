//! Graphcal TIR: typed intermediate representation and dimension checking.
#![allow(
    unused_assignments,
    reason = "miette derive macro generates false-positive unused_assignments warnings"
)]
#![allow(
    clippy::result_large_err,
    reason = "GraphcalError is inherently large and only constructed on the error path"
)]

pub mod dim_check;
pub mod tir;
