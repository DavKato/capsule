# Scroll-region display architecture

The display module uses stateless sequential printing — each `display::*` call writes to stdout and forgets. Tool call dot updates use `cursor::MoveUp(1)` to overwrite the previous line, but this breaks when any output is interleaved between a tool call and its result (the cursor lands on the wrong line). We decided to migrate to DECSTBM scroll regions, splitting the terminal into a scrolling content area and a fixed status panel.

## Considered options

- **Offset counting** — track how many lines were printed since each tool call, `MoveUp(N)` by the correct amount. Fragile: must account for line wrapping (`ceil(visible_width / terminal_width)`), terminal resizes, and multi-byte characters. This is what `indicatif::MultiProgress` does internally — proven but complex bookkeeping.
- **Save/Restore cursor (`ESC 7` / `ESC 8`)** — terminal-native, but only one position can be saved at a time. Cannot handle multiple pending tool calls.
- **Absolute positioning (`CSI row;col H`)** — requires knowing the absolute row, which needs `cursor::position()`. That function sends `ESC[6n` and waits for a terminal response — unreliable in non-TTY contexts (2-second timeout on piped output).
- **DECSTBM scroll regions** — reserve a fixed region for updatable status elements; streaming content scrolls naturally above it. Status lines never move, so updating them is a simple `MoveTo`. Supported across modern terminals (xterm, iTerm2, Alacritty, Windows Terminal, GNOME Terminal). `crossterm` supports it.

## Consequences

- The display module gains a lifecycle (`init` / `teardown`) and becomes stateful — it must track the scroll region boundaries and status panel contents.
- TTY detection required: scroll regions don't work in piped output. The display module needs a fallback mode (plain sequential output, no in-place updates) for non-TTY.
- Terminal resize events must be handled to adjust scroll region boundaries.
- Opens up features beyond dot updates: persistent session info bar, progress indicators, stage status summary.
- The `MoveUp(1)` dot-update pattern in `tool_result_to()` is replaced entirely.

## Amendment: tool calls belong in the scroll region (2026-05-11)

The original design placed tool call dots in the status panel. This was a misunderstanding — it solved the `MoveUp(1)` rendering bug by making tool call history invisible. Once a tool call completed, its information was lost from the status panel, leaving the user with no record of what happened during the run.

**Corrected design:** Tool calls render in the scroll region, not the status panel. The status panel's status row is reserved for stage/iteration/model/duration — no tool call indicators.

- **Pending**: light gray blinking dot (ANSI `SlowBlink` attribute) + tool name + truncated args
- **Completed**: dot updated in-place to solid green (success) or red (failure) via offset counting + `MoveUp(N)`, with a sub-line showing duration
- **Offset counting**: capsule is the sole writer to the scroll region, so line offsets are reliable. Each pending tool call stores its offset; all offsets increment on every scroll-region write. If a tool call scrolls off-screen (offset > scroll region height), the in-place update is skipped.
- **Multiple pending calls**: no cap; each tracked by ID with its own offset
- **Nested tool calls**: when a tool call carries a `parent_tool_use_id` (sub-agent invocations), the display derives nesting depth from the parent's entry and renders a multi-level prefix (`│  ` per ancestor level, then `├─ `). Both TTY and non-TTY paths render the prefix; offset tracking accounts for the prefix width

## References

- [DECSTBM escape sequence (`ESC [ top ; bottom r`)](https://vt100.net/docs/vt102-ug/chapter5.html)
- [ANSI escape codes cheatsheet](https://gist.github.com/fnky/458719343aabd01cfb17a3a4f7296797) — see "Scroll" section
- [pdanford/TerminalScrollRegionsDisplay](https://github.com/pdanford/TerminalScrollRegionsDisplay) — Python example of multiple scroll regions for independently-updating terminal sections
- `crossterm` supports scroll regions; see `crossterm::terminal` and `crossterm::cursor::MoveTo`
