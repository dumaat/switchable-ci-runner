//! Self-install: the downloaded .exe, run once, sets everything up and hands
//! over to the installed copy, which shows a balloon. No installer, no script.
//!
//! Steps: refuse to run elevated; ask which repository; download the official
//! GitHub runner and check its SHA-256; register it with the label `switchable`
//! using the owner's `gh`; copy this program to %LOCALAPPDATA%; start it at logon.
//!
//! The installer sets up runner 1. When the owner asks for more parallel jobs,
//! the agent adds runners 2, 3, ... the same way (`add_runner`), with runner 1's
//! repository and labels, and removes the extra ones when asked for fewer.

use super::Paths;
use sha2::{Digest, Sha256};
use std::fs;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use switchable_ci_runner::parallel::{slot_dir, slot_index, slot_name};
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
        let zip = runner_zip(&paths.data)?;
        extract(&zip, &paths.runner_dir)?;
        let name = format!(
            "{LABEL}-{}",
            std::env::var("COMPUTERNAME").unwrap_or_else(|_| "pc".into())
        );
        configure(&paths.runner_dir, &repo, &name, LABEL)?;
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

/// A field of the runner's own `.runner` file (JSON, with a byte-order mark).
fn runner_field(runner_dir: &Path, field: &str) -> Option<String> {
    let text = fs::read_to_string(runner_dir.join(".runner")).ok()?;
    let value: serde_json::Value =
        serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    Some(value.get(field)?.as_str()?.to_string())
}

/// The repository the runner is registered to, from the runner's own `.runner` file.
pub fn runner_repo(runner_dir: &Path) -> Option<String> {
    let url = runner_field(runner_dir, "gitHubUrl")?;
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

/// The latest official runner zip, downloaded once and checked against the
/// SHA-256 in its release notes; later runners reuse the checked file.
fn runner_zip(data: &Path) -> Result<PathBuf, String> {
    let tag = gh(&[
        "api",
        "repos/actions/runner/releases/latest",
        "--jq",
        ".tag_name",
    ])?;
    let version = tag.trim().trim_start_matches('v').to_string();
    let dir = data.join("download");
    let name = format!("actions-runner-win-x64-{version}.zip");
    let zip = dir.join(&name);
    // Only a file that passed the check is ever renamed to this name.
    if zip.is_file() {
        return Ok(zip);
    }
    let notes = gh(&[
        "api",
        &format!("repos/actions/runner/releases/tags/{}", tag.trim()),
        "--jq",
        ".body",
    ])?;
    let expected = published_sha(&notes)
        .ok_or_else(|| format!("no win-x64 SHA-256 in the runner {version} release notes"))?;

    fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
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
    Ok(zip)
}

fn extract(zip: &Path, dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    // Windows' own tar (bsdtar) reads zip files.
    run_quiet(
        "tar",
        &[
            "-xf",
            &zip.display().to_string(),
            "-C",
            &dir.display().to_string(),
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

fn configure(dir: &Path, repo: &str, name: &str, labels: &str) -> Result<(), String> {
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
            name,
            "--labels",
            labels,
            "--work",
            "_work",
            // A runner of the same name left on GitHub (a crash, a reinstall) is taken over.
            "--replace",
        ])
        .current_dir(dir)
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|e| format!("cannot run config.cmd: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("config.cmd failed ({status})"))
    }
}

/// Adds runner `k` next to runner 1: same repository, same custom labels (an
/// older install may use its own label), name `<runner 1's name>-k`.
pub fn add_runner(data: &Path, first: &Path, dir: &Path, k: usize) -> Result<(), String> {
    let repo = runner_repo(first).ok_or("runner 1 is not registered to a repository")?;
    let first_name = runner_field(first, "agentName").ok_or("runner 1 has no name")?;
    let labels = custom_labels(&repo, &first_name).unwrap_or_else(|| LABEL.to_string());
    if !dir.join("bin").join("Runner.Listener.exe").is_file() {
        let zip = runner_zip(data)?;
        extract(&zip, dir)?;
    }
    configure(dir, &repo, &slot_name(&first_name, k), &labels)
}

/// The labels a runner was registered with, minus the ones GitHub adds itself.
fn custom_labels(repo: &str, name: &str) -> Option<String> {
    let safe = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.');
    if !name.chars().all(safe) {
        return None;
    }
    let jq = format!(
        r#".runners[] | select(.name == "{name}") | .labels[] | select(.type == "custom") | .name"#
    );
    let out = gh(&[
        "api",
        "--paginate",
        &format!("repos/{repo}/actions/runners"),
        "--jq",
        &jq,
    ])
    .ok()?;
    let labels: Vec<&str> = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    (!labels.is_empty()).then(|| labels.join(","))
}

/// Unregisters a runner from GitHub. The files stay, so adding it back later
/// skips the download.
pub fn remove_runner(dir: &Path) -> Result<(), String> {
    let repo = runner_repo(dir).ok_or("the runner is not registered to a repository")?;
    let token = gh(&[
        "api",
        "-X",
        "POST",
        &format!("repos/{repo}/actions/runners/remove-token"),
        "--jq",
        ".token",
    ])?;
    let status = Command::new("cmd")
        .args(["/C", "config.cmd", "remove", "--token", token.trim()])
        .current_dir(dir)
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|e| format!("cannot run config.cmd: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("config.cmd remove failed ({status})"))
    }
}

/// The configured runner folders next to runner 1, by slot number.
pub fn runner_slots(first: &Path) -> Vec<usize> {
    let (Some(parent), Some(base)) = (first.parent(), first.file_name()) else {
        return Vec::new();
    };
    let base = base.to_string_lossy();
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut ks: Vec<usize> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().join(".runner").is_file())
        .filter_map(|e| slot_index(&base, &e.file_name().to_string_lossy()))
        .collect();
    ks.sort_unstable();
    ks
}

/// `switchable-ci-runner.exe --uninstall`: stops the agent and its runners,
/// removes it from startup, and unregisters every runner from GitHub.
/// The files stay in %LOCALAPPDATA%\switchable-ci-runner.
pub fn uninstall(paths: &Paths) -> Result<String, String> {
    stop_old_agent();
    let _ = run_quiet(
        "reg",
        &[
            "delete",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
            "/v",
            "switchable-ci-runner",
            "/f",
        ],
    );
    let slots = runner_slots(&paths.runner_dir);
    let errors: Vec<String> = slots
        .iter()
        .map(|&k| slot_dir(&paths.runner_dir, k))
        .filter_map(|dir| {
            remove_runner(&dir)
                .err()
                .map(|e| format!("{}: {e}", dir.display()))
        })
        .collect();
    if errors.is_empty() {
        Ok(format!(
            "Uninstalled: {} runner(s) removed from GitHub, and it no longer starts with Windows.\n\n\
             The files stay in {}; delete the folder by hand if you want them gone.",
            slots.len(),
            paths.data.display()
        ))
    } else {
        Err(format!(
            "Stopped and removed from startup, but some runners could not be removed from \
             GitHub (remove them in the repository's Settings > Actions > Runners):\n{}",
            errors.join("\n")
        ))
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
        assert_eq!(runner_repo(&dir).as_deref(), Some("octo-org/octo-repo"));
    }

    #[test]
    fn finds_only_configured_runner_folders() {
        let data = std::env::temp_dir().join(format!("scr-slots-{}", std::process::id()));
        let _ = fs::remove_dir_all(&data);
        for (folder, configured) in [
            ("runner", true),
            ("runner-2", false),
            ("runner-3", true),
            ("runner-x", true),
            ("download", true),
        ] {
            let dir = data.join(folder);
            fs::create_dir_all(&dir).unwrap();
            if configured {
                fs::write(dir.join(".runner"), "{}").unwrap();
            }
        }
        assert_eq!(runner_slots(&data.join("runner")), vec![1, 3]);
        let _ = fs::remove_dir_all(&data);
    }
}
