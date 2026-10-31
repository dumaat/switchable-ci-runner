//! The Windows agent: every few seconds, read idle time and power, decide,
//! and start or stop the runners (one per job that may run at once).
//! Between ticks, keep the tray responsive.

mod ask;
mod install;
mod job;
mod pat;
mod pool;
mod runner;
mod sys;
mod tray;

use pool::{Pool, Summary};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime};
use switchable_ci_runner::config::{self, Config};
use switchable_ci_runner::decision::{Inputs, Reason, Wanted, decide};
use switchable_ci_runner::parallel::{self, Hardware};
use switchable_ci_runner::runs::Runs;
use tray::{Action, Tray};

const TICK: Duration = Duration::from_secs(5);
const PUMP: Duration = Duration::from_millis(100);

pub struct Paths {
    pub data: PathBuf,
    pub config: PathBuf,
    pub agent_log: PathBuf,
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
        runner_dir: data.join("runner"),
        installed_exe: data.join("bin").join("switchable-ci-runner.exe"),
        data,
    })
}

/// Started from anywhere but the install folder, the program installs itself
/// and hands over to the installed copy; started from there, it is the agent.
/// `--uninstall`, from anywhere, undoes the install.
pub fn main() -> Result<(), String> {
    let paths = paths()?;
    if std::env::args().any(|a| a == "--uninstall") {
        let text = install::uninstall(&paths)?;
        tray::message("switchable-ci-runner", &text, false);
        return Ok(());
    }
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

/// The repository's runs, and the question in flight about some of them.
struct Asking {
    repo: Option<String>,
    snapshots: Option<Receiver<Vec<u64>>>,
    runs: Runs,
    answer: Option<Receiver<bool>>,
    asked: Vec<u64>,
}

/// The config as the owner left it, with `parallel_jobs` kept within the physical
/// cores (Auto stays Auto). A value outside is logged and shown.
fn checked(mut config: Config, paths: &Paths, hw: Hardware, log: &dyn Fn(&str)) -> Config {
    if config.runner_dir.as_os_str().is_empty() {
        config.runner_dir = paths.runner_dir.clone();
    }
    let (n, warning) = parallel::clamp(config.parallel_jobs, hw);
    if let Some(w) = warning {
        log(&w);
        tray::notify("switchable-ci-runner", &w, true);
    }
    config.parallel_jobs = n;
    config
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn run(paths: &Paths, just_installed: bool) -> Result<(), String> {
    let log = |line: &str| append_log(&paths.agent_log, line);
    let hw = sys::hardware();
    let standby = sys::modern_standby();
    let mut config = checked(
        config::load(&paths.config).map_err(|e| e.to_string())?,
        paths,
        hw,
        &log,
    );
    let mut stamp = modified(&paths.config);
    log(&format!(
        "agent started ({} parallel jobs; {} physical cores, {} GB, Auto {}; modern standby: {standby})",
        parallel::wanted(config.parallel_jobs, hw),
        hw.cores,
        hw.ram_bytes >> 30,
        parallel::auto_jobs(hw),
    ));

    let tray = Tray::new(&config, hw)?;
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
    let mut pool = Pool::new(
        config.runner_dir.clone(),
        paths.data.clone(),
        config.cpu_limit_percent,
    )?;
    let mut asking = Asking {
        snapshots: repo.clone().map(ask::watch_runs),
        repo,
        runs: Runs::default(),
        answer: None,
        asked: Vec::new(),
    };
    let mut custom_box: Option<Receiver<String>> = None;
    let mut last_tick: Option<Instant> = None;

    loop {
        sys::pump_messages();
        let mut changed = false;
        for action in tray.actions() {
            match action {
                Action::SetMode(mode) => config.mode = mode,
                Action::SetBatterySendsToGithub(on) => config.battery_sends_to_github = on,
                Action::SetCpuLimit(percent) => {
                    config.cpu_limit_percent = percent;
                    if let Err(e) = pool.set_cpu_cap(percent) {
                        log(&e);
                    }
                }
                Action::SetParallelJobs(n) => config.parallel_jobs = n,
                Action::AskParallelJobs => {
                    if custom_box.is_none() {
                        custom_box = Some(ask::ask_parallel_jobs(
                            parallel::wanted(config.parallel_jobs, hw),
                            parallel::max_jobs(hw),
                        ));
                    }
                    // The click toggled the checkmark; the number decides it.
                    tray.sync(&config);
                    continue;
                }
                Action::SetPat => {
                    set_pat_in_background(asking.repo.clone());
                    continue;
                }
                Action::Quit => {
                    log("quit from the menu");
                    pool.stop_all();
                    sys::keep_awake(false, standby);
                    return Ok(());
                }
            }
            changed = true;
        }
        if let Some(rx) = &custom_box
            && let Ok(text) = rx.try_recv()
        {
            custom_box = None;
            match parallel::parse_custom(&text, usize::from(parallel::max_jobs(hw))) {
                Ok(Some(n)) => {
                    config.parallel_jobs = n;
                    changed = true;
                }
                Ok(None) => {}
                Err(e) => tray::notify("switchable-ci-runner", &e, true),
            }
        }
        if changed {
            tray.sync(&config);
            if let Err(e) = config::save(&paths.config, &config) {
                log(&format!("cannot save config: {e}"));
            }
            stamp = modified(&paths.config);
        }
        // An answer acts at once, not at the next tick: the CI route step is waiting.
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
                asking.runs.take(&asking.asked);
            }
            asking.asked.clear();
            changed = true;
        }
        let due = last_tick.is_none_or(|t| t.elapsed() >= TICK);
        if due && modified(&paths.config) != stamp {
            stamp = modified(&paths.config);
            if reload(paths, hw, &mut config, &pool, &tray, &log) {
                changed = true;
            }
        }
        if changed || due {
            tick(&config, hw, standby, &mut pool, &mut asking, &tray, &log);
            last_tick = Some(Instant::now());
        }
        sleep(PUMP);
    }
}

/// The owner edited config.json by hand: it wins, as a menu click would.
/// A file that does not parse keeps the settings in use, with a message.
fn reload(
    paths: &Paths,
    hw: Hardware,
    config: &mut Config,
    pool: &Pool,
    tray: &Tray,
    log: &dyn Fn(&str),
) -> bool {
    match config::load(&paths.config) {
        Ok(new) => {
            let new = checked(new, paths, hw, log);
            if new.cpu_limit_percent != config.cpu_limit_percent
                && let Err(e) = pool.set_cpu_cap(new.cpu_limit_percent)
            {
                log(&e);
            }
            *config = Config {
                runner_dir: config.runner_dir.clone(),
                ..new
            };
            tray.sync(config);
            log("config file changed: reloaded");
            true
        }
        Err(e) => {
            let text = format!("{e}; keeping the previous settings");
            log(&text);
            tray::notify("switchable-ci-runner", &text, true);
            false
        }
    }
}

fn tick(
    config: &Config,
    hw: Hardware,
    standby: bool,
    pool: &mut Pool,
    asking: &mut Asking,
    tray: &Tray,
    log: &dyn Fn(&str),
) {
    // Runs that started since the last tick; drained every time so none is asked about late.
    let mut new_runs = Vec::new();
    if let Some(rx) = &asking.snapshots {
        for active in rx.try_iter() {
            new_runs.extend(asking.runs.update(&active));
        }
    }
    let inputs = Inputs {
        mode: config.mode,
        // Unknown idle time counts as "in use"; unknown power counts as battery.
        idle_for: sys::idle_for().unwrap_or(Duration::ZERO),
        on_battery: sys::on_battery().unwrap_or(true),
        battery_sends_to_github: config.battery_sends_to_github,
        run_taken: asking.runs.any_taken(),
    };
    let (wanted, reason) = decide(inputs);
    if !new_runs.is_empty() {
        if wanted == Wanted::Online {
            // The route step sees free runners: these runs come here, all their jobs.
            asking.runs.take(&new_runs);
        } else if reason == Reason::InUse
            && let Some(repo) = &asking.repo
        {
            asking.asked.extend(&new_runs);
            if asking.answer.is_none() {
                log("new run while in use: asking");
                asking.answer = Some(ask::ask(repo));
            }
        }
    }
    let summary = pool.tick(
        usize::from(parallel::wanted(config.parallel_jobs, hw)),
        wanted == Wanted::Online,
        log,
    );
    if !summary.configured {
        tray.show(
            &format!("Runner not configured in {}", config.runner_dir.display()),
            None,
            false,
        );
        return;
    }
    // A job anywhere keeps the PC awake (with Modern Standby, the screen too: standby
    // cuts the network); a busy runner is never stopped (`parallel::plan`).
    sys::keep_awake(summary.busy > 0, standby);
    tray.show(
        &status_line(reason, summary),
        Some(reason),
        summary.busy > 0,
    );
}

fn status_line(reason: Reason, summary: Summary) -> String {
    let base = match reason {
        Reason::Idle => "Online: taking jobs (idle)",
        Reason::RunTaken => "Online: finishing a run this PC took",
        Reason::Always => "Online: taking every job (Always)",
        Reason::InUse => "You are using the PC: I ask before each run",
        Reason::OnBattery => "Offline: on battery; jobs go to GitHub",
        Reason::Disabled => "Disabled: jobs go to GitHub",
    };
    match (summary.busy, summary.wanted) {
        (0, 1) => base.to_string(),
        (0, n) => format!("{base} ({n} runners)"),
        (1, 1) => format!("Running a job · {base}"),
        (b, n) => format!("Running {b} of {n} jobs · {base}"),
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
