//! Bounded cooperative cancellation checkpoints for tight evaluator kernels.

/// Calls a cancellation token at deterministic bounded work intervals.
///
/// Kernels that loop without evaluating an expression per iteration (dense
/// linear algebra, aggregations) step this once per unit of work, so
/// cancellation is observed within [`Self::INTERVAL`] units without paying
/// for a token check on every unit.
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
}
