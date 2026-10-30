// No console window when started from the Startup entry.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod windows;

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    match windows::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            // A GUI process has no console to print to; the agent log is the record.
            if let Some(dir) = std::env::var_os("LOCALAPPDATA") {
                let path = std::path::Path::new(&dir)
                    .join("switchable-ci-runner")
                    .join("agent.log");
                let _ = std::fs::write(path, format!("fatal: {e}\n"));
            }
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("switchable-ci-runner: only Windows is supported so far; Linux is the next slice.");
    std::process::ExitCode::FAILURE
}
