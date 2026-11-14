//! Bounded, per-document single-flight scheduling of blocking analyses.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Semaphore;
use tower_lsp::lsp_types::Url;

use crate::analysis_schedule_state::AnalysisScheduleState;
use graphcal_compiler::cancellation::{CancellationSource, CancellationToken};

/// Global cap for CPU-bound analyses. Permits are owned by the blocking
/// closures, not their async waiters, so a timed-out but not-yet-cooperating
/// operation continues to consume its permit instead of allowing runaway
/// replacement work to accumulate.
const MAX_CONCURRENT_ANALYSES: usize = 2;

/// Bounded, per-document single-flight coordination for blocking analysis.
#[derive(Debug)]
pub(super) struct AnalysisScheduler {
    /// Closing entries remain only while old work is unwinding. A quick reopen
    /// reuses that gate; quiescent closed entries are removed.
    states: Mutex<HashMap<Url, ScheduledDocumentState>>,
    global_gate: Arc<Semaphore>,
}

#[derive(Debug)]
struct ScheduledDocumentState {
    core: AnalysisScheduleState,
    document_gate: Arc<Semaphore>,
}

impl ScheduledDocumentState {
    fn new(is_open: bool) -> Self {
        Self {
            core: AnalysisScheduleState::new(is_open),
            document_gate: Arc::new(Semaphore::new(1)),
        }
    }
}

impl AnalysisScheduler {
    pub(super) fn new() -> Self {
        Self {
            states: Mutex::new(HashMap::new()),
            global_gate: Arc::new(Semaphore::new(MAX_CONCURRENT_ANALYSES)),
        }
    }

    pub(super) fn open_document(&self, uri: &Url) {
        self.states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(uri.clone())
            .and_modify(|state| state.core.open())
            .or_insert_with(|| ScheduledDocumentState::new(true));
    }

    /// Register one revision before it waits for a worker permit. A later edit
    /// can therefore cancel queued as well as running work.
    pub(super) fn register(&self, uri: &Url, generation: u64) -> ScheduledAnalysis {
        let mut states = self
            .states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // A missing entry here belongs to a stale task racing with
        // `did_close`; genuine open/change/save paths call `open_document`
        // first. Keep it closed so `finish` can remove it.
        let state = states
            .entry(uri.clone())
            .or_insert_with(|| ScheduledDocumentState::new(false));
        let source = state.core.register(generation);
        let document_gate = Arc::clone(&state.document_gate);
        drop(states);
        ScheduledAnalysis {
            source,
            document_gate,
            global_gate: Arc::clone(&self.global_gate),
        }
    }

    pub(super) fn cancel_document(&self, uri: &Url) {
        let states = self
            .states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(state) = states.get(uri) {
            state.core.cancel_all();
        }
    }

    pub(super) fn close_document(&self, uri: &Url) {
        let mut states = self
            .states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let remove = states.get_mut(uri).is_some_and(|state| state.core.close());
        if remove {
            states.remove(uri);
        }
    }

    pub(super) fn cancel_all(&self) {
        let states = self
            .states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for state in states.values() {
            state.core.cancel_all();
        }
    }

    pub(super) fn finish(&self, uri: &Url, generation: u64) {
        let mut states = self
            .states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let remove = states
            .get_mut(uri)
            .is_some_and(|state| state.core.finish(generation));
        if remove {
            states.remove(uri);
        }
    }
}

#[derive(Debug)]
pub(super) struct ScheduledAnalysis {
    pub(super) source: CancellationSource,
    pub(super) document_gate: Arc<Semaphore>,
    pub(super) global_gate: Arc<Semaphore>,
}

/// Wait for a worker permit while periodically observing the core's std-only
/// cancellation token. Dropping and retrying the acquire future removes stale
/// revisions from Tokio's waiter queue, so repeated edits cannot accumulate an
/// unbounded pending backlog behind one slow analysis.
pub(super) async fn acquire_analysis_permit(
    gate: Arc<Semaphore>,
    cancellation: &CancellationToken,
) -> std::result::Result<Option<tokio::sync::OwnedSemaphorePermit>, tokio::sync::AcquireError> {
    const CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(25);

    loop {
        if cancellation.is_cancelled() {
            return Ok(None);
        }
        match tokio::time::timeout(
            CANCELLATION_POLL_INTERVAL,
            Arc::clone(&gate).acquire_owned(),
        )
        .await
        {
            Ok(result) => return result.map(Some),
            Err(_elapsed) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use tower_lsp::lsp_types::Url;

    use super::AnalysisScheduler;

    #[test]
    fn scheduler_cancels_registered_document_work() {
        let scheduler = AnalysisScheduler::new();
        let uri = Url::parse("file:///cancel.gcl").unwrap();
        scheduler.open_document(&uri);
        let registration = scheduler.register(&uri, 1);
        let cancellation = registration.source.token();

        assert!(!cancellation.is_cancelled());
        scheduler.cancel_document(&uri);
        assert!(cancellation.is_cancelled());

        scheduler.finish(&uri, 1);
        let next = scheduler.register(&uri, 2);
        assert!(!next.source.is_cancelled());
        scheduler.close_document(&uri);
        scheduler.finish(&uri, 2);
        assert!(
            !scheduler
                .states
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_key(&uri)
        );
    }
}
