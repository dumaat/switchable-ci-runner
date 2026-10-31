//! Whether the free runners should be online right now (a busy runner is
//! never stopped, whatever this says; see `parallel::plan`). Pure: no clock, no system calls,
//! so every case is a row in a table test.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// How long without keyboard or mouse before the machine counts as idle.
pub const IDLE_THRESHOLD: Duration = Duration::from_secs(5 * 60);

/// The mode the owner picks in the tray menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    /// Take jobs while the machine is idle; ask while it is in use.
    Automatic,
    /// Take every job, idle or not, without asking.
    Always,
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
    /// A run this machine took (idle, Always, or the owner's "yes") is still
    /// going: its later jobs were routed here and must find a runner.
    pub run_taken: bool,
}

/// Whether the free runners should be up (online, take jobs) or down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wanted {
    Online,
    Offline,
}

/// Why the decision came out the way it did, shown in the tray menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Disabled,
    /// The owner chose to take every job.
    Always,
    OnBattery,
    Idle,
    /// In use, but a run this machine took (or the owner said yes to) is still going.
    RunTaken,
    /// In use: the agent asks before taking a job; no answer means GitHub.
    InUse,
}

pub fn decide(inputs: Inputs) -> (Wanted, Reason) {
    if inputs.mode == Mode::Disabled {
        return (Wanted::Offline, Reason::Disabled);
    }
    if inputs.on_battery && inputs.battery_sends_to_github {
        return (Wanted::Offline, Reason::OnBattery);
    }
    if inputs.mode == Mode::Always {
        return (Wanted::Online, Reason::Always);
    }
    if inputs.idle_for >= IDLE_THRESHOLD {
        (Wanted::Online, Reason::Idle)
    } else if inputs.run_taken {
        (Wanted::Online, Reason::RunTaken)
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

    fn inputs(mode: Mode, idle_for: Duration, on_battery: bool, option: bool) -> Inputs {
        Inputs {
            mode,
            idle_for,
            on_battery,
            battery_sends_to_github: option,
            run_taken: false,
        }
    }

    fn said_yes(mode: Mode, idle_for: Duration, on_battery: bool, option: bool) -> Inputs {
        Inputs {
            run_taken: true,
            ..inputs(mode, idle_for, on_battery, option)
        }
    }

    #[test]
    fn a_taken_run_keeps_runners_online_while_in_use() {
        assert_eq!(
            decide(said_yes(Mode::Automatic, IN_USE, false, true)),
            (Wanted::Online, Reason::RunTaken)
        );
    }

    #[test]
    fn yes_never_overrides_disabled_or_the_battery_option() {
        assert_eq!(
            decide(said_yes(Mode::Disabled, IN_USE, false, true)),
            (Wanted::Offline, Reason::Disabled)
        );
        assert_eq!(
            decide(said_yes(Mode::Automatic, IN_USE, true, true)),
            (Wanted::Offline, Reason::OnBattery)
        );
    }

    #[test]
    fn always_takes_jobs_while_in_use_but_respects_the_battery_option() {
        assert_eq!(
            decide(inputs(Mode::Always, IN_USE, false, true)),
            (Wanted::Online, Reason::Always)
        );
        assert_eq!(
            decide(inputs(Mode::Always, IN_USE, true, false)),
            (Wanted::Online, Reason::Always)
        );
        assert_eq!(
            decide(inputs(Mode::Always, IN_USE, true, true)),
            (Wanted::Offline, Reason::OnBattery)
        );
    }

    #[test]
    fn no_answer_while_in_use_stays_offline() {
        // The popup timing out or "no" both leave run_taken false.
        assert_eq!(
            decide(inputs(Mode::Automatic, IN_USE, false, true)),
            (Wanted::Offline, Reason::InUse)
        );
    }

    #[test]
    fn every_case_of_the_table() {
        use Mode::*;
        use Reason as R;
        use Wanted::*;
        let table = [
            // mode, idle, battery, option -> wanted, reason
            (Automatic, IDLE, false, true, Online, R::Idle),
            (Automatic, JUST_UNDER, false, true, Offline, R::InUse),
            (Automatic, IN_USE, false, true, Offline, R::InUse),
            (Automatic, IDLE, true, true, Offline, R::OnBattery),
            (Automatic, IDLE, true, false, Online, R::Idle),
            (Automatic, IN_USE, true, false, Offline, R::InUse),
            (Disabled, IDLE, false, true, Offline, R::Disabled),
            (Disabled, IDLE, false, false, Offline, R::Disabled),
        ];
        for (mode, idle, battery, option, wanted, reason) in table {
            let got = decide(inputs(mode, idle, battery, option));
            assert_eq!(
                got,
                (wanted, reason),
                "{mode:?} idle={idle:?} battery={battery} option={option}"
            );
        }
    }
}
