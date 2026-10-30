# Design

## The problem

GitHub-hosted runners can be slow for heavy builds, and a plain self-hosted runner is all or nothing: when the machine is busy, asleep or on battery, jobs wait in the queue for up to 24 hours. The goal: use your own machine when it suits you, GitHub's runners otherwise, without jobs getting stuck.

## Facts that shape the design

- **`runs-on` with a list is AND, not OR.** `runs-on: [self-hosted, switchable]` needs one runner with every label. GitHub has no "my runner, otherwise yours" ([community request](https://github.com/orgs/community/discussions/20019)).
- **A queued job waits up to 24 h** for a matching runner; `timeout-minutes` only counts once the job starts. So the choice must be made before the job is queued, with the machine known to be available.
- **A runner cannot refuse a job once assigned.** Any question to the owner happens before routing.
- **Listing runners** (`GET /repos/{owner}/{repo}/actions/runners`, fields `status` and `busy`) needs repository admin rights; the workflow's `GITHUB_TOKEN` cannot do it, hence the PAT.
- **Idle time and power** on Windows: `GetLastInputInfo` and `GetSystemPowerStatus`. They only see the interactive session, so the agent is a tray app in the user's session, not a service.

## The pieces

1. **Route step** (`action.yml`), on `ubuntu-latest`: polls the runners API for up to `wait-seconds`; outputs `["self-hosted", label]` if a matching runner is online and free, the fallback otherwise.
2. **Tray agent**: every 5 s reads idle time and power, decides (a pure function, `src/decision.rs`), and starts or stops the official runner. It stops it with Ctrl+C so GitHub sees it go offline at once, never mid-job, and keeps the PC awake only while a job runs. Modes: Automatic (idle: run; in use: ask), Always (run everything), Disabled; an option sends everything to GitHub on battery.
3. **Priority and CPU cap**: the runner starts inside a Windows job object. Every process a CI job starts joins it, gets the Idle priority class (the lowest), and, if the owner picked one, a hard CPU cap (`JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP`). Closing the job kills whatever a job left behind. Priority covers the CPU only, not disk or memory.
4. **The question**: while the owner uses the PC, the agent lists the repository's active runs with the owner's `gh`; a new one opens a 45 s message box. Yes brings the runner online for up to 10 minutes; no or no answer leaves it offline, and the route step falls back.

## Alternatives considered

- **A repository variable written by the agent** (`vars.RUNNER`): simpler, no route step, but no question possible and a stale value strands jobs for 24 h.
- **Manual choice per run** (`workflow_dispatch` input): no automation.
- **Run on both and keep the first**: doubles the work, still strands the self-hosted copy.
- **Existing route actions** ([`jimmygchen/runner-fallback-action`](https://github.com/jimmygchen/runner-fallback-action), archived; fork [`mikehardy/runner-fallback-action`](https://github.com/mikehardy/runner-fallback-action)): they check once and do not wait for the owner's answer, so this repository ships its own small step.

## Known limits

- When the machine is off, disabled, or the owner says no, the route step waits the full `wait-seconds` before falling back.
- The question appears for every new run of the repository, including workflows that would not use the machine.
- Windows only so far; Linux (logind or Wayland idle, UPower, the KSNI tray) is next.
