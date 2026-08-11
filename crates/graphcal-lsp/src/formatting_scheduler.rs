//! Bounded imperative shell for synchronous document formatting.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use graphcal_compiler::cancellation::{CancellationSource, CancellationToken};
use graphcal_eval::loader::LoaderArtifactByteLimits;
use tokio::sync::Semaphore;
use tower_lsp::lsp_types::TextEdit;

use crate::formatting::format_document_with_cancellation;

/// One formatter worker reserves a 64 MiB stack segment while rendering.
/// Keep this gate intentionally smaller than the analysis worker pool.
const MAX_CONCURRENT_FORMATS: NonZeroUsize = NonZeroUsize::MIN;
const FORMATTING_TIMEOUT: Duration = Duration::from_secs(10);

type FormattingJobResult = Result<Option<Vec<TextEdit>>, Box<graphcal_fmt::FormatError>>;

#[derive(Debug, Clone, Copy)]
struct FormattingPolicy {
    source_byte_limit: u64,
    max_concurrent: NonZeroUsize,
    timeout: Duration,
}

impl FormattingPolicy {
    #[cfg(test)]
    const fn new(source_byte_limit: u64, max_concurrent: NonZeroUsize, timeout: Duration) -> Self {
        Self {
            source_byte_limit,
            max_concurrent,
            timeout,
        }
    }
}

impl Default for FormattingPolicy {
    fn default() -> Self {
        Self {
            source_byte_limit: LoaderArtifactByteLimits::default().source_file_bytes(),
            max_concurrent: MAX_CONCURRENT_FORMATS,
            timeout: FORMATTING_TIMEOUT,
        }
    }
}

/// Typed failure modes at the async formatting boundary.
#[derive(Debug, thiserror::Error)]
pub enum FormattingTaskError {
    #[error("document has {actual} bytes, exceeding the formatting source limit of {limit} bytes")]
    SourceTooLarge { actual: u64, limit: u64 },
    #[error("formatting timed out after {0:?}")]
    Timeout(Duration),
    #[error("formatting was cancelled")]
    Cancelled,
    #[error("formatting worker panicked: {0}")]
    WorkerPanicked(String),
    #[error("formatting worker gate closed unexpectedly")]
    WorkerGateClosed,
    #[error(transparent)]
    Formatter(Box<graphcal_fmt::FormatError>),
}

#[derive(Debug)]
pub struct FormattingScheduler {
    gate: Arc<Semaphore>,
    policy: FormattingPolicy,
}

impl FormattingScheduler {
    #[must_use]
    pub fn new() -> Self {
        Self::with_policy(FormattingPolicy::default())
    }

    fn with_policy(policy: FormattingPolicy) -> Self {
        Self {
            gate: Arc::new(Semaphore::new(policy.max_concurrent.get())),
            policy,
        }
    }

    pub async fn format(
        &self,
        source: Arc<String>,
    ) -> Result<Option<Vec<TextEdit>>, FormattingTaskError> {
        self.run_job(source, |source, cancellation| {
            format_document_with_cancellation(&source, &cancellation)
        })
        .await
    }

    async fn run_job<Job>(
        &self,
        source: Arc<String>,
        job: Job,
    ) -> Result<Option<Vec<TextEdit>>, FormattingTaskError>
    where
        Job: FnOnce(Arc<String>, CancellationToken) -> FormattingJobResult + Send + 'static,
    {
        let actual = u64::try_from(source.len()).unwrap_or(u64::MAX);
        if actual > self.policy.source_byte_limit {
            return Err(FormattingTaskError::SourceTooLarge {
                actual,
                limit: self.policy.source_byte_limit,
            });
        }

        let cancellation = CancelOnDrop::new();
        let worker_token = cancellation.token();
        let gate = Arc::clone(&self.gate);
        let operation = async move {
            let permit = gate
                .acquire_owned()
                .await
                .map_err(|_| FormattingTaskError::WorkerGateClosed)?;
            let task = tokio::task::spawn_blocking(move || {
                // A timed-out/dropped async waiter cannot admit replacement
                // work until this synchronous worker has actually exited.
                let _permit = permit;
                job(source, worker_token)
            });
            match task.await {
                Ok(Ok(edits)) => Ok(edits),
                Ok(Err(error)) if matches!(*error, graphcal_fmt::FormatError::Cancelled(_)) => {
                    Err(FormattingTaskError::Cancelled)
                }
                Ok(Err(error)) => Err(FormattingTaskError::Formatter(error)),
                Err(error) => Err(FormattingTaskError::WorkerPanicked(error.to_string())),
            }
        };

        tokio::time::timeout(self.policy.timeout, operation)
            .await
            .unwrap_or_else(|_| {
                cancellation.cancel();
                Err(FormattingTaskError::Timeout(self.policy.timeout))
            })
    }
}

#[derive(Debug)]
struct CancelOnDrop(CancellationSource);

impl CancelOnDrop {
    fn new() -> Self {
        Self(CancellationSource::new())
    }

    fn token(&self) -> CancellationToken {
        self.0.token()
    }

    fn cancel(&self) {
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Instant;

    use super::*;

    fn policy(source_byte_limit: u64, timeout: Duration) -> FormattingPolicy {
        FormattingPolicy::new(source_byte_limit, NonZeroUsize::MIN, timeout)
    }

    #[tokio::test]
    async fn rejects_oversized_source_before_parsing() {
        let scheduler = FormattingScheduler::with_policy(policy(4, Duration::from_secs(1)));
        let result = scheduler
            .format(Arc::new("not valid Graphcal".to_string()))
            .await;

        assert!(matches!(
            result,
            Err(FormattingTaskError::SourceTooLarge {
                actual: 18,
                limit: 4
            })
        ));
    }

    #[tokio::test]
    async fn normal_formatting_output_is_unchanged() {
        let scheduler = FormattingScheduler::with_policy(policy(1024, Duration::from_secs(1)));
        let edits = scheduler
            .format(Arc::new("node   x:   Dimensionless  =  1;\n".to_string()))
            .await
            .unwrap()
            .unwrap();

        assert_eq!(edits[0].new_text, "node x: Dimensionless = 1;\n");
    }

    #[tokio::test]
    async fn concurrent_jobs_never_exceed_the_worker_bound() {
        let scheduler = Arc::new(FormattingScheduler::with_policy(policy(
            1024,
            Duration::from_secs(2),
        )));
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let jobs = (0..4)
            .map(|_| {
                let scheduler = Arc::clone(&scheduler);
                let active = Arc::clone(&active);
                let maximum = Arc::clone(&maximum);
                tokio::spawn(async move {
                    scheduler
                        .run_job(Arc::new(String::new()), move |_, _| {
                            let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                            maximum.fetch_max(now, Ordering::SeqCst);
                            std::thread::sleep(Duration::from_millis(20));
                            active.fetch_sub(1, Ordering::SeqCst);
                            Ok(None)
                        })
                        .await
                })
            })
            .collect::<Vec<_>>();

        for job in jobs {
            assert!(job.await.unwrap().is_ok());
        }
        assert_eq!(maximum.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn timeout_cancels_cooperative_work_and_retains_its_permit_until_exit() {
        let timeout = Duration::from_millis(100);
        let scheduler = FormattingScheduler::with_policy(policy(1024, timeout));
        let starts = Arc::new(AtomicUsize::new(0));
        let cancellation_observed = Arc::new(AtomicBool::new(false));
        let starts_for_first = Arc::clone(&starts);
        let observed_by_first = Arc::clone(&cancellation_observed);
        let first = scheduler
            .run_job(Arc::new(String::new()), move |_, cancellation| {
                starts_for_first.fetch_add(1, Ordering::SeqCst);
                while !cancellation.is_cancelled() {
                    std::thread::sleep(Duration::from_millis(1));
                }
                observed_by_first.store(true, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(150));
                Ok(None)
            })
            .await;
        assert!(matches!(first, Err(FormattingTaskError::Timeout(value)) if value == timeout));

        let starts_for_second = Arc::clone(&starts);
        let second = scheduler
            .run_job(Arc::new(String::new()), move |_, _| {
                starts_for_second.fetch_add(1, Ordering::SeqCst);
                Ok(None)
            })
            .await;
        assert!(matches!(second, Err(FormattingTaskError::Timeout(value)) if value == timeout));
        assert_eq!(starts.load(Ordering::SeqCst), 1);

        let deadline = Instant::now() + Duration::from_secs(1);
        while !cancellation_observed.load(Ordering::SeqCst) && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert!(cancellation_observed.load(Ordering::SeqCst));
        tokio::time::sleep(Duration::from_millis(180)).await;

        let starts_for_third = Arc::clone(&starts);
        let third = scheduler
            .run_job(Arc::new(String::new()), move |_, _| {
                starts_for_third.fetch_add(1, Ordering::SeqCst);
                Ok(None)
            })
            .await;
        assert!(third.is_ok());
        assert_eq!(starts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn dropping_request_cancels_worker_and_eventually_releases_permit() {
        let scheduler = Arc::new(FormattingScheduler::with_policy(policy(
            1024,
            Duration::from_secs(2),
        )));
        let started = Arc::new(AtomicBool::new(false));
        let cancellation_observed = Arc::new(AtomicBool::new(false));
        let scheduler_task = Arc::clone(&scheduler);
        let started_by_worker = Arc::clone(&started);
        let observed_by_worker = Arc::clone(&cancellation_observed);
        let request = tokio::spawn(async move {
            scheduler_task
                .run_job(Arc::new(String::new()), move |_, cancellation| {
                    started_by_worker.store(true, Ordering::SeqCst);
                    while !cancellation.is_cancelled() {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    observed_by_worker.store(true, Ordering::SeqCst);
                    Ok(None)
                })
                .await
        });
        let deadline = Instant::now() + Duration::from_secs(1);
        while !started.load(Ordering::SeqCst) && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert!(started.load(Ordering::SeqCst));

        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        let deadline = Instant::now() + Duration::from_secs(1);
        while !cancellation_observed.load(Ordering::SeqCst) && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert!(cancellation_observed.load(Ordering::SeqCst));

        assert!(
            scheduler
                .run_job(Arc::new(String::new()), |_, _| Ok(None))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn slow_job_does_not_retain_unrelated_state_lock() {
        let scheduler = Arc::new(FormattingScheduler::with_policy(policy(
            1024,
            Duration::from_secs(1),
        )));
        let state = Arc::new(tokio::sync::RwLock::new(()));
        let source = {
            let _snapshot = state.read().await;
            Arc::new(String::new())
        };
        let scheduler_task = Arc::clone(&scheduler);
        let formatting = tokio::spawn(async move {
            scheduler_task
                .run_job(source, |_, _| {
                    std::thread::sleep(Duration::from_millis(100));
                    Ok(None)
                })
                .await
        });
        tokio::time::sleep(Duration::from_millis(10)).await;

        assert!(
            tokio::time::timeout(Duration::from_millis(20), state.write())
                .await
                .is_ok()
        );
        assert!(formatting.await.unwrap().is_ok());
    }
}
