//! What the runner should be doing right now. Pure: no clock, no system calls,
//! so every case is a row in a table test.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// How long without keyboard or mouse before the machine counts as idle.
pub const IDLE_THRESHOLD: Duration = Duration::from_secs(5 * 60);

/// The mode the owner picks in the tray menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    /// Take jobs while the machine is idle.
    Automatic,
    /// Never take jobs.
    Disabled,
}

/// Everything the decision depends on, read once per tick.
#[derive(Debug, Clone, Copy)]
pub struct Inputs {
    pub mode: Mode,
    pub idle_for: Duration,
    pub on_battery: bool,
    /// The menu option "On battery, send to GitHub".
    pub battery_sends_to_github: bool,
    /// A job is executing on this machine right now.
    pub job_running: bool,
}

/// Whether the runner process should be up (online, takes jobs) or down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wanted {
    Online,
    Offline,
}

/// Why the decision came out the way it did, shown in the tray menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    JobRunning,
    Disabled,
    OnBattery,
    Idle,
    InUse,
}

pub fn decide(inputs: Inputs) -> (Wanted, Reason) {
    // A job that already started is never cut: GitHub would mark it failed.
    if inputs.job_running {
        return (Wanted::Online, Reason::JobRunning);
    }
    if inputs.mode == Mode::Disabled {
        return (Wanted::Offline, Reason::Disabled);
    }
    if inputs.on_battery && inputs.battery_sends_to_github {
        return (Wanted::Offline, Reason::OnBattery);
    }
    if inputs.idle_for >= IDLE_THRESHOLD {
        (Wanted::Online, Reason::Idle)
    } else {
        (Wanted::Offline, Reason::InUse)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IN_USE: Duration = Duration::from_secs(30);
    const JUST_UNDER: Duration = Duration::from_secs(5 * 60 - 1);
    const IDLE: Duration = Duration::from_secs(5 * 60);

    fn inputs(mode: Mode, idle_for: Duration, on_battery: bool, option: bool, job: bool) -> Inputs {
        Inputs {
            mode,
            idle_for,
            on_battery,
            battery_sends_to_github: option,
            job_running: job,
        }
    }

    #[test]
    fn every_case_of_the_table() {
        use Mode::*;
        use Reason as R;
        use Wanted::*;
        let table = [
            // mode, idle, battery, option, job -> wanted, reason
            (Automatic, IDLE, false, true, false, Online, R::Idle),
            (Automatic, JUST_UNDER, false, true, false, Offline, R::InUse),
            (Automatic, IN_USE, false, true, false, Offline, R::InUse),
            (Automatic, IDLE, true, true, false, Offline, R::OnBattery),
            (Automatic, IDLE, true, false, false, Online, R::Idle),
            (Automatic, IN_USE, true, false, false, Offline, R::InUse),
            (Disabled, IDLE, false, true, false, Offline, R::Disabled),
            (Disabled, IDLE, false, false, false, Offline, R::Disabled),
            // A running job keeps the runner up whatever else changed.
            (Automatic, IN_USE, false, true, true, Online, R::JobRunning),
            (Automatic, IDLE, true, true, true, Online, R::JobRunning),
            (Disabled, IN_USE, false, true, true, Online, R::JobRunning),
        ];
        for (mode, idle, battery, option, job, wanted, reason) in table {
            let got = decide(inputs(mode, idle, battery, option, job));
            assert_eq!(
                got,
                (wanted, reason),
                "{mode:?} idle={idle:?} battery={battery} option={option} job={job}"
            );
        }
    }
}
