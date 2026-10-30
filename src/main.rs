// No console window: the program is a tray icon.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod windows;

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    match windows::main() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            if let Ok(paths) = windows::paths() {
                windows::append_log(&paths.agent_log, &format!("fatal: {e}"));
            }
            // A GUI process has no console: the owner sees the error in a message box.
            show_error(&e);
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(windows)]
fn show_error(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    let wide = |s: &str| {
        s.encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<u16>>()
    };
    let (text, caption) = (wide(text), wide("switchable-ci-runner"));
    // SAFETY: both strings are NUL-terminated UTF-16 that outlive the call.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("switchable-ci-runner: only Windows is supported so far; Linux is the next slice.");
    std::process::ExitCode::FAILURE
}
