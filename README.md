# capsule

Runs Claude Code inside a Docker container against your repo, working through GitHub issues autonomously.

> **Note:** This is for my own simple usage for now. It works for me and my setup (Arch Linux) but if you're going to use it you'll probably need some tweaking.

Each iteration runs Claude Code in an isolated container. You control what Claude sees via a prompt file and optional hook scripts. The loop runs until Claude signals completion or the iteration limit is reached.

## Requirements

- Docker
- `gh` CLI authenticated (`gh auth login`)
- Claude Code installed and authenticated on the host

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/DavKato/capsule/main/install.sh | bash
```

Installs the binary to `~/.local/bin` and sets up shell completions (bash, zsh, fish). No Rust toolchain required.

<details>
<summary>Manual install</summary>

Download the archive for your platform from [GitHub Releases](https://github.com/DavKato/capsule/releases), extract, and place the binary on your `$PATH`:

```sh
curl -L https://github.com/DavKato/capsule/releases/latest/download/capsule-<triple>.tar.gz | tar xz
mv capsule ~/.local/bin/
```

Where `<triple>` is one of: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`.

Then set up completions manually:

```sh
capsule completion bash > ~/.local/share/bash-completion/completions/capsule  # bash
capsule completion zsh  > ~/.zsh/completions/_capsule                          # zsh
capsule completion fish > ~/.config/fish/completions/capsule.fish              # fish
```

</details>

## Usage

```sh
capsule run --iterations 5
```

```sh
capsule run --iterations 1 --rebuild          # force-rebuild the Docker image
capsule run --iterations 3 --verbose          # show unfiltered container output
capsule run --model claude-opus-4-6 --iterations 2
capsule run --capsule-dir path/to/.capsule    # use a non-default config directory
capsule completion bash | source              # enable tab-completion in the current shell
capsule update                                # download and install the latest release
```

## Config directory

Place a `.capsule/` directory in your repo to configure behaviour:

| File | Purpose |
|------|---------|
| `prompt.md` | Base prompt passed to Claude each iteration |
| `config.yml` | Default flag values (overridden by CLI flags and env vars) |
| `.env` | Secrets and per-container env vars (should be gitignored) |
| `Dockerfile` | Extends the base image with repo-specific tooling |
| `before-all.sh` | Runs once on the host before any container starts |
| `before-each.sh` | Runs inside the container before Claude starts each iteration |

See `example/.capsule/` for annotated examples of each file.

## Prompt

`capsule` is prompt-agnostic — it injects no context on its own. Place your prompt at `.capsule/prompt.md` (or pass `--prompt path/to/other.md`).

Use `before-each.sh` to prepend dynamic context (e.g. git log, open issues) to `/home/claude/prompt.txt` before Claude reads it. See `example/.capsule/before-each.sh` for a working example.

## Config file

`.capsule/config.yml` accepts the same keys as the CLI flags, as defaults:

```yaml
iterations: 3
model: claude-sonnet-4-6
git_identity: user  # or: capsule
```

Precedence: **CLI flag → config.yml → default**.

See `example/.capsule/config.yml` for all keys with descriptions.

## Hooks

**`before-all.sh`** — runs once on the host before the first container starts. Use it for pre-flight checks (e.g. verifying a database container is up). Non-zero exit aborts the entire run.

**`before-each.sh`** — runs inside the container before Claude starts each iteration. Can modify `/home/claude/prompt.txt` to inject dynamic context. Non-zero exit aborts that iteration.

The following environment variables are available inside `before-each.sh`:

| Variable | Description |
|---|---|
| `CAPSULE_WORKSPACE` | Absolute path to the workspace inside the container (mirrors the host path) |

Both hooks receive variables from `.capsule/.env`.

## Releasing

Requires [`cargo-release`](https://github.com/crate-ci/cargo-release):

```sh
cargo install cargo-release
```

Then bump the version, tag, and push in one command:

```sh
cargo release patch --execute   # or: minor, major
```

This updates `Cargo.toml`, commits, creates the version tag, and pushes. GitHub Actions then builds binaries for all four targets and attaches them to the GitHub Release automatically.

## How it works

1. Resolves config from `config.yml`, CLI flags, and env vars
2. Runs pre-flight checks (Docker daemon reachable, prompt file present)
3. Sources `.capsule/.env` into the host environment
4. Builds the base `capsule` image if not cached (or if `--rebuild` is passed)
5. Builds a repo-specific `capsule-<basename>` image if `.capsule/Dockerfile` exists
6. Runs `before-all.sh` if present
7. For each iteration: mounts the prompt, runs `before-each.sh` inside the container, pipes the prompt to Claude Code, and streams output through `jq`
8. Exits early when Claude calls `submit_verdict` (pass exits 0, fail exits non-zero) or the iteration budget is exhausted (implicit fail)
