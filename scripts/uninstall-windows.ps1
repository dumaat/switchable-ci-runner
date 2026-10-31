# Stops the agent, removes it from startup, and unregisters every runner (runner, runner-2, ...) from GitHub.
# The program does the same with: "%LOCALAPPDATA%\switchable-ci-runner\bin\switchable-ci-runner.exe" --uninstall
#   powershell -ExecutionPolicy Bypass -File scripts\uninstall-windows.ps1 -Repo owner/name
param([Parameter(Mandatory = $true)][string]$Repo)
$ErrorActionPreference = "Stop"

$data = Join-Path $env:LOCALAPPDATA "switchable-ci-runner"
Get-Process switchable-ci-runner, Runner.Listener -ErrorAction SilentlyContinue | Stop-Process
Remove-ItemProperty "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run" -Name "switchable-ci-runner" -ErrorAction SilentlyContinue

$runners = @(Get-ChildItem $data -Directory -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -match '^runner(-\d+)?$' -and (Test-Path (Join-Path $_.FullName ".runner")) })
foreach ($runner in $runners) {
    $token = gh api -X POST "repos/$Repo/actions/runners/remove-token" --jq .token
    Push-Location $runner.FullName
    try { & .\config.cmd remove --token $token } finally { Pop-Location; Remove-Variable token }
}
Write-Host "Uninstalled $($runners.Count) runner(s). The files stay in $data; delete the folder by hand if you want them gone."
