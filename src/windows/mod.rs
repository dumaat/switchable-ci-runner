//! The Windows agent: every few seconds, read idle time and power, decide,
//! and start or stop the runner. Between ticks, keep the tray responsive.

mod runner;
mod sys;
mod tray;

use runner::Runner;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::thread::sleep;
use std::time::{Duration, Instant};
use switchable_ci_runner::config::{self, Config};
use switchable_ci_runner::decision::{Inputs, Reason, Wanted, decide};
use tray::{Action, Tray};

const TICK: Duration = Duration::from_secs(5);
const PUMP: Duration = Duration::from_millis(100);

struct Paths {
    config: PathBuf,
    agent_log: PathBuf,
    runner_log: PathBuf,
    default_runner_dir: PathBuf,
}

fn paths() -> Result<Paths, String> {
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
        default_runner_dir: data.join("runner"),
    })
}

pub fn run() -> Result<(), String> {
    let paths = paths()?;
    let mut config = config::load(&paths.config).map_err(|e| e.to_string())?;
    if config.runner_dir.as_os_str().is_empty() {
        config.runner_dir = paths.default_runner_dir.clone();
    }
    let log = |line: &str| append_log(&paths.agent_log, line);
    log("agent started");

    let tray = Tray::new(config.mode, config.battery_sends_to_github)?;
    let mut runner = Runner::new(config.runner_dir.clone(), paths.runner_log.clone());
    let mut last_tick: Option<Instant> = None;

    loop {
        sys::pump_messages();
        let mut changed = false;
        for action in tray.actions() {
            match action {
                Action::SetMode(mode) => config.mode = mode,
                Action::SetBatterySendsToGithub(on) => config.battery_sends_to_github = on,
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
        if changed || last_tick.is_none_or(|t| t.elapsed() >= TICK) {
            tick(&config, &mut runner, &tray, &log);
            last_tick = Some(Instant::now());
        }
        sleep(PUMP);
    }
}

fn tick(config: &Config, runner: &mut Runner, tray: &Tray, log: &dyn Fn(&str)) {
    if !runner.is_installed() {
        tray.show(
            &format!("Runner not configured in {}", runner.dir().display()),
            None,
        );
        return;
    }
    let alive = runner.is_alive();
    let job_running = alive && runner::job_running();
    let inputs = Inputs {
        mode: config.mode,
        // Unknown idle time counts as "in use"; unknown power counts as battery.
        idle_for: sys::idle_for().unwrap_or(Duration::ZERO),
        on_battery: sys::on_battery().unwrap_or(true),
        battery_sends_to_github: config.battery_sends_to_github,
        job_running,
    };
    let (wanted, reason) = decide(inputs);
    match (wanted, alive) {
        (Wanted::Online, false) => match runner.start() {
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
        Reason::InUse => "Offline: you are using the PC; jobs go to GitHub",
        Reason::OnBattery => "Offline: on battery; jobs go to GitHub",
        Reason::Disabled => "Disabled: jobs go to GitHub",
    }
}

fn append_log(path: &PathBuf, line: &str) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{now} {line}");
    }
}
