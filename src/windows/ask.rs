//! Mode 2: while the owner is using the PC, a new CI run triggers a question.
//! The watcher lists the repository's active runs with the owner's `gh`; the
//! question is a message box that closes itself after 45 s, and no answer means GitHub.

use std::collections::HashSet;
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

/// Ids of runs that became active after the agent started. Runs already
/// active at start are not asked about: they were routed before we existed.
pub fn watch_runs(repo: String) -> Receiver<u64> {
    let (tx, rx) = channel();
    thread::spawn(move || {
        let mut seen = HashSet::new();
        let mut first = true;
        loop {
            if let Some(ids) = active_runs(&repo) {
                for id in ids {
                    if seen.insert(id) && !first && tx.send(id).is_err() {
                        return;
                    }
                }
                first = false;
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
            &format!("repos/{repo}/actions/runs?per_page=10"),
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
