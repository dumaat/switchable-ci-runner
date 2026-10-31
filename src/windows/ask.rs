//! Mode 2: while the owner is using the PC, a new CI run triggers a question.
//! The watcher lists the repository's active runs with the owner's `gh`; the
//! question is a message box that closes itself after 45 s, and no answer means GitHub.

use std::os::windows::process::CommandExt;
use std::process::Command;
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::Duration;
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_ICONQUESTION, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO,
};

pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(45);
const POLL: Duration = Duration::from_secs(10);

#[link(name = "user32")]
unsafe extern "system" {
    // Exported by user32 since Windows XP, though not in the public headers.
    fn MessageBoxTimeoutW(
        hwnd: *mut core::ffi::c_void,
        text: *const u16,
        caption: *const u16,
        style: u32,
        language: u16,
        milliseconds: u32,
    ) -> i32;
}

/// The ids of the repository's active runs, a fresh list every poll. Which
/// ones are new, and which ones this machine took, is `runs::Runs`'s job.
pub fn watch_runs(repo: String) -> Receiver<Vec<u64>> {
    let (tx, rx) = channel();
    thread::spawn(move || {
        loop {
            // A failed poll sends nothing: the last list stays the agent's view.
            if let Some(ids) = active_runs(&repo)
                && tx.send(ids).is_err()
            {
                return;
            }
            thread::sleep(POLL);
        }
    });
    rx
}

fn active_runs(repo: &str) -> Option<Vec<u64>> {
    let out = Command::new("gh")
        .args([
            "api",
            &format!("repos/{repo}/actions/runs?per_page=30"),
            "--jq",
            r#".workflow_runs[] | select(.status=="queued" or .status=="in_progress") | .id"#,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .collect(),
    )
}

/// Asks in the background; the receiver gets `true` only for an explicit "Yes".
pub fn ask(repo: &str) -> Receiver<bool> {
    let (tx, rx) = channel();
    let text = format!(
        "A CI run started in {repo}.\n\nRun its jobs on this PC?\n\n\
         No answer in {} s sends them to GitHub.",
        ANSWER_TIMEOUT.as_secs()
    );
    thread::spawn(move || {
        let text = wide(&text);
        let caption = wide("switchable-ci-runner");
        // SAFETY: both strings are NUL-terminated UTF-16 that outlive the call.
        let answer = unsafe {
            MessageBoxTimeoutW(
                std::ptr::null_mut(),
                text.as_ptr(),
                caption.as_ptr(),
                MB_YESNO | MB_ICONQUESTION | MB_TOPMOST | MB_SETFOREGROUND,
                0,
                ANSWER_TIMEOUT.as_millis() as u32,
            )
        };
        let _ = tx.send(answer == IDYES);
    });
    rx
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// "Parallel jobs > Custom…": a box for the number, off the tray thread.
/// The receiver gets what was typed; an empty text is a cancel.
pub fn ask_parallel_jobs(current: u8, cores: usize) -> Receiver<String> {
    let (tx, rx) = channel();
    let script = format!(
        "Add-Type -AssemblyName Microsoft.VisualBasic; \
         [Microsoft.VisualBasic.Interaction]::InputBox(\
         'How many CI jobs may run at once on this PC? (1 to {cores}; one runner each)', \
         'switchable-ci-runner', '{current}')"
    );
    thread::spawn(move || {
        let text = Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        let _ = tx.send(text);
    });
    rx
}
