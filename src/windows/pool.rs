//! The runners, one per job that may run at once. Runner 1 lives in the
//! folder older versions used; runner k in `runner-k` next to it. On each tick
//! the pool reads every runner's state, asks `parallel::plan` what to do, and
//! does it. Registering and removing runners talk to GitHub, so they run in
//! the background and report back on a channel.

use super::install;
use super::job::Job;
use super::runner::{self, Runner};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};
use switchable_ci_runner::parallel::{self, Op, Slot};

/// A registration that failed is tried again after this long, or when the
/// owner changes the number of runners.
const REGISTER_RETRY: Duration = Duration::from_secs(10 * 60);

pub struct Pool {
    base: PathBuf,
    data: PathBuf,
    shared: Job,
    runners: BTreeMap<usize, Runner>,
    working: HashSet<usize>,
    failed: HashMap<usize, Instant>,
    last_wanted: usize,
    done_tx: Sender<(usize, Op, Result<(), String>)>,
    done_rx: Receiver<(usize, Op, Result<(), String>)>,
}

/// What the tray shows.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    /// Runner 1 is configured; without it there is nothing to run.
    pub configured: bool,
    pub busy: usize,
    pub wanted: usize,
}

impl Pool {
    pub fn new(base: PathBuf, data: PathBuf, cpu_limit_percent: u8) -> Result<Pool, String> {
        let (done_tx, done_rx) = channel();
        Ok(Pool {
            base,
            data,
            shared: Job::shared(cpu_limit_percent)?,
            runners: BTreeMap::new(),
            working: HashSet::new(),
            failed: HashMap::new(),
            last_wanted: 0,
            done_tx,
            done_rx,
        })
    }

    /// The new cap applies at once, to the jobs already running too.
    pub fn set_cpu_cap(&self, percent: u8) -> Result<(), String> {
        self.shared.set_cpu_cap(percent)
    }

    pub fn tick(&mut self, wanted: usize, online: bool, log: &dyn Fn(&str)) -> Summary {
        if wanted != self.last_wanted {
            self.failed.clear();
            self.last_wanted = wanted;
        }
        for (k, op, result) in self.done_rx.try_iter() {
            self.working.remove(&k);
            match result {
                Ok(()) => log(&format!("runner {k}: {op:?} done")),
                Err(e) => {
                    log(&format!("runner {k}: {op:?} failed: {e}"));
                    self.failed.insert(k, Instant::now());
                }
            }
        }
        self.failed.retain(|_, t| t.elapsed() < REGISTER_RETRY);

        for k in parallel::slots_to_check(wanted, &self.found()) {
            self.runners.entry(k).or_insert_with(|| {
                Runner::new(
                    parallel::slot_dir(&self.base, k),
                    parallel::slot_log(&self.data, k),
                )
            });
        }
        let first_installed = self.runners.get(&1).is_some_and(Runner::is_installed);
        let parents = runner::job_parents();
        let mut slots = Vec::new();
        for (&k, r) in self.runners.iter_mut() {
            if let Some(line) = r.check_exit() {
                log(&line);
            }
            let pid = r.pid();
            slots.push(Slot {
                k,
                installed: r.is_installed(),
                alive: pid.is_some(),
                busy: pid.is_some_and(|pid| parents.as_ref().is_none_or(|p| p.contains(&pid))),
                updating: r.updating(),
                working: self.working.contains(&k) || self.failed.contains_key(&k),
                waiting_retry: r.waiting_retry(),
            });
        }

        for (k, op) in parallel::plan(&slots, wanted, online && first_installed) {
            self.apply(k, op, first_installed, log);
        }
        // Forget runners that are gone (removed and not wanted) so the map stays small.
        let found = self.found();
        self.runners
            .retain(|k, r| *k <= wanted || found.contains(k) || r.is_alive());

        Summary {
            configured: first_installed,
            busy: slots.iter().filter(|s| s.busy).count(),
            wanted,
        }
    }

    fn apply(&mut self, k: usize, op: Op, first_installed: bool, log: &dyn Fn(&str)) {
        let Some(r) = self.runners.get_mut(&k) else {
            return;
        };
        match op {
            Op::Start => match r.start(&self.shared) {
                Ok(()) => log(&format!("runner {k} started")),
                Err(e) => {
                    log(&format!("runner {k}: {e}"));
                    r.delay_retry();
                }
            },
            Op::Stop => {
                r.stop();
                log(&format!("runner {k} stopped"));
            }
            // Runner 1 is registered by the installer; the others copy its repository and labels.
            Op::Register if k >= 2 && first_installed => {
                let (first, dir, data, tx) = (
                    self.base.clone(),
                    r.dir().to_path_buf(),
                    self.data.clone(),
                    self.done_tx.clone(),
                );
                self.working.insert(k);
                log(&format!("runner {k}: registering"));
                std::thread::spawn(move || {
                    let _ = tx.send((k, op, install::add_runner(&data, &first, &dir, k)));
                });
            }
            Op::Register => {}
            Op::Unregister => {
                let (dir, tx) = (r.dir().to_path_buf(), self.done_tx.clone());
                self.working.insert(k);
                log(&format!("runner {k}: removing (more runners than wanted)"));
                std::thread::spawn(move || {
                    let _ = tx.send((k, op, install::remove_runner(&dir)));
                });
            }
        }
    }

    /// Runner folders that are configured on disk, by slot number.
    fn found(&self) -> Vec<usize> {
        install::runner_slots(&self.base)
    }

    /// Quit: every runner goes offline. Never called while a job runs unless
    /// the owner quits, which is the owner's call.
    pub fn stop_all(&mut self) {
        for r in self.runners.values_mut() {
            r.stop();
        }
    }
}
