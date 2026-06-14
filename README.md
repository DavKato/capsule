# capsule

Orchestrates multi-stage Claude Code pipelines inside Docker containers.

## Requirements

- Docker
- Claude Code installed and authenticated on the host

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/DavKato/capsule/main/install.sh | bash
```

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

## Quick start

Add the skill to your repo and ask your AI to set up capsule:

```sh
npx skills@latest add davkato/capsule/skills
```

The skill points the agent to `capsule explain` and `capsule templates list`, which is enough to initialize, customize, or debug a `.capsule/` setup. If you prefer not to install the skill, copy the [skill instruction](skills/capsule/SKILL.md) into your prompt directly.

If you prefer to set up without AI, run `capsule init` and pick a template interactively, then edit the generated `.capsule/` files from there.

## Learn more

Capsule ships self-describing documentation via `capsule explain`:

```sh
capsule explain                     # topic index with task recipes
capsule explain commands            # subcommands, flags, and defaults
capsule explain setup-files         # what each .capsule/ file owns
capsule explain --all               # everything at once
```

You can also browse the [`topics/`](topics/) directory directly.

This repo's own [`.capsule/`](.capsule/) is a working example — a multi-stage pipeline that capsule uses to develop itself.

## Development

Build from source and install as `capsule-dev` to avoid conflicting with a release install:

```sh
make install-dev    # build and install as capsule-dev
make uninstall-dev  # remove capsule-dev
```

## Releasing

Releases are managed by the [`/release` skill](.claude/skills/release/SKILL.md).
