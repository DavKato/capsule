# capsule

Runs Claude Code inside a Docker container against your repo, working through GitHub issues autonomously.

> **Note:** This is for my own simple usage for now. It works for me and my setup (Arch Linux) but if you're going to use it you'll probably need some tweaking.

Each iteration runs Claude Code in an isolated container. You control what Claude sees via a prompt file and optional setup commands. The loop runs until Claude signals completion or the iteration limit is reached.

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

### Claude Code skill

```sh
npx skills@latest add davkat/capsule/capsule
```

## Usage

```sh
capsule run
```

```sh
capsule run --rebuild                              # force-rebuild the Docker image
capsule run --verbose                              # show unfiltered container output
capsule run --model claude-opus-4-6
capsule run --capsule-dir path/to/.capsule         # use a non-default config directory
capsule run --log-file run.log                     # tee run output to a file
capsule run --env PARENT=79                        # inject run-scoped parameters into containers and setup commands
capsule run --max-stages 5                         # override the global stage safety cap
capsule completion bash | source                   # enable tab-completion in the current shell
capsule update                                     # download and install the latest release
```

```sh
capsule resume                                # resume from last interrupted run
capsule resume --env KEY=newvalue             # resume with env override (merges on top of persisted pairs)
```

```sh
capsule check                                 # validate .capsule/ directory structure
capsule init --template ralph-loop              # bootstrap .capsule/ from a template
capsule templates list                        # browse available templates
capsule explain mental-model                  # show agent-targeted documentation topics
```

## Config directory

Place a `.capsule/` directory in your repo to configure behaviour:

| File | Purpose |
|------|---------|
| `prompt.md` | Base prompt passed to Claude each iteration |
| `config.yml` | Default flag values (overridden by CLI flags and env vars) |
| `.env` | Secrets and per-container env vars (should be gitignored) |
| `Dockerfile` | Extends the base image with repo-specific tooling |
| `setup` (in config.yml) | Setup command or script path, run on the host before containers start and/or inside each container before Claude starts |

See [`templates/single-stage/.capsule/`](templates/single-stage/.capsule/) for a minimal single-stage setup and [`templates/ralph-loop/.capsule/`](templates/ralph-loop/.capsule/) for a multi-stage pipeline.

## Prompt

`capsule` is prompt-agnostic — it injects no context on its own. Place your prompt at `.capsule/prompt.md`.

Use the `setup` field in `config.yml` to run commands before Claude starts. A per-stage `setup` runs inside the container and can modify `/home/claude/prompt.txt` to inject dynamic context (e.g. git log, open issues).

## Config file

`.capsule/config.yml` accepts the same keys as the CLI flags, as defaults:

```yaml
stages:
  - name: main
    prompt: prompt.md
model: claude-sonnet-4-6
commit_as: user       # or: capsule
github_token_from: local  # or: env
max_stages: 50
setup: scripts/bootstrap.sh  # optional: host-side setup command or script path
```

Precedence: **CLI flag → config.yml → default**.

See [`templates/ralph-loop/.capsule/config.yml`](templates/ralph-loop/.capsule/config.yml) for a multi-stage example.

## Setup

The `setup` field in `config.yml` runs a command or script at two levels:

**Top-level `setup`** — runs once on the host before the first container starts. Use it for pre-flight checks (e.g. verifying a database container is up). Non-zero exit aborts the entire run. Receives `.env` defaults plus `--env` overrides.

**Per-stage `setup`** — runs inside the container before Claude starts each stage invocation. Can modify `/home/claude/prompt.txt` to inject dynamic context. Non-zero exit aborts that invocation.

```yaml
setup: scripts/bootstrap.sh             # top-level: runs on host
stages:
  - name: main
    prompt: prompts/main.md
    setup: pip install -r requirements.txt  # per-stage: runs in container
```

The value can be an inline shell command (contains whitespace) or a path to a script file relative to `.capsule/`. Script files must be executable (`chmod +x`). `capsule check` validates both forms.

## Development

Build from source and install as `capsule-dev` to avoid conflicting with a release install:

```sh
make install-dev    # build and install as capsule-dev
make uninstall-dev  # remove capsule-dev
```

This places the binary at `~/.cargo/bin/capsule-dev`. Use it when testing against a repo that capsule itself manages (e.g. running capsule on its own codebase), since the agent's `cargo build` inside the container would overwrite `target/debug/capsule`.

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
6. Runs the top-level `setup` command if configured
7. For each stage invocation: mounts the prompt, runs per-stage `setup` inside the container (if configured), pipes the prompt to Claude Code, and renders streaming output with color-coded status
8. Exits early when Claude calls `submit_verdict` (pass exits 0, fail exits non-zero) or the stage budget is exhausted (implicit fail)
