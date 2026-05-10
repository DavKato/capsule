use crossterm::{
    cursor,
    style::{Color, Print, ResetColor, SetForegroundColor},
    terminal,
    terminal::ClearType,
    QueueableCommand,
};
use std::io::{stderr, stdout, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::pipeline::RetryInfo;
use crate::verdict::{Verdict, VerdictStatus};

pub const GREEN: Color = Color::Green;
pub const RED: Color = Color::Red;
pub const CYAN: Color = Color::Cyan;
pub const YELLOW: Color = Color::Yellow;

const PANEL_HEIGHT: u16 = 3;
const MIN_TERM_HEIGHT: u16 = 12;

struct DisplayState {
    term_width: u16,
    term_height: u16,
    stage_name: String,
    iteration: u32,
    model: String,
    start_time: Instant,
    token_warning: Option<String>,
}

impl DisplayState {
    fn new(term_w: u16, term_h: u16) -> Self {
        Self {
            term_width: term_w,
            term_height: term_h,
            stage_name: String::new(),
            iteration: 0,
            model: String::new(),
            start_time: Instant::now(),
            token_warning: None,
        }
    }

    fn separator_row(&self) -> u16 {
        self.term_height.saturating_sub(PANEL_HEIGHT)
    }

    fn info_row(&self) -> u16 {
        self.term_height.saturating_sub(PANEL_HEIGHT - 1)
    }

    fn status_row(&self) -> u16 {
        self.term_height.saturating_sub(PANEL_HEIGHT - 2)
    }
}

static STATE: OnceLock<Mutex<Option<DisplayState>>> = OnceLock::new();

static LAST_WAS_TEXT: AtomicBool = AtomicBool::new(false);

fn get_state() -> &'static Mutex<Option<DisplayState>> {
    STATE.get_or_init(|| Mutex::new(None))
}

fn is_in_tty_mode() -> bool {
    get_state()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some()
}

pub fn init() {
    if !stdout().is_terminal() {
        return;
    }
    let (term_w, term_h) = terminal::size().unwrap_or((80, 24));
    if term_h < MIN_TERM_HEIGHT {
        return;
    }
    {
        let mut guard = get_state().lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(DisplayState::new(term_w, term_h));
    }
    setup_scroll_region(term_w, term_h);

    // Register a panic hook so the scroll region and cursor state are restored
    // even when the process panics instead of calling teardown() explicitly.
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        teardown();
        prev(info);
    }));
}

/// Returns `true` when the display module is in TTY mode (scroll-region panel active).
///
/// Callers can use this to skip rendering that only makes sense in an interactive
/// terminal — e.g., progress indicators or cursor-position-dependent output.
pub fn is_tty() -> bool {
    is_in_tty_mode()
}

pub fn teardown() {
    let had_state = {
        let mut guard = get_state().lock().unwrap_or_else(|e| e.into_inner());
        let had = guard.is_some();
        *guard = None;
        had
    };
    if !had_state {
        return;
    }
    let mut out = stdout();
    // Reset scroll region to full screen.
    out.write_all(b"\x1b[r").ok();
    // Clear panel rows.
    let (_, term_h) = terminal::size().unwrap_or((80, 24));
    for row in term_h.saturating_sub(PANEL_HEIGHT)..term_h {
        out.queue(cursor::MoveTo(0, row)).ok();
        out.queue(terminal::Clear(ClearType::CurrentLine)).ok();
    }
    out.queue(cursor::MoveTo(0, term_h.saturating_sub(1))).ok();
    out.flush().ok();
}

pub fn set_token_warning(msg: Option<&str>) {
    let mut guard = get_state().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(state) = guard.as_mut() {
        state.token_warning = msg.map(str::to_owned);
        // Collect the values needed for drawing before dropping the guard.
        let (tw, info) = (state.term_width, state.info_row());
        let info_text = build_info_text(state);
        drop(guard);
        draw_panel_info_row_raw(tw, info, &info_text);
    }
}

fn setup_scroll_region(term_w: u16, term_h: u16) {
    let scroll_bottom = term_h.saturating_sub(PANEL_HEIGHT); // 1-indexed == this value
    let mut out = stdout();

    // Set DECSTBM scroll region (rows are 1-indexed in the escape sequence).
    out.write_all(format!("\x1b[1;{}r", scroll_bottom).as_bytes())
        .ok();

    // Draw separator.
    let sep_row = scroll_bottom; // 0-indexed (crossterm MoveTo is 0-indexed)
    out.queue(cursor::MoveTo(0, sep_row)).ok();
    out.queue(SetForegroundColor(CYAN)).ok();
    out.queue(Print("─".repeat(term_w as usize))).ok();
    out.queue(ResetColor).ok();

    // Clear info and status rows.
    for row in (sep_row + 1)..term_h {
        out.queue(cursor::MoveTo(0, row)).ok();
        out.queue(terminal::Clear(ClearType::CurrentLine)).ok();
    }

    // Position cursor at the last row of the scroll region so subsequent
    // output scrolls naturally within the content area.
    out.queue(cursor::MoveTo(0, scroll_bottom.saturating_sub(1)))
        .ok();
    out.flush().ok();
}

fn handle_resize_if_needed(guard: &mut Option<DisplayState>) -> bool {
    let current = terminal::size().unwrap_or((80, 24));
    if let Some(state) = guard.as_mut() {
        if state.term_width != current.0 || state.term_height != current.1 {
            state.term_width = current.0;
            state.term_height = current.1;
            setup_scroll_region(current.0, current.1);
            return true;
        }
    }
    false
}

fn build_info_text(state: &DisplayState) -> String {
    let duration = state.start_time.elapsed();
    let base = format!(
        "Stage: {}  Iter: {}  Model: {}  Duration: {}",
        state.stage_name,
        state.iteration,
        state.model,
        format_duration(duration),
    );
    if let Some(warn) = &state.token_warning {
        format!("{base}  ⚠ {warn}")
    } else {
        base
    }
}

fn draw_panel_info_row_raw(term_w: u16, info_row: u16, text: &str) {
    let padded = pad_or_truncate(text, term_w as usize);
    let mut out = stdout();
    out.queue(cursor::SavePosition).ok();
    out.queue(cursor::MoveTo(0, info_row)).ok();
    out.queue(terminal::Clear(ClearType::CurrentLine)).ok();
    out.queue(SetForegroundColor(CYAN)).ok();
    out.queue(Print(&padded)).ok();
    out.queue(ResetColor).ok();
    out.queue(cursor::RestorePosition).ok();
    out.flush().ok();
}

fn terminal_width() -> u16 {
    terminal::size().map(|(w, _)| w).unwrap_or(80)
}

/// Build the plain-text content lines for a stage header box.
/// Returns strings without box borders or ANSI codes.
fn header_content_lines(
    stage_name: &str,
    iteration: u32,
    model: &str,
    retry: Option<&RetryInfo>,
) -> Vec<String> {
    let mut lines = vec![
        format!("Stage: {stage_name}"),
        format!("Iteration: {iteration}"),
        format!("Model: {model}"),
    ];
    if let Some(r) = retry {
        let max_str = match r.max {
            Some(m) => m.to_string(),
            None => "∞".to_string(),
        };
        lines.push(format!("Retry: {} / {}", r.current, max_str));
    }
    lines
}

/// Pad `text` to `width` characters, or truncate with a trailing space if too long.
fn pad_or_truncate(text: &str, width: usize) -> String {
    let char_count = text.chars().count();
    if char_count + 1 > width {
        let truncated: String = text.chars().take(width.saturating_sub(1)).collect();
        format!("{truncated} ")
    } else {
        format!("{:<width$}", text, width = width)
    }
}

fn render_box<W: Write + QueueableCommand>(
    out: &mut W,
    content: &[String],
    term_w: usize,
) -> std::io::Result<()> {
    let max_content = content.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let box_w = (max_content + 4).min(term_w);
    let inner_w = box_w.saturating_sub(2);

    let top = format!("┌{}┐", "─".repeat(box_w.saturating_sub(2)));
    let bot = format!("└{}┘", "─".repeat(box_w.saturating_sub(2)));

    out.queue(SetForegroundColor(CYAN))?;
    out.queue(Print(format!("{top}\n")))?;
    for line in content {
        let padded = format!(" {line}");
        let cell = pad_or_truncate(&padded, inner_w);
        out.queue(Print(format!("│{cell}│\n")))?;
    }
    out.queue(Print(format!("{bot}\n")))?;
    out.queue(ResetColor)?;
    out.flush()
}

/// Render a bordered stage-header box to stdout.
///
/// ```text
/// ┌─────────────────────────────────┐
/// │ Stage: implementer              │
/// │ Iteration: 1                    │
/// │ Model: claude-opus-4-6          │
/// │ Retry: 2 / 3                   │  ← only when retrying
/// └─────────────────────────────────┘
/// ```
pub fn stage_header(stage_name: &str, iteration: u32, model: &str, retry: Option<&RetryInfo>) {
    LAST_WAS_TEXT.store(false, Ordering::Relaxed);
    let lines = header_content_lines(stage_name, iteration, model, retry);
    let term_w = terminal_width() as usize;
    render_box(&mut stdout(), &lines, term_w).ok();

    // Update panel state in TTY mode.
    let mut guard = get_state().lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        return;
    }
    handle_resize_if_needed(&mut guard);
    if let Some(state) = guard.as_mut() {
        state.stage_name = stage_name.to_owned();
        state.iteration = iteration;
        state.model = model.to_owned();
        state.start_time = Instant::now();
        let info_text = build_info_text(state);
        let (tw, sep, info, status_row) = (
            state.term_width,
            state.separator_row(),
            state.info_row(),
            state.status_row(),
        );
        drop(guard);
        draw_panel_separator_raw(tw, sep);
        draw_panel_info_row_raw(tw, info, &info_text);
        clear_panel_row(status_row);
    }
}

fn draw_panel_separator_raw(term_w: u16, sep_row: u16) {
    let dashes = "─".repeat(term_w as usize);
    let mut out = stdout();
    out.queue(cursor::SavePosition).ok();
    out.queue(cursor::MoveTo(0, sep_row)).ok();
    out.queue(SetForegroundColor(CYAN)).ok();
    out.queue(Print(&dashes)).ok();
    out.queue(ResetColor).ok();
    out.queue(cursor::RestorePosition).ok();
    out.flush().ok();
}

fn clear_panel_row(row: u16) {
    let mut out = stdout();
    out.queue(cursor::SavePosition).ok();
    out.queue(cursor::MoveTo(0, row)).ok();
    out.queue(terminal::Clear(ClearType::CurrentLine)).ok();
    out.queue(cursor::RestorePosition).ok();
    out.flush().ok();
}

/// Print a yellow warning icon followed by `msg` to stderr.
pub fn warning(msg: &str) {
    warning_to(&mut stderr(), msg).ok();
}

fn warning_to<W: Write + QueueableCommand>(out: &mut W, msg: &str) -> std::io::Result<()> {
    out.queue(SetForegroundColor(YELLOW))?;
    out.queue(Print("⚠ "))?;
    out.queue(ResetColor)?;
    out.queue(Print(format!("{msg}\n")))?;
    out.flush()
}

/// Print a neutral informational line to stderr.
pub fn info(msg: &str) {
    info_to(&mut stderr(), msg).ok();
}

fn info_to<W: Write + QueueableCommand>(out: &mut W, msg: &str) -> std::io::Result<()> {
    out.queue(Print(format!("{msg}\n")))?;
    out.flush()
}

/// Render a bordered notice box using the standard `┌┐└┘` character set.
///
/// Accepts plain content lines (without borders); the box is sized to fit
/// the widest line and capped at terminal width.
pub fn notice_box(lines: &[String]) {
    notice_box_to(&mut stdout(), lines, terminal_width() as usize).ok();
}

fn notice_box_to<W: Write + QueueableCommand>(
    out: &mut W,
    lines: &[String],
    term_w: usize,
) -> std::io::Result<()> {
    render_box(out, lines, term_w)
}

const TOOL_ARGS_MAX: usize = 60;

pub fn tool_call(name: &str, args: &str) {
    LAST_WAS_TEXT.store(false, Ordering::Relaxed);
    let display_args: String = if args.chars().count() > TOOL_ARGS_MAX {
        let s: String = args.chars().take(TOOL_ARGS_MAX).collect();
        format!("{s}…")
    } else {
        args.to_owned()
    };

    if is_in_tty_mode() {
        // Update panel status row.
        let label = if display_args.is_empty() {
            name.to_owned()
        } else {
            format!("{name}  {display_args}")
        };
        let mut guard = get_state().lock().unwrap_or_else(|e| e.into_inner());
        handle_resize_if_needed(&mut guard);
        if let Some(state) = guard.as_mut() {
            let info_text = build_info_text(state);
            let (tw, info_r, status_r) = (state.term_width, state.info_row(), state.status_row());
            drop(guard);
            // Refresh duration on info row too.
            draw_panel_info_row_raw(tw, info_r, &info_text);
            draw_panel_status_row_raw(tw, status_r, YELLOW, &label);
        }
    } else {
        tool_call_to(&mut stdout(), name, &display_args).ok();
    }
}

fn draw_panel_status_row_raw(term_w: u16, status_row: u16, color: Color, label: &str) {
    let name_part = pad_or_truncate(label, (term_w as usize).saturating_sub(4));
    let mut out = stdout();
    out.queue(cursor::SavePosition).ok();
    out.queue(cursor::MoveTo(0, status_row)).ok();
    out.queue(terminal::Clear(ClearType::CurrentLine)).ok();
    out.queue(Print("  ")).ok();
    out.queue(SetForegroundColor(color)).ok();
    out.queue(Print("●")).ok();
    out.queue(ResetColor).ok();
    out.queue(Print(format!(" {name_part}"))).ok();
    out.queue(cursor::RestorePosition).ok();
    out.flush().ok();
}

fn tool_call_to<W: Write + QueueableCommand>(
    out: &mut W,
    name: &str,
    display_args: &str,
) -> std::io::Result<()> {
    out.queue(SetForegroundColor(YELLOW))?;
    out.queue(Print("  ● "))?;
    out.queue(ResetColor)?;
    out.queue(Print(format!("{name}  {display_args}\n")))?;
    out.flush()
}

pub fn tool_result(name: &str, success: bool) {
    LAST_WAS_TEXT.store(false, Ordering::Relaxed);
    let color = if success { GREEN } else { RED };

    if is_in_tty_mode() {
        let mut guard = get_state().lock().unwrap_or_else(|e| e.into_inner());
        handle_resize_if_needed(&mut guard);
        if let Some(state) = guard.as_mut() {
            let (tw, status_r) = (state.term_width, state.status_row());
            drop(guard);
            draw_panel_status_row_raw(tw, status_r, color, name);
        }
    } else {
        tool_result_to(&mut stdout(), name, success).ok();
    }
}

fn tool_result_to<W: Write + QueueableCommand>(
    out: &mut W,
    name: &str,
    success: bool,
) -> std::io::Result<()> {
    let color = if success { GREEN } else { RED };
    out.queue(Print("  "))?;
    out.queue(SetForegroundColor(color))?;
    out.queue(Print("● "))?;
    out.queue(ResetColor)?;
    out.queue(Print(format!("{name}\n")))?;
    out.flush()
}

/// Print agent text (thinking or content) with a dim-white block dot on the first
/// line of each new block, and indented continuation lines within the same block.
pub fn agent_text(text: &str) {
    let mut last = LAST_WAS_TEXT.load(Ordering::Relaxed);
    agent_text_to(&mut stdout(), text, &mut last).ok();
    LAST_WAS_TEXT.store(last, Ordering::Relaxed);
}

fn agent_text_to<W: Write + QueueableCommand>(
    out: &mut W,
    text: &str,
    last_was_text: &mut bool,
) -> std::io::Result<()> {
    if *last_was_text {
        out.queue(Print("    "))?;
    } else {
        out.queue(SetForegroundColor(Color::DarkGrey))?;
        out.queue(Print("  · "))?;
        out.queue(ResetColor)?;
    }
    out.queue(Print(text))?;
    out.queue(Print("\n"))?;
    out.flush()?;
    *last_was_text = true;
    Ok(())
}

const NOTES_MAX: usize = 60;
const SESSION_ID_MAX: usize = 32;

fn verdict_color_label(status: &VerdictStatus) -> (Color, &'static str) {
    match status {
        VerdictStatus::Pass => (GREEN, "pass"),
        VerdictStatus::Fail => (RED, "fail"),
        VerdictStatus::Done => (CYAN, "done"),
    }
}

/// Render a color-coded single-line verdict label to stdout.
pub fn verdict(status: &VerdictStatus) {
    verdict_to(&mut stdout(), status).ok();
}

fn verdict_to<W: Write + QueueableCommand>(
    out: &mut W,
    status: &VerdictStatus,
) -> std::io::Result<()> {
    let (color, label) = verdict_color_label(status);
    out.queue(SetForegroundColor(color))?;
    out.queue(Print(format!("{label}\n")))?;
    out.queue(ResetColor)?;
    out.flush()
}

fn format_duration(d: Duration) -> String {
    let total = d.as_secs();
    format!("{:02}:{:02}", total / 60, total % 60)
}

/// Render a bordered session-footer box to stdout after a stage completes.
///
/// `verdict` is `None` for an implicit fail (stage exited without emitting a verdict).
pub fn session_footer(verdict: Option<&Verdict>, duration: Duration, session_id: Option<&str>) {
    LAST_WAS_TEXT.store(false, Ordering::Relaxed);
    session_footer_to(&mut stdout(), verdict, duration, session_id).ok();
}

fn session_footer_to<W: Write + QueueableCommand>(
    out: &mut W,
    verdict: Option<&Verdict>,
    duration: Duration,
    session_id: Option<&str>,
) -> std::io::Result<()> {
    let term_w = terminal_width() as usize;

    let (status_color, status_label) = match verdict {
        Some(v) => verdict_color_label(&v.status),
        None => (RED, "fail"),
    };

    let notes_text = verdict.and_then(|v| v.notes.as_deref()).map(|n| {
        if n.chars().count() > NOTES_MAX {
            let s: String = n.chars().take(NOTES_MAX).collect();
            format!("{s}…")
        } else {
            n.to_string()
        }
    });

    let session_text = session_id.map(|id| {
        if id.chars().count() > SESSION_ID_MAX {
            let s: String = id.chars().take(SESSION_ID_MAX).collect();
            format!("{s}…")
        } else {
            id.to_string()
        }
    });

    let status_plain = format!("Status: {status_label}");
    let duration_line = format!("Duration: {}", format_duration(duration));
    let notes_line = notes_text.as_deref().map(|n| format!("Notes: {n}"));
    let session_line = session_text.as_deref().map(|s| format!("Session: {s}"));

    let plain_lines: Vec<&str> = {
        let mut v: Vec<&str> = vec![&status_plain, &duration_line];
        if let Some(ref nl) = notes_line {
            v.push(nl.as_str());
        }
        if let Some(ref sl) = session_line {
            v.push(sl.as_str());
        }
        v
    };

    let max_content = plain_lines
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0);
    let box_w = (max_content + 4).min(term_w);
    let inner_w = box_w.saturating_sub(2);
    let horiz = "─".repeat(box_w.saturating_sub(2));

    out.queue(SetForegroundColor(CYAN))?;
    out.queue(Print(format!("┌{horiz}┐\n")))?;

    render_footer_status_line(out, status_label, status_color, inner_w)?;

    for line in &plain_lines[1..] {
        render_footer_plain_line(out, line, inner_w)?;
    }

    out.queue(SetForegroundColor(CYAN))?;
    out.queue(Print(format!("└{horiz}┘\n")))?;
    out.queue(ResetColor)?;
    out.flush()
}

fn render_footer_status_line<W: Write + QueueableCommand>(
    out: &mut W,
    label: &str,
    color: Color,
    inner_w: usize,
) -> std::io::Result<()> {
    let prefix = " Status: ";
    let prefix_len = prefix.chars().count();
    let value_w = inner_w.saturating_sub(prefix_len.min(inner_w));
    let value_display = pad_or_truncate(label, value_w);

    out.queue(SetForegroundColor(CYAN))?;
    out.queue(Print("│"))?;
    out.queue(ResetColor)?;
    out.queue(Print(prefix))?;
    out.queue(SetForegroundColor(color))?;
    out.queue(Print(&value_display))?;
    out.queue(SetForegroundColor(CYAN))?;
    out.queue(Print("│\n"))?;
    out.queue(ResetColor)?;
    Ok(())
}

fn render_footer_plain_line<W: Write + QueueableCommand>(
    out: &mut W,
    line: &str,
    inner_w: usize,
) -> std::io::Result<()> {
    let padded = format!(" {line}");
    let cell = pad_or_truncate(&padded, inner_w);
    out.queue(SetForegroundColor(CYAN))?;
    out.queue(Print(format!("│{cell}│\n")))?;
    out.queue(ResetColor)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_without_retry_has_three_lines() {
        let lines = header_content_lines("builder", 1, "claude-opus-4-6", None);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "Stage: builder");
        assert_eq!(lines[1], "Iteration: 1");
        assert_eq!(lines[2], "Model: claude-opus-4-6");
    }

    #[test]
    fn header_with_finite_retry_shows_retry_line() {
        let retry = RetryInfo {
            current: 2,
            max: Some(3),
        };
        let lines = header_content_lines("builder", 2, "claude-opus-4-6", Some(&retry));
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[3], "Retry: 2 / 3");
    }

    #[test]
    fn header_with_unlimited_retry_shows_infinity() {
        let retry = RetryInfo {
            current: 1,
            max: None,
        };
        let lines = header_content_lines("builder", 1, "claude-opus-4-6", Some(&retry));
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[3], "Retry: 1 / ∞");
    }

    #[test]
    fn narrow_terminal_truncates_content() {
        let content = ["Stage: implementer_long_name".to_string()];
        let mut buf: Vec<u8> = Vec::new();
        let _ = render_box(&mut buf, &content, 20);
        let output = String::from_utf8_lossy(&buf);
        assert!(output.contains("┌"), "output must contain top-left corner");
        assert!(
            !output.contains("implementer_long_name"),
            "long content must be truncated"
        );
    }

    #[test]
    fn box_renders_without_error() {
        let lines = header_content_lines(
            "implementer",
            1,
            "claude-opus-4-6",
            Some(&RetryInfo {
                current: 2,
                max: Some(3),
            }),
        );
        let mut buf: Vec<u8> = Vec::new();
        // render_box may fail if crossterm's queue fails on a non-tty buffer;
        // we only assert it doesn't panic.
        let _ = render_box(&mut buf, &lines, 80);
    }

    #[test]
    fn warning_renders_icon_in_yellow_and_message() {
        let mut buf: Vec<u8> = Vec::new();
        warning_to(&mut buf, "something went wrong").unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("⚠"), "warning must contain the warning icon");
        assert!(
            out.contains("something went wrong"),
            "warning must contain the message"
        );
        assert!(
            contains_seq(&buf, YELLOW_ANSI),
            "warning must emit yellow color escape; output: {out:?}"
        );
    }

    #[test]
    fn warning_icon_precedes_message() {
        let mut buf: Vec<u8> = Vec::new();
        warning_to(&mut buf, "bad news").unwrap();
        let out = String::from_utf8_lossy(&buf);
        let icon_pos = out.find('⚠').expect("icon must be present");
        let msg_pos = out.find("bad news").expect("message must be present");
        assert!(icon_pos < msg_pos, "icon must appear before message");
    }

    #[test]
    fn info_renders_message_without_icon() {
        let mut buf: Vec<u8> = Vec::new();
        info_to(&mut buf, "build complete").unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("build complete"), "info must contain message");
        assert!(!out.contains('⚠'), "info must not contain warning icon");
    }

    #[test]
    fn info_does_not_emit_color_escapes() {
        let mut buf: Vec<u8> = Vec::new();
        info_to(&mut buf, "neutral message").unwrap();
        assert!(
            !buf.windows(2).any(|w| w == b"\x1b["),
            "info must not emit ANSI escape codes"
        );
    }

    #[test]
    fn tool_call_renders_name_and_args() {
        let mut buf: Vec<u8> = Vec::new();
        tool_call_to(&mut buf, "Bash", "ls -la").unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("Bash"), "tool name must appear in output");
        assert!(out.contains("ls -la"), "args must appear in output");
        assert!(out.contains("●"), "dot must appear in output");
    }

    #[test]
    fn tool_call_long_args_truncates() {
        let long_args: String = "a".repeat(TOOL_ARGS_MAX + 1);
        let truncated: String = long_args.chars().take(TOOL_ARGS_MAX).collect();
        let display_args = format!("{truncated}…");
        let mut buf: Vec<u8> = Vec::new();
        tool_call_to(&mut buf, "Bash", &display_args).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("…"), "truncated args must end with ellipsis");
        assert!(
            !out.contains(&long_args),
            "full long args must not appear verbatim"
        );
    }

    #[test]
    fn tool_call_short_args_not_truncated() {
        let short_args = "short";
        let mut buf: Vec<u8> = Vec::new();
        tool_call_to(&mut buf, "Read", short_args).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains(short_args), "short args must appear verbatim");
        assert!(!out.contains("…"), "short args must not be truncated");
    }

    #[test]
    fn tool_result_non_tty_no_cursor_up_emits_green_dot() {
        let mut buf: Vec<u8> = Vec::new();
        tool_result_to(&mut buf, "Bash", true).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            !buf.windows(4).any(|w| w == b"\x1b[1A"),
            "non-TTY tool_result must not emit cursor-up; output: {out:?}"
        );
        assert!(out.contains("Bash"), "tool name must appear");
        assert!(out.contains("●"), "dot must appear");
        assert!(
            contains_seq(&buf, GREEN_ANSI),
            "green escape must be emitted for success; output: {out:?}"
        );
    }

    #[test]
    fn tool_result_non_tty_no_cursor_up_emits_red_dot() {
        let mut buf: Vec<u8> = Vec::new();
        tool_result_to(&mut buf, "Write", false).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            !buf.windows(4).any(|w| w == b"\x1b[1A"),
            "non-TTY tool_result must not emit cursor-up on failure; output: {out:?}"
        );
        assert!(out.contains("Write"), "tool name must appear on failure");
        assert!(
            contains_seq(&buf, RED_ANSI),
            "red escape must be emitted for failure; output: {out:?}"
        );
    }

    const DARK_GREY_ANSI: &[u8] = b"\x1b[38;5;8m";

    #[test]
    fn agent_text_first_call_emits_dot_and_text() {
        let mut buf: Vec<u8> = Vec::new();
        let mut last = false;
        agent_text_to(&mut buf, "hello world", &mut last).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("hello world"), "text must appear");
        assert!(out.contains('·'), "dot must appear on first line");
        assert!(
            contains_seq(&buf, DARK_GREY_ANSI),
            "dot must use dark grey color; output: {out:?}"
        );
        assert!(last, "last_was_text must be true after call");
    }

    #[test]
    fn agent_text_continuation_indents_without_dot() {
        let mut buf: Vec<u8> = Vec::new();
        let mut last = true;
        agent_text_to(&mut buf, "second line", &mut last).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("second line"), "text must appear");
        assert!(!out.contains('·'), "no dot on continuation line");
        assert!(
            out.starts_with("    "),
            "continuation must be indented with 4 spaces"
        );
    }

    #[test]
    fn agent_text_body_not_dimmed() {
        let mut buf: Vec<u8> = Vec::new();
        let mut last = false;
        agent_text_to(&mut buf, "body text", &mut last).unwrap();
        // ESC[2m is the dim attribute — must not appear anywhere in the output
        assert!(
            !buf.windows(4).any(|w| w == b"\x1b[2m"),
            "agent_text must not emit dim escape code on body text"
        );
    }

    // Crossterm emits 256-color (8-bit) SGR sequences on non-tty buffers.
    const GREEN_ANSI: &[u8] = b"\x1b[38;5;10m";
    const RED_ANSI: &[u8] = b"\x1b[38;5;9m";
    const CYAN_ANSI: &[u8] = b"\x1b[38;5;14m";
    const YELLOW_ANSI: &[u8] = b"\x1b[38;5;11m";

    fn contains_seq(buf: &[u8], seq: &[u8]) -> bool {
        buf.windows(seq.len()).any(|w| w == seq)
    }

    #[test]
    fn verdict_pass_renders_green() {
        let mut buf: Vec<u8> = Vec::new();
        verdict_to(&mut buf, &VerdictStatus::Pass).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("pass"), "pass label must appear");
        assert!(
            contains_seq(&buf, GREEN_ANSI),
            "green color escape must be emitted for pass; output: {out:?}"
        );
    }

    #[test]
    fn verdict_fail_renders_red() {
        let mut buf: Vec<u8> = Vec::new();
        verdict_to(&mut buf, &VerdictStatus::Fail).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("fail"), "fail label must appear");
        assert!(
            contains_seq(&buf, RED_ANSI),
            "red color escape must be emitted for fail; output: {out:?}"
        );
    }

    #[test]
    fn verdict_done_renders_cyan() {
        let mut buf: Vec<u8> = Vec::new();
        verdict_to(&mut buf, &VerdictStatus::Done).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("done"), "done label must appear");
        assert!(
            contains_seq(&buf, CYAN_ANSI),
            "cyan color escape must be emitted for done; output: {out:?}"
        );
    }

    #[test]
    fn session_footer_pass_contains_status_and_duration() {
        let v = Verdict {
            status: VerdictStatus::Pass,
            notes: None,
        };
        let mut buf: Vec<u8> = Vec::new();
        session_footer_to(&mut buf, Some(&v), Duration::from_secs(83), None).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("pass"), "pass label must appear in footer");
        assert!(out.contains("01:23"), "duration must be formatted as MM:SS");
        assert!(out.contains("┌"), "top border must appear");
        assert!(out.contains("└"), "bottom border must appear");
    }

    #[test]
    fn session_footer_fail_shows_red_status() {
        let v = Verdict {
            status: VerdictStatus::Fail,
            notes: None,
        };
        let mut buf: Vec<u8> = Vec::new();
        session_footer_to(&mut buf, Some(&v), Duration::from_secs(5), None).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("fail"), "fail label must appear");
        assert!(
            contains_seq(&buf, RED_ANSI),
            "red escape must be emitted for fail status; output: {out:?}"
        );
    }

    #[test]
    fn session_footer_done_shows_cyan_status() {
        let v = Verdict {
            status: VerdictStatus::Done,
            notes: None,
        };
        let mut buf: Vec<u8> = Vec::new();
        session_footer_to(&mut buf, Some(&v), Duration::from_secs(0), None).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("done"), "done label must appear");
    }

    #[test]
    fn session_footer_none_verdict_is_implicit_fail() {
        let mut buf: Vec<u8> = Vec::new();
        session_footer_to(&mut buf, None, Duration::from_secs(10), None).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("fail"), "implicit fail must show 'fail'");
        assert!(
            contains_seq(&buf, RED_ANSI),
            "red escape must be emitted for implicit fail; output: {out:?}"
        );
    }

    #[test]
    fn session_footer_notes_truncated_when_long() {
        let long_notes = "x".repeat(100);
        let v = Verdict {
            status: VerdictStatus::Pass,
            notes: Some(long_notes),
        };
        let mut buf: Vec<u8> = Vec::new();
        session_footer_to(&mut buf, Some(&v), Duration::from_secs(0), None).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            out.contains("…"),
            "long notes must be truncated with ellipsis"
        );
        assert!(
            !out.contains(&"x".repeat(100)),
            "full long notes must not appear verbatim"
        );
    }

    #[test]
    fn session_footer_session_id_displayed() {
        let v = Verdict {
            status: VerdictStatus::Pass,
            notes: None,
        };
        let mut buf: Vec<u8> = Vec::new();
        session_footer_to(
            &mut buf,
            Some(&v),
            Duration::from_secs(0),
            Some("sess_abc123"),
        )
        .unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            out.contains("sess_abc123"),
            "session id must appear in footer"
        );
    }

    #[test]
    fn session_footer_session_id_truncated_when_long() {
        let long_id = "s".repeat(64);
        let v = Verdict {
            status: VerdictStatus::Pass,
            notes: None,
        };
        let mut buf: Vec<u8> = Vec::new();
        session_footer_to(&mut buf, Some(&v), Duration::from_secs(0), Some(&long_id)).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("…"), "long session id must be truncated");
        assert!(
            !out.contains(&long_id),
            "full long session id must not appear"
        );
    }

    #[test]
    fn notice_box_contains_content_in_bordered_box() {
        let lines = vec!["Hello, notice!".to_string(), "Second line".to_string()];
        let mut buf: Vec<u8> = Vec::new();
        notice_box_to(&mut buf, &lines, 80).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            out.contains("Hello, notice!"),
            "notice_box must include first line"
        );
        assert!(
            out.contains("Second line"),
            "notice_box must include second line"
        );
        assert!(out.contains("┌"), "notice_box must have top border");
        assert!(out.contains("└"), "notice_box must have bottom border");
    }

    #[test]
    fn notice_box_multibyte_char_correct_width() {
        // ∞ is 3 bytes but 1 display column; box width must be based on char count
        let line = "Retry: 1 / ∞".to_string();
        let char_count = line.chars().count(); // 12
        let mut buf: Vec<u8> = Vec::new();
        notice_box_to(&mut buf, &[line], 80).unwrap();
        let out = String::from_utf8_lossy(&buf);
        // box_w = char_count + 4, dashes = box_w - 2 = char_count + 2
        // The exact top border must appear; with .len() it would have char_count + 4 dashes
        let expected_top = format!("┌{}┐", "─".repeat(char_count + 2));
        assert!(
            out.contains(&expected_top),
            "box width must be based on char count, not byte length; out: {out:?}"
        );
    }

    #[test]
    fn format_duration_zero() {
        assert_eq!(format_duration(Duration::from_secs(0)), "00:00");
    }

    #[test]
    fn format_duration_one_minute_twenty_three() {
        assert_eq!(format_duration(Duration::from_secs(83)), "01:23");
    }

    #[test]
    fn format_duration_over_an_hour() {
        assert_eq!(format_duration(Duration::from_secs(3723)), "62:03");
    }

    #[test]
    fn init_and_teardown_do_not_panic() {
        // init() detects non-TTY (test runner has no terminal) and returns early.
        // teardown() with no active state is a no-op.
        // Neither must panic.
        teardown();
    }

    #[test]
    fn set_token_warning_with_no_state_does_not_panic() {
        // No state (non-TTY) — set_token_warning must be a safe no-op.
        set_token_warning(Some("token expires in 5 min"));
        set_token_warning(None);
    }

    #[test]
    fn is_tty_returns_false_in_non_tty_context() {
        // Test runner has no TTY, so init() is a no-op and is_tty() must return false.
        assert!(
            !is_tty(),
            "is_tty() must return false when stdout is not a terminal"
        );
    }

    #[test]
    fn init_does_not_panic_in_non_tty_context() {
        // In test context stdout is not a terminal; init() must return early without panic.
        init();
        // is_tty() must still be false since init() detected non-TTY and exited early.
        assert!(!is_tty());
    }

    #[test]
    fn teardown_after_non_tty_init_does_not_panic() {
        // Calling teardown() after a non-TTY init() (no active state) must be a safe no-op.
        init();
        teardown();
    }
}
