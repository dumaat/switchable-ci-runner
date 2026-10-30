//! Starts and stops the official GitHub runner (`bin\Runner.Listener.exe run`).
//!
//! The stop is a Ctrl+C to the runner's hidden console, so the runner tells
//! GitHub it is going offline. Killing it instead would leave it "online" on
//! GitHub for a while, and a job routed to it then would sit in the queue.

use super::job::Job;
use std::fs::OpenOptions;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};
use windows_sys::Win32::System::Console::{
    AttachConsole, CTRL_C_EVENT, FreeConsole, GenerateConsoleCtrlEvent, SetConsoleCtrlHandler,
};
use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, IDLE_PRIORITY_CLASS};

/// How long a graceful stop may take before the runner is killed.
const STOP_GRACE: Duration = Duration::from_secs(30);

pub struct Runner {
    dir: PathBuf,
    log: PathBuf,
    child: Option<Child>,
    // Dropped after the runner stops: closing it kills anything a job left behind.
    job: Option<Job>,
}

impl Runner {
    pub fn new(dir: PathBuf, log: PathBuf) -> Self {
        Runner {
            dir,
            log,
            child: None,
            job: None,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn listener(&self) -> PathBuf {
        self.dir.join("bin").join("Runner.Listener.exe")
    }

    /// The runner was configured (`config.cmd`) in this folder.
    pub fn is_installed(&self) -> bool {
        self.listener().is_file() && self.dir.join(".runner").is_file()
    }

    pub fn is_alive(&mut self) -> bool {
        match &mut self.child {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => false,
        }
    }

    /// Starts the runner at the lowest priority, with an optional hard CPU cap.
    /// A new cap applies from the next start, never in the middle of a job.
    pub fn start(&mut self, cpu_limit_percent: u8) -> Result<(), String> {
        if self.is_alive() {
            return Ok(());
        }
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log)
            .map_err(|e| format!("cannot open {}: {e}", self.log.display()))?;
        let err = log
            .try_clone()
            .map_err(|e| format!("cannot open {}: {e}", self.log.display()))?;
        let child = Command::new(self.listener())
            .arg("run")
            .current_dir(&self.dir)
            // A console with no window: invisible, but Ctrl+C can still reach it.
            .creation_flags(CREATE_NO_WINDOW | IDLE_PRIORITY_CLASS)
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(err)
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", self.listener().display()))?;
        let job = Job::new(cpu_limit_percent)?;
        if let Err(e) = job.assign(&child) {
            let mut child = child;
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        self.child = Some(child);
        self.job = Some(job);
        Ok(())
    }

    pub fn stop(&mut self) {
        // Held until this function returns, after the runner has exited: then the
        // job closes and kills anything a CI job left running.
        let _job = self.job.take();
        let Some(mut child) = self.child.take() else {
            return;
        };
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        send_ctrl_c(child.id());
        let deadline = Instant::now() + STOP_GRACE;
        while Instant::now() < deadline {
            if matches!(child.try_wait(), Ok(Some(_)) | Err(_)) {
                return;
            }
            sleep(Duration::from_millis(200));
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// A job is executing: the runner starts `Runner.Worker.exe` for each job.
pub fn job_running() -> bool {
    let output = Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq Runner.Worker.exe", "/NH"])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    match output {
        Ok(out) => String::from_utf8_lossy(&out.stdout).contains("Runner.Worker.exe"),
        // Unknown counts as running, so the runner is never cut mid-job.
        Err(_) => true,
    }
}

fn send_ctrl_c(pid: u32) {
    // SAFETY: plain Win32 console calls on our own (GUI, console-less) process.
    // We attach to the runner's console, ignore Ctrl+C ourselves, send it, and detach.
    unsafe {
        FreeConsole();
        if AttachConsole(pid) == 0 {
            return;
        }
        SetConsoleCtrlHandler(None, 1);
        GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0);
        FreeConsole();
    }
    // Give the signal time to arrive before we stop ignoring it.
    sleep(Duration::from_millis(500));
    // SAFETY: restores the default handling for this process.
    unsafe {
        SetConsoleCtrlHandler(None, 0);
    }
}
