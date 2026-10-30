# switchable-ci-runner

Run your GitHub Actions jobs on your own machine **when you want**, automatically, and fall back to GitHub-hosted runners when you don't.

> Status: v0.1, Windows. Linux comes next. Design: [docs/design.md](docs/design.md).

## Install (Windows)

1. Install the [GitHub CLI](https://cli.github.com) and log in: `gh auth login` (you must be an admin of the repository).
2. Download `switchable-ci-runner.exe` from the [latest release](https://github.com/dumaat/switchable-ci-runner/releases/latest) and double-click it (**not** "Run as administrator").
3. Type the repository (`owner/name`). It downloads the official GitHub runner, checks its SHA-256, registers it with the label `pc-dono`, starts with Windows, and a message says it is installed and where to find it.

The icon sits in the tray (bottom right). Its menu: **Automatic** (idle: run; in use: ask), **Always** (run every job), **Disabled**, **On battery, send jobs to GitHub**, **Set CI token (PAT)…** (saved as the repo secret `RUNNER_STATUS_TOKEN` through `gh`, not kept on the PC), **Quit**. To update, run a newer .exe: it replaces the installed one. Files and logs: `%LOCALAPPDATA%\switchable-ci-runner`. Uninstall: `scripts\uninstall-windows.ps1 -Repo owner/name`.

## Why

GitHub-hosted runners are slow for heavy builds, and on free accounts private-repo minutes run out (Windows minutes count double). Your own laptop or desktop is often faster, with warm caches. But a plain self-hosted runner is all or nothing: if your machine is busy, asleep, on battery or on a train, jobs pile up in the queue for up to 24 hours.

`switchable-ci-runner` is a small tray app (like Tailscale: an icon, a menu, no window) that decides, job by job, whether your machine takes it:

| Mode | What happens |
|---|---|
| **Idle ≥ 5 min** | Your machine runs every job that arrives. |
| **Idle < 5 min** | A notification asks "Run here or send to GitHub?". No answer in 45 s → GitHub. |
| **Disabled** | Never runs; everything goes to GitHub. |
| *Option:* **On battery → GitHub** | Toggle in the menu. |

It is meant for students and small teams who have a capable machine but not a big GitHub plan. **Linux is a first-class target**, alongside Windows.

## How it works

GitHub has no built-in "use my runner, otherwise yours". A job waiting for an offline self-hosted runner just waits. So the decision is made **before** the jobs are queued:

1. A **route** step on `ubuntu-latest` (this repository is the action) asks the GitHub API whether your runner is online and free, waiting up to 90 s for your answer, and outputs where the real jobs go.
2. The **tray agent** starts and stops the official runner, so "online" always means "willing". While you use the PC, it watches the repository's runs with your `gh` and asks you.

```yaml
jobs:
  route:
    runs-on: ubuntu-latest
    outputs:
      runner: ${{ steps.route.outputs.runner }}
    steps:
      - id: route
        uses: dumaat/switchable-ci-runner@v0.1.1
        with:
          token: ${{ secrets.RUNNER_STATUS_TOKEN }}   # can list the repo's runners
          fallback: '"windows-latest"'
  build:
    needs: route
    runs-on: ${{ fromJSON(needs.route.outputs.runner) }}
```

Known cost: when your machine is off, disabled or you say no, the route step waits the full 90 s before falling back.

## Security

A self-hosted runner executes whatever the workflow says, with your user's permissions. Use it **only on repositories you trust**, keep "Run workflows from fork pull requests" off, and never on public repositories that accept outside PRs. See GitHub's [security hardening guide](https://docs.github.com/actions/security-guides/security-hardening-for-github-actions#hardening-for-self-hosted-runners).

## License

MIT
