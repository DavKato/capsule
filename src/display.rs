use crossterm::{
    cursor,
    style::{
        Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
    },
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
    args: String,
    start_time: Instant,
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
    offset_tracker: OffsetTracker,
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
            offset_tracker: OffsetTracker::new(),
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
static PANIC_HOOK_SET: AtomicBool = AtomicBool::new(false);
static TIMER_WAKE: OnceLock<(Mutex<()>, Condvar)> = OnceLock::new();

fn timer_wake() -> &'static (Mutex<()>, Condvar) {
    TIMER_WAKE.get_or_init(|| (Mutex::new(()), Condvar::new()))
}

struct ToolCallInfo {
    name: String,
    args: String,
    started_at: Instant,
}

fn tool_call_cache() -> &'static Mutex<HashMap<String, ToolCallInfo>> {
    static CACHE: OnceLock<Mutex<HashMap<String, ToolCallInfo>>> = OnceLock::new();
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
    setup_scroll_region_to(&mut stdout().lock(), term_w, term_h);

    // Register a panic hook so the scroll region and cursor state are restored
    // even when the process panics instead of calling teardown() explicitly.
    // Guard with a flag so repeated init() → teardown() → init() cycles don't stack hooks.
    if !PANIC_HOOK_SET.swap(true, Ordering::SeqCst) {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            teardown();
            prev(info);
        }));
    }
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
    let mut out = stdout().lock();
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
    let mut out = stdout().lock();
    let mut guard = get_state().lock().unwrap_or_else(|e| e.into_inner());
    handle_resize_if_needed(&mut guard, &mut out);
    if let Some(state) = guard.as_mut() {
        let info_text = build_info_text(state);
        let (tw, info) = (state.term_width, state.info_row());
        drop(guard);
        draw_panel_info_row_to(&mut out, tw, info, &info_text);
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
            state.offset_tracker.clear();
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
    tool_call_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    let mut out = stdout().lock();
    let mut guard = get_state().lock().unwrap_or_else(|e| e.into_inner());
    handle_resize_if_needed(&mut guard, &mut out);
    if let Some(state) = guard.as_mut() {
        state.stage_name.clear();
        state.iteration = 0;
        state.model.clear();
        state.token_warning = None;
        state.active_tool_calls.clear();
        state.offset_tracker.clear();
        let (info_r, status_r) = (state.info_row(), state.status_row());
        drop(guard);
        clear_panel_row_to(&mut out, info_r);
        clear_panel_row_to(&mut out, status_r);
    }
}

pub fn set_token_warning(msg: Option<&str>) {
    let mut out = stdout().lock();
    let mut guard = get_state().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(state) = guard.as_mut() {
        state.token_warning = msg.map(str::to_owned);
        let (tw, info) = (state.term_width, state.info_row());
        let info_text = build_info_text(state);
        drop(guard);
        draw_panel_info_row_to(&mut out, tw, info, &info_text);
    }
}

fn setup_scroll_region_to<W: Write + QueueableCommand>(out: &mut W, term_w: u16, term_h: u16) {
    let scroll_bottom = term_h.saturating_sub(PANEL_HEIGHT); // 1-indexed == this value

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
fn handle_resize_if_needed<W: Write + QueueableCommand>(
    guard: &mut Option<DisplayState>,
    out: &mut W,
) -> bool {
    let current = terminal::size().unwrap_or((80, 24));
    if let Some(state) = guard.as_mut() {
        if state.term_width != current.0 || state.term_height != current.1 {
            let old_width = state.term_width;
            state.term_width = current.0;
            state.term_height = current.1;
            state.offset_tracker.recalculate(old_width, current.0);
            setup_scroll_region_to(out, current.0, current.1);
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

fn draw_panel_info_row_to<W: Write + QueueableCommand>(
    out: &mut W,
    term_w: u16,
    info_row: u16,
    text: &str,
) {
    let padded = pad_or_truncate(text, term_w as usize);
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

const MAX_DISPLAY_WIDTH: usize = 120;

fn render_stage_header_to<W: Write + QueueableCommand>(
    out: &mut W,
    stage_name: &str,
    iteration: u32,
    model: &str,
    retry: Option<&RetryInfo>,
    term_w: usize,
) -> std::io::Result<()> {
    let term_w = term_w.min(MAX_DISPLAY_WIDTH);
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

    out.queue(SetForegroundColor(Color::DarkGrey))?;
    out.queue(Print(prefix))?;
    out.queue(ResetColor)?;
    out.queue(Print(&content))?;
    out.queue(SetForegroundColor(Color::DarkGrey))?;
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
    let mut out = stdout().lock();
    render_stage_header_to(&mut out, stage_name, iteration, model, retry, term_w).ok();

    {
        let mut guard = get_state().lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            drop(out);
            return;
        }
        handle_resize_if_needed(&mut guard, &mut out);
        if let Some(state) = guard.as_ref() {
            let (tw, sep) = (state.term_width, state.separator_row());
            drop(guard);
            draw_panel_separator_to(&mut out, tw, sep);
        }
    }
    drop(out);
    set_stage(stage_name, iteration, model);
}

fn draw_panel_separator_to<W: Write + QueueableCommand>(out: &mut W, term_w: u16, sep_row: u16) {
    let dashes = "─".repeat(term_w as usize);
    out.queue(cursor::SavePosition).ok();
    out.queue(cursor::MoveTo(0, sep_row)).ok();
    out.queue(SetForegroundColor(Color::DarkGrey)).ok();
    out.queue(Print(&dashes)).ok();
    out.queue(ResetColor).ok();
    out.queue(cursor::RestorePosition).ok();
    out.flush().ok();
}

fn clear_panel_row_to<W: Write + QueueableCommand>(out: &mut W, row: u16) {
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

pub fn println(msg: &str) {
    println_to(&mut stdout().lock(), msg).ok();
}

fn println_to<W: Write + QueueableCommand>(out: &mut W, msg: &str) -> std::io::Result<()> {
    out.queue(Print(format!("{msg}\n")))?;
    out.flush()
}

pub fn print(msg: &str) {
    print_to(&mut stdout().lock(), msg).ok();
}

fn print_to<W: Write + QueueableCommand>(out: &mut W, msg: &str) -> std::io::Result<()> {
    out.queue(Print(msg))?;
    out.flush()
}

/// Render a bordered notice box using the standard `┌┐└┘` character set.
///
/// Accepts plain content lines (without borders); the box is sized to fit
/// the widest line and capped at terminal width.
pub fn notice_box(lines: &[String]) {
    notice_box_to(&mut stdout().lock(), lines, terminal_width() as usize).ok();
}

fn notice_box_to<W: Write + QueueableCommand>(
    out: &mut W,
    lines: &[String],
    term_w: usize,
) -> std::io::Result<()> {
    render_box(out, lines, term_w)
}

const TOOL_ARGS_MAX: usize = 60;

fn render_tty_tool_call_to<W: Write + QueueableCommand>(
    out: &mut W,
    name: &str,
    display_args: &str,
) -> std::io::Result<()> {
    out.queue(Print("  "))?;
    out.queue(SetAttribute(Attribute::SlowBlink))?;
    out.queue(SetForegroundColor(Color::DarkGrey))?;
    out.queue(Print("●"))?;
    out.queue(SetAttribute(Attribute::Reset))?;
    out.queue(Print(format!(" {name}  {display_args}\n")))?;
    out.flush()
}

fn render_tty_tool_result_to<W: Write + QueueableCommand>(
    out: &mut W,
    name: &str,
    args: &str,
    duration: Duration,
    success: bool,
    offset: Option<u16>,
) -> std::io::Result<()> {
    let color = if success { GREEN } else { RED };
    let label = if success { "Done" } else { "Failed" };

    match offset {
        Some(n) => {
            out.queue(cursor::SavePosition)?;
            out.queue(cursor::MoveUp(n))?;
            out.queue(cursor::MoveToColumn(0))?;
            out.queue(terminal::Clear(ClearType::CurrentLine))?;
            out.queue(Print("  "))?;
            out.queue(SetForegroundColor(color))?;
            out.queue(Print("●"))?;
            out.queue(ResetColor)?;
            out.queue(Print(format!(" {name}  {args}")))?;
            out.queue(cursor::RestorePosition)?;
        }
        None => {
            out.queue(Print("  "))?;
            out.queue(SetForegroundColor(color))?;
            out.queue(Print("●"))?;
            out.queue(ResetColor)?;
            out.queue(Print(format!(" {name}  {args}\n")))?;
        }
    }
    out.queue(Print(format!(
        "    {label} ({:.1}s)\n",
        duration.as_secs_f64()
    )))?;
    out.flush()
}

pub fn tool_call(name: &str, args: &str, id: &str) {
    LAST_WAS_TEXT.store(false, Ordering::Relaxed);

    let display_args: String = if args.chars().count() > TOOL_ARGS_MAX {
        let s: String = args.chars().take(TOOL_ARGS_MAX).collect();
        format!("{s}…")
    } else {
        args.to_owned()
    };

    let mut out = stdout().lock();
    let mut guard = get_state().lock().unwrap_or_else(|e| e.into_inner());
    handle_resize_if_needed(&mut guard, &mut out);
    if let Some(state) = guard.as_mut() {
        // Increment existing entries before registering this one.
        let visible_width = 4 + name.chars().count() + 2 + display_args.chars().count();
        state
            .offset_tracker
            .increment_all(visible_width, state.term_width);
        state.offset_tracker.register(id, 1);
        state.active_tool_calls.push(ToolCallEntry {
            id: id.to_owned(),
            name: name.to_owned(),
            args: display_args.clone(),
            start_time: Instant::now(),
        });
        drop(guard);
        render_tty_tool_call_to(&mut out, name, &display_args).ok();
    } else {
        drop(guard);
        tool_call_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                id.to_owned(),
                ToolCallInfo {
                    name: name.to_owned(),
                    args: display_args.clone(),
                    started_at: Instant::now(),
                },
            );
        tool_call_to(&mut out, name, &display_args).ok();
    }
}

#[allow(dead_code)]
fn draw_panel_status_row_to<W: Write + QueueableCommand>(
    out: &mut W,
    term_w: u16,
    status_row: u16,
    entries: &[(Color, String)],
) {
    if entries.is_empty() {
        out.queue(cursor::SavePosition).ok();
        out.queue(cursor::MoveTo(0, status_row)).ok();
        out.queue(terminal::Clear(ClearType::CurrentLine)).ok();
        out.queue(cursor::RestorePosition).ok();
        out.flush().ok();
        return;
    }
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

    let mut out = stdout().lock();
    let mut guard = get_state().lock().unwrap_or_else(|e| e.into_inner());
    handle_resize_if_needed(&mut guard, &mut out);
    if let Some(state) = guard.as_mut() {
        let entry_pos = state.active_tool_calls.iter().position(|e| e.id == id);
        let entry = entry_pos.map(|i| state.active_tool_calls.remove(i));
        let (name, args, duration) = match entry {
            Some(e) => (e.name, e.args, e.start_time.elapsed()),
            None => ("unknown".to_owned(), String::new(), Duration::ZERO),
        };
        let scroll_height = state.separator_row();
        let offset = state.offset_tracker.get_offset(id, scroll_height);
        state.offset_tracker.remove(id);
        let tw = state.term_width;
        drop(guard);

        render_tty_tool_result_to(&mut out, &name, &args, duration, success, offset).ok();

        // Account for the sub-line (and the solid-dot line in the off-screen case).
        let label = if success { "Done" } else { "Failed" };
        let sub_visible = format!("    {label} ({:.1}s)", duration.as_secs_f64())
            .chars()
            .count();
        let mut guard = get_state().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(state) = guard.as_mut() {
            if offset.is_none() {
                // Off-screen path wrote 2 lines: solid dot + sub-line.
                let line1_visible = 4 + name.chars().count() + 2 + args.chars().count();
                state.offset_tracker.increment_all(line1_visible, tw);
            }
            state.offset_tracker.increment_all(sub_visible, tw);
        }
    } else {
        drop(guard);
        let info = tool_call_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        let (name, args, duration) = match info {
            Some(i) => (i.name, i.args, i.started_at.elapsed()),
            None => ("unknown".to_owned(), String::new(), Duration::ZERO),
        };
        tool_result_to(&mut out, &name, &args, duration, success).ok();
    }
}

fn tool_result_to<W: Write + QueueableCommand>(
    out: &mut W,
    name: &str,
    args: &str,
    duration: Duration,
    success: bool,
) -> std::io::Result<()> {
    let color = if success { GREEN } else { RED };
    let status = if success { "Done" } else { "Failed" };
    out.queue(Print("  "))?;
    out.queue(SetForegroundColor(color))?;
    out.queue(Print("● "))?;
    out.queue(ResetColor)?;
    out.queue(Print(format!("{name}  {args}\n")))?;
    out.queue(Print(format!(
        "    {status} ({:.1}s)\n",
        duration.as_secs_f64()
    )))?;
    out.flush()
}

/// Print agent text (thinking or content) with a dim-white block dot on the first
/// line of each new block, and indented continuation lines within the same block.
pub fn agent_text(text: &str) {
    let mut last = LAST_WAS_TEXT.load(Ordering::Relaxed);
    let visible_width = 4 + text.chars().count(); // "  ● " or "    " (4 chars) + text
    let mut out = stdout().lock();
    agent_text_to(&mut out, text, &mut last).ok();
    LAST_WAS_TEXT.store(last, Ordering::Relaxed);

    let mut guard = get_state().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(state) = guard.as_mut() {
        state
            .offset_tracker
            .increment_all(visible_width, state.term_width);
    }
}

fn agent_text_to<W: Write + QueueableCommand>(
    out: &mut W,
    text: &str,
    last_was_text: &mut bool,
) -> std::io::Result<()> {
    if *last_was_text {
        out.queue(Print("    "))?;
    } else {
        out.queue(Print("  ● "))?;
    }
    out.queue(Print(text))?;
    out.queue(Print("\n"))?;
    out.flush()?;
    *last_was_text = true;
    Ok(())
}

const SESSION_ID_MAX: usize = 40;

fn verdict_color_label(status: &VerdictStatus) -> (Color, &'static str) {
    match status {
        VerdictStatus::Pass => (GREEN, "pass"),
        VerdictStatus::Fail => (RED, "fail"),
        VerdictStatus::Done => (CYAN, "done"),
    }
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

fn local_timestamp() -> String {
    unsafe {
        let mut t: libc::time_t = 0;
        libc::time(&mut t);
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
        )
    }
}

struct FooterData<'a> {
    stage_name: &'a str,
    iteration: u32,
    verdict: Option<&'a Verdict>,
    duration: Duration,
    session_id: Option<&'a str>,
    timestamp: &'a str,
}

/// Render a session footer to stdout after a stage completes.
///
/// `verdict` is `None` for an implicit fail (stage exited without emitting a verdict).
pub fn session_footer(
    stage_name: &str,
    iteration: u32,
    verdict: Option<&Verdict>,
    duration: Duration,
    session_id: Option<&str>,
) {
    LAST_WAS_TEXT.store(false, Ordering::Relaxed);
    let ts = local_timestamp();
    session_footer_to(
        &mut stdout().lock(),
        &FooterData {
            stage_name,
            iteration,
            verdict,
            duration,
            session_id,
            timestamp: &ts,
        },
        terminal_width() as usize,
    )
    .ok();
}

const FOOTER_BG: Color = Color::AnsiValue(236);
const FOOTER_BAR: Color = Color::DarkGrey;

fn card_line_styled<W, F>(
    out: &mut W,
    block_w: usize,
    content_width: usize,
    render_content: F,
) -> std::io::Result<()>
where
    W: Write + QueueableCommand,
    F: FnOnce(&mut W) -> std::io::Result<()>,
{
    out.queue(SetBackgroundColor(FOOTER_BG))?;
    out.queue(SetForegroundColor(FOOTER_BAR))?;
    out.queue(Print("▎"))?;
    out.queue(ResetColor)?;
    out.queue(SetBackgroundColor(FOOTER_BG))?;
    render_content(out)?;
    out.queue(SetAttribute(Attribute::Reset))?;
    out.queue(SetBackgroundColor(FOOTER_BG))?;
    let pad = block_w.saturating_sub(1 + content_width);
    out.queue(Print(" ".repeat(pad)))?;
    out.queue(ResetColor)?;
    out.queue(Print("\n"))?;
    Ok(())
}

fn card_line<W: Write + QueueableCommand>(
    out: &mut W,
    text: &str,
    block_w: usize,
) -> std::io::Result<()> {
    let content = format!(" {text}");
    let w = content.chars().count();
    card_line_styled(out, block_w, w, |out| {
        out.queue(Print(content))?;
        Ok(())
    })
}

fn session_footer_to<W: Write + QueueableCommand>(
    out: &mut W,
    data: &FooterData,
    term_w: usize,
) -> std::io::Result<()> {
    let block_w = term_w.min(MAX_DISPLAY_WIDTH);

    let (status_color, status_label) = match data.verdict {
        Some(v) => verdict_color_label(&v.status),
        None => (RED, "fail"),
    };
    let status_upper = status_label.to_uppercase();
    let duration_str = format_duration(data.duration);

    let title = format!(
        " {} · iter {} completed at {}",
        data.stage_name, data.iteration, data.timestamp
    );

    out.queue(Print("\n"))?;

    card_line(out, "", block_w)?;

    card_line_styled(out, block_w, title.chars().count(), |out| {
        out.queue(SetForegroundColor(Color::Reset))?;
        out.queue(Print(&title))?;
        Ok(())
    })?;

    let sep = format!(" {}", "─".repeat(block_w.saturating_sub(4)));
    card_line_styled(out, block_w, sep.chars().count(), |out| {
        out.queue(SetForegroundColor(FOOTER_BAR))?;
        out.queue(Print(&sep))?;
        Ok(())
    })?;

    let label = " Status:   ";
    let status_w = label.chars().count() + status_upper.chars().count();
    card_line_styled(out, block_w, status_w, |out| {
        out.queue(Print(label))?;
        out.queue(SetForegroundColor(status_color))?;
        out.queue(SetAttribute(Attribute::Bold))?;
        out.queue(Print(&status_upper))?;
        Ok(())
    })?;

    card_line(out, &format!("Duration: {duration_str}"), block_w)?;

    if let Some(id) = data.session_id {
        let truncated_id = if id.chars().count() > SESSION_ID_MAX {
            let s: String = id.chars().take(SESSION_ID_MAX).collect();
            format!("{s}…")
        } else {
            id.to_string()
        };
        card_line(out, &format!("Session:  {truncated_id}"), block_w)?;
    }

    if let Some(notes) = data.verdict.and_then(|v| v.notes.as_deref()) {
        card_line(out, "Notes:", block_w)?;
        let wrap_width = block_w.saturating_sub(5);
        let wrapped = wrap_text(notes, wrap_width);
        for line in &wrapped {
            card_line(out, &format!("  {line}"), block_w)?;
        }
    }

    card_line(out, "", block_w)?;

    out.flush()
}

struct OffsetTracker {
    entries: HashMap<String, u16>,
}

impl OffsetTracker {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
    }

    fn register(&mut self, id: &str, initial_lines: u16) {
        self.entries.insert(id.to_string(), initial_lines);
    }

    fn increment_all(&mut self, visible_width: usize, term_width: u16) {
        let term_width = term_width as usize;
        let delta = if term_width == 0 {
            0u16
        } else {
            visible_width.div_ceil(term_width) as u16
        };
        for offset in self.entries.values_mut() {
            *offset = offset.saturating_add(delta);
        }
    }

    fn get_offset(&self, id: &str, scroll_height: u16) -> Option<u16> {
        let &offset = self.entries.get(id)?;
        if offset > scroll_height {
            None
        } else {
            Some(offset)
        }
    }

    fn remove(&mut self, id: &str) {
        self.entries.remove(id);
    }

    fn recalculate(&mut self, old_width: u16, new_width: u16) {
        if new_width == 0 {
            return;
        }
        let old_width = old_width as usize;
        let new_width = new_width as usize;
        for offset in self.entries.values_mut() {
            let raw = (*offset as usize) * old_width;
            *offset = raw.div_ceil(new_width) as u16;
        }
    }
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

    #[test]
    fn stage_header_caps_at_120_columns() {
        let mut buf: Vec<u8> = Vec::new();
        render_stage_header_to(&mut buf, "s", 1, "m", None, 200).unwrap();
        let visible = strip_ansi(&String::from_utf8_lossy(&buf));
        let line = visible.lines().next().unwrap_or("");
        assert_eq!(
            line.chars().count(),
            120,
            "header must cap at 120 even on wide terminals; line: {line:?}"
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
    fn println_to_writes_msg_with_newline() {
        let mut buf: Vec<u8> = Vec::new();
        println_to(&mut buf, "hello world").unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("hello world"), "message must appear");
        assert!(out.ends_with('\n'), "output must end with newline");
    }

    #[test]
    fn print_to_writes_msg_without_newline() {
        let mut buf: Vec<u8> = Vec::new();
        print_to(&mut buf, "no newline").unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("no newline"), "message must appear");
        assert!(!out.ends_with('\n'), "output must not end with newline");
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
        tool_result_to(&mut buf, "Bash", "ls -la", Duration::from_millis(123), true).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            !buf.windows(4).any(|w| w == b"\x1b[1A"),
            "non-TTY tool_result must not emit cursor-up; output: {out:?}"
        );
        assert!(out.contains("Bash"), "tool name must appear");
        assert!(out.contains("ls -la"), "args must appear");
        assert!(out.contains("Done"), "done label must appear on success");
        assert!(out.contains("0.1s"), "duration must appear formatted");
        assert!(out.contains("●"), "dot must appear");
        assert!(
            contains_seq(&buf, GREEN_ANSI),
            "green escape must be emitted for success; output: {out:?}"
        );
    }

    #[test]
    fn tool_result_non_tty_no_cursor_up_emits_red_dot() {
        let mut buf: Vec<u8> = Vec::new();
        tool_result_to(
            &mut buf,
            "Write",
            "path/to/file",
            Duration::from_millis(456),
            false,
        )
        .unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            !buf.windows(4).any(|w| w == b"\x1b[1A"),
            "non-TTY tool_result must not emit cursor-up on failure; output: {out:?}"
        );
        assert!(out.contains("Write"), "tool name must appear on failure");
        assert!(out.contains("path/to/file"), "args must appear on failure");
        assert!(
            out.contains("Failed"),
            "failed label must appear on failure"
        );
        assert!(out.contains("0.5s"), "duration must appear formatted");
        assert!(
            contains_seq(&buf, RED_ANSI),
            "red escape must be emitted for failure; output: {out:?}"
        );
    }

    #[test]
    fn agent_text_first_call_emits_dot_and_text() {
        let mut buf: Vec<u8> = Vec::new();
        let mut last = false;
        agent_text_to(&mut buf, "hello world", &mut last).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("hello world"), "text must appear");
        assert!(out.contains('●'), "dot must appear on first line");
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

    fn footer(
        buf: &mut Vec<u8>,
        stage: &str,
        iter: u32,
        verdict: Option<&Verdict>,
        secs: u64,
        session: Option<&str>,
    ) {
        session_footer_to(
            buf,
            &FooterData {
                stage_name: stage,
                iteration: iter,
                verdict,
                duration: Duration::from_secs(secs),
                session_id: session,
                timestamp: "2026-05-11 14:32",
            },
            80,
        )
        .unwrap();
    }

    #[test]
    fn session_footer_starts_with_blank_line() {
        let v = Verdict {
            status: VerdictStatus::Pass,
            notes: None,
        };
        let mut buf: Vec<u8> = Vec::new();
        footer(&mut buf, "build", 1, Some(&v), 83, None);
        let out = String::from_utf8_lossy(&buf);
        assert!(
            out.starts_with('\n'),
            "footer must begin with a blank line (margin-top)"
        );
    }

    #[test]
    fn session_footer_title_contains_stage_iter_and_timestamp() {
        let v = Verdict {
            status: VerdictStatus::Pass,
            notes: None,
        };
        let mut buf: Vec<u8> = Vec::new();
        footer(&mut buf, "build", 2, Some(&v), 83, None);
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("build"), "title must contain stage name");
        assert!(out.contains("iter 2"), "title must contain iteration");
        assert!(
            out.contains("2026-05-11 14:32"),
            "title must contain timestamp"
        );
        assert!(
            out.contains("completed at"),
            "title must contain 'completed at'"
        );
    }

    #[test]
    fn session_footer_pass_contains_status_and_duration() {
        let v = Verdict {
            status: VerdictStatus::Pass,
            notes: None,
        };
        let mut buf: Vec<u8> = Vec::new();
        footer(&mut buf, "build", 1, Some(&v), 83, None);
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("PASS"), "PASS label must appear in footer");
        assert!(out.contains("01:23"), "duration must be formatted as MM:SS");
    }

    #[test]
    fn session_footer_fail_shows_red_status() {
        let v = Verdict {
            status: VerdictStatus::Fail,
            notes: None,
        };
        let mut buf: Vec<u8> = Vec::new();
        footer(&mut buf, "build", 1, Some(&v), 5, None);
        assert!(
            contains_seq(&buf, RED_ANSI),
            "red escape must be emitted for fail status"
        );
    }

    #[test]
    fn session_footer_done_shows_cyan_status() {
        let v = Verdict {
            status: VerdictStatus::Done,
            notes: None,
        };
        let mut buf: Vec<u8> = Vec::new();
        footer(&mut buf, "build", 1, Some(&v), 0, None);
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("DONE"), "DONE label must appear");
    }

    #[test]
    fn session_footer_none_verdict_is_implicit_fail() {
        let mut buf: Vec<u8> = Vec::new();
        footer(&mut buf, "build", 1, None, 10, None);
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("FAIL"), "implicit fail must show 'FAIL'");
        assert!(
            contains_seq(&buf, RED_ANSI),
            "red escape must be emitted for implicit fail"
        );
    }

    #[test]
    fn session_footer_uses_left_bar_not_pipe() {
        let v = Verdict {
            status: VerdictStatus::Pass,
            notes: None,
        };
        let mut buf: Vec<u8> = Vec::new();
        footer(&mut buf, "build", 1, Some(&v), 0, None);
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains('▎'), "footer must use ▎ left bar");
        assert!(!out.contains("│"), "footer must not use │ pipe gutter");
    }

    #[test]
    fn session_footer_has_dark_background() {
        let v = Verdict {
            status: VerdictStatus::Pass,
            notes: None,
        };
        let mut buf: Vec<u8> = Vec::new();
        footer(&mut buf, "build", 1, Some(&v), 0, None);
        // AnsiValue(236) background: ESC[48;5;236m
        let bg_ansi: &[u8] = b"\x1b[48;5;236m";
        assert!(
            contains_seq(&buf, bg_ansi),
            "footer must use AnsiValue(236) background"
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
        footer(&mut buf, "build", 1, Some(&v), 0, None);
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("word"), "notes must appear in full");
        assert!(out.contains("Notes:"), "notes section must have a label");
    }

    #[test]
    fn session_footer_session_id_displayed() {
        let v = Verdict {
            status: VerdictStatus::Pass,
            notes: None,
        };
        let mut buf: Vec<u8> = Vec::new();
        footer(&mut buf, "build", 1, Some(&v), 0, Some("sess_abc123"));
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
        footer(&mut buf, "build", 1, Some(&v), 0, Some(&long_id));
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("…"), "long session id must be truncated");
        assert!(
            !out.contains(&long_id),
            "full long session id must not appear"
        );
    }

    #[test]
    fn session_footer_no_session_line_when_absent() {
        let v = Verdict {
            status: VerdictStatus::Pass,
            notes: None,
        };
        let mut buf: Vec<u8> = Vec::new();
        footer(&mut buf, "build", 1, Some(&v), 0, None);
        let out = String::from_utf8_lossy(&buf);
        assert!(
            !out.contains("Session:"),
            "session line must be absent when no session id"
        );
    }

    #[test]
    fn session_footer_caps_at_120_columns() {
        let v = Verdict {
            status: VerdictStatus::Pass,
            notes: None,
        };
        let mut buf: Vec<u8> = Vec::new();
        session_footer_to(
            &mut buf,
            &FooterData {
                stage_name: "build",
                iteration: 1,
                verdict: Some(&v),
                duration: Duration::from_secs(0),
                session_id: None,
                timestamp: "2026-05-11 14:32",
            },
            200,
        )
        .unwrap();
        let out = String::from_utf8_lossy(&buf);
        let visible = strip_ansi(&out);
        for line in visible.lines() {
            if line.is_empty() {
                continue;
            }
            assert!(
                line.chars().count() <= 120,
                "footer line must not exceed 120 columns; got {}: {line:?}",
                line.chars().count()
            );
        }
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
            offset_tracker: OffsetTracker::new(),
        };
        let text = build_info_text(&state);
        assert!(text.contains("reviewer"), "stage name must appear");
        assert!(text.contains("3"), "iteration must appear");
        assert!(text.contains("claude-opus-4-6"), "model must appear");
        assert!(text.contains("00:"), "duration must appear in MM:SS format");
    }

    // ── wrap_text edge cases ──────────────────────────────────────────────────

    #[test]
    fn wrap_text_empty_input_returns_single_empty_string() {
        let lines = wrap_text("", 40);
        assert_eq!(lines, vec!["".to_string()], "empty input must yield [\"\"]");
    }

    #[test]
    fn wrap_text_zero_width_returns_full_text_unchanged() {
        let text = "hello world foo bar";
        let lines = wrap_text(text, 0);
        assert_eq!(
            lines,
            vec![text.to_string()],
            "zero width must return the whole text as a single element"
        );
    }

    #[test]
    fn wrap_text_single_word_exceeding_width_placed_on_own_line() {
        // A word longer than max_width cannot be split; it occupies its own line.
        let lines = wrap_text("superlongword", 5);
        assert_eq!(
            lines,
            vec!["superlongword".to_string()],
            "word exceeding width must appear on its own line without truncation"
        );
    }

    #[test]
    fn wrap_text_wraps_at_word_boundary() {
        // Width 10 fits "hello" (5) but not "hello world" (11).
        let lines = wrap_text("hello world", 10);
        assert_eq!(lines.len(), 2, "text must wrap into two lines");
        assert_eq!(lines[0], "hello");
        assert_eq!(lines[1], "world");
    }

    // ── TTY drawing path tests via _to sinks ─────────────────────────────────

    #[test]
    fn setup_scroll_region_emits_decstbm_sequence() {
        let mut buf: Vec<u8> = Vec::new();
        // term_h=24, PANEL_HEIGHT=3 → scroll_bottom=21 → "\x1b[1;21r"
        setup_scroll_region_to(&mut buf, 80, 24);
        let out = String::from_utf8_lossy(&buf);
        assert!(
            out.contains("\x1b[1;21r"),
            "DECSTBM scroll-region escape must be emitted; output: {out:?}"
        );
    }

    #[test]
    fn setup_scroll_region_emits_separator_line_in_cyan() {
        let mut buf: Vec<u8> = Vec::new();
        setup_scroll_region_to(&mut buf, 80, 24);
        let out = String::from_utf8_lossy(&buf);
        assert!(
            out.contains('─'),
            "separator line must use ─ characters; output: {out:?}"
        );
        assert!(
            contains_seq(&buf, CYAN_ANSI),
            "separator line must use cyan color; output: {out:?}"
        );
    }

    #[test]
    fn draw_panel_info_row_emits_moveto_and_cyan() {
        let mut buf: Vec<u8> = Vec::new();
        // info_row=22 → crossterm MoveTo(0, 22) → \x1b[23;1H
        draw_panel_info_row_to(&mut buf, 80, 22, "Stage: foo  Iter: 1  Model: test");
        let out = String::from_utf8_lossy(&buf);
        // Verify a MoveTo escape was emitted (format: ESC [ row ; col H)
        assert!(
            out.contains("\x1b[23;1H"),
            "MoveTo(0, 22) must emit \\x1b[23;1H; output: {out:?}"
        );
        assert!(
            contains_seq(&buf, CYAN_ANSI),
            "info row must use cyan color; output: {out:?}"
        );
        assert!(
            out.contains("Stage: foo"),
            "info text must appear in output; output: {out:?}"
        );
    }

    #[test]
    fn draw_panel_status_row_emits_moveto_and_tool_dot() {
        let entries = vec![(YELLOW, "Bash".to_string())];
        let mut buf: Vec<u8> = Vec::new();
        // status_row=23 → crossterm MoveTo(0, 23) → \x1b[24;1H
        draw_panel_status_row_to(&mut buf, 80, 23, &entries);
        let out = String::from_utf8_lossy(&buf);
        assert!(
            out.contains("\x1b[24;1H"),
            "MoveTo(0, 23) must emit \\x1b[24;1H; output: {out:?}"
        );
        assert!(
            out.contains("●"),
            "tool dot must appear in status row; output: {out:?}"
        );
        assert!(
            out.contains("Bash"),
            "tool name must appear in status row; output: {out:?}"
        );
        assert!(
            contains_seq(&buf, YELLOW_ANSI),
            "in-progress tool dot must use yellow; output: {out:?}"
        );
    }

    #[test]
    fn draw_panel_status_row_green_dot_after_success() {
        let entries = vec![(GREEN, "Read".to_string())];
        let mut buf: Vec<u8> = Vec::new();
        draw_panel_status_row_to(&mut buf, 80, 20, &entries);
        assert!(
            contains_seq(&buf, GREEN_ANSI),
            "completed tool dot must use green; output: {:?}",
            String::from_utf8_lossy(&buf)
        );
    }

    #[test]
    fn draw_panel_status_row_empty_entries_emits_clear() {
        let mut buf: Vec<u8> = Vec::new();
        draw_panel_status_row_to(&mut buf, 80, 20, &[]);
        // Should emit a ClearType::CurrentLine (\x1b[2K) — no dot
        let out = String::from_utf8_lossy(&buf);
        assert!(
            !out.contains("●"),
            "empty entries must not emit a dot; output: {out:?}"
        );
    }

    #[test]
    fn offset_tracker_register_and_increment() {
        let mut tracker = OffsetTracker::new();
        tracker.register("tool1", 1);
        tracker.increment_all(20, 80);
        assert_eq!(tracker.get_offset("tool1", 100), Some(2));
    }

    #[test]
    fn offset_tracker_increment_line_wrapping() {
        let mut tracker = OffsetTracker::new();
        tracker.register("tool1", 0);
        tracker.increment_all(100, 80);
        assert_eq!(tracker.get_offset("tool1", 100), Some(2));
    }

    #[test]
    fn offset_tracker_off_screen_returns_none() {
        let mut tracker = OffsetTracker::new();
        tracker.register("tool1", 10);
        assert_eq!(tracker.get_offset("tool1", 5), None);
    }

    #[test]
    fn offset_tracker_on_screen_returns_some() {
        let mut tracker = OffsetTracker::new();
        tracker.register("tool1", 3);
        assert_eq!(tracker.get_offset("tool1", 10), Some(3));
    }

    #[test]
    fn offset_tracker_remove_makes_get_return_none() {
        let mut tracker = OffsetTracker::new();
        tracker.register("tool1", 1);
        tracker.remove("tool1");
        assert_eq!(tracker.get_offset("tool1", 100), None);
    }

    #[test]
    fn offset_tracker_multiple_concurrent_entries() {
        let mut tracker = OffsetTracker::new();
        tracker.register("tool1", 1);
        tracker.increment_all(80, 80);
        tracker.register("tool2", 1);
        tracker.increment_all(80, 80);
        assert_eq!(tracker.get_offset("tool1", 100), Some(3));
        assert_eq!(tracker.get_offset("tool2", 100), Some(2));
        tracker.remove("tool1");
        assert_eq!(tracker.get_offset("tool1", 100), None);
        assert_eq!(tracker.get_offset("tool2", 100), Some(2));
    }

    #[test]
    fn offset_tracker_recalculate_on_resize() {
        let mut tracker = OffsetTracker::new();
        tracker.register("tool1", 2);
        tracker.recalculate(80, 40);
        assert_eq!(tracker.get_offset("tool1", 100), Some(4));
    }

    // Crossterm emits ESC[5m for SlowBlink attribute.
    const BLINK_ANSI: &[u8] = b"\x1b[5m";
    // Color::DarkGrey → AnsiValue(8) → ESC[38;5;8m
    const DARK_GREY_ANSI: &[u8] = b"\x1b[38;5;8m";

    #[test]
    fn tty_tool_call_emits_slow_blink_and_dark_grey_dot() {
        let mut buf: Vec<u8> = Vec::new();
        render_tty_tool_call_to(&mut buf, "Bash", "ls -la").unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            contains_seq(&buf, BLINK_ANSI),
            "TTY tool call dot must use SlowBlink attribute; output: {out:?}"
        );
        assert!(
            contains_seq(&buf, DARK_GREY_ANSI),
            "TTY tool call dot must use DarkGrey color; output: {out:?}"
        );
        assert!(
            out.contains("●"),
            "dot character must appear; output: {out:?}"
        );
    }

    #[test]
    fn tty_tool_call_includes_name_and_args() {
        let mut buf: Vec<u8> = Vec::new();
        render_tty_tool_call_to(&mut buf, "Read", "src/main.rs").unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            out.contains("Read"),
            "tool name must appear; output: {out:?}"
        );
        assert!(
            out.contains("src/main.rs"),
            "args must appear; output: {out:?}"
        );
        assert!(out.ends_with('\n'), "output must end with newline");
    }

    #[test]
    fn tty_tool_result_in_place_emits_cursor_up() {
        let mut buf: Vec<u8> = Vec::new();
        render_tty_tool_result_to(
            &mut buf,
            "Bash",
            "ls -la",
            Duration::from_millis(200),
            true,
            Some(3),
        )
        .unwrap();
        let out = String::from_utf8_lossy(&buf);
        // cursor::MoveUp(3) → ESC[3A
        assert!(
            buf.windows(4).any(|w| w == b"\x1b[3A"),
            "in-place update must emit cursor-up(3); output: {out:?}"
        );
    }

    #[test]
    fn tty_tool_result_in_place_emits_green_dot_and_sub_line() {
        let mut buf: Vec<u8> = Vec::new();
        render_tty_tool_result_to(
            &mut buf,
            "Read",
            "foo.rs",
            Duration::from_millis(200),
            true,
            Some(2),
        )
        .unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            contains_seq(&buf, GREEN_ANSI),
            "success result must use green dot; output: {out:?}"
        );
        assert!(
            out.contains("Done"),
            "Done label must appear; output: {out:?}"
        );
        assert!(
            out.contains("0.2s"),
            "duration must appear; output: {out:?}"
        );
    }

    #[test]
    fn tty_tool_result_in_place_red_dot_on_failure() {
        let mut buf: Vec<u8> = Vec::new();
        render_tty_tool_result_to(
            &mut buf,
            "Write",
            "out.rs",
            Duration::from_millis(300),
            false,
            Some(1),
        )
        .unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            contains_seq(&buf, RED_ANSI),
            "failure result must use red dot; output: {out:?}"
        );
        assert!(
            out.contains("Failed"),
            "Failed label must appear; output: {out:?}"
        );
    }

    #[test]
    fn tty_tool_result_off_screen_no_cursor_up() {
        let mut buf: Vec<u8> = Vec::new();
        render_tty_tool_result_to(
            &mut buf,
            "Bash",
            "ls",
            Duration::from_millis(100),
            true,
            None,
        )
        .unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            !buf.windows(3).any(|w| w == b"\x1b[A" || {
                w.len() >= 4 && w[0] == b'\x1b' && w[1] == b'[' && w[w.len() - 1] == b'A'
            }),
            "off-screen result must not emit cursor-up; output: {out:?}"
        );
        assert!(
            out.contains("Bash"),
            "tool name must appear in off-screen result; output: {out:?}"
        );
        assert!(
            out.contains("Done"),
            "Done label must appear; output: {out:?}"
        );
    }

    #[test]
    fn tty_tool_result_off_screen_emits_dot_and_sub_line() {
        let mut buf: Vec<u8> = Vec::new();
        render_tty_tool_result_to(
            &mut buf,
            "Grep",
            "pattern",
            Duration::from_millis(450),
            true,
            None,
        )
        .unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("●"), "dot must appear; output: {out:?}");
        assert!(
            out.contains("Grep"),
            "tool name must appear; output: {out:?}"
        );
        assert!(out.contains("pattern"), "args must appear; output: {out:?}");
        assert!(
            out.contains("Done"),
            "Done label must appear; output: {out:?}"
        );
        assert!(
            out.contains("0.5s"),
            "duration must appear; output: {out:?}"
        );
        assert!(
            contains_seq(&buf, GREEN_ANSI),
            "green must be emitted for success; output: {out:?}"
        );
    }

    #[test]
    fn tty_tool_result_args_preserved_in_updated_line() {
        let mut buf: Vec<u8> = Vec::new();
        render_tty_tool_result_to(
            &mut buf,
            "Read",
            "my/special/path.rs",
            Duration::from_millis(50),
            true,
            Some(5),
        )
        .unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            out.contains("my/special/path.rs"),
            "args must be preserved in the in-place updated line; output: {out:?}"
        );
    }
}
