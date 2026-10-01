//! Cooperative cancellation observed by bounded filesystem reads.

/// Cooperative cancellation observed while an I/O implementation reads in
/// bounded chunks.
pub trait CancellationSignal {
    /// Whether the caller no longer needs the operation's result.
    fn is_cancelled(&self) -> bool;
}

impl<F> CancellationSignal for F
where
    F: Fn() -> bool,
{
    fn is_cancelled(&self) -> bool {
        self()
    }
}

/// Cancellation signal for non-interactive operations.
#[derive(Debug, Clone, Copy, Default)]
pub struct NeverCancel;

impl CancellationSignal for NeverCancel {
    fn is_cancelled(&self) -> bool {
        false
    }
}
