//! The Windows agent: every few seconds, read idle time and power, decide,
//! and start or stop the runner. Between ticks, keep the tray responsive.

mod ask;
mod install;
mod job;
mod pat;
mod runner;
mod sys;
mod tray;

use runner::Runner;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::thread::sleep;
use std::time::{Duration, Instant};
use switchable_ci_runner::config::{self, Config};
use switchable_ci_runner::decision::{Inputs, Reason, Wanted, decide};
use tray::{Action, Tray};

const TICK: Duration = Duration::from_secs(5);
const PUMP: Duration = Duration::from_millis(100);
/// After a "yes", how long the runner stays up waiting for the job to arrive.
const YES_WINDOW: Duration = Duration::from_secs(10 * 60);

pub struct Paths {
    pub data: PathBuf,
    pub config: PathBuf,
    pub agent_log: PathBuf,
    pub runner_log: PathBuf,
    pub runner_dir: PathBuf,
    pub installed_exe: PathBuf,
}

pub fn paths() -> Result<Paths, String> {
    let roaming = std::env::var_os("APPDATA").ok_or("APPDATA is not set")?;
    let local = std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is not set")?;
    let data = PathBuf::from(local).join("switchable-ci-runner");
    fs::create_dir_all(&data).map_err(|e| format!("cannot create {}: {e}", data.display()))?;
    Ok(Paths {
        config: PathBuf::from(roaming)
            .join("switchable-ci-runner")
            .join("config.json"),
        agent_log: data.join("agent.log"),
        runner_log: data.join("runner.log"),
        runner_dir: data.join("runner"),
        installed_exe: data.join("bin").join("switchable-ci-runner.exe"),
        data,
    })
}

/// Started from anywhere but the install folder, the program installs itself
/// and hands over to the installed copy; started from there, it is the agent.
pub fn main() -> Result<(), String> {
    let paths = paths()?;
    let me = std::env::current_exe().map_err(|e| format!("cannot find myself: {e}"))?;
    if !same_file(&me, &paths.installed_exe) {
        return install::install(&paths, &me);
    }
    let just_installed = std::env::args().any(|a| a == "--installed");
    run(&paths, just_installed)
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// The question in flight and the "yes" that is still valid.
struct Asking {
    repo: Option<String>,
    new_runs: Option<Receiver<u64>>,
    answer: Option<Receiver<bool>>,
    yes_until: Option<Instant>,
}

fn run(paths: &Paths, just_installed: bool) -> Result<(), String> {
    let mut config = config::load(&paths.config).map_err(|e| e.to_string())?;
    if config.runner_dir.as_os_str().is_empty() {
        config.runner_dir = paths.runner_dir.clone();
    }
    let log = |line: &str| append_log(&paths.agent_log, line);
    log("agent started");

    let tray = Tray::new(&config)?;
    let repo = install::runner_repo(&config.runner_dir);
    if just_installed {
        tray::notify(
            "switchable-ci-runner is installed",
            &format!(
                "Installed for {}.\n\n\
                 Find it in the tray, bottom right of the taskbar: the colored dot \
                 (if hidden, click the ^ arrow). Click it for the options.\n\n\
                 It starts with Windows. Files and logs:\n{}",
                repo.as_deref().unwrap_or("your repository"),
                paths.data.display()
            ),
            false,
        );
    }
    let mut runner = Runner::new(config.runner_dir.clone(), paths.runner_log.clone());
    let mut asking = Asking {
        new_runs: repo.clone().map(ask::watch_runs),
        repo,
        answer: None,
        yes_until: None,
    };
    let mut last_tick: Option<Instant> = None;

    loop {
        sys::pump_messages();
        let mut changed = false;
        for action in tray.actions() {
            match action {
                Action::SetMode(mode) => config.mode = mode,
                Action::SetBatterySendsToGithub(on) => config.battery_sends_to_github = on,
                Action::SetCpuLimit(percent) => config.cpu_limit_percent = percent,
                Action::SetPat => {
                    set_pat_in_background(asking.repo.clone());
                    continue;
                }
                Action::Quit => {
                    log("quit from the menu");
                    runner.stop();
                    sys::keep_awake(false);
                    return Ok(());
                }
            }
            changed = true;
        }
        if changed && let Err(e) = config::save(&paths.config, &config) {
            log(&format!("cannot save config: {e}"));
        }
        // An answer acts at once, not at the next tick: the CI router is waiting.
        if let Some(rx) = &asking.answer
            && let Ok(yes) = rx.try_recv()
        {
            asking.answer = None;
            log(if yes {
                "owner said yes"
            } else {
                "owner said no or did not answer"
            });
            if yes {
                asking.yes_until = Some(Instant::now() + YES_WINDOW);
            }
            changed = true;
        }
        if changed || last_tick.is_none_or(|t| t.elapsed() >= TICK) {
            tick(&config, &mut runner, &mut asking, &tray, &log);
            last_tick = Some(Instant::now());
        }
        sleep(PUMP);
    }
}

fn tick(
    config: &Config,
    runner: &mut Runner,
    asking: &mut Asking,
    tray: &Tray,
    log: &dyn Fn(&str),
) {
    // Runs that started since the last tick; drained every time so none is asked about late.
    let new_runs = asking
        .new_runs
        .as_ref()
        .map(|rx| rx.try_iter().count())
        .unwrap_or(0);
    if !runner.is_installed() {
        tray.show(
            &format!("Runner not configured in {}", runner.dir().display()),
            None,
        );
        return;
    }
    let alive = runner.is_alive();
    let job_running = alive && runner::job_running();
    if job_running {
        // The job the "yes" was for has arrived; the next one gets its own question.
        asking.yes_until = None;
    }
    let inputs = Inputs {
        mode: config.mode,
        // Unknown idle time counts as "in use"; unknown power counts as battery.
        idle_for: sys::idle_for().unwrap_or(Duration::ZERO),
        on_battery: sys::on_battery().unwrap_or(true),
        battery_sends_to_github: config.battery_sends_to_github,
        job_running,
        owner_said_yes: asking.yes_until.is_some_and(|t| Instant::now() < t),
    };
    let (wanted, reason) = decide(inputs);
    if reason == Reason::InUse
        && new_runs > 0
        && asking.answer.is_none()
        && let Some(repo) = &asking.repo
    {
        log("new run while in use: asking");
        asking.answer = Some(ask::ask(repo));
    }
    match (wanted, alive) {
        (Wanted::Online, false) => match runner.start(config.cpu_limit_percent) {
            Ok(()) => log(&format!("runner started ({reason:?})")),
            Err(e) => {
                log(&e);
                tray.show(&format!("Cannot start the runner: {e}"), None);
                return;
            }
        },
        (Wanted::Offline, true) => {
            runner.stop();
            log(&format!("runner stopped ({reason:?})"));
        }
        _ => {}
    }
    sys::keep_awake(job_running);
    tray.show(status_line(reason), Some(reason));
}

fn status_line(reason: Reason) -> &'static str {
    match reason {
        Reason::JobRunning => "Running a job",
        Reason::Idle => "Online: taking jobs (idle)",
        Reason::OwnerSaidYes => "Online: you said yes",
        Reason::Always => "Online: taking every job (Always)",
        Reason::InUse => "You are using the PC: I ask before each run",
        Reason::OnBattery => "Offline: on battery; jobs go to GitHub",
        Reason::Disabled => "Disabled: jobs go to GitHub",
    }
}

pub fn append_log(path: &Path, line: &str) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{now} {line}");
    }
}

/// The token box and `gh` run off the tray thread; the result is a message box.
fn set_pat_in_background(repo: Option<String>) {
    let Some(repo) = repo else {
        tray::notify(
            "switchable-ci-runner",
            "The runner is not registered to a repository yet.",
            true,
        );
        return;
    };
    std::thread::spawn(move || match pat::set_pat(&repo) {
        Ok(true) => tray::notify(
            "switchable-ci-runner",
            &format!("Saved as the secret {} of {repo}.", pat::SECRET),
            false,
        ),
        Ok(false) => {}
        Err(e) => tray::notify(
            "switchable-ci-runner",
            &format!("Could not save the token: {e}"),
            true,
        ),
    });
}
