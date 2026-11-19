//! Cancellation tied to the lifetime of an async waiter.
//!
//! tower-lsp answers `$/cancelRequest` by dropping the request's future, and a
//! timeout drops the future it guards. Blocking work started by such a future
//! keeps running unless something tells it to stop: this guard cancels its
//! token when the waiter that owns it is dropped.

use graphcal_compiler::cancellation::{CancellationSource, CancellationToken};

/// Cancels its token when dropped.
#[derive(Debug)]
pub struct CancelOnDrop(CancellationSource);

impl CancelOnDrop {
    #[must_use]
    pub fn new() -> Self {
        Self(CancellationSource::new())
    }

    #[must_use]
    pub fn token(&self) -> CancellationToken {
        self.0.token()
    }

    pub fn cancel(&self) {
        self.0.cancel();
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_the_guard_cancels_its_tokens() {
        let guard = CancelOnDrop::new();
        let token = guard.token();
        assert!(!token.is_cancelled());
        drop(guard);
        assert!(token.is_cancelled());
    }
}
