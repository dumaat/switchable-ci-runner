# switchable-ci-runner

Run your GitHub Actions jobs on your own machine **when you want**, automatically, and fall back to GitHub-hosted runners when you don't.

**In one line:** a self-hosted runner with automatic fallback to GitHub-hosted runners, plus a tray app (Windows; Linux next) that runs CI on your PC when it is idle, asks you when you are using it, and never strands a job in the queue.

Use it if you want to:
- use your laptop or desktop as a GitHub Actions runner without jobs getting stuck when it is off, asleep or busy;
- save GitHub Actions minutes on private repositories (Windows minutes count double);
- get "self-hosted runner, otherwise `ubuntu-latest`/`windows-latest`" in `runs-on`, which GitHub does not offer natively;
- run CI on your own PC at the lowest priority, with a CPU cap, so it never slows you down.

> Status: v0.1.3, Windows. Linux comes next. Design: [docs/design.md](docs/design.md).

## Install (Windows)

1. Install the [GitHub CLI](https://cli.github.com) and log in: `gh auth login` (you must be an admin of the repository).
2. Download `switchable-ci-runner.exe` from the [latest release](https://github.com/dumaat/switchable-ci-runner/releases/latest) and double-click it (**not** "Run as administrator").
3. Type the repository (`owner/name`). It downloads the official GitHub runner, checks its SHA-256, registers it with the label `switchable`, starts with Windows, and a message says it is installed and where to find it.

The icon sits in the tray (bottom right; click the ^ arrow if it is hidden). Its menu:

| Item | What it does |
|---|---|
| **Automatic** / **Always** / **Disabled** | The mode (table below). |
| **On battery, send jobs to GitHub** | On battery, jobs go to GitHub in any mode. |
| **CPU limit** → 25 / 50 / 75 / 100% | Hard cap on the CPU the runner and everything a job starts can use. Applies from the next runner start, never mid-job. |
| **Set CI token (PAT)…** | Saves the token the route step needs (below). |
| **Quit** | Stops the runner and the agent until the next logon. |

CI jobs **always run at the lowest Windows priority** ("Idle"): whatever you are doing comes first, and when the PC is idle the CI still gets the whole CPU. The CPU limit is an extra, hard cap on top.

Settings: `%APPDATA%\switchable-ci-runner\config.json`. Files and logs: `%LOCALAPPDATA%\switchable-ci-runner`. To update, run a newer .exe: it replaces the installed one and keeps your settings and runner. Uninstall: `scripts\uninstall-windows.ps1 -Repo owner/name`.

## Why

GitHub-hosted runners are slow for heavy builds, and on free accounts private-repo minutes run out (Windows minutes count double). Your own laptop or desktop is often faster, with warm caches. But a plain self-hosted runner is all or nothing: if your machine is busy, asleep, on battery or on a train, jobs pile up in the queue for up to 24 hours.

`switchable-ci-runner` is a small tray app (like Tailscale: an icon, a menu, no window) that decides, job by job, whether your machine takes it:

| Mode (tray menu) | What happens |
|---|---|
| **Automatic**, idle ≥ 5 min | Your machine runs every job that arrives. |
| **Automatic**, in use | A window asks "Run its jobs on this PC?". Yes → here. No, or no answer in 45 s → GitHub. |
| **Always** | Every job runs here, idle or not, without asking. |
| **Disabled** | Never runs; everything goes to GitHub. |
| *Option:* **On battery, send jobs to GitHub** | On battery, jobs go to GitHub in any mode. |

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
        uses: dumaat/switchable-ci-runner@v0.1.3
        with:
          token: ${{ secrets.RUNNER_STATUS_TOKEN }}   # can list the repo's runners
          fallback: '"windows-latest"'
          # label: switchable      # the label your runner was registered with
          # wait-seconds: 90       # how long to wait for your answer
  build:
    needs: route
    runs-on: ${{ fromJSON(needs.route.outputs.runner) }}
```

Known cost: when your machine is off, disabled or you say no, the route step waits the full 90 s before falling back.

## The CI token (PAT)

The route step must see whether your runner is online, and GitHub's built-in `GITHUB_TOKEN` cannot list runners. So it needs a personal access token:

1. GitHub → Settings → Developer settings → [Fine-grained tokens](https://github.com/settings/personal-access-tokens/new).
2. Repository access: **only** your repository. Permissions: **Administration: Read-only**. Pick an expiry.
3. Tray menu → **Set CI token (PAT)…** → paste → Save. It becomes the repository secret `RUNNER_STATUS_TOKEN`; nothing stays on your PC.

## Security

A self-hosted runner executes whatever the workflow says, with your user's permissions. Use it **only on repositories you trust**, keep "Run workflows from fork pull requests" off, and never on public repositories that accept outside PRs. See GitHub's [security hardening guide](https://docs.github.com/actions/security-guides/security-hardening-for-github-actions#hardening-for-self-hosted-runners).

## FAQ

**Can `runs-on` fall back to a GitHub-hosted runner when my self-hosted runner is offline?**
Not by itself: a list in `runs-on` means all labels (AND), and a job for an offline runner waits up to 24 h. The route step in this repository checks your runner first and outputs either it or the fallback, as in the example above.

**How is this different from `runner-fallback-action`?**
Those actions check once whether the runner is online. Here a tray agent also decides whether the runner should be online (idle, in use, battery, your answer), and the route step waits for your answer.

**Does it slow down my PC while I work?**
In Automatic mode it asks first. Jobs always run at the Idle priority class, with an optional hard CPU cap.

**Does it work on Linux or macOS?**
Windows today; Linux is the next target. The route step runs on any GitHub-hosted runner.

**Is it safe for public repositories?**
Only if no outside pull request can run workflows on it. See Security.

## License

MIT
