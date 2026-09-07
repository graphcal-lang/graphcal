//! Test-only observations of costly pipeline boundaries.
//!
//! Production observation is a no-op. Counters are thread-local, so concurrent
//! test operations cannot contaminate one another. They count the documented
//! boundary events, not arbitrary allocations or every Rust `Clone` call.

#[derive(Debug, Clone, Copy)]
pub enum Event {
    #[cfg(test)]
    ImportedBodyReference,
    #[cfg(test)]
    UnsharedImportedBody,
    PlanConstruction,
    ScheduleConstruction,
    ImportedSourceResolution,
    FrameExecution,
    ConstructorResolution,
    PresentationEvaluation,
}

#[cfg(not(test))]
#[inline]
pub const fn record(_event: Event) {}

#[cfg(test)]
pub fn record(event: Event) {
    record_many(event, 1);
}

#[cfg(test)]
pub use observer::{measure, record_many};

/// Observe the actual canonical/importer addresses, not an inactive clone hook.
#[cfg(test)]
pub fn record_imported_body<T>(canonical: &T, imported: &T) {
    record(Event::ImportedBodyReference);
    if !std::ptr::eq(canonical, imported) {
        record(Event::UnsharedImportedBody);
    }
}

#[cfg(test)]
mod observer {
    use super::Event;
    use std::cell::Cell;

    #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
    pub struct Counts {
        pub imported_body_references: u64,
        pub unshared_imported_bodies: u64,
        pub plan_constructions: u64,
        pub schedule_constructions: u64,
        pub imported_source_resolutions: u64,
        pub frame_executions: u64,
        pub constructor_resolutions: u64,
        pub presentation_evaluations: u64,
    }

    thread_local! { static COUNTS: Cell<Counts> = Cell::new(Counts::default()); }

    pub fn record_many(event: Event, amount: u64) {
        COUNTS.with(|cell| {
            let mut counts = cell.get();
            let count = match event {
                Event::ImportedBodyReference => &mut counts.imported_body_references,
                Event::UnsharedImportedBody => &mut counts.unshared_imported_bodies,
                Event::PlanConstruction => &mut counts.plan_constructions,
                Event::ScheduleConstruction => &mut counts.schedule_constructions,
                Event::ImportedSourceResolution => &mut counts.imported_source_resolutions,
                Event::FrameExecution => &mut counts.frame_executions,
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
                imported_body_references: after
                    .imported_body_references
                    .saturating_sub(before.imported_body_references),
                unshared_imported_bodies: after
                    .unshared_imported_bodies
                    .saturating_sub(before.unshared_imported_bodies),
                plan_constructions: after
                    .plan_constructions
                    .saturating_sub(before.plan_constructions),
                schedule_constructions: after
                    .schedule_constructions
                    .saturating_sub(before.schedule_constructions),
                imported_source_resolutions: after
                    .imported_source_resolutions
                    .saturating_sub(before.imported_source_resolutions),
                frame_executions: after
                    .frame_executions
                    .saturating_sub(before.frame_executions),
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
