//! Checked evaluator work accounting and bounded cooperative checkpoints.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use thiserror::Error;

/// Maximum arithmetic work charged to one declaration evaluation.
pub(super) const DEFAULT_WORK_LIMIT: u64 = 10_000_000;

/// Checked estimate of primitive kernel operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct WorkAmount(u64);

impl WorkAmount {
    /// Form a checked product estimate and apply a fixed algorithm multiplier.
    pub(super) fn checked_product(
        factors: &[usize],
        multiplier: u64,
    ) -> Result<Self, WorkAmountError> {
        let product = factors.iter().try_fold(multiplier, |product, factor| {
            let factor = u64::try_from(*factor).map_err(|_| WorkAmountError::Overflow)?;
            product.checked_mul(factor).ok_or(WorkAmountError::Overflow)
        })?;
        Ok(Self(product))
    }

    const fn get(self) -> u64 {
        self.0
    }
}

/// Failure to compute a bounded work estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub(super) enum WorkAmountError {
    #[error("kernel work estimate overflowed")]
    Overflow,
}

/// Shared, monotonically consumed work allowance for one declaration body.
#[derive(Debug, Clone)]
pub struct WorkBudget {
    remaining: Arc<AtomicU64>,
    limit: u64,
}

impl Default for WorkBudget {
    fn default() -> Self {
        Self::new(DEFAULT_WORK_LIMIT)
    }
}

impl WorkBudget {
    pub(super) fn new(limit: u64) -> Self {
        Self {
            remaining: Arc::new(AtomicU64::new(limit)),
            limit,
        }
    }

    pub(super) fn consume(&self, amount: WorkAmount) -> Result<(), WorkBudgetError> {
        self.remaining
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                remaining.checked_sub(amount.get())
            })
            .map(|_| ())
            .map_err(|remaining| WorkBudgetError::Exhausted {
                requested: amount.get(),
                remaining,
                limit: self.limit,
            })
    }
}

/// A checked kernel exceeded its declaration-scoped evaluator budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub(super) enum WorkBudgetError {
    #[error(
        "requires {requested} operations, but only {remaining} remain in the {limit}-operation evaluation budget"
    )]
    Exhausted {
        requested: u64,
        remaining: u64,
        limit: u64,
    },
}

/// Calls a cancellation token at deterministic bounded work intervals.
pub(super) struct KernelCheckpoint<'a> {
    cancellation: &'a graphcal_compiler::cancellation::CancellationToken,
    remaining: usize,
}

impl<'a> KernelCheckpoint<'a> {
    const INTERVAL: usize = 1_024;

    pub(super) const fn new(
        cancellation: &'a graphcal_compiler::cancellation::CancellationToken,
    ) -> Self {
        Self {
            cancellation,
            remaining: 0,
        }
    }

    pub(super) fn step(&mut self) -> Result<(), graphcal_compiler::cancellation::Cancelled> {
        if self.remaining == 0 {
            self.cancellation.checkpoint()?;
            self.remaining = Self::INTERVAL;
        }
        self.remaining = self.remaining.saturating_sub(1);
        Ok(())
    }

    pub(super) fn boundary(&self) -> Result<(), graphcal_compiler::cancellation::Cancelled> {
        self.cancellation.checkpoint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoints_observe_cancellation_within_the_fixed_interval() {
        let source = graphcal_compiler::cancellation::CancellationSource::new();
        let token = source.token();
        let mut checkpoint = KernelCheckpoint::new(&token);
        checkpoint.step().unwrap();
        source.cancel();

        let observed = (0..KernelCheckpoint::INTERVAL).find_map(|_| checkpoint.step().err());
        assert_eq!(observed, Some(graphcal_compiler::cancellation::Cancelled));
    }

    #[test]
    fn work_products_and_cumulative_consumption_are_checked() {
        let budget = WorkBudget::new(10);
        budget
            .consume(WorkAmount::checked_product(&[2, 2], 1).unwrap())
            .unwrap();
        budget
            .consume(WorkAmount::checked_product(&[3, 2], 1).unwrap())
            .unwrap();
        assert!(matches!(
            budget.consume(WorkAmount::checked_product(&[1], 1).unwrap()),
            Err(WorkBudgetError::Exhausted {
                requested: 1,
                remaining: 0,
                ..
            })
        ));
    }
}
