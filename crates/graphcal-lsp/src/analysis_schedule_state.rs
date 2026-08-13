//! Pure state transitions for per-document analysis cancellation and cleanup.
//!
//! The server shell owns URI lookup and worker permits. This core owns only the
//! lifecycle of one document's generations, which keeps cancellation decisions
//! deterministic and small enough for exhaustive Loom schedules.

use std::collections::HashMap;

use graphcal_compiler::cancellation::CancellationSource;

pub trait ScheduleCancellation: Clone {
    fn new_active() -> Self;
    fn cancel(&self);
}

impl ScheduleCancellation for CancellationSource {
    fn new_active() -> Self {
        Self::new()
    }

    fn cancel(&self) {
        Self::cancel(self);
    }
}

/// State for one document's queued and running analysis generations.
#[derive(Debug)]
pub struct AnalysisScheduleState<C = CancellationSource> {
    cancellations: HashMap<u64, C>,
    is_open: bool,
}

impl<C: ScheduleCancellation> AnalysisScheduleState<C> {
    pub(crate) fn new(is_open: bool) -> Self {
        Self {
            cancellations: HashMap::new(),
            is_open,
        }
    }

    pub(crate) const fn open(&mut self) {
        self.is_open = true;
    }

    /// Register a generation, cancelling an accidentally duplicated generation.
    pub(crate) fn register(&mut self, generation: u64) -> C {
        let source = C::new_active();
        if let Some(replaced) = self.cancellations.insert(generation, source.clone()) {
            replaced.cancel();
        }
        source
    }

    pub(crate) fn cancel_all(&self) {
        self.cancellations
            .values()
            .for_each(ScheduleCancellation::cancel);
    }

    /// Close the document and report whether its quiescent state can be removed.
    pub(crate) fn close(&mut self) -> bool {
        self.is_open = false;
        self.cancel_all();
        self.cancellations.is_empty()
    }

    /// Finish one generation and report whether its closed state can be removed.
    pub(crate) fn finish(&mut self, generation: u64) -> bool {
        self.cancellations.remove(&generation);
        !self.is_open && self.cancellations.is_empty()
    }

    #[cfg(test)]
    fn is_quiescent(&self) -> bool {
        self.cancellations.is_empty()
    }
}

#[cfg(all(test, not(loom)))]
mod tests {
    use super::*;

    #[test]
    fn close_cancels_work_and_finish_makes_closed_state_removable() {
        let mut state = AnalysisScheduleState::<CancellationSource>::new(true);
        let first = state.register(1);
        let second = state.register(2);

        assert!(!state.close());
        assert!(first.is_cancelled());
        assert!(second.is_cancelled());
        assert!(!state.finish(1));
        assert!(state.finish(2));
        assert!(state.is_quiescent());
    }

    #[test]
    fn reopening_preserves_running_generation_until_it_finishes() {
        let mut state = AnalysisScheduleState::<CancellationSource>::new(false);
        let source = state.register(1);
        state.open();

        assert!(!state.finish(1));
        assert!(!source.is_cancelled());
        assert!(state.is_quiescent());
    }

    #[test]
    fn duplicate_generation_cancels_the_replaced_source() {
        let mut state = AnalysisScheduleState::<CancellationSource>::new(true);
        let replaced = state.register(1);
        let current = state.register(1);

        assert!(replaced.is_cancelled());
        assert!(!current.is_cancelled());
    }
}

#[cfg(all(test, loom))]
mod loom_tests {
    use loom::sync::atomic::{AtomicBool, Ordering};
    use loom::sync::{Arc, Mutex};
    use loom::thread;

    use super::*;

    #[derive(Clone, Debug)]
    struct LoomCancellation(Arc<AtomicBool>);

    impl ScheduleCancellation for LoomCancellation {
        fn new_active() -> Self {
            Self(Arc::new(AtomicBool::new(false)))
        }

        fn cancel(&self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }

    impl LoomCancellation {
        fn is_cancelled(&self) -> bool {
            self.0.load(Ordering::Relaxed)
        }
    }

    #[test]
    fn close_racing_finish_never_loses_cancellation_or_cleanup() {
        loom::model(|| {
            let state = Arc::new(Mutex::new(AnalysisScheduleState::<LoomCancellation>::new(
                true,
            )));
            let source = state.lock().unwrap().register(1);

            let closing_state = Arc::clone(&state);
            let close = thread::spawn(move || closing_state.lock().unwrap().close());
            let finishing_state = Arc::clone(&state);
            let finish = thread::spawn(move || finishing_state.lock().unwrap().finish(1));

            let close_removed = close.join().unwrap();
            let finish_removed = finish.join().unwrap();
            assert!(state.lock().unwrap().is_quiescent());
            if finish_removed {
                assert!(!close_removed);
                assert!(source.is_cancelled());
            } else {
                assert!(close_removed);
            }
        });
    }

    #[test]
    fn cancel_racing_replacement_cancels_every_source_it_observes() {
        loom::model(|| {
            let state = Arc::new(Mutex::new(AnalysisScheduleState::<LoomCancellation>::new(
                true,
            )));
            let replaced = state.lock().unwrap().register(1);

            let cancelling_state = Arc::clone(&state);
            let cancel = thread::spawn(move || cancelling_state.lock().unwrap().cancel_all());
            let replacing_state = Arc::clone(&state);
            let replacement = thread::spawn(move || replacing_state.lock().unwrap().register(1));

            cancel.join().unwrap();
            let current = replacement.join().unwrap();
            assert!(replaced.is_cancelled());
            state.lock().unwrap().cancel_all();
            assert!(current.is_cancelled());
        });
    }
}
