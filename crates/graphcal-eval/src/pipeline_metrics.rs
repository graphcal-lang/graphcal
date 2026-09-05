//! Test-only observations of costly pipeline boundaries.
//!
//! Production observation is a no-op. Counters are thread-local, so concurrent
//! test operations cannot contaminate one another. They count the documented
//! boundary events, not arbitrary allocations or every Rust `Clone` call.

#[derive(Debug, Clone, Copy)]
pub enum Event {
    DagBodyCopy,
    PlanConstruction,
    ConstructorResolution,
    PresentationEvaluation,
}

#[inline]
pub fn record(event: Event) {
    #[cfg(test)]
    record_many(event, 1);
    #[cfg(not(test))]
    let _ = event;
}

#[cfg(test)]
mod observer {
    use super::Event;
    use std::cell::Cell;

    #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
    pub struct Counts {
        pub dag_body_copies: u64,
        pub plan_constructions: u64,
        pub constructor_resolutions: u64,
        pub presentation_evaluations: u64,
    }

    thread_local! { static COUNTS: Cell<Counts> = Cell::new(Counts::default()); }

    pub fn record_many(event: Event, amount: u64) {
        COUNTS.with(|cell| {
            let mut counts = cell.get();
            let count = match event {
                Event::DagBodyCopy => &mut counts.dag_body_copies,
                Event::PlanConstruction => &mut counts.plan_constructions,
                Event::ConstructorResolution => &mut counts.constructor_resolutions,
                Event::PresentationEvaluation => &mut counts.presentation_evaluations,
            };
            *count = count.saturating_add(amount);
            cell.set(counts);
        });
    }

    pub fn measure<T>(operation: impl FnOnce() -> T) -> (T, Counts) {
        let before = COUNTS.with(Cell::get);
        let value = operation();
        let after = COUNTS.with(Cell::get);
        (
            value,
            Counts {
                dag_body_copies: after.dag_body_copies.saturating_sub(before.dag_body_copies),
                plan_constructions: after
                    .plan_constructions
                    .saturating_sub(before.plan_constructions),
                constructor_resolutions: after
                    .constructor_resolutions
                    .saturating_sub(before.constructor_resolutions),
                presentation_evaluations: after
                    .presentation_evaluations
                    .saturating_sub(before.presentation_evaluations),
            },
        )
    }
}

#[cfg(test)]
pub use observer::{measure, record_many};
