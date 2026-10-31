# Design

## The problem

GitHub-hosted runners can be slow for heavy builds, and a plain self-hosted runner is all or nothing: when the machine is busy, asleep or on battery, jobs wait in the queue for up to 24 hours. The goal: use your own machine when it suits you, GitHub's runners otherwise, without jobs getting stuck.

## Facts that shape the design

- **`runs-on` with a list is AND, not OR.** `runs-on: [self-hosted, switchable]` needs one runner with every label. GitHub has no "my runner, otherwise yours" ([community request](https://github.com/orgs/community/discussions/20019)).
- **A queued job waits up to 24 h** for a matching runner; `timeout-minutes` only counts once the job starts. So the choice must be made before the job is queued, with the machine known to be available.
- **A runner cannot refuse a job once assigned.** Any question to the owner happens before routing.
- **Listing runners** (`GET /repos/{owner}/{repo}/actions/runners`, fields `status` and `busy`) needs repository admin rights; the workflow's `GITHUB_TOKEN` cannot do it, hence the PAT.
- **Idle time and power** on Windows: `GetLastInputInfo` and `GetSystemPowerStatus`. They only see the interactive session, so the agent is a tray app in the user's session, not a service.
- **One runner runs one job at a time.** Parallel jobs need one registered runner per job, each in its own folder.
- **The route decides once per run, but jobs arrive one by one** (a matrix, a job that `needs` another). If the runners went offline between two jobs of a run routed here, the later jobs would wait in the queue.
- **The runner updates itself.** When GitHub asks, the listener starts an update script and exits with code 3 or 4. The script waits for it to exit, swaps the files and writes `update.finished`. `run.cmd` then starts the listener again (see the official `run-helper.cmd.template` and `update.cmd.template`). The agent starts the listener directly, so it does the same.

## The pieces

1. **Route step** (`action.yml`), on `ubuntu-latest`: polls the runners API for up to `wait-seconds`. It outputs `["self-hosted", label]` once at least `min-free` matching runners are online and free, and the fallback otherwise.
2. **Tray agent**: every 5 s it reads idle time and power and decides whether the free runners should be online (a pure function, `src/decision.rs`). Modes: Automatic (idle: run; in use: ask), Always (run everything), Disabled; an option sends everything to GitHub on battery. It stops a runner with Ctrl+C, so GitHub sees it go offline at once, and keeps the PC awake only while a job runs. On a PC with Modern Standby (most laptops since 2020), Windows goes to standby when the screen turns off, the lock screen's short timeout included, and standby cuts the network; a system-required request alone did not stop it (seen in v0.1.4: the jobs failed with "lost communication"). There the agent also keeps the screen on while a job runs (`ES_DISPLAY_REQUIRED`); the session can still lock.
3. **Runners** (`src/parallel.rs`, pure; `src/windows/pool.rs`): `parallel_jobs` runners (0 is Auto: one per 4 physical cores and per 8 GB, at least one; `parallel::auto_jobs`), `runner`, `runner-2`, ... On each tick, `parallel::plan` turns the state of each runner (configured, alive, busy, updating) into start, stop, register and unregister actions. A runner that is busy or updating is never touched. Runners beyond the number wanted are stopped and removed from GitHub once free; their files stay. A runner counts as busy while a `Runner.Worker.exe` is its child (from a process snapshot). If the snapshot fails, every runner counts as busy. New runners copy runner 1's repository and custom labels. They are registered in the background with the owner's `gh`, from the same SHA-256-checked zip.
4. **Taken runs** (`src/runs.rs`): a run that starts while the free runners are online (idle, Always, or the owner said yes) is taken. Until it ends, the runners stay online, so its later jobs find one.
5. **Priority and CPU cap**: one shared job object holds every runner. Every process a CI job starts joins it and gets the Idle priority class (the lowest). If the owner picked a cap, the job gets a hard one too (`JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP`), for all runners together, changed live. Each runner also has its own job, nested inside the shared one: stopping a runner closes its job and kills what its jobs left behind, and nothing of the others. During a runner's self-update, its job stays open so the update script is not killed. Priority covers the CPU only, not disk or memory.
6. **The question**: while the owner uses the PC, the agent lists the repository's active runs with the owner's `gh`, and a new one opens a 45 s message box. Yes takes the run. No, or no answer, leaves the runners offline, and the route step falls back.
7. **Settings**: the tray menu, or `config.json` edited by hand, which is reloaded when it changes. A `parallel_jobs` above the physical cores is lowered to them with a message; 0 is Auto, never zero runners. A file that does not parse keeps the settings in use.

## Alternatives considered

- **A repository variable written by the agent** (`vars.RUNNER`): simpler, no route step, but no question possible and a stale value strands jobs for 24 h.
- **Manual choice per run** (`workflow_dispatch` input): no automation.
- **Run on both and keep the first**: doubles the work, still strands the self-hosted copy.
- **Parallel jobs: one CPU cap per runner** instead of one shared cap. The menu's "50%" would then mean 50% per job, and four jobs could take the whole machine. A shared outer job makes the number mean what it says.
- **Parallel jobs: keep the "yes" for a time window** (v0.1.0 to v0.1.3 kept the runner up for 10 minutes after a yes, until the first job arrived). With several jobs per run, the window closed after the first job and stranded the rest. Taking the whole run fixes that.
- **Parallel jobs: a cap at the logical threads** (v0.1.4). Sixteen runners on 8 cores and 32 GB starved the machine, and GitHub failed jobs with "lost communication". Auto and the cap now count physical cores and memory.
- **Modern Standby: ask the owner to allow the network in standby** (`powercfg ... CONNECTIVITYINSTANDBY`). It is a system setting, often locked by group policy on work laptops, and the processes are still throttled in standby. Keeping the screen on while a job runs works without changing any setting.
- **Parallel jobs: copy runner 1's folder** instead of a fresh extract: it would copy runner 1's credentials and identity.
- **Disable the runner's self-update** (`--disableupdate`) instead of handling it: GitHub stops sending jobs to runners that fall too far behind, so the agent would have to update them itself.
- **Existing route actions** ([`jimmygchen/runner-fallback-action`](https://github.com/jimmygchen/runner-fallback-action), archived; fork [`mikehardy/runner-fallback-action`](https://github.com/mikehardy/runner-fallback-action)): they check once and do not wait for the owner's answer, so this repository ships its own small step.

## Known limits

- When the machine is off, disabled, or the owner says no, the route step waits the full `wait-seconds` before falling back.
- The question appears for every new run of the repository, including workflows that would not use the machine. Likewise, any run that starts while the runners are online is taken and keeps them online until it ends, even one that went to GitHub.
- While a taken run keeps the runners online, a new run can see them free and come here without a question.
- Two runs starting together can both count the same free runners. The extra jobs wait on this machine; they do not go to GitHub.
- Jobs at the same time share the machine. Single-instance programs, fixed ports and fixed paths need `concurrency` in the workflow. Each runner has its own work folder (cold caches, more disk). There is no memory cap.
- In Disabled mode, or on battery with the option on, jobs already routed here for a taken run wait in the queue: the owner's choice wins.
- If the agent dies while removing a runner, the runner stays registered and offline. The next start removes it; GitHub also deletes offline runners after 14 days. A reinstall takes over the same names (`--replace`).
- Windows only so far; Linux (logind or Wayland idle, UPower, the KSNI tray) is next.
