# Stops the agent, removes it from startup, and unregisters the runner from GitHub.
#   powershell -ExecutionPolicy Bypass -File scripts\uninstall-windows.ps1 -Repo owner/name
param([Parameter(Mandatory = $true)][string]$Repo)
$ErrorActionPreference = "Stop"

$data = Join-Path $env:LOCALAPPDATA "switchable-ci-runner"
Get-Process switchable-ci-runner, Runner.Listener -ErrorAction SilentlyContinue | Stop-Process
Remove-ItemProperty "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run" -Name "switchable-ci-runner" -ErrorAction SilentlyContinue

$runner = Join-Path $data "runner"
if (Test-Path (Join-Path $runner ".runner")) {
    $token = gh api -X POST "repos/$Repo/actions/runners/remove-token" --jq .token
    Push-Location $runner
    try { & .\config.cmd remove --token $token } finally { Pop-Location; Remove-Variable token }
}
Write-Host "Uninstalled. The files stay in $data; delete the folder by hand if you want them gone."
