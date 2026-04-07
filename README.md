# capsule

Runs Claude Code inside a Docker container against your repo, working through GitHub issues autonomously.

Each iteration assembles a prompt from recent commits and open issues, runs Claude Code in an isolated container, and loops until Claude signals it has no more tasks or the iteration limit is reached.

## Requirements

- Docker
- `gh` CLI authenticated (`gh auth login`)
- Claude Code installed and authenticated on the host

## Usage

From your repo directory:

```sh
capsule <iterations>
```

```sh
FORCE_REBUILD=1 capsule 1   # force-rebuild the base image
```

## How it works

1. Builds a base `capsule` image (Arch Linux + git + gh + Claude Code) if not cached
2. Builds a repo-specific `capsule-<repo>` image if `.capsule/Dockerfile` exists
3. Each iteration assembles a prompt from recent commits + open GitHub issues and pipes it to Claude Code
4. Claude's output streams to the terminal; the run ends when Claude outputs `<promise>NO MORE TASKS</promise>` or the iteration limit is reached

## Repo configuration

Place a `.capsule/` directory in your repo to customise the environment:

| File | Purpose |
|------|---------|
| `Dockerfile` | Extends the base image with repo-specific tooling (e.g. pnpm, python) |
| `setup.sh` | Runs inside the container before Claude starts (e.g. install deps) |
| `.env` | Environment variables for the container (e.g. service hostnames) |

See `example/.capsule/` for annotated examples of each file.

## Prompt

`prompt.md` in this directory is the base prompt injected every iteration, after the commit and issue context. Edit it to change how Claude prioritises and approaches tasks.
