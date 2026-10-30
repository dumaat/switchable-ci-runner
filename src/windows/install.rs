//! Self-install: the downloaded .exe, run once, sets everything up and hands
//! over to the installed copy, which shows a balloon. No installer, no script.
//!
//! Steps: refuse to run elevated; ask which repository; download the official
//! GitHub runner and check its SHA-256; register it with the label `switchable`
//! using the owner's `gh`; copy this program to %LOCALAPPDATA%; start it at logon.

use super::Paths;
use sha2::{Digest, Sha256};
use std::fs;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, DETACHED_PROCESS};

pub const LABEL: &str = "switchable";

pub fn install(paths: &Paths, me: &Path) -> Result<(), String> {
    if elevated() {
        return Err(
            "Run me without \"Run as administrator\": CI jobs would get admin rights.".into(),
        );
    }
    gh(&["auth", "status"]).map_err(|_| {
        "Install the GitHub CLI (winget install GitHub.cli) and run: gh auth login".to_string()
    })?;
    if runner_repo(&paths.runner_dir).is_none() {
        let repo = ask_repo()?;
        download_runner(paths)?;
        register_runner(paths, &repo)?;
    }
    stop_old_agent();
    let bin = paths
        .installed_exe
        .parent()
        .ok_or("install folder has no parent")?;
    fs::create_dir_all(bin).map_err(|e| format!("cannot create {}: {e}", bin.display()))?;
    fs::copy(me, &paths.installed_exe)
        .map_err(|e| format!("cannot copy to {}: {e}", paths.installed_exe.display()))?;
    let exe = paths.installed_exe.display().to_string();
    run_quiet(
        "reg",
        &[
            "add",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
            "/v",
            "switchable-ci-runner",
            "/d",
            &format!("\"{exe}\""),
            "/f",
        ],
    )?;
    Command::new(&paths.installed_exe)
        .arg("--installed")
        .creation_flags(DETACHED_PROCESS)
        .spawn()
        .map_err(|e| format!("cannot start {exe}: {e}"))?;
    Ok(())
}

/// The repository the runner is registered to, from the runner's own `.runner` file.
pub fn runner_repo(runner_dir: &Path) -> Option<String> {
    let text = fs::read_to_string(runner_dir.join(".runner")).ok()?;
    let value: serde_json::Value =
        serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    let url = value.get("gitHubUrl")?.as_str()?;
    let repo = url
        .trim_end_matches('/')
        .strip_prefix("https://github.com/")?;
    is_repo(repo).then(|| repo.to_string())
}

fn is_repo(s: &str) -> bool {
    let mut parts = s.split('/');
    let ok = |p: Option<&str>| {
        p.is_some_and(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
    };
    ok(parts.next()) && ok(parts.next()) && parts.next().is_none()
}

fn ask_repo() -> Result<String, String> {
    let script = "Add-Type -AssemblyName Microsoft.VisualBasic; \
        [Microsoft.VisualBasic.Interaction]::InputBox(\
        'Which GitHub repository should this PC run CI for? (owner/name)', \
        'switchable-ci-runner')";
    let out = Command::new("powershell")
        .args(["-NoProfile", "-Command", script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("cannot ask for the repository: {e}"))?;
    let repo = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if repo.is_empty() {
        return Err("Installation cancelled: no repository given.".into());
    }
    if !is_repo(&repo) {
        return Err(format!("\"{repo}\" is not owner/name."));
    }
    Ok(repo)
}

fn download_runner(paths: &Paths) -> Result<(), String> {
    let tag = gh(&[
        "api",
        "repos/actions/runner/releases/latest",
        "--jq",
        ".tag_name",
    ])?;
    let version = tag.trim().trim_start_matches('v').to_string();
    let notes = gh(&[
        "api",
        &format!("repos/actions/runner/releases/tags/{}", tag.trim()),
        "--jq",
        ".body",
    ])?;
    let expected = published_sha(&notes)
        .ok_or_else(|| format!("no win-x64 SHA-256 in the runner {version} release notes"))?;

    let dir = paths.data.join("download");
    fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let name = format!("actions-runner-win-x64-{version}.zip");
    let zip = dir.join(&name);
    let part = dir.join(format!("{name}.part"));
    let url = format!("https://github.com/actions/runner/releases/download/v{version}/{name}");
    run_quiet(
        "curl.exe",
        &["-fsSL", "-o", &part.display().to_string(), &url],
    )?;
    let bytes = fs::read(&part).map_err(|e| format!("cannot read the download: {e}"))?;
    let got = hex(&Sha256::digest(&bytes));
    if got != expected {
        let _ = fs::remove_file(&part);
        return Err(format!(
            "runner download failed the SHA-256 check (expected {expected}, got {got})"
        ));
    }
    fs::rename(&part, &zip).map_err(|e| format!("cannot keep the download: {e}"))?;
    fs::create_dir_all(&paths.runner_dir)
        .map_err(|e| format!("cannot create {}: {e}", paths.runner_dir.display()))?;
    // Windows' own tar (bsdtar) reads zip files.
    run_quiet(
        "tar",
        &[
            "-xf",
            &zip.display().to_string(),
            "-C",
            &paths.runner_dir.display().to_string(),
        ],
    )
}

/// The runner's release notes carry `<!-- BEGIN SHA win-x64 -->hash<!-- END SHA win-x64 -->`.
fn published_sha(notes: &str) -> Option<String> {
    let start = notes.find("<!-- BEGIN SHA win-x64 -->")? + "<!-- BEGIN SHA win-x64 -->".len();
    let hash: String = notes[start..].chars().take(64).collect();
    (hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit())).then(|| hash.to_lowercase())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn register_runner(paths: &Paths, repo: &str) -> Result<(), String> {
    let token = gh(&[
        "api",
        "-X",
        "POST",
        &format!("repos/{repo}/actions/runners/registration-token"),
        "--jq",
        ".token",
    ])
    .map_err(|e| {
        format!("cannot get a registration token for {repo} (are you an admin of it?): {e}")
    })?;
    let name = format!(
        "{LABEL}-{}",
        std::env::var("COMPUTERNAME").unwrap_or_else(|_| "pc".into())
    );
    let status = Command::new("cmd")
        .args([
            "/C",
            "config.cmd",
            "--unattended",
            "--url",
            &format!("https://github.com/{repo}"),
            "--token",
            token.trim(),
            "--name",
            &name,
            "--labels",
            LABEL,
            "--work",
            "_work",
            "--replace",
        ])
        .current_dir(&paths.runner_dir)
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|e| format!("cannot run config.cmd: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("config.cmd failed ({status})"))
    }
}

/// An older copy of the agent (and the runner it started) would keep the files locked.
fn stop_old_agent() {
    let me = std::process::id().to_string();
    let _ = run_quiet(
        "taskkill",
        &[
            "/F",
            "/IM",
            "switchable-ci-runner.exe",
            "/FI",
            &format!("PID ne {me}"),
        ],
    );
    let _ = run_quiet("taskkill", &["/F", "/IM", "Runner.Listener.exe"]);
}

/// High integrity level ("Run as administrator") shows up as this SID in the token groups.
fn elevated() -> bool {
    Command::new("whoami")
        .arg("/groups")
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("S-1-16-12288"))
        .unwrap_or(false)
}

fn gh(args: &[&str]) -> Result<String, String> {
    let out = Command::new("gh")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("cannot run gh: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn run_quiet(program: &str, args: &[&str]) -> Result<(), String> {
    let status = Command::new(program)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} failed ({status})"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_published_sha() {
        let notes = "x <!-- BEGIN SHA win-x64 -->A1F17AA987FC8158BC7521D284912267A0D01B7C2C4684B33C86783B5E8221CC<!-- END SHA win-x64 --> y";
        assert_eq!(
            published_sha(notes).as_deref(),
            Some("a1f17aa987fc8158bc7521d284912267a0d01b7c2c4684b33c86783b5e8221cc")
        );
        assert_eq!(published_sha("no hash here"), None);
        assert_eq!(published_sha("<!-- BEGIN SHA win-x64 -->abc"), None);
    }

    #[test]
    fn repo_names() {
        assert!(is_repo("octo-org/octo-repo"));
        assert!(!is_repo("dumaat"));
        assert!(!is_repo("a/b/c"));
        assert!(!is_repo("a/b c"));
        assert!(!is_repo("/b"));
    }

    #[test]
    fn repo_from_runner_file_with_bom() {
        let dir = std::env::temp_dir().join(format!("scr-runner-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(".runner"),
            "\u{feff}{\"gitHubUrl\": \"https://github.com/octo-org/octo-repo\"}",
        )
        .unwrap();
        assert_eq!(
            runner_repo(&dir).as_deref(),
            Some("octo-org/octo-repo")
        );
    }
}
