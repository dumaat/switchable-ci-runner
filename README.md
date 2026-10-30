# switchable-ci-runner

Run your GitHub Actions jobs on your own machine **when you want**, automatically, and fall back to GitHub-hosted runners when you don't.

> Status: first Windows version (automatic and disabled modes, battery option). The "ask me" popup and Linux come next. Design: [docs/design.md](docs/design.md).

## Install (Windows)

```powershell
cargo build --release
powershell -ExecutionPolicy Bypass -File scripts\install-windows.ps1 -Repo owner/name
```

It registers the official runner with the label `pc-dono` (change with `-Label`), adds the agent to your startup and starts it. Undo with `scripts\uninstall-windows.ps1`.

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

GitHub has no built-in "use my runner, otherwise yours". A job waiting for an offline self-hosted runner just waits. So the decision is made **before** the job is queued:

1. A tiny **router job** runs on `ubuntu-latest`, asks the GitHub API whether your runner is online and free, and outputs where the real jobs should go.
2. The **tray agent** on your machine starts and stops the official GitHub runner according to the mode, so "online" always means "willing".

```yaml
jobs:
  route:
    uses: dumaat/switchable-ci-runner/.github/workflows/route.yml@v1   # planned
  build:
    needs: route
    runs-on: ${{ fromJSON(needs.route.outputs.runner) }}
```

## Security

A self-hosted runner executes whatever the workflow says, with your user's permissions. Use it **only on repositories you trust**, keep "Run workflows from fork pull requests" off, and never on public repositories that accept outside PRs. See GitHub's [security hardening guide](https://docs.github.com/actions/security-guides/security-hardening-for-github-actions#hardening-for-self-hosted-runners).

## License

MIT
