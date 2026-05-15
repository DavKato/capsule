# commands

Load when you need to know how to invoke capsule subcommands — which command runs a pipeline, bootstraps setup, validates config, or surfaces docs.

## capsule run

Starts a pipeline run. Executes `config.yml` inside a Docker container.

```sh
capsule run                                    # run with config.yml defaults
capsule run --iterations 5                     # cap at 5 loop iterations (flat-form)
capsule run --input "fix issue #42"            # pipeline input: injected into first stage, first invocation only
capsule run --env PARENT=79                    # run environment: available in all containers and hooks
capsule run --model claude-opus-4-7            # override Claude model
capsule run --rebuild                          # force-rebuild Docker image (bypass layer cache)
capsule run --verbose                          # show unfiltered container output
```

| Flag | Default | Purpose |
|------|---------|---------|
| `--iterations` | config | Flat-form iteration cap |
| `--input` | — | Pipeline input; first stage, first invocation only |
| `--env KEY=VALUE` | — | Run environment; injected into all containers and hooks. Repeatable |
| `--model` | config | Claude model override |
| `--prompt` | `<capsule-dir>/prompt.md` | Path to the prompt file |
| `--rebuild` | false | Bypass Docker layer cache |
| `--verbose` | false | Print verbose diagnostic output |
| `--capsule-dir` | `.capsule` | Config directory path |
| `--git-identity` | `user` | Git commit identity: `user` (host config) or `capsule` (generic) |
| `--github local\|global` | — | Inject `GH_TOKEN` into containers |
| `--log-file` | — | Write run output to a file in addition to the terminal |
| `--min-token-lifetime-minutes` | — | Prompt before starting if access token expires within threshold |

## capsule resume

Resumes from the last interrupted run using `.capsule/last-run.json`. Use after auth failures, network drops, or manual interruptions.

```sh
capsule resume
capsule resume --env KEY=newvalue    # merge run environment on top of persisted pairs
```

## capsule check

Validates `.capsule/` structure. Run after every structural edit to `config.yml`.

```sh
capsule check
capsule check --capsule-dir path/to/.capsule
```

Exits non-zero on errors. Checks: route targets resolve, prompt files exist, hook scripts present, loop nesting valid.

## capsule explain

Loads agent-targeted documentation topics.

```sh
capsule explain                              # print topic index with recipes
capsule explain mental-model                 # load one topic
capsule explain setup-files commands         # load multiple topics in one call
capsule explain --all                        # dump all topics
```

## capsule templates list and capsule init

Enumerate available templates, then bootstrap `.capsule/` from one. Agent path: always use `templates list` first.

```sh
capsule templates list                            # list templates (deterministic, no TTY required)
capsule init --template ralph-loop                # copy template into .capsule/
capsule init --template single-iter --force       # overwrite existing .capsule/
```

Do not run bare `capsule init` from a script or agent — it requires a TTY and blocks on interactive input.
