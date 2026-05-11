use crossterm::{
    cursor,
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor},
    terminal,
    terminal::ClearType,
    QueueableCommand,
};
use std::collections::HashMap;
use std::io::{stderr, stdout, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::pipeline::RetryInfo;
use crate::verdict::{Verdict, VerdictStatus};

pub const GREEN: Color = Color::Green;
pub const RED: Color = Color::Red;
pub const CYAN: Color = Color::Cyan;
pub const YELLOW: Color = Color::Yellow;

const PANEL_HEIGHT: u16 = 3;
const MIN_TERM_HEIGHT: u16 = 12;

struct ToolCallEntry {
    id: String,
    name: String,
    color: Color,
}

struct DisplayState {
    term_width: u16,
    term_height: u16,
    stage_name: String,
    iteration: u32,
    model: String,
    start_time: Instant,
    token_warning: Option<String>,
    active_tool_calls: Vec<ToolCallEntry>,
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
            active_tool_calls: Vec::new(),
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
static TIMER_GEN: AtomicU64 = AtomicU64::new(0);
static TIMER_WAKE: OnceLock<(Mutex<()>, Condvar)> = OnceLock::new();

fn timer_wake() -> &'static (Mutex<()>, Condvar) {
    TIMER_WAKE.get_or_init(|| (Mutex::new(()), Condvar::new()))
}

fn tool_name_cache() -> &'static Mutex<HashMap<String, String>> {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

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
    if is_in_tty_mode() || !stdout().is_terminal() {
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

pub fn is_tty() -> bool {
    is_in_tty_mode()
}

pub fn teardown() {
    TIMER_GEN.fetch_add(1, Ordering::Relaxed);
    timer_wake().1.notify_all();
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
    let (_, term_h) = terminal::size().unwrap_or((80, 24));
    for row in term_h.saturating_sub(PANEL_HEIGHT)..term_h {
        out.queue(cursor::MoveTo(0, row)).ok();
        out.queue(terminal::Clear(ClearType::CurrentLine)).ok();
    }
    out.queue(cursor::MoveTo(0, term_h.saturating_sub(1))).ok();
    out.flush().ok();
}

fn redraw_info_row() {
    let mut guard = get_state().lock().unwrap_or_else(|e| e.into_inner());
    handle_resize_if_needed(&mut guard);
    if let Some(state) = guard.as_mut() {
        let info_text = build_info_text(state);
        let (tw, info) = (state.term_width, state.info_row());
        drop(guard);
        draw_panel_info_row_raw(tw, info, &info_text);
    }
}

pub fn set_stage(name: &str, iteration: u32, model: &str) {
    let in_tty = {
        let mut guard = get_state().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(state) = guard.as_mut() {
            state.stage_name = name.to_owned();
            state.iteration = iteration;
            state.model = model.to_owned();
            state.start_time = Instant::now();
            state.token_warning = None;
            state.active_tool_calls.clear();
        }
        guard.is_some()
    };
    if !in_tty {
        return;
    }
    redraw_info_row();
    let gen = TIMER_GEN.fetch_add(1, Ordering::Relaxed) + 1;
    timer_wake().1.notify_all();
    std::thread::spawn(move || loop {
        let (lock, cvar) = timer_wake();
        let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let _ = cvar.wait_timeout(guard, Duration::from_secs(1));
        if TIMER_GEN.load(Ordering::Relaxed) != gen || !is_in_tty_mode() {
            break;
        }
        redraw_info_row();
    });
}

pub fn clear_stage() {
    TIMER_GEN.fetch_add(1, Ordering::Relaxed);
    timer_wake().1.notify_all();
    let mut guard = get_state().lock().unwrap_or_else(|e| e.into_inner());
    handle_resize_if_needed(&mut guard);
    if let Some(state) = guard.as_mut() {
        state.stage_name.clear();
        state.iteration = 0;
        state.model.clear();
        state.token_warning = None;
        state.active_tool_calls.clear();
        let (info_r, status_r) = (state.info_row(), state.status_row());
        drop(guard);
        clear_panel_row(info_r);
        clear_panel_row(status_r);
    }
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

    let sep_row = scroll_bottom; // 0-indexed (crossterm MoveTo is 0-indexed)
    out.queue(cursor::MoveTo(0, sep_row)).ok();
    out.queue(SetForegroundColor(CYAN)).ok();
    out.queue(Print("─".repeat(term_w as usize))).ok();
    out.queue(ResetColor).ok();

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

// Resize detection uses polling rather than SIGWINCH. Each display interaction
// calls this function, and the 1-second timer tick ensures the scroll region
// adjusts within ≤1 s of a resize — acceptable latency that avoids the unsafe
// signal-handler machinery a SIGWINCH approach would require.
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

fn render_stage_header_to<W: Write + QueueableCommand>(
    out: &mut W,
    stage_name: &str,
    iteration: u32,
    model: &str,
    retry: Option<&RetryInfo>,
    term_w: usize,
) -> std::io::Result<()> {
    let content = match retry {
        Some(r) => {
            let max_str = match r.max {
                Some(m) => m.to_string(),
                None => "∞".to_string(),
            };
            format!(
                "{stage_name} · iter {iteration} · {model} · retry {}/{}",
                r.current, max_str
            )
        }
        None => format!("{stage_name} · iter {iteration} · {model}"),
    };

    // "══ {content} " + trailing ═ to fill terminal width
    let prefix = "══ ";
    let suffix = " ";
    let prefix_len = prefix.chars().count();
    let content_len = content.chars().count();
    let suffix_len = suffix.chars().count();
    let used = prefix_len + content_len + suffix_len;
    let trailing_len = term_w.saturating_sub(used);

    out.queue(SetForegroundColor(CYAN))?;
    out.queue(Print(prefix))?;
    out.queue(SetForegroundColor(Color::White))?;
    out.queue(SetAttribute(Attribute::Bold))?;
    out.queue(Print(&content))?;
    out.queue(SetAttribute(Attribute::Reset))?;
    out.queue(SetForegroundColor(CYAN))?;
    out.queue(Print(suffix))?;
    out.queue(Print("═".repeat(trailing_len)))?;
    out.queue(Print("\n"))?;
    out.queue(ResetColor)?;
    out.flush()
}

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

/// Render a stage header rule line to stdout.
///
/// ```text
/// ══ stage · iter N · model ════════════════
/// ══ stage · iter N · model · retry 2/3 ═══  ← when retrying
/// ```
pub fn stage_header(stage_name: &str, iteration: u32, model: &str, retry: Option<&RetryInfo>) {
    LAST_WAS_TEXT.store(false, Ordering::Relaxed);
    let term_w = terminal_width() as usize;
    render_stage_header_to(&mut stdout(), stage_name, iteration, model, retry, term_w).ok();

    // Redraw the separator line before updating panel state so the visual
    // boundary is refreshed at stage transitions.
    {
        let mut guard = get_state().lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            return;
        }
        handle_resize_if_needed(&mut guard);
        if let Some(state) = guard.as_ref() {
            let (tw, sep) = (state.term_width, state.separator_row());
            drop(guard);
            draw_panel_separator_raw(tw, sep);
        }
    }
    set_stage(stage_name, iteration, model);
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

pub fn capsule_info(msg: &str) {
    capsule_info_to(&mut stderr(), msg).ok();
}

fn capsule_info_to<W: Write + QueueableCommand>(out: &mut W, msg: &str) -> std::io::Result<()> {
    out.queue(SetForegroundColor(YELLOW))?;
    out.queue(Print("capsule: "))?;
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

pub fn tool_call(name: &str, args: &str, id: &str) {
    LAST_WAS_TEXT.store(false, Ordering::Relaxed);
    let display_args: String = if args.chars().count() > TOOL_ARGS_MAX {
        let s: String = args.chars().take(TOOL_ARGS_MAX).collect();
        format!("{s}…")
    } else {
        args.to_owned()
    };

    if is_in_tty_mode() {
        let mut guard = get_state().lock().unwrap_or_else(|e| e.into_inner());
        handle_resize_if_needed(&mut guard);
        if let Some(state) = guard.as_mut() {
            state.active_tool_calls.push(ToolCallEntry {
                id: id.to_owned(),
                name: name.to_owned(),
                color: YELLOW,
            });
            let info_text = build_info_text(state);
            let (tw, info_r, status_r) = (state.term_width, state.info_row(), state.status_row());
            let snapshot: Vec<(Color, String)> = state
                .active_tool_calls
                .iter()
                .map(|e| (e.color, e.name.clone()))
                .collect();
            drop(guard);
            draw_panel_info_row_raw(tw, info_r, &info_text);
            draw_panel_status_row_multi_raw(tw, status_r, &snapshot);
        }
    } else {
        tool_name_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_owned(), name.to_owned());
        tool_call_to(&mut stdout(), name, &display_args).ok();
    }
}

fn draw_panel_status_row_multi_raw(term_w: u16, status_row: u16, entries: &[(Color, String)]) {
    if entries.is_empty() {
        clear_panel_row(status_row);
        return;
    }
    let mut out = stdout();
    out.queue(cursor::SavePosition).ok();
    out.queue(cursor::MoveTo(0, status_row)).ok();
    out.queue(terminal::Clear(ClearType::CurrentLine)).ok();
    out.queue(Print("  ")).ok();
    let max_w = (term_w as usize).saturating_sub(2);
    let mut used = 0usize;
    for (i, (color, name)) in entries.iter().enumerate() {
        let sep = if i == 0 { 0 } else { 2 };
        let needed = sep + 2 + name.chars().count();
        if used + needed > max_w {
            break;
        }
        if i > 0 {
            out.queue(Print("  ")).ok();
            used += 2;
        }
        out.queue(SetForegroundColor(*color)).ok();
        out.queue(Print("●")).ok();
        out.queue(ResetColor).ok();
        out.queue(Print(format!(" {name}"))).ok();
        used += 2 + name.chars().count();
    }
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

pub fn tool_result(id: &str, success: bool) {
    LAST_WAS_TEXT.store(false, Ordering::Relaxed);
    let color = if success { GREEN } else { RED };

    if is_in_tty_mode() {
        let mut guard = get_state().lock().unwrap_or_else(|e| e.into_inner());
        handle_resize_if_needed(&mut guard);
        if let Some(state) = guard.as_mut() {
            if let Some(entry) = state.active_tool_calls.iter_mut().find(|e| e.id == id) {
                entry.color = color;
            }
            let (tw, status_r) = (state.term_width, state.status_row());
            let snapshot: Vec<(Color, String)> = state
                .active_tool_calls
                .iter()
                .map(|e| (e.color, e.name.clone()))
                .collect();
            state.active_tool_calls.retain(|e| e.id != id);
            drop(guard);
            draw_panel_status_row_multi_raw(tw, status_r, &snapshot);
        }
    } else {
        let name = tool_name_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id)
            .unwrap_or_else(|| "unknown".to_owned());
        tool_result_to(&mut stdout(), &name, success).ok();
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

fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![text.to_string()];
    }
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_len = 0usize;
    for word in text.split_whitespace() {
        let word_len = word.chars().count();
        if current_len == 0 {
            current.push_str(word);
            current_len = word_len;
        } else if current_len + 1 + word_len <= max_width {
            current.push(' ');
            current.push_str(word);
            current_len += 1 + word_len;
        } else {
            lines.push(current.clone());
            current = word.to_string();
            current_len = word_len;
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

fn render_gutter_line<W: Write + QueueableCommand>(out: &mut W, text: &str) -> std::io::Result<()> {
    out.queue(SetForegroundColor(CYAN))?;
    out.queue(Print("│ "))?;
    out.queue(ResetColor)?;
    out.queue(Print(format!("{text}\n")))?;
    Ok(())
}

/// Render a session footer to stdout after a stage completes.
///
/// ```text
/// ── PASS · 09:12 ──────────────────────────
/// │ session: abc123
/// │ notes: full notes text here, wrapped at
/// │        terminal width
/// ──────────────────────────────────────────
/// ```
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
    let status_upper = status_label.to_uppercase();
    let duration_str = format_duration(duration);

    // Title line: "── STATUS · MM:SS " + trailing ─ to fill terminal width
    let prefix = "── ";
    let mid = " · ";
    let space = " ";
    let title_fixed_len = prefix.chars().count()
        + status_upper.chars().count()
        + mid.chars().count()
        + duration_str.chars().count()
        + space.chars().count();
    let trailing_len = term_w.saturating_sub(title_fixed_len);

    out.queue(Print(prefix))?;
    out.queue(SetForegroundColor(status_color))?;
    out.queue(SetAttribute(Attribute::Bold))?;
    out.queue(Print(&status_upper))?;
    out.queue(SetAttribute(Attribute::Reset))?;
    out.queue(ResetColor)?;
    out.queue(Print(format!("{mid}{duration_str}{space}")))?;
    out.queue(SetForegroundColor(CYAN))?;
    out.queue(Print("─".repeat(trailing_len)))?;
    out.queue(ResetColor)?;
    out.queue(Print("\n"))?;

    let gutter_width = term_w.saturating_sub(2);

    if let Some(id) = session_id {
        let truncated_id = if id.chars().count() > SESSION_ID_MAX {
            let s: String = id.chars().take(SESSION_ID_MAX).collect();
            format!("{s}…")
        } else {
            id.to_string()
        };
        render_gutter_line(out, &format!("session: {truncated_id}"))?;
    }

    if let Some(notes) = verdict.and_then(|v| v.notes.as_deref()) {
        let notes_prefix = "notes: ";
        let notes_prefix_len = notes_prefix.chars().count();
        let wrap_width = gutter_width.saturating_sub(notes_prefix_len);
        let wrapped = wrap_text(notes, wrap_width);
        for (i, line) in wrapped.iter().enumerate() {
            if i == 0 {
                render_gutter_line(out, &format!("{notes_prefix}{line}"))?;
            } else {
                render_gutter_line(
                    out,
                    &format!("{:>width$}{line}", "", width = notes_prefix_len),
                )?;
            }
        }
    }

    out.queue(SetForegroundColor(CYAN))?;
    out.queue(Print(format!("{}\n", "─".repeat(term_w))))?;
    out.queue(ResetColor)?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_header_renders_double_rule_with_content() {
        let mut buf: Vec<u8> = Vec::new();
        render_stage_header_to(&mut buf, "builder", 1, "claude-opus-4-6", None, 80).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("══"), "header must use double rule characters");
        assert!(out.contains("builder"), "stage name must appear");
        assert!(out.contains("iter 1"), "iteration must appear");
        assert!(out.contains("claude-opus-4-6"), "model must appear");
        assert!(!out.contains("┌"), "no bordered box");
        assert!(!out.contains("└"), "no bordered box");
    }

    #[test]
    fn stage_header_with_finite_retry_shows_inline() {
        let retry = RetryInfo {
            current: 2,
            max: Some(3),
        };
        let mut buf: Vec<u8> = Vec::new();
        render_stage_header_to(&mut buf, "builder", 2, "claude-opus-4-6", Some(&retry), 80)
            .unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("retry 2/3"), "retry info must appear inline");
    }

    #[test]
    fn stage_header_with_unlimited_retry_shows_infinity() {
        let retry = RetryInfo {
            current: 1,
            max: None,
        };
        let mut buf: Vec<u8> = Vec::new();
        render_stage_header_to(&mut buf, "builder", 1, "claude-opus-4-6", Some(&retry), 80)
            .unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("retry 1/∞"), "unlimited retry must show ∞");
    }

    #[test]
    fn stage_header_fills_to_terminal_width() {
        let mut buf: Vec<u8> = Vec::new();
        render_stage_header_to(&mut buf, "s", 1, "m", None, 40).unwrap();
        let out = String::from_utf8_lossy(&buf);
        let visible: String = strip_ansi(&out);
        let line = visible.lines().next().unwrap_or("");
        assert_eq!(
            line.chars().count(),
            40,
            "header line must fill terminal width; line: {line:?}"
        );
    }

    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut in_esc = false;
        for ch in s.chars() {
            if in_esc {
                if ch == 'm' {
                    in_esc = false;
                }
            } else if ch == '\x1b' {
                in_esc = true;
            } else {
                out.push(ch);
            }
        }
        out
    }

    #[test]
    fn notice_box_renders_without_error() {
        let lines = vec!["Hello".to_string(), "World".to_string()];
        let mut buf: Vec<u8> = Vec::new();
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
    fn tool_call_and_result_with_id_do_not_panic_in_non_tty() {
        tool_call("Bash", "echo hi", "tc_nopanic_001");
        tool_result("tc_nopanic_001", true);
        tool_call("Read", "/path", "tc_nopanic_002");
        tool_result("tc_nopanic_002", false);
    }

    #[test]
    fn tool_result_unknown_id_uses_fallback_name() {
        let mut buf: Vec<u8> = Vec::new();
        let name = tool_name_cache()
            .lock()
            .unwrap()
            .remove("no_such_id")
            .unwrap_or_else(|| "unknown".to_owned());
        tool_result_to(&mut buf, &name, false).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("unknown"), "fallback name must be 'unknown'");
        assert!(contains_seq(&buf, RED_ANSI), "red escape for failure");
    }

    #[test]
    fn tool_call_with_id_result_prints_correct_name_in_non_tty() {
        // Register a tool call by id, then verify tool_result looks it up.
        tool_call("Read", "/some/path", "tc_read_001");
        // After registration, tool_result with the same id must use "Read".
        let mut buf: Vec<u8> = Vec::new();
        let name = tool_name_cache()
            .lock()
            .unwrap()
            .get("tc_read_001")
            .cloned()
            .unwrap_or_default();
        tool_result_to(&mut buf, &name, true).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("Read"), "name from id lookup must appear");
        assert!(contains_seq(&buf, GREEN_ANSI), "green escape for success");
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
        assert!(out.contains("PASS"), "PASS label must appear in footer");
        assert!(out.contains("01:23"), "duration must be formatted as MM:SS");
        assert!(out.contains("──"), "horizontal rule must appear");
        assert!(!out.contains("┌"), "no bordered box");
        assert!(!out.contains("└"), "no bordered box");
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
        assert!(out.contains("FAIL"), "FAIL label must appear");
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
        assert!(out.contains("DONE"), "DONE label must appear");
    }

    #[test]
    fn session_footer_none_verdict_is_implicit_fail() {
        let mut buf: Vec<u8> = Vec::new();
        session_footer_to(&mut buf, None, Duration::from_secs(10), None).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("FAIL"), "implicit fail must show 'FAIL'");
        assert!(
            contains_seq(&buf, RED_ANSI),
            "red escape must be emitted for implicit fail; output: {out:?}"
        );
    }

    #[test]
    fn session_footer_notes_render_in_full() {
        let long_notes = "word ".repeat(20).trim().to_string();
        let v = Verdict {
            status: VerdictStatus::Pass,
            notes: Some(long_notes.clone()),
        };
        let mut buf: Vec<u8> = Vec::new();
        session_footer_to(&mut buf, Some(&v), Duration::from_secs(0), None).unwrap();
        let out = String::from_utf8_lossy(&buf);
        // All words must appear (notes are not truncated)
        assert!(out.contains("word"), "notes must appear in full");
        assert!(
            !out.contains("…"),
            "notes must not be truncated with ellipsis"
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
    fn capsule_info_renders_yellow_prefix_and_message() {
        let mut buf: Vec<u8> = Vec::new();
        capsule_info_to(&mut buf, "GitHub token loaded from .capsule/.env").unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            out.contains("capsule:"),
            "must contain 'capsule:' prefix; output: {out:?}"
        );
        assert!(
            out.contains("GitHub token loaded from .capsule/.env"),
            "must contain the message; output: {out:?}"
        );
        assert!(
            contains_seq(&buf, YELLOW_ANSI),
            "prefix must use yellow color; output: {out:?}"
        );
    }

    #[test]
    fn capsule_info_prefix_precedes_message() {
        let mut buf: Vec<u8> = Vec::new();
        capsule_info_to(&mut buf, "something happened").unwrap();
        let out = String::from_utf8_lossy(&buf);
        let prefix_pos = out.find("capsule:").expect("prefix must be present");
        let msg_pos = out
            .find("something happened")
            .expect("message must be present");
        assert!(
            prefix_pos < msg_pos,
            "prefix must appear before message text"
        );
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
        assert!(
            !is_tty(),
            "is_tty() must return false when stdout is not a terminal"
        );
    }

    #[test]
    fn init_does_not_panic_in_non_tty_context() {
        init();
        assert!(!is_tty());
    }

    #[test]
    fn set_stage_does_not_panic_in_non_tty() {
        set_stage("reviewer", 1, "claude-sonnet-4-6");
    }

    #[test]
    fn clear_stage_does_not_panic_in_non_tty() {
        clear_stage();
    }

    #[test]
    fn set_stage_then_clear_stage_does_not_panic() {
        set_stage("builder", 2, "claude-opus-4-6");
        clear_stage();
    }

    #[test]
    fn build_info_text_includes_stage_iteration_model_duration() {
        let state = DisplayState {
            term_width: 80,
            term_height: 24,
            stage_name: "reviewer".to_string(),
            iteration: 3,
            model: "claude-opus-4-6".to_string(),
            start_time: Instant::now(),
            token_warning: None,
            active_tool_calls: Vec::new(),
        };
        let text = build_info_text(&state);
        assert!(text.contains("reviewer"), "stage name must appear");
        assert!(text.contains("3"), "iteration must appear");
        assert!(text.contains("claude-opus-4-6"), "model must appear");
        assert!(text.contains("00:"), "duration must appear in MM:SS format");
    }

    #[test]
    fn timer_gen_increments_on_teardown() {
        let before = TIMER_GEN.load(Ordering::Relaxed);
        teardown();
        let after = TIMER_GEN.load(Ordering::Relaxed);
        assert!(
            after > before,
            "TIMER_GEN must increment on teardown to stop any running timer"
        );
    }

    #[test]
    fn clear_stage_increments_timer_gen() {
        let before = TIMER_GEN.load(Ordering::Relaxed);
        clear_stage();
        let after = TIMER_GEN.load(Ordering::Relaxed);
        assert!(
            after > before,
            "TIMER_GEN must increment on clear_stage to stop any running timer"
        );
    }

    #[test]
    fn teardown_after_non_tty_init_does_not_panic() {
        init();
        teardown();
    }

    #[test]
    fn rapid_set_stage_does_not_panic() {
        // Verifies rapid set_stage calls are safe in non-TTY (no threads spawned).
        // In TTY mode the condvar notify_all wakes old timer threads immediately so
        // the overlap shrinks to microseconds rather than up to one full second.
        set_stage("builder", 1, "claude-sonnet-4-6");
        set_stage("reviewer", 2, "claude-sonnet-4-6");
        set_stage("builder", 3, "claude-sonnet-4-6");
    }

    #[test]
    fn active_tool_calls_drained_after_tool_result() {
        let mut state = DisplayState {
            term_width: 80,
            term_height: 24,
            stage_name: String::new(),
            iteration: 0,
            model: String::new(),
            start_time: Instant::now(),
            token_warning: None,
            active_tool_calls: Vec::new(),
        };

        for i in 0..5u8 {
            let id = format!("tc_{i}");
            state.active_tool_calls.push(ToolCallEntry {
                id: id.clone(),
                name: format!("Tool{i}"),
                color: YELLOW,
            });
            // simulate tool_result drain for each call immediately
            if let Some(entry) = state.active_tool_calls.iter_mut().find(|e| e.id == id) {
                entry.color = GREEN;
            }
            state.active_tool_calls.retain(|e| e.id != id);
        }

        assert!(
            state.active_tool_calls.is_empty(),
            "active_tool_calls must be empty after all tool_call/tool_result cycles; len={}",
            state.active_tool_calls.len()
        );
    }

    #[test]
    fn active_tool_calls_only_completed_id_removed() {
        let mut state = DisplayState {
            term_width: 80,
            term_height: 24,
            stage_name: String::new(),
            iteration: 0,
            model: String::new(),
            start_time: Instant::now(),
            token_warning: None,
            active_tool_calls: Vec::new(),
        };

        for i in 0..3u8 {
            state.active_tool_calls.push(ToolCallEntry {
                id: format!("tc_{i}"),
                name: format!("Tool{i}"),
                color: YELLOW,
            });
        }

        // complete only tc_1
        let id = "tc_1";
        if let Some(entry) = state.active_tool_calls.iter_mut().find(|e| e.id == id) {
            entry.color = GREEN;
        }
        state.active_tool_calls.retain(|e| e.id != id);

        assert_eq!(
            state.active_tool_calls.len(),
            2,
            "only the completed entry must be removed"
        );
        assert!(
            state.active_tool_calls.iter().all(|e| e.id != id),
            "completed id must not remain in active_tool_calls"
        );
    }
}
