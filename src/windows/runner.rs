//! Starts and stops one official GitHub runner (`bin\Runner.Listener.exe run`).
//!
//! The stop is a Ctrl+C to the runner's hidden console, so the runner tells
//! GitHub it is going offline. Killing it instead would leave it "online" on
//! GitHub for a while, and a job routed to it then would sit in the queue.
//!
//! The agent starts the listener itself, not `run.cmd`, so it also does what
//! `run.cmd` does when the listener exits on its own: after exit code 3 or 4
//! the runner is updating itself (its update script swaps the files and writes
//! `update.finished`); it is started again only once the update is done.

use super::job::Job;
use std::fs::{self, OpenOptions};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};
use switchable_ci_runner::parallel::{Exit, classify_exit};
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Console::{
    AttachConsole, CTRL_C_EVENT, FreeConsole, GenerateConsoleCtrlEvent, SetConsoleCtrlHandler,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, IDLE_PRIORITY_CLASS};

/// How long a graceful stop may take before the runner is killed.
const STOP_GRACE: Duration = Duration::from_secs(30);
/// The longest an update may take before the runner is started anyway.
const UPDATE_LIMIT: Duration = Duration::from_secs(5 * 60);
/// After a crash, the wait before the next start grows by this much, up to `RETRY_MAX`.
const RETRY_STEP: Duration = Duration::from_secs(30);
const RETRY_MAX: Duration = Duration::from_secs(5 * 60);
/// A runner up this long is healthy again: the next crash waits only `RETRY_STEP`.
const HEALTHY_AFTER: Duration = Duration::from_secs(2 * 60);

pub struct Runner {
    dir: PathBuf,
    log: PathBuf,
    child: Option<Child>,
    started_at: Option<Instant>,
    // Dropped after the runner stops: closing it kills anything its jobs left behind.
    job: Option<Job>,
    updating_since: Option<Instant>,
    retry_at: Option<Instant>,
    crashes: u32,
}

impl Runner {
    pub fn new(dir: PathBuf, log: PathBuf) -> Self {
        Runner {
            dir,
            log,
            child: None,
            started_at: None,
            job: None,
            updating_since: None,
            retry_at: None,
            crashes: 0,
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

    /// The listener's process id while it runs; its jobs are its child processes.
    pub fn pid(&mut self) -> Option<u32> {
        if self.is_alive() {
            self.child.as_ref().map(Child::id)
        } else {
            None
        }
    }

    /// Notices a listener that exited on its own (an update or a crash) and
    /// returns what happened, for the log. Call once per tick.
    pub fn check_exit(&mut self) -> Option<String> {
        let child = self.child.as_mut()?;
        let code = match child.try_wait() {
            Ok(None) => {
                if self
                    .started_at
                    .is_some_and(|t| t.elapsed() >= HEALTHY_AFTER)
                {
                    self.crashes = 0;
                }
                return None;
            }
            Ok(Some(status)) => status.code(),
            Err(_) => None,
        };
        self.child = None;
        self.started_at = None;
        match classify_exit(code) {
            Exit::Updating => {
                // The update script is in this runner's job: keep the job open
                // until it is done, or closing it would kill the update halfway.
                self.updating_since = Some(Instant::now());
                Some(format!("{}: updating itself", self.dir.display()))
            }
            Exit::Crashed => {
                self.job = None;
                self.delay_retry();
                let wait = self
                    .retry_at
                    .map(|t| t.saturating_duration_since(Instant::now()))
                    .unwrap_or_default();
                Some(format!(
                    "{}: runner exited ({code:?}); next start in {} s",
                    self.dir.display(),
                    wait.as_secs()
                ))
            }
        }
    }

    /// The runner's own update is still going. Done when the update script
    /// wrote `update.finished`, when nothing is left in the job, or after `UPDATE_LIMIT`.
    pub fn updating(&mut self) -> bool {
        let Some(since) = self.updating_since else {
            return false;
        };
        let flag = self.dir.join("update.finished");
        let done = flag.is_file()
            || self.job.as_ref().and_then(Job::active_processes) == Some(0)
            || since.elapsed() >= UPDATE_LIMIT;
        if !done {
            return true;
        }
        let _ = fs::remove_file(flag);
        self.job = None;
        self.updating_since = None;
        false
    }

    /// After a crash, the next start waits a little, so a broken runner does not spin.
    pub fn waiting_retry(&self) -> bool {
        self.retry_at.is_some_and(|t| Instant::now() < t)
    }

    /// A start that failed waits like a crash before the next try.
    pub fn delay_retry(&mut self) {
        self.crashes = self.crashes.saturating_add(1);
        self.retry_at = Some(Instant::now() + (RETRY_STEP * self.crashes).min(RETRY_MAX));
    }

    /// Starts the runner in the shared job (Idle priority, the CPU cap) and in its own.
    pub fn start(&mut self, shared: &Job) -> Result<(), String> {
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
        // Shared first: the runner's own job then nests inside it.
        let job = Job::per_runner().and_then(|own| {
            shared.assign(&child)?;
            own.assign(&child)?;
            Ok(own)
        });
        let job = match job {
            Ok(job) => job,
            Err(e) => {
                let mut child = child;
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        };
        self.child = Some(child);
        self.started_at = Some(Instant::now());
        self.job = Some(job);
        self.retry_at = None;
        Ok(())
    }

    pub fn stop(&mut self) {
        // Held until this function returns, after the runner has exited: then the
        // job closes and kills anything a CI job left running.
        let _job = self.job.take();
        self.started_at = None;
        self.updating_since = None;
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

/// The parent process of every `Runner.Worker.exe`: the runner starts one
/// worker per job, so a listener that is a parent here is running a job.
/// `None` when Windows cannot list processes; the caller then treats every
/// runner as busy, so none is cut mid-job.
pub fn job_parents() -> Option<Vec<u32>> {
    // SAFETY: a process snapshot; the handle is closed below.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut parents = Vec::new();
    // SAFETY: plain data; dwSize set as the API requires.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: `snap` is a valid snapshot and `entry` a valid, writable entry.
    let mut ok = unsafe { Process32FirstW(snap, &mut entry) } != 0;
    while ok {
        let len = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
        if name.eq_ignore_ascii_case("Runner.Worker.exe") {
            parents.push(entry.th32ParentProcessID);
        }
        // SAFETY: as above.
        ok = unsafe { Process32NextW(snap, &mut entry) } != 0;
    }
    // SAFETY: we own the snapshot handle and close it once.
    unsafe {
        CloseHandle(snap);
    }
    Some(parents)
}

fn send_ctrl_c(pid: u32) {
    // SAFETY: plain Win32 console calls on our own (GUI, console-less) process.
    // We attach to the runner's console, ignore Ctrl+C ourselves, send it, and detach.
    // Each runner has its own console, so the others do not get it.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_list_is_readable() {
        // No runner runs in the tests; the list is still there, just empty or not.
        assert!(job_parents().is_some());
    }
}
