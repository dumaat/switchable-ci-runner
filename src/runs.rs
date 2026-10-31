//! The repository's CI runs as the agent sees them, and which ones this machine took.
//!
//! The route step decides once per run, at its start, but the run's jobs reach
//! the runners one by one (a matrix, a job that `needs` another). So a run the
//! machine took keeps the runners online until it ends: otherwise its later
//! jobs, already routed here, would wait in the queue for a runner that left.

use std::collections::HashSet;

#[derive(Debug, Default)]
pub struct Runs {
    seen: HashSet<u64>,
    active: HashSet<u64>,
    taken: HashSet<u64>,
    started: bool,
}

impl Runs {
    /// A fresh list of the active runs; returns the ones not seen before.
    /// Runs already active in the first list are not new: they were routed
    /// before the agent started.
    pub fn update(&mut self, active: &[u64]) -> Vec<u64> {
        self.active = active.iter().copied().collect();
        self.taken.retain(|id| self.active.contains(id));
        let new: Vec<u64> = active
            .iter()
            .copied()
            .filter(|id| self.seen.insert(*id))
            .collect();
        if self.started {
            new
        } else {
            self.started = true;
            Vec::new()
        }
    }

    /// The machine takes these runs: it stays online until they end.
    pub fn take(&mut self, ids: &[u64]) {
        self.taken
            .extend(ids.iter().copied().filter(|id| self.active.contains(id)));
    }

    /// A run this machine took is still going.
    pub fn any_taken(&self) -> bool {
        !self.taken.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_active_at_start_are_not_new() {
        let mut runs = Runs::default();
        assert_eq!(runs.update(&[1, 2]), Vec::<u64>::new());
        assert_eq!(runs.update(&[1, 2, 3]), vec![3]);
        assert_eq!(runs.update(&[2, 3]), Vec::<u64>::new());
    }

    #[test]
    fn a_taken_run_holds_until_it_ends() {
        let mut runs = Runs::default();
        runs.update(&[]);
        let new = runs.update(&[7]);
        runs.take(&new);
        assert!(runs.any_taken());
        runs.update(&[7, 8]);
        assert!(runs.any_taken());
        runs.update(&[8]);
        assert!(!runs.any_taken());
    }

    #[test]
    fn a_run_that_already_ended_cannot_be_taken() {
        // The owner answered "yes" after the run finished.
        let mut runs = Runs::default();
        runs.update(&[]);
        runs.update(&[5]);
        runs.update(&[]);
        runs.take(&[5]);
        assert!(!runs.any_taken());
    }
}
