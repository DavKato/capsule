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

## References

- [DECSTBM escape sequence (`ESC [ top ; bottom r`)](https://vt100.net/docs/vt102-ug/chapter5.html)
- [ANSI escape codes cheatsheet](https://gist.github.com/fnky/458719343aabd01cfb17a3a4f7296797) — see "Scroll" section
- [pdanford/TerminalScrollRegionsDisplay](https://github.com/pdanford/TerminalScrollRegionsDisplay) — Python example of multiple scroll regions for independently-updating terminal sections
- `crossterm` supports scroll regions; see `crossterm::terminal` and `crossterm::cursor::MoveTo`
