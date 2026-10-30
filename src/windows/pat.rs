//! "Set CI token (PAT)…": the route step in CI needs a token that can list the
//! repository's runners. The owner pastes it into a masked box; it goes straight
//! to the repository secret RUNNER_STATUS_TOKEN through `gh` and is not kept here.

use std::io::Write;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

pub const SECRET: &str = "RUNNER_STATUS_TOKEN";

const PROMPT: &str = r#"
Add-Type -AssemblyName System.Windows.Forms
$f = New-Object Windows.Forms.Form
$f.Text = 'switchable-ci-runner'; $f.Width = 470; $f.Height = 180
$f.TopMost = $true; $f.StartPosition = 'CenterScreen'; $f.FormBorderStyle = 'FixedDialog'
$l = New-Object Windows.Forms.Label
$l.Text = 'Fine-grained PAT for this repository (Administration: read). It is saved as the repository secret RUNNER_STATUS_TOKEN and not kept on this PC.'
$l.SetBounds(10, 10, 440, 40)
$t = New-Object Windows.Forms.TextBox
$t.UseSystemPasswordChar = $true; $t.SetBounds(10, 55, 430, 22)
$b = New-Object Windows.Forms.Button
$b.Text = 'Save'; $b.SetBounds(360, 90, 80, 26); $b.DialogResult = 'OK'; $f.AcceptButton = $b
$f.Controls.AddRange(@($l, $t, $b))
if ($f.ShowDialog() -eq 'OK') { [Console]::Out.Write($t.Text) }
"#;

/// Ok(true) saved, Ok(false) cancelled by the owner.
pub fn set_pat(repo: &str) -> Result<bool, String> {
    let out = Command::new("powershell")
        .args(["-NoProfile", "-STA", "-Command", PROMPT])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("cannot open the token box: {e}"))?;
    let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if token.is_empty() {
        return Ok(false);
    }
    // The token goes through stdin, never on a command line other processes could read.
    let mut child = Command::new("gh")
        .args(["secret", "set", SECRET, "--repo", repo])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run gh: {e}"))?;
    child
        .stdin
        .take()
        .ok_or("cannot write to gh")?
        .write_all(token.as_bytes())
        .map_err(|e| format!("cannot write to gh: {e}"))?;
    let result = child
        .wait_with_output()
        .map_err(|e| format!("gh did not finish: {e}"))?;
    if result.status.success() {
        Ok(true)
    } else {
        Err(String::from_utf8_lossy(&result.stderr).trim().to_string())
    }
}
