# Claude Code-style streaming output with crossterm

Capsule's terminal output uses raw `println!`/`eprintln!` with no color or structure, plus a jq filter (`stream_display.jq`) for Claude's streaming JSON. Capsule chrome (stage headers, verdicts, warnings) blends into agent output, making runs hard to scan. We decided to replace `stream_display.jq` with a Rust display module using `crossterm`, adopting Claude Code's streaming line-by-line rendering style.

## Considered options

- **OpenCode TUI style** — bordered blocks, thick left gutters, grouped sections. Rejected because capsule is a streaming log viewer (fire-and-watch), not an interactive chat. OpenCode's block grouping assumes re-renderable viewports, which don't fit a pure streaming model. Faking bordered blocks in a stream breaks on long runs, piped output, and scrollback.
- **Claude Code streaming style** — one line per event, colored status dots, ANSI cursor-up for in-place updates. Matches capsule's execution model and is simpler to implement line-by-line without block-state tracking.

## Consequences

- `stream_display.jq` becomes dead code once the Rust display module is complete.
- `crossterm` added as a dependency (colors + cursor control).
- `StreamParser` extended to extract tool-call results (success/failure), not just verdicts.
- `entrypoint.sh` loud hook echo lines removed; proper hook rendering deferred to a future Docker-crate migration.
- Context window usage display deferred to a later iteration.
