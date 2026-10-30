# Installs switchable-ci-runner for the current Windows user:
#   1. downloads the official GitHub runner, checks its SHA-256, and registers it
#      to one repository with a label (it is NOT installed as a service: the agent starts it);
#   2. copies the agent, adds it to this user's startup, and starts it.
# Needs: the GitHub CLI (`gh`) logged in with admin access to the repository.
#
#   powershell -ExecutionPolicy Bypass -File scripts\install-windows.ps1 -Repo owner/name
param(
    [Parameter(Mandatory = $true)][string]$Repo,
    [string]$Label = "pc-dono",
    [string]$RunnerVersion = "2.338.2",
    [string]$Agent
)
$ErrorActionPreference = "Stop"
# Windows PowerShell 5.1 leaves $PSScriptRoot empty inside param defaults, so the default is set here.
if (-not $Agent) { $Agent = Join-Path $PSScriptRoot "..\target\release\switchable-ci-runner.exe" }

$data = Join-Path $env:LOCALAPPDATA "switchable-ci-runner"
$runner = Join-Path $data "runner"
$zip = Join-Path $data "download\actions-runner-win-x64-$RunnerVersion.zip"
New-Item -ItemType Directory -Force (Split-Path $zip) | Out-Null

if (-not (Test-Path $Agent)) { throw "Agent not found at $Agent. Build it with: cargo build --release" }

# 1. The runner, checked against the SHA-256 published in its release notes.
$body = gh api "repos/actions/runner/releases/tags/v$RunnerVersion" --jq .body
$expected = [regex]::Match($body, '<!-- BEGIN SHA win-x64 -->([0-9a-f]{64})').Groups[1].Value
if (-not $expected) { throw "No SHA-256 for win-x64 in the v$RunnerVersion release notes" }
if (-not (Test-Path $zip)) {
    Invoke-WebRequest "https://github.com/actions/runner/releases/download/v$RunnerVersion/actions-runner-win-x64-$RunnerVersion.zip" -OutFile $zip
}
$got = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
if ($got -ne $expected) { Remove-Item $zip; throw "SHA-256 mismatch: expected $expected, got $got" }

if (-not (Test-Path (Join-Path $runner ".runner"))) {
    Expand-Archive $zip -DestinationPath $runner -Force
    $token = gh api -X POST "repos/$Repo/actions/runners/registration-token" --jq .token
    Push-Location $runner
    try {
        & .\config.cmd --unattended --url "https://github.com/$Repo" --token $token `
            --name "$Label-$env:COMPUTERNAME" --labels $Label --work _work --replace
        if ($LASTEXITCODE -ne 0) { throw "config.cmd failed ($LASTEXITCODE)" }
    } finally { Pop-Location; Remove-Variable token }
}

# 2. The agent, started now and at every logon of this user.
$bin = Join-Path $data "bin"
New-Item -ItemType Directory -Force $bin | Out-Null
Get-Process switchable-ci-runner -ErrorAction SilentlyContinue | Stop-Process
Copy-Item $Agent (Join-Path $bin "switchable-ci-runner.exe") -Force
$exe = Join-Path $bin "switchable-ci-runner.exe"
Set-ItemProperty "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run" -Name "switchable-ci-runner" -Value "`"$exe`""
Start-Process $exe
Write-Host "Installed. Look for the dot in the tray (bottom right). Logs: $data"
