# switchable-ci-runner

Run your GitHub Actions jobs on your own machine **when you want**, automatically, and fall back to GitHub-hosted runners when you don't.

**In one line:** a self-hosted runner with automatic fallback to GitHub-hosted runners, plus a tray app (Windows; Linux next) that runs CI on your PC when it is idle, asks you when you are using it, and never strands a job in the queue.

Use it if you want to:
- use your laptop or desktop as a GitHub Actions runner without jobs getting stuck when it is off, asleep or busy;
- save GitHub Actions minutes on private repositories (Windows minutes count double);
- get "self-hosted runner, otherwise `ubuntu-latest`/`windows-latest`" in `runs-on`, which GitHub does not offer natively;
- run CI on your own PC at the lowest priority, with a CPU cap, so it never slows you down;
- run several jobs at once on a machine with many cores (one runner per job).

> Status: v0.1.5, Windows. Linux comes next. Design: [docs/design.md](docs/design.md).

## Install (Windows)

1. Install the [GitHub CLI](https://cli.github.com) and log in: `gh auth login` (you must be an admin of the repository).
2. Download `switchable-ci-runner.exe` from the [latest release](https://github.com/dumaat/switchable-ci-runner/releases/latest) and double-click it (**not** "Run as administrator").
3. Type the repository (`owner/name`). It downloads the official GitHub runner, checks its SHA-256, registers it with the label `switchable`, starts with Windows, and a message says it is installed and where to find it.

The icon sits in the tray (bottom right; click the ^ arrow if it is hidden). Its menu:

| Item | What it does |
|---|---|
| **Automatic** / **Always** / **Disabled** | The mode (table below). |
| **On battery, send jobs to GitHub** | On battery, jobs go to GitHub in any mode. |
| **CPU limit for all jobs** → 25 / 50 / 75 / 100% | Hard cap on the CPU that all runners and everything their jobs start can use, together. Applies at once, to running jobs too. |
| **Parallel jobs** → Auto / 1 / 2 / 4 / 8 / Custom… | How many jobs may run at once, one runner each ([below](#parallel-jobs)). Auto, the default, picks the number from your cores and memory. Custom… takes any number up to your physical cores. |
| **Set CI token (PAT)…** | Saves the token the route step needs (below). |
| **Quit** | Stops the runners and the agent until the next logon. |

CI jobs **always run at the lowest Windows priority** ("Idle"): whatever you are doing comes first, and when the PC is idle the CI still gets the whole CPU. The CPU limit is an extra, hard cap on top.

Settings: `%APPDATA%\switchable-ci-runner\config.json` (`mode`, `battery_sends_to_github`, `cpu_limit_percent`, `parallel_jobs`). You can edit it by hand: the agent reloads it within 5 s, and a file that does not parse keeps the current settings and shows a message. Files and logs: `%LOCALAPPDATA%\switchable-ci-runner` (`agent.log`, `runner.log`, `runner-2.log`, ...). To update, run a newer .exe: it replaces the installed one and keeps your settings and runners.

Uninstall: `"%LOCALAPPDATA%\switchable-ci-runner\bin\switchable-ci-runner.exe" --uninstall` (or `scripts\uninstall-windows.ps1 -Repo owner/name`). It unregisters every runner from GitHub and leaves the files in place.

## Why

GitHub-hosted runners are slow for heavy builds, and on free accounts private-repo minutes run out (Windows minutes count double). Your own laptop or desktop is often faster, with warm caches. But a plain self-hosted runner is all or nothing: if your machine is busy, asleep, on battery or on a train, jobs pile up in the queue for up to 24 hours.

`switchable-ci-runner` is a small tray app (like Tailscale: an icon, a menu, no window) that decides, job by job, whether your machine takes it:

| Mode (tray menu) | What happens |
|---|---|
| **Automatic**, idle ≥ 5 min | Your machine runs every job that arrives. |
| **Automatic**, in use | A window asks "Run its jobs on this PC?". Yes → the whole run here. No, or no answer in 45 s → GitHub. |
| **Always** | Every job runs here, idle or not, without asking. |
| **Disabled** | Never runs; everything goes to GitHub. |
| *Option:* **On battery, send jobs to GitHub** | On battery, jobs go to GitHub in any mode. |

A run your machine took keeps the runners online until it ends, even if you come back to the PC. Its later jobs (a matrix, or a job that `needs` another) were routed here when the run started, and they must find a runner.

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
        uses: dumaat/switchable-ci-runner@v0.1.5
        with:
          token: ${{ secrets.RUNNER_STATUS_TOKEN }}   # can list the repo's runners
          fallback: '"windows-latest"'
          # label: switchable      # the label your runner was registered with
          # wait-seconds: 90       # how long to wait for your answer
          # min-free: 1            # free runners needed (parallel jobs: the run's width)
  build:
    needs: route
    runs-on: ${{ fromJSON(needs.route.outputs.runner) }}
```

Known cost: when your machine is off, disabled or you say no, the route step waits the full 90 s before falling back.

## Parallel jobs

One official runner runs one job at a time. To run several jobs at once, the agent keeps several runners: `runner`, `runner-2`, `runner-3`, ... in `%LOCALAPPDATA%\switchable-ci-runner`, each with its own work folder. Set the number in **Parallel jobs**, or with `parallel_jobs` in the config file (`0` is Auto).

- **Auto (the default):** one job per 4 physical cores and per 8 GB of memory, whichever is fewer, and at least one. 8 cores and 32 GB give 2. A build already uses every thread by itself, and the linker and a test browser take a few GB each, so more runners than that make every job slower. Too many runners can also starve the runners' own connection, and GitHub then fails the jobs with "The self-hosted runner lost communication with the server". The number is in the menu item and in `agent.log`.
- **By hand:** up to the physical cores (logical threads do not count). A higher number in the config file is lowered, with a message.

- **More runners:** the agent registers the new ones in the background with your `gh`. They get the same repository and labels as the first runner, and are named `<first runner's name>-2`, `-3`, and so on.
- **Fewer runners:** the extra ones are removed from GitHub as soon as they are free; one in the middle of a job finishes it first. Their files stay, so adding them back skips the download.
- **Never mid-job:** whatever the mode or the number, a runner running a job is never stopped.
- **CPU:** all runners share one job object, so the Idle priority and the CPU limit cover all of them together.
- **Routing:** with `min-free`, the route step sends a run here only if that many runners are free (for a matrix of 4, use `min-free: 4`). Two runs that start at the same moment can both see the same free runners. The extra jobs then wait here for a runner instead of going to GitHub.

The agent cannot handle these for you:

- **Single-instance tools and fixed paths.** Jobs running at the same time share the machine: a program that allows only one instance (a CAD tool, an emulator), a fixed port, a fixed temp file or output folder. Serialize those jobs in the workflow with [`concurrency`](https://docs.github.com/actions/writing-workflows/choosing-what-your-workflow-does/control-the-concurrency-of-workflows-and-jobs):
  ```yaml
  cad-export:
    concurrency: { group: cad-tool, cancel-in-progress: false }
  ```
- **Caches.** Each runner has its own work folder, so the first job on each one builds from scratch, and each takes its own disk space (a Rust `target/` can take several GB). Tool caches in your user profile (`~/.cargo`, `~/.rustup`, npm) are shared. They lock themselves, so jobs wait for each other there instead of breaking.
- **Memory.** N jobs need N times the RAM, and the CPU limit does not cover memory.

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

**Can it run several jobs at the same time?**
Yes: set **Parallel jobs** in the tray, one runner per job. See [Parallel jobs](#parallel-jobs).

**Is it safe for public repositories?**
Only if no outside pull request can run workflows on it. See Security.

## License

MIT
