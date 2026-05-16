Available topics. Load only what your current task needs.

  mental-model     The Pipeline / Stage / Loop / Verdict / Routing model.
                   Load before reasoning about routing or picking pass/fail/done.

  setup-files      What each file in .capsule/ owns.
                   Load before editing any .capsule/ file.

  pipeline-shapes  Decision tree: single-stage vs ralph-loop vs other.
                   Load before picking a template or proposing a shape change.

  prompt-writing   Verdict contract, note-injection, role framing.
                   Load before authoring or editing a stage prompt.

  common-edits     Rename / add / remove a stage; add a hook.
                   Load before structural changes to config.yml.

  commands         capsule subcommands the agent uses (templates, init, check, run).
                   Load when unsure which command to invoke.

Common task recipes (load multiple topics in one call):

  greenfield setup   → capsule explain mental-model setup-files pipeline-shapes prompt-writing commands
  rename a stage     → capsule explain mental-model setup-files common-edits
  write a stage      → capsule explain mental-model prompt-writing setup-files
  debug routing      → capsule explain mental-model common-edits

For everything at once:
  capsule explain --all
