//! Several jobs at once: one official runner per job, each in its own folder
//! (`runner`, `runner-2`, `runner-3`, ...). This module is the pure part: how
//! many runners, their names, and what to do with each one on a tick.

use std::path::{Path, PathBuf};

/// Quick choices in the tray menu; "Custom…" takes any number up to the cores.
pub const CHOICES: [u8; 4] = [1, 2, 4, 8];

/// The number the owner asked for, kept within 1..=cores. A value outside
/// comes back clamped with a warning: 0 never turns into "no runner at all"
/// (that is what Disabled is for), and more runners than cores only slows every job.
pub fn clamp(requested: u8, cores: usize) -> (u8, Option<String>) {
    let max = u8::try_from(cores.max(1)).unwrap_or(u8::MAX);
    if requested == 0 {
        (
            1,
            Some("parallel_jobs 0 is not valid; using 1 (use Disabled to run no jobs)".into()),
        )
    } else if requested > max {
        (
            max,
            Some(format!(
                "parallel_jobs {requested} is more than the {max} cores; using {max}"
            )),
        )
    } else {
        (requested, None)
    }
}

/// What the owner typed in the "Custom…" box. Empty is a cancel, not zero.
pub fn parse_custom(text: &str, cores: usize) -> Result<Option<u8>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let max = cores.max(1);
    match text.parse::<usize>() {
        Ok(n) if (1..=max).contains(&n) => Ok(u8::try_from(n).ok()),
        _ => Err(format!("\"{text}\" is not a number from 1 to {max}.")),
    }
}

/// Runner 1 keeps the folder older versions used, so an upgrade moves nothing.
pub fn slot_dir(base: &Path, k: usize) -> PathBuf {
    if k <= 1 {
        base.to_path_buf()
    } else {
        let name = base.file_name().map(|n| n.to_string_lossy().into_owned());
        base.with_file_name(format!("{}-{k}", name.as_deref().unwrap_or("runner")))
    }
}

/// The slot number of a folder next to `base`: `runner` is 1, `runner-3` is 3.
pub fn slot_index(base_name: &str, folder: &str) -> Option<usize> {
    if folder == base_name {
        return Some(1);
    }
    let k: usize = folder
        .strip_prefix(base_name)?
        .strip_prefix('-')?
        .parse()
        .ok()?;
    (k >= 2).then_some(k)
}

/// `runner.log`, `runner-2.log`, ...
pub fn slot_log(data: &Path, k: usize) -> PathBuf {
    if k <= 1 {
        data.join("runner.log")
    } else {
        data.join(format!("runner-{k}.log"))
    }
}

/// The name runner `k` registers with on GitHub: runner 1's name plus `-k`.
pub fn slot_name(first: &str, k: usize) -> String {
    if k <= 1 {
        first.to_string()
    } else {
        format!("{first}-{k}")
    }
}

/// What the agent knows about one runner on a tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Slot {
    pub k: usize,
    /// Configured in its folder (`config.cmd` ran): it exists on GitHub.
    pub installed: bool,
    /// The runner process is up.
    pub alive: bool,
    /// Running a job, or not known to be free: never stopped.
    pub busy: bool,
    /// The runner exited to update itself and the update has not finished.
    pub updating: bool,
    /// A registration or removal is in progress in the background.
    pub working: bool,
    /// After a crash, the runner waits a little before the next start.
    pub waiting_retry: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Start,
    Stop,
    Register,
    Unregister,
}

/// What to do with each runner, given how many the owner wants and whether
/// free runners should be online. A busy or updating runner is never touched:
/// a job that started is never cut, and an update is never interrupted.
pub fn plan(slots: &[Slot], wanted: usize, online: bool) -> Vec<(usize, Op)> {
    let mut ops = Vec::new();
    for s in slots {
        if s.busy || s.updating || s.working {
            continue;
        }
        let op = if s.k <= wanted {
            if !s.installed {
                Some(Op::Register)
            } else if online && !s.alive && !s.waiting_retry {
                Some(Op::Start)
            } else if !online && s.alive {
                Some(Op::Stop)
            } else {
                None
            }
        } else if s.alive {
            // More runners than wanted: this one goes once it is free.
            Some(Op::Stop)
        } else if s.installed {
            Some(Op::Unregister)
        } else {
            None
        };
        if let Some(op) = op {
            ops.push((s.k, op));
        }
    }
    ops
}

/// The slots to look at: 1..=wanted, plus any extra folder still configured.
pub fn slots_to_check(wanted: usize, found: &[usize]) -> Vec<usize> {
    let mut ks: Vec<usize> = (1..=wanted.max(1)).collect();
    for &k in found {
        if !ks.contains(&k) {
            ks.push(k);
        }
    }
    ks.sort_unstable();
    ks
}

/// How the official runner's exit code reads (its `run-helper.cmd` uses the
/// same codes): 3 and 4 mean "updating myself, start me again when done".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Updating,
    Crashed,
}

pub fn classify_exit(code: Option<i32>) -> Exit {
    match code {
        Some(3) | Some(4) => Exit::Updating,
        _ => Exit::Crashed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_keeps_values_in_range_and_warns() {
        assert_eq!(clamp(4, 16), (4, None));
        assert_eq!(clamp(16, 16), (16, None));
        let (n, w) = clamp(0, 16);
        assert_eq!(n, 1);
        assert!(w.is_some());
        let (n, w) = clamp(32, 16);
        assert_eq!(n, 16);
        assert!(w.is_some());
        // A machine that reports no cores still gets one runner.
        assert_eq!(clamp(1, 0), (1, None));
    }

    #[test]
    fn custom_box() {
        assert_eq!(parse_custom(" 6 ", 16), Ok(Some(6)));
        assert_eq!(parse_custom("", 16), Ok(None));
        assert_eq!(parse_custom("   ", 16), Ok(None));
        assert!(parse_custom("0", 16).is_err());
        assert!(parse_custom("17", 16).is_err());
        assert!(parse_custom("two", 16).is_err());
        assert!(parse_custom("-1", 16).is_err());
    }

    #[test]
    fn every_menu_choice_is_valid_on_a_big_machine() {
        for n in CHOICES {
            assert_eq!(clamp(n, 64), (n, None));
        }
    }

    #[test]
    fn folders_names_and_logs() {
        let base = Path::new(r"C:\data\runner");
        assert_eq!(slot_dir(base, 1), PathBuf::from(r"C:\data\runner"));
        assert_eq!(slot_dir(base, 3), PathBuf::from(r"C:\data\runner-3"));
        assert_eq!(slot_index("runner", "runner"), Some(1));
        assert_eq!(slot_index("runner", "runner-3"), Some(3));
        assert_eq!(slot_index("runner", "runner-1"), None);
        assert_eq!(slot_index("runner", "runner-x"), None);
        assert_eq!(slot_index("runner", "runners"), None);
        assert_eq!(slot_index("runner", "download"), None);
        let data = Path::new(r"C:\data");
        assert_eq!(slot_log(data, 1), data.join("runner.log"));
        assert_eq!(slot_log(data, 2), data.join("runner-2.log"));
        assert_eq!(slot_name("switchable-PC", 1), "switchable-PC");
        assert_eq!(slot_name("switchable-PC", 2), "switchable-PC-2");
    }

    fn slot(k: usize, installed: bool, alive: bool, busy: bool) -> Slot {
        Slot {
            k,
            installed,
            alive,
            busy,
            ..Slot::default()
        }
    }

    #[test]
    fn online_starts_every_wanted_runner_and_registers_missing_ones() {
        let slots = [
            slot(1, true, false, false),
            slot(2, true, true, false),
            slot(3, false, false, false),
        ];
        assert_eq!(
            plan(&slots, 3, true),
            vec![(1, Op::Start), (3, Op::Register)]
        );
    }

    #[test]
    fn offline_stops_only_free_runners() {
        let slots = [
            slot(1, true, true, true),
            slot(2, true, true, false),
            slot(3, true, false, false),
        ];
        assert_eq!(plan(&slots, 3, false), vec![(2, Op::Stop)]);
    }

    #[test]
    fn fewer_runners_removes_free_extras_and_waits_for_busy_ones() {
        let slots = [
            slot(1, true, true, false),
            slot(2, true, true, true),
            slot(3, true, true, false),
            slot(4, true, false, false),
        ];
        assert_eq!(
            plan(&slots, 1, true),
            vec![(3, Op::Stop), (4, Op::Unregister)]
        );
    }

    #[test]
    fn updating_working_and_retrying_runners_are_left_alone() {
        let updating = Slot {
            updating: true,
            ..slot(1, true, false, false)
        };
        let working = Slot {
            working: true,
            ..slot(2, false, false, false)
        };
        let retry = Slot {
            waiting_retry: true,
            ..slot(3, true, false, false)
        };
        assert_eq!(plan(&[updating, working, retry], 3, true), vec![]);
        // An extra runner being updated is removed only after the update.
        let extra = Slot {
            updating: true,
            ..slot(4, true, false, false)
        };
        assert_eq!(plan(&[extra], 1, true), vec![]);
    }

    #[test]
    fn extra_folders_are_checked_until_removed() {
        assert_eq!(slots_to_check(2, &[1, 2, 5]), vec![1, 2, 5]);
        assert_eq!(slots_to_check(0, &[]), vec![1]);
    }

    #[test]
    fn exit_codes() {
        assert_eq!(classify_exit(Some(3)), Exit::Updating);
        assert_eq!(classify_exit(Some(4)), Exit::Updating);
        assert_eq!(classify_exit(Some(2)), Exit::Crashed);
        assert_eq!(classify_exit(None), Exit::Crashed);
    }
}
