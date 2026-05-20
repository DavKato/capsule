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
use std::io::{stderr, stdout, BufWriter, IsTerminal, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::pipeline::RetryInfo;
use crate::verdict::{Verdict, VerdictStatus};

const GREEN: Color = Color::Green;
const RED: Color = Color::Red;
const CYAN: Color = Color::Cyan;
const YELLOW: Color = Color::Yellow;

const PANEL_HEIGHT: u16 = 3;
const MIN_TERM_HEIGHT: u16 = 12;

struct ToolCallEntry {
    name: String,
    args: String,
    start_time: Instant,
}

struct AgentBuffer {
    tool_call_count: u32,
    start_time: Instant,
    token_snapshot: Option<u64>,
    current_tool_name: String,
    current_tool_args: String,
    last_result_success: Option<bool>,
    has_live_line: bool,
}

impl AgentBuffer {
    fn new(token_snapshot: Option<u64>) -> Self {
        Self {
            tool_call_count: 0,
            start_time: Instant::now(),
            token_snapshot,
            current_tool_name: String::new(),
            current_tool_args: String::new(),
            last_result_success: None,
            has_live_line: false,
        }
    }

    fn push_tool_call(&mut self, name: &str, args: &str) {
        self.tool_call_count += 1;
        self.current_tool_name = name.to_owned();
        self.current_tool_args = args.to_owned();
        self.last_result_success = None;
    }

    fn summary_line(&self, token_count: Option<u64>, elapsed: Duration) -> String {
        let delta = match (token_count, self.token_snapshot) {
            (Some(cur), Some(snap)) => Some(cur.saturating_sub(snap)),
            (Some(cur), None) => Some(cur),
            _ => None,
        };
        let token_part = match delta {
            Some(n) => format!(" · {}", format_tokens_short(n)),
            None => String::new(),
        };
        let noun = if self.tool_call_count == 1 {
            "tool use"
        } else {
            "tool uses"
        };
        format!(
            "Done ({} {noun}{} · {:.1}s)",
            self.tool_call_count,
            token_part,
            elapsed.as_secs_f64()
        )
    }
}

fn agent_buffer_cache() -> &'static Mutex<HashMap<String, AgentBuffer>> {
    static CACHE: OnceLock<Mutex<HashMap<String, AgentBuffer>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn nested_call_parent_map() -> &'static Mutex<HashMap<String, String>> {
    static MAP: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

const LIVE_NESTED_PREFIX: &str = "⎿  ";

fn live_nested_visible_width(name: &str, args: &str) -> usize {
    LIVE_NESTED_PREFIX.chars().count() + 2 + name.chars().count() + 2 + args.chars().count()
}

fn render_live_nested_line_to<W: Write + QueueableCommand>(
    out: &mut W,
    name: &str,
    args: &str,
) -> std::io::Result<()> {
    out.queue(SetForegroundColor(Color::DarkGrey))?;
    out.queue(Print(LIVE_NESTED_PREFIX))?;
    out.queue(SetAttribute(Attribute::SlowBlink))?;
    out.queue(Print("●"))?;
    out.queue(SetAttribute(Attribute::Reset))?;
    out.queue(Print(format!(" {name}  {args}\n")))?;
    out.flush()
}

fn overwrite_live_nested_line_to<W: Write + QueueableCommand>(
    out: &mut W,
    name: &str,
    args: &str,
    offset: u16,
) -> std::io::Result<()> {
    out.queue(cursor::SavePosition)?;
    out.queue(cursor::MoveUp(offset))?;
    out.queue(cursor::MoveToColumn(0))?;
    out.queue(terminal::Clear(ClearType::CurrentLine))?;
    out.queue(SetForegroundColor(Color::DarkGrey))?;
    out.queue(Print(LIVE_NESTED_PREFIX))?;
    out.queue(SetAttribute(Attribute::SlowBlink))?;
    out.queue(Print("●"))?;
    out.queue(SetAttribute(Attribute::Reset))?;
    out.queue(Print(format!(" {name}  {args}")))?;
    out.queue(cursor::RestorePosition)?;
    out.flush()
}

fn update_live_nested_dot_to<W: Write + QueueableCommand>(
    out: &mut W,
    name: &str,
    args: &str,
    success: bool,
    offset: u16,
) -> std::io::Result<()> {
    let color = if success { GREEN } else { RED };
    out.queue(cursor::SavePosition)?;
    out.queue(cursor::MoveUp(offset))?;
    out.queue(cursor::MoveToColumn(0))?;
    out.queue(terminal::Clear(ClearType::CurrentLine))?;
    out.queue(SetForegroundColor(Color::DarkGrey))?;
    out.queue(Print(LIVE_NESTED_PREFIX))?;
    out.queue(SetForegroundColor(color))?;
    out.queue(Print("●"))?;
    out.queue(ResetColor)?;
    out.queue(Print(format!(" {name}  {args}")))?;
    out.queue(cursor::RestorePosition)?;
    out.flush()
}

fn replace_live_line_with_summary_to<W: Write + QueueableCommand>(
    out: &mut W,
    summary: &str,
    offset: u16,
) -> std::io::Result<()> {
    out.queue(cursor::SavePosition)?;
    out.queue(cursor::MoveUp(offset))?;
    out.queue(cursor::MoveToColumn(0))?;
    out.queue(terminal::Clear(ClearType::CurrentLine))?;
    out.queue(SetForegroundColor(Color::DarkGrey))?;
    out.queue(Print(LIVE_NESTED_PREFIX))?;
    out.queue(ResetColor)?;
    out.queue(Print(summary))?;
    out.queue(cursor::RestorePosition)?;
    out.flush()
}

fn format_tokens_short(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M tokens", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k tokens", n as f64 / 1_000.0)
    } else {
        format!("{n} tokens")
    }
}

struct DisplayState {
    term_width: u16,
    term_height: u16,
    stage_name: String,
    iteration: u32,
    model: String,
    start_time: Instant,
    token_warning: Option<String>,
    usage_tokens: Option<u64>,
    active_tool_calls: Vec<(String, ToolCallEntry)>,
    offset_tracker: OffsetTracker,
    agent_buffers: HashMap<String, AgentBuffer>,
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
            usage_tokens: None,
            active_tool_calls: Vec::new(),
            offset_tracker: OffsetTracker::new(),
            agent_buffers: HashMap::new(),
        }
    }

    fn separator_row(&self) -> u16 {
        self.term_height.saturating_sub(PANEL_HEIGHT)
    }

    fn info_row(&self) -> u16 {
        self.term_height.saturating_sub(PANEL_HEIGHT - 1)
    }

    fn warning_row(&self) -> u16 {
        self.term_height.saturating_sub(PANEL_HEIGHT - 2)
    }
}

static STATE: OnceLock<Mutex<Option<DisplayState>>> = OnceLock::new();

static LAST_WAS_TEXT: AtomicBool = AtomicBool::new(false);
static LAST_TEXT_WAS_LIST_ITEM: AtomicBool = AtomicBool::new(false);
static TIMER_GEN: AtomicU64 = AtomicU64::new(0);
static PANIC_HOOK_SET: AtomicBool = AtomicBool::new(false);
static TIMER_WAKE: OnceLock<(Mutex<()>, Condvar)> = OnceLock::new();

fn timer_wake() -> &'static (Mutex<()>, Condvar) {
    TIMER_WAKE.get_or_init(|| (Mutex::new(()), Condvar::new()))
}

static LOG_FILE: OnceLock<Mutex<BufWriter<std::fs::File>>> = OnceLock::new();

pub fn set_log_file(path: &Path) -> anyhow::Result<()> {
    let file = std::fs::File::create(path)
        .map_err(|e| anyhow::anyhow!("failed to open log file {}: {e}", path.display()))?;
    LOG_FILE
        .set(Mutex::new(BufWriter::new(file)))
        .map_err(|_| anyhow::anyhow!("log file already set"))?;
    Ok(())
}

fn log_write(text: &str) {
    if let Some(m) = LOG_FILE.get() {
        if let Ok(mut w) = m.lock() {
            let _ = w.write_all(text.as_bytes());
        }
    }
}

fn log_line(text: &str) {
    if let Some(m) = LOG_FILE.get() {
        if let Ok(mut w) = m.lock() {
            let _ = w.write_all(text.as_bytes());
            let _ = w.write_all(b"\n");
            let _ = w.flush();
        }
    }
}

fn tool_call_cache() -> &'static Mutex<HashMap<String, ToolCallEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<String, ToolCallEntry>>> = OnceLock::new();
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
        let segments = build_info_segments(state);
        let (tw, info, warn_r) = (state.term_width, state.info_row(), state.warning_row());
        let warning = state.token_warning.clone();
        drop(guard);
        draw_panel_info_row_to(&mut out, tw, info, &segments);
        draw_warning_row_to(&mut out, tw, warn_r, warning.as_deref());
    }
}

pub fn set_stage(name: &str, iteration: u32, model: &str) {
    nested_call_parent_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    let in_tty = {
        let mut guard = get_state().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(state) = guard.as_mut() {
            state.stage_name = name.to_owned();
            state.iteration = iteration;
            state.model = model.to_owned();
            state.start_time = Instant::now();
            state.token_warning = None;
            state.usage_tokens = None;
            state.active_tool_calls.clear();
            state.offset_tracker.clear();
            state.agent_buffers.clear();
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
    nested_call_parent_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    agent_buffer_cache()
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
        state.usage_tokens = None;
        state.active_tool_calls.clear();
        state.offset_tracker.clear();
        state.agent_buffers.clear();
        let (info_r, warn_r) = (state.info_row(), state.warning_row());
        drop(guard);
        clear_panel_row_to(&mut out, info_r);
        clear_panel_row_to(&mut out, warn_r);
    }
}

pub fn set_token_warning(msg: Option<&str>) {
    let mut out = stdout().lock();
    let mut guard = get_state().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(state) = guard.as_mut() {
        state.token_warning = msg.map(str::to_owned);
        let tw = state.term_width;
        let warn_r = state.warning_row();
        let warning = state.token_warning.clone();
        drop(guard);
        draw_warning_row_to(&mut out, tw, warn_r, warning.as_deref());
    }
}

pub fn set_usage(total_tokens: u64) {
    let mut out = stdout().lock();
    let mut guard = get_state().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(state) = guard.as_mut() {
        state.usage_tokens = Some(total_tokens);
        let segments = build_info_segments(state);
        let (tw, info) = (state.term_width, state.info_row());
        drop(guard);
        draw_panel_info_row_to(&mut out, tw, info, &segments);
    }
}

fn setup_scroll_region_to<W: Write + QueueableCommand>(out: &mut W, term_w: u16, term_h: u16) {
    let scroll_bottom = term_h.saturating_sub(PANEL_HEIGHT); // 1-indexed == this value

    // Push pre-existing terminal content into scrollback so the scroll region
    // doesn't overwrite it (fixes #184).
    for _ in 0..term_h {
        out.write_all(b"\n").ok();
    }

    // Set DECSTBM scroll region (rows are 1-indexed in the escape sequence).
    out.write_all(format!("\x1b[1;{}r", scroll_bottom).as_bytes())
        .ok();

    let sep_row = scroll_bottom; // 0-indexed (crossterm MoveTo is 0-indexed)
    out.queue(cursor::MoveTo(0, sep_row)).ok();
    out.queue(SetForegroundColor(Color::DarkGrey)).ok();
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

fn format_token_count(total: u64) -> String {
    if total >= 1_000_000 {
        format!("{:.1}M used", total as f64 / 1_000_000.0)
    } else if total >= 1_000 {
        format!("{:.1}k used", total as f64 / 1_000.0)
    } else {
        format!("{total} used")
    }
}

fn build_info_segments(state: &DisplayState) -> Vec<String> {
    let duration = state.start_time.elapsed();
    let mut segs = vec![
        state.stage_name.clone(),
        format!("iter {}", state.iteration),
        state.model.clone(),
        format_duration(duration),
    ];
    if let Some(tokens) = state.usage_tokens {
        segs.push(format_token_count(tokens));
    }
    segs
}

const STATUS_BAR_COLOR: Color = Color::AnsiValue(236);
const STATUS_DIM: Color = Color::DarkGrey;
const STATUS_SEP: &str = " │ ";

fn draw_panel_info_row_to<W: Write + QueueableCommand>(
    out: &mut W,
    term_w: u16,
    info_row: u16,
    segments: &[String],
) {
    out.queue(cursor::SavePosition).ok();
    out.queue(cursor::MoveTo(0, info_row)).ok();
    out.queue(terminal::Clear(ClearType::CurrentLine)).ok();
    out.queue(SetBackgroundColor(STATUS_BAR_COLOR)).ok();

    out.queue(SetForegroundColor(FOOTER_BAR)).ok();
    out.queue(Print("▎")).ok();
    out.queue(Print(" ")).ok();
    let mut content_w: usize = 2; // "▎ "

    for (i, seg) in segments.iter().enumerate() {
        if i == 0 {
            out.queue(SetForegroundColor(Color::Reset)).ok();
            out.queue(SetAttribute(Attribute::Bold)).ok();
        } else {
            out.queue(SetForegroundColor(STATUS_DIM)).ok();
            out.queue(Print(STATUS_SEP)).ok();
            content_w += STATUS_SEP.chars().count();
            out.queue(SetForegroundColor(Color::Reset)).ok();
        }
        out.queue(Print(seg)).ok();
        content_w += seg.chars().count();
        if i == 0 {
            out.queue(SetAttribute(Attribute::Reset)).ok();
            out.queue(SetBackgroundColor(STATUS_BAR_COLOR)).ok();
        }
    }

    let pad = (term_w as usize).saturating_sub(content_w);
    out.queue(Print(" ".repeat(pad))).ok();
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
            format!(
                "{stage_name} · iter {iteration} · {model} · retry {}/{}",
                r.current, r.max
            )
        }
        None => format!("{stage_name} · iter {iteration} · {model}"),
    };

    let prefix = "══ ";
    let suffix = " ";
    let prefix_len = prefix.chars().count();
    let content_len = content.chars().count();
    let suffix_len = suffix.chars().count();
    let used = prefix_len + content_len + suffix_len;
    let trailing_len = term_w.saturating_sub(used);

    out.queue(Print("\n"))?;
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
    LAST_TEXT_WAS_LIST_ITEM.store(false, Ordering::Relaxed);
    let term_w = terminal_width() as usize;
    let mut out = stdout().lock();
    render_stage_header_to(&mut out, stage_name, iteration, model, retry, term_w).ok();
    match retry {
        Some(r) => log_line(&format!(
            "\n══ {stage_name} · iter {iteration} · {model} · retry {}/{} ══",
            r.current, r.max
        )),
        None => log_line(&format!(
            "\n══ {stage_name} · iter {iteration} · {model} ══"
        )),
    }

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

fn draw_warning_row_to<W: Write + QueueableCommand>(
    out: &mut W,
    term_w: u16,
    warn_row: u16,
    warning: Option<&str>,
) {
    out.queue(cursor::SavePosition).ok();
    out.queue(cursor::MoveTo(0, warn_row)).ok();
    out.queue(terminal::Clear(ClearType::CurrentLine)).ok();
    if let Some(msg) = warning {
        out.queue(SetBackgroundColor(STATUS_BAR_COLOR)).ok();
        out.queue(SetForegroundColor(FOOTER_BAR)).ok();
        out.queue(Print("▎")).ok();
        out.queue(Print(" ")).ok();
        out.queue(SetForegroundColor(YELLOW)).ok();
        let text = format!("⚠ {msg}");
        let max = (term_w as usize).saturating_sub(2);
        let truncated: String = text.chars().take(max).collect();
        let content_w = 2 + truncated.chars().count();
        out.queue(Print(&truncated)).ok();
        let pad = (term_w as usize).saturating_sub(content_w);
        out.queue(Print(" ".repeat(pad))).ok();
        out.queue(ResetColor).ok();
    }
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
    log_line(&format!("⚠ {msg}"));
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
    log_line(&format!("capsule: {msg}"));
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
    log_line(msg);
}

fn info_to<W: Write + QueueableCommand>(out: &mut W, msg: &str) -> std::io::Result<()> {
    out.queue(Print(format!("{msg}\n")))?;
    out.flush()
}

/// Print a dimmed informational line to stderr.
pub fn dim_info(msg: &str) {
    dim_info_to(&mut stderr(), msg).ok();
    log_line(msg);
}

fn dim_info_to<W: Write + QueueableCommand>(out: &mut W, msg: &str) -> std::io::Result<()> {
    out.queue(SetAttribute(Attribute::Dim))?;
    out.queue(Print(format!("{msg}\n")))?;
    out.queue(SetAttribute(Attribute::Reset))?;
    out.flush()
}

pub fn println(msg: &str) {
    println_to(&mut stdout().lock(), msg).ok();
    log_line(msg);
}

fn println_to<W: Write + QueueableCommand>(out: &mut W, msg: &str) -> std::io::Result<()> {
    out.queue(Print(format!("{msg}\n")))?;
    out.flush()
}

pub fn print(msg: &str) {
    print_to(&mut stdout().lock(), msg).ok();
    log_write(msg);
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
    for line in lines {
        log_line(line);
    }
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
    out.queue(Print("\n"))?;
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
    success: bool,
    offset: Option<u16>,
) -> std::io::Result<()> {
    let color = if success { GREEN } else { RED };
    match offset {
        Some(n) => {
            out.queue(cursor::SavePosition)?;
            out.queue(cursor::MoveUp(n))?;
            out.queue(cursor::MoveToColumn(0))?;
            out.queue(terminal::Clear(ClearType::CurrentLine))?;
            out.queue(SetForegroundColor(color))?;
            out.queue(Print("●"))?;
            out.queue(ResetColor)?;
            out.queue(Print(format!(" {name}  {args}")))?;
            out.queue(cursor::RestorePosition)?;
        }
        None => {
            out.queue(SetForegroundColor(color))?;
            out.queue(Print("●"))?;
            out.queue(ResetColor)?;
            out.queue(Print(format!(" {name}  {args}\n")))?;
        }
    }
    out.flush()
}

fn render_tty_agent_summary_to<W: Write + QueueableCommand>(
    out: &mut W,
    name: &str,
    args: &str,
    summary: &str,
    offset: Option<u16>,
) -> std::io::Result<()> {
    let suffix = format!("  {summary}");
    match offset {
        Some(n) => {
            out.queue(cursor::SavePosition)?;
            out.queue(cursor::MoveUp(n))?;
            out.queue(cursor::MoveToColumn(0))?;
            out.queue(terminal::Clear(ClearType::CurrentLine))?;
            out.queue(SetForegroundColor(GREEN))?;
            out.queue(Print("●"))?;
            out.queue(ResetColor)?;
            out.queue(Print(format!(" {name}  {args}")))?;
            out.queue(SetForegroundColor(Color::DarkGrey))?;
            out.queue(Print(&suffix))?;
            out.queue(ResetColor)?;
            out.queue(cursor::RestorePosition)?;
        }
        None => {
            out.queue(SetForegroundColor(GREEN))?;
            out.queue(Print("●"))?;
            out.queue(ResetColor)?;
            out.queue(Print(format!(" {name}  {args}")))?;
            out.queue(SetForegroundColor(Color::DarkGrey))?;
            out.queue(Print(&suffix))?;
            out.queue(ResetColor)?;
            out.queue(Print("\n"))?;
        }
    }
    out.flush()
}

fn agent_summary_line_to<W: Write + QueueableCommand>(
    out: &mut W,
    name: &str,
    args: &str,
    summary: &str,
) -> std::io::Result<()> {
    out.queue(SetForegroundColor(GREEN))?;
    out.queue(Print("● "))?;
    out.queue(ResetColor)?;
    out.queue(Print(format!("{name}  {args}")))?;
    out.queue(SetForegroundColor(Color::DarkGrey))?;
    out.queue(Print(format!("  {summary}")))?;
    out.queue(ResetColor)?;
    out.queue(Print("\n"))?;
    out.flush()
}

pub fn tool_call(name: &str, args: &str, id: &str, parent_tool_use_id: Option<&str>) {
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
        if let Some(pid) = parent_tool_use_id {
            nested_call_parent_map()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(id.to_owned(), pid.to_owned());

            let token_snap = state.usage_tokens;
            let tw = state.term_width;

            let (is_first, live_w) = {
                let buf = state
                    .agent_buffers
                    .entry(pid.to_owned())
                    .or_insert_with(|| AgentBuffer::new(token_snap));
                let is_first = buf.tool_call_count == 0;
                buf.push_tool_call(name, &display_args);
                if is_first {
                    buf.has_live_line = true;
                }
                (is_first, live_nested_visible_width(name, &display_args))
            };

            if is_first {
                let live_key = format!("{pid}:live");
                state.offset_tracker.increment_all(live_w, tw);
                state.offset_tracker.register(&live_key, live_w, tw);
                drop(guard);
                render_live_nested_line_to(&mut out, name, &display_args).ok();
            } else {
                let live_key = format!("{pid}:live");
                let scroll_h = state.separator_row();
                let offset = state.offset_tracker.get_offset(&live_key, scroll_h);
                drop(guard);
                if let Some(off) = offset {
                    overwrite_live_nested_line_to(&mut out, name, &display_args, off).ok();
                }
            }
            return;
        }
        log_line(&format!("\n● {name}  {display_args}"));
        state.offset_tracker.increment_all(1, state.term_width);
        let visible_width = 2 + name.chars().count() + 2 + display_args.chars().count();
        state
            .offset_tracker
            .increment_all(visible_width, state.term_width);
        state
            .offset_tracker
            .register(id, visible_width, state.term_width);
        state.active_tool_calls.push((
            id.to_owned(),
            ToolCallEntry {
                name: name.to_owned(),
                args: display_args.clone(),
                start_time: Instant::now(),
            },
        ));
        drop(guard);
        render_tty_tool_call_to(&mut out, name, &display_args).ok();
    } else {
        drop(guard);
        if let Some(pid) = parent_tool_use_id {
            agent_buffer_cache()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry(pid.to_owned())
                .or_insert_with(|| AgentBuffer::new(None))
                .push_tool_call(name, &display_args);
            return;
        }
        log_line(&format!("\n● {name}  {display_args}"));
        tool_call_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                id.to_owned(),
                ToolCallEntry {
                    name: name.to_owned(),
                    args: display_args.clone(),
                    start_time: Instant::now(),
                },
            );
        tool_call_to(&mut out, name, &display_args).ok();
    }
}

fn tool_call_to<W: Write + QueueableCommand>(
    out: &mut W,
    name: &str,
    display_args: &str,
) -> std::io::Result<()> {
    out.queue(Print("\n"))?;
    out.queue(SetForegroundColor(YELLOW))?;
    out.queue(Print("● "))?;
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
        let entry_pos = state
            .active_tool_calls
            .iter()
            .position(|(eid, _)| eid == id);
        let entry = entry_pos.map(|i| state.active_tool_calls.remove(i));
        let Some((_, e)) = entry else {
            // Not a parent call – check if it's a nested call result.
            let parent_id = nested_call_parent_map()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(id);
            if let Some(pid) = parent_id {
                let scroll_h = state.separator_row();
                let render_info = if let Some(buf) = state.agent_buffers.get_mut(&pid) {
                    buf.last_result_success = Some(success);
                    if buf.has_live_line {
                        let live_key = format!("{pid}:live");
                        let off = state.offset_tracker.get_offset(&live_key, scroll_h);
                        Some((
                            buf.current_tool_name.clone(),
                            buf.current_tool_args.clone(),
                            off,
                        ))
                    } else {
                        None
                    }
                } else {
                    None
                };
                drop(guard);
                if let Some((cur_name, cur_args, Some(off))) = render_info {
                    update_live_nested_dot_to(&mut out, &cur_name, &cur_args, success, off).ok();
                }
            } else {
                drop(guard);
            }
            return;
        };
        let (name, args, duration) = (e.name, e.args, e.start_time.elapsed());

        if let Some(buf) = state.agent_buffers.remove(id) {
            let summary = buf.summary_line(state.usage_tokens, buf.start_time.elapsed());
            let has_live_line = buf.has_live_line;
            let live_key = format!("{id}:live");
            let scroll_height = state.separator_row();
            let live_offset = state.offset_tracker.get_offset(&live_key, scroll_height);
            let header_offset = state.offset_tracker.get_offset(id, scroll_height);
            state.offset_tracker.remove(id);
            state.offset_tracker.remove(&live_key);
            let tw = state.term_width;
            drop(guard);

            let append_summary_fallback = |out: &mut std::io::StdoutLock| {
                agent_summary_line_to(out, &name, &args, &summary).ok();
                let suffix_len = 2 + summary.chars().count();
                let line_visible = 2 + name.chars().count() + 2 + args.chars().count() + suffix_len;
                let mut guard = get_state().lock().unwrap_or_else(|e| e.into_inner());
                if let Some(state) = guard.as_mut() {
                    state.offset_tracker.increment_all(line_visible, tw);
                }
            };

            if has_live_line {
                if let Some(off) = live_offset {
                    replace_live_line_with_summary_to(&mut out, &summary, off).ok();
                } else {
                    append_summary_fallback(&mut out);
                }
            } else if header_offset.is_some() {
                render_tty_agent_summary_to(&mut out, &name, &args, &summary, header_offset).ok();
            } else {
                append_summary_fallback(&mut out);
            }
            log_line(&format!("● {name}  {args}  {summary}"));
            return;
        }

        let scroll_height = state.separator_row();
        let offset = state.offset_tracker.get_offset(id, scroll_height);
        state.offset_tracker.remove(id);
        let tw = state.term_width;
        drop(guard);

        render_tty_tool_result_to(&mut out, &name, &args, success, offset).ok();
        log_tool_result(&name, &args, duration, success);

        if offset.is_none() {
            let line_visible = 2 + name.chars().count() + 2 + args.chars().count();
            let mut guard = get_state().lock().unwrap_or_else(|e| e.into_inner());
            if let Some(state) = guard.as_mut() {
                state.offset_tracker.increment_all(line_visible, tw);
            }
        }
    } else {
        drop(guard);
        // Check for a buffered agent in the non-TTY cache.
        let buf = agent_buffer_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        if let Some(buf) = buf {
            let info = tool_call_cache()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(id);
            if let Some(i) = info {
                let summary = buf.summary_line(None, buf.start_time.elapsed());
                agent_summary_line_to(&mut out, &i.name, &i.args, &summary).ok();
                log_line(&format!("● {}  {}  {summary}", i.name, i.args));
            }
            return;
        }
        let info = tool_call_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        let Some(i) = info else {
            return;
        };
        let (name, args, duration) = (i.name, i.args, i.start_time.elapsed());
        tool_result_to(&mut out, &name, &args, success).ok();
        log_tool_result(&name, &args, duration, success);
    }
}

fn log_tool_result(name: &str, args: &str, duration: Duration, success: bool) {
    let status = if success { "Done" } else { "Failed" };
    log_line(&format!(
        "● {name}  {args}  {status} ({:.1}s)",
        duration.as_secs_f64()
    ));
}

fn tool_result_to<W: Write + QueueableCommand>(
    out: &mut W,
    name: &str,
    args: &str,
    success: bool,
) -> std::io::Result<()> {
    let color = if success { GREEN } else { RED };
    out.queue(SetForegroundColor(color))?;
    out.queue(Print("● "))?;
    out.queue(ResetColor)?;
    out.queue(Print(format!("{name}  {args}\n")))?;
    out.flush()
}

fn is_list_item(s: &str) -> bool {
    if s.starts_with("- ") || s.starts_with("* ") || s.starts_with("+ ") {
        return true;
    }
    let rest = s.trim_start_matches(|c: char| c.is_ascii_digit());
    rest.starts_with(". ") && rest.len() < s.len()
}

/// Print agent text (thinking or content) with a dot on the first
/// line of each new block, and indented continuation lines within the same block.
/// Text is wrapped at `content_width` so wrapped portions also get indented.
/// If `parent_tool_use_id` matches an active agent buffer, the text is silently discarded.
pub fn agent_text(text: &str, parent_tool_use_id: Option<&str>) {
    if text.is_empty() {
        return;
    }
    if let Some(pid) = parent_tool_use_id {
        let guard = get_state().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(state) = guard.as_ref() {
            if state.agent_buffers.contains_key(pid) {
                return;
            }
        } else if agent_buffer_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(pid)
        {
            return;
        }
    }
    let mut last = LAST_WAS_TEXT.load(Ordering::Relaxed);
    let last_list = LAST_TEXT_WAS_LIST_ITEM.load(Ordering::Relaxed);
    let was_text = last;
    let term_w = terminal_width() as usize;
    let content_width = term_w.min(MAX_DISPLAY_WIDTH).saturating_sub(2);
    let wrapped = wrap_text(text, content_width);

    let is_list_cont =
        !was_text && last_list && wrapped.first().map(|s| is_list_item(s)).unwrap_or(false);

    let mut out = stdout().lock();
    agent_text_to(&mut out, &wrapped, &mut last, last_list).ok();
    LAST_WAS_TEXT.store(last, Ordering::Relaxed);
    let ends_list = wrapped.last().map(|s| is_list_item(s)).unwrap_or(false);
    LAST_TEXT_WAS_LIST_ITEM.store(ends_list, Ordering::Relaxed);
    if LOG_FILE.get().is_some() {
        if !was_text && !is_list_cont {
            log_write("\n");
        }
        for (i, line) in wrapped.iter().enumerate() {
            if i == 0 && !was_text && !is_list_cont {
                log_line(&format!("● {line}"));
            } else {
                log_line(&format!("  {line}"));
            }
        }
    }

    let mut guard = get_state().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(state) = guard.as_mut() {
        if !was_text && !is_list_cont {
            state.offset_tracker.increment_all(1, state.term_width);
        }
        for line in &wrapped {
            let vw = 2 + line.chars().count();
            state.offset_tracker.increment_all(vw, state.term_width);
        }
    }
}

fn agent_text_to<W: Write + QueueableCommand>(
    out: &mut W,
    lines: &[String],
    last_was_text: &mut bool,
    last_text_was_list: bool,
) -> std::io::Result<()> {
    let is_list_cont = !*last_was_text
        && last_text_was_list
        && lines.first().map(|s| is_list_item(s)).unwrap_or(false);
    if !*last_was_text && !is_list_cont {
        out.queue(Print("\n"))?;
    }
    for (i, line) in lines.iter().enumerate() {
        if i == 0 && !*last_was_text && !is_list_cont {
            out.queue(Print("● "))?;
        } else {
            out.queue(Print("  "))?;
        }
        out.queue(Print(line))?;
        out.queue(Print("\n"))?;
    }
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
        return text.lines().map(String::from).collect();
    }
    let mut out: Vec<String> = Vec::new();
    for input_line in text.lines() {
        let mut current = String::new();
        let mut current_len = 0usize;
        for word in input_line.split_whitespace() {
            let word_len = word.chars().count();
            if current_len == 0 {
                current.push_str(word);
                current_len = word_len;
            } else if current_len + 1 + word_len <= max_width {
                current.push(' ');
                current.push_str(word);
                current_len += 1 + word_len;
            } else {
                out.push(current.clone());
                current = word.to_string();
                current_len = word_len;
            }
        }
        out.push(current);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn local_timestamp() -> String {
    let now = chrono::Local::now();
    now.format("%Y-%m-%d %H:%M").to_string()
}

struct FooterData<'a> {
    stage_name: &'a str,
    iteration: u32,
    verdict: Option<&'a Verdict>,
    duration: Duration,
    session_id: Option<&'a str>,
    context_usage: Option<&'a str>,
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
    context_usage: Option<&str>,
) {
    LAST_WAS_TEXT.store(false, Ordering::Relaxed);
    LAST_TEXT_WAS_LIST_ITEM.store(false, Ordering::Relaxed);
    let ts = local_timestamp();
    session_footer_to(
        &mut stdout().lock(),
        &FooterData {
            stage_name,
            iteration,
            verdict,
            duration,
            session_id,
            context_usage,
            timestamp: &ts,
        },
        terminal_width() as usize,
    )
    .ok();
    if LOG_FILE.get().is_some() {
        let (_, status_label) = match verdict {
            Some(v) => verdict_color_label(&v.status),
            None => (RED, "fail"),
        };
        log_line("");
        log_line(&format!(
            "{stage_name} · iter {iteration} completed at {ts}"
        ));
        log_line(&format!("Status:     {}", status_label.to_uppercase()));
        log_line(&format!("Duration:   {}", format_duration(duration)));
        if let Some(usage) = context_usage {
            log_line(&format!("Context:    {usage}"));
        }
        if let Some(id) = session_id {
            log_line(&format!("Session ID: {id}"));
        }
        if let Some(notes) = verdict.and_then(|v| v.notes.as_deref()) {
            log_line(&format!("Notes: {notes}"));
        }
    }
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

    let label = " Status:     ";
    let status_w = label.chars().count() + status_upper.chars().count();
    card_line_styled(out, block_w, status_w, |out| {
        out.queue(Print(label))?;
        out.queue(SetForegroundColor(status_color))?;
        out.queue(SetAttribute(Attribute::Bold))?;
        out.queue(Print(&status_upper))?;
        Ok(())
    })?;

    card_line(out, &format!("Duration:   {duration_str}"), block_w)?;

    if let Some(usage) = data.context_usage {
        card_line(out, &format!("Context:    {usage}"), block_w)?;
    }

    if let Some(id) = data.session_id {
        let truncated_id = if id.chars().count() > SESSION_ID_MAX {
            let s: String = id.chars().take(SESSION_ID_MAX).collect();
            format!("{s}…")
        } else {
            id.to_string()
        };
        card_line(out, &format!("Session ID: {truncated_id}"), block_w)?;
    }

    if let Some(notes) = data.verdict.and_then(|v| v.notes.as_deref()) {
        card_line(out, "", block_w)?;
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
    entries: HashMap<String, (u16, usize)>,
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

    fn lines_for_width(visible_width: usize, term_width: u16) -> u16 {
        let tw = term_width as usize;
        if tw == 0 {
            return 1;
        }
        visible_width.div_ceil(tw) as u16
    }

    fn register(&mut self, id: &str, visible_width: usize, term_width: u16) {
        self.entries.insert(
            id.to_string(),
            (
                Self::lines_for_width(visible_width, term_width),
                visible_width,
            ),
        );
    }

    fn increment_all(&mut self, visible_width: usize, term_width: u16) {
        let delta = Self::lines_for_width(visible_width, term_width);
        for (offset, _) in self.entries.values_mut() {
            *offset = offset.saturating_add(delta);
        }
    }

    fn get_offset(&self, id: &str, scroll_height: u16) -> Option<u16> {
        let &(offset, _) = self.entries.get(id)?;
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
        for (offset, visible_width) in self.entries.values_mut() {
            let own_old = Self::lines_for_width(*visible_width, old_width);
            let own_new = Self::lines_for_width(*visible_width, new_width);
            let other = (*offset).saturating_sub(own_old) as usize;
            let other_new = (other * old_width as usize).div_ceil(new_width as usize) as u16;
            *offset = own_new + other_new;
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
    fn stage_header_with_retry_shows_inline() {
        let retry = RetryInfo { current: 2, max: 3 };
        let mut buf: Vec<u8> = Vec::new();
        render_stage_header_to(&mut buf, "builder", 2, "claude-opus-4-6", Some(&retry), 80)
            .unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("retry 2/3"), "retry info must appear inline");
    }

    #[test]
    fn stage_header_starts_with_blank_line() {
        let mut buf: Vec<u8> = Vec::new();
        render_stage_header_to(&mut buf, "s", 1, "m", None, 80).unwrap();
        let out = String::from_utf8_lossy(&buf);
        let visible = strip_ansi(&out);
        assert!(
            visible.starts_with('\n'),
            "header must begin with a blank line"
        );
    }

    #[test]
    fn stage_header_fills_to_terminal_width() {
        let mut buf: Vec<u8> = Vec::new();
        render_stage_header_to(&mut buf, "s", 1, "m", None, 40).unwrap();
        let out = String::from_utf8_lossy(&buf);
        let visible: String = strip_ansi(&out);
        let line = visible.lines().find(|l| !l.is_empty()).unwrap_or("");
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
        let line = visible.lines().find(|l| !l.is_empty()).unwrap_or("");
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
    fn tool_call_no_nesting_prefix() {
        let mut buf: Vec<u8> = Vec::new();
        tool_call_to(&mut buf, "Bash", "ls").unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(!out.contains("├"), "tool call must not emit nesting prefix");
    }

    #[test]
    fn tool_result_non_tty_no_cursor_up_emits_green_dot() {
        let mut buf: Vec<u8> = Vec::new();
        tool_result_to(&mut buf, "Bash", "ls -la", true).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            !buf.windows(4).any(|w| w == b"\x1b[1A"),
            "non-TTY tool_result must not emit cursor-up; output: {out:?}"
        );
        assert!(out.contains("Bash"), "tool name must appear");
        assert!(out.contains("ls -la"), "args must appear");
        assert!(out.contains("●"), "dot must appear");
        assert!(
            contains_seq(&buf, GREEN_ANSI),
            "green escape must be emitted for success; output: {out:?}"
        );
    }

    #[test]
    fn tool_result_non_tty_no_cursor_up_emits_red_dot() {
        let mut buf: Vec<u8> = Vec::new();
        tool_result_to(&mut buf, "Write", "path/to/file", false).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            !buf.windows(4).any(|w| w == b"\x1b[1A"),
            "non-TTY tool_result must not emit cursor-up on failure; output: {out:?}"
        );
        assert!(out.contains("Write"), "tool name must appear on failure");
        assert!(out.contains("path/to/file"), "args must appear on failure");
        assert!(
            contains_seq(&buf, RED_ANSI),
            "red escape must be emitted for failure; output: {out:?}"
        );
    }

    #[test]
    fn tool_result_non_tty_no_nesting_prefix() {
        let mut buf: Vec<u8> = Vec::new();
        tool_result_to(&mut buf, "Bash", "ls", true).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            !out.contains("├"),
            "tool result must not emit nesting prefix"
        );
    }

    #[test]
    fn agent_text_first_call_emits_dot_and_text() {
        let mut buf: Vec<u8> = Vec::new();
        let mut last = false;
        agent_text_to(&mut buf, &["hello world".to_string()], &mut last, false).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("hello world"), "text must appear");
        assert!(out.contains('●'), "dot must appear on first line");
        assert!(last, "last_was_text must be true after call");
    }

    #[test]
    fn agent_text_first_call_emits_blank_line_before_dot() {
        let mut buf: Vec<u8> = Vec::new();
        let mut last = false;
        agent_text_to(&mut buf, &["hello".to_string()], &mut last, false).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            out.starts_with('\n'),
            "new section must start with a blank line"
        );
    }

    #[test]
    fn agent_text_continuation_indents_without_dot() {
        let mut buf: Vec<u8> = Vec::new();
        let mut last = true;
        agent_text_to(&mut buf, &["second line".to_string()], &mut last, false).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("second line"), "text must appear");
        assert!(!out.contains('●'), "no dot on continuation line");
        assert!(
            out.starts_with("  "),
            "continuation must be indented with 2 spaces"
        );
    }

    #[test]
    fn agent_text_continuation_no_blank_line() {
        let mut buf: Vec<u8> = Vec::new();
        let mut last = true;
        agent_text_to(&mut buf, &["continued".to_string()], &mut last, false).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            !out.starts_with('\n'),
            "continuation must not start with a blank line"
        );
    }

    #[test]
    fn agent_text_wrapped_lines_all_indented() {
        let mut buf: Vec<u8> = Vec::new();
        let mut last = false;
        let lines = vec![
            "first line".to_string(),
            "second line".to_string(),
            "third line".to_string(),
        ];
        agent_text_to(&mut buf, &lines, &mut last, false).unwrap();
        let out = String::from_utf8_lossy(&buf);
        let text_lines: Vec<&str> = out.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(text_lines.len(), 3);
        assert!(
            text_lines[0].starts_with("● "),
            "first line must start with dot; got: {:?}",
            text_lines[0]
        );
        assert!(
            text_lines[1].starts_with("  "),
            "second line must be indented; got: {:?}",
            text_lines[1]
        );
        assert!(
            text_lines[2].starts_with("  "),
            "third line must be indented; got: {:?}",
            text_lines[2]
        );
    }

    #[test]
    fn agent_text_body_not_dimmed() {
        let mut buf: Vec<u8> = Vec::new();
        let mut last = false;
        agent_text_to(&mut buf, &["body text".to_string()], &mut last, false).unwrap();
        assert!(
            !buf.windows(4).any(|w| w == b"\x1b[2m"),
            "agent_text must not emit dim escape code on body text"
        );
    }

    #[test]
    fn is_list_item_detects_unordered() {
        assert!(is_list_item("- item"));
        assert!(is_list_item("* item"));
        assert!(is_list_item("+ item"));
        assert!(is_list_item("- "), "bare dash-space is a valid list marker");
        assert!(!is_list_item("plain text"));
        assert!(!is_list_item(""));
    }

    #[test]
    fn is_list_item_detects_ordered() {
        assert!(is_list_item("1. first"));
        assert!(is_list_item("2. second"));
        assert!(is_list_item("10. tenth"));
        assert!(!is_list_item("no dot"));
        assert!(!is_list_item(". no number"));
    }

    #[test]
    fn list_continuation_skips_blank_line_and_dot() {
        let mut buf: Vec<u8> = Vec::new();
        let mut last = false; // simulate post-tool-call state
        agent_text_to(&mut buf, &["2. second item".to_string()], &mut last, true).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            !out.starts_with('\n'),
            "list continuation must not emit a blank line"
        );
        assert!(!out.contains('●'), "list continuation must not emit a dot");
        assert!(out.contains("2. second item"), "text must appear");
    }

    #[test]
    fn list_start_after_non_list_emits_blank_line_and_dot() {
        let mut buf: Vec<u8> = Vec::new();
        let mut last = false; // post-tool-call state, but previous was NOT a list
        agent_text_to(&mut buf, &["1. first item".to_string()], &mut last, false).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            out.starts_with('\n'),
            "new list section must start with a blank line"
        );
        assert!(out.contains('●'), "new list section must emit a dot");
    }

    #[test]
    fn non_list_after_list_emits_blank_line_and_dot() {
        let mut buf: Vec<u8> = Vec::new();
        let mut last = false; // post-tool-call state, previous ended with list item
        agent_text_to(
            &mut buf,
            &["Some paragraph text".to_string()],
            &mut last,
            true,
        )
        .unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            out.starts_with('\n'),
            "non-list block after list must get a blank line"
        );
        assert!(
            out.contains('●'),
            "non-list block after list must get a dot"
        );
    }

    // Crossterm emits 256-color (8-bit) SGR sequences on non-tty buffers.
    const GREEN_ANSI: &[u8] = b"\x1b[38;5;10m";
    const RED_ANSI: &[u8] = b"\x1b[38;5;9m";
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
                context_usage: None,
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
                context_usage: None,
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
    fn build_info_segments_includes_stage_iteration_model_duration() {
        let state = DisplayState {
            term_width: 80,
            term_height: 24,
            stage_name: "reviewer".to_string(),
            iteration: 3,
            model: "claude-opus-4-6".to_string(),
            start_time: Instant::now(),
            token_warning: None,
            usage_tokens: None,
            active_tool_calls: Vec::new(),
            offset_tracker: OffsetTracker::new(),
            agent_buffers: HashMap::new(),
        };
        let segs = build_info_segments(&state);
        assert_eq!(segs[0], "reviewer");
        assert_eq!(segs[1], "iter 3");
        assert_eq!(segs[2], "claude-opus-4-6");
        assert!(
            segs[3].contains("00:"),
            "duration must appear in MM:SS format"
        );
        assert_eq!(
            segs.len(),
            4,
            "no extra segments when usage/warning are None"
        );
    }

    #[test]
    fn format_token_count_zero() {
        assert_eq!(format_token_count(0), "0 used");
    }

    #[test]
    fn format_token_count_below_k_threshold() {
        assert_eq!(format_token_count(999), "999 used");
    }

    #[test]
    fn format_token_count_exactly_one_k() {
        assert_eq!(format_token_count(1_000), "1.0k used");
    }

    #[test]
    fn format_token_count_one_point_five_k() {
        assert_eq!(format_token_count(1_500), "1.5k used");
    }

    #[test]
    fn format_token_count_33k() {
        assert_eq!(format_token_count(33_300), "33.3k used");
    }

    #[test]
    fn format_token_count_128k() {
        assert_eq!(format_token_count(128_000), "128.0k used");
    }

    #[test]
    fn format_token_count_exactly_one_m() {
        assert_eq!(format_token_count(1_000_000), "1.0M used");
    }

    #[test]
    fn format_token_count_one_point_five_m() {
        assert_eq!(format_token_count(1_500_000), "1.5M used");
    }

    #[test]
    fn build_info_segments_includes_usage_when_set() {
        let state = DisplayState {
            term_width: 80,
            term_height: 24,
            stage_name: "review".to_string(),
            iteration: 2,
            model: "opus".to_string(),
            start_time: Instant::now(),
            token_warning: None,
            usage_tokens: Some(33_300),
            active_tool_calls: Vec::new(),
            offset_tracker: OffsetTracker::new(),
            agent_buffers: HashMap::new(),
        };
        let segs = build_info_segments(&state);
        assert!(
            segs.iter().any(|s| s.contains("33.3k used")),
            "usage segment must appear when set; got: {segs:?}"
        );
    }

    #[test]
    fn build_info_segments_omits_usage_when_none() {
        let state = DisplayState {
            term_width: 80,
            term_height: 24,
            stage_name: "review".to_string(),
            iteration: 2,
            model: "opus".to_string(),
            start_time: Instant::now(),
            token_warning: None,
            usage_tokens: None,
            active_tool_calls: Vec::new(),
            offset_tracker: OffsetTracker::new(),
            agent_buffers: HashMap::new(),
        };
        let segs = build_info_segments(&state);
        assert!(
            !segs.iter().any(|s| s.contains("used")),
            "usage segment must be absent when None; got: {segs:?}"
        );
    }

    // ── wrap_text edge cases ──────────────────────────────────────────────────

    #[test]
    fn wrap_text_empty_input_returns_single_empty_string() {
        let lines = wrap_text("", 40);
        assert_eq!(lines, vec!["".to_string()], "empty input must yield [\"\"]");
    }

    #[test]
    fn wrap_text_zero_width_returns_lines_unsplit() {
        let lines = wrap_text("hello world\nfoo bar", 0);
        assert_eq!(
            lines,
            vec!["hello world".to_string(), "foo bar".to_string()],
            "zero width must preserve newlines but not wrap further"
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
        let lines = wrap_text("hello world", 10);
        assert_eq!(lines.len(), 2, "text must wrap into two lines");
        assert_eq!(lines[0], "hello");
        assert_eq!(lines[1], "world");
    }

    #[test]
    fn wrap_text_preserves_newlines() {
        let lines = wrap_text("line one\nline two\nline three", 40);
        assert_eq!(
            lines,
            vec![
                "line one".to_string(),
                "line two".to_string(),
                "line three".to_string(),
            ],
            "explicit newlines must produce separate output lines"
        );
    }

    #[test]
    fn wrap_text_wraps_within_newline_separated_lines() {
        let lines = wrap_text("hello world\nfoo bar baz", 10);
        assert_eq!(
            lines,
            vec![
                "hello".to_string(),
                "world".to_string(),
                "foo bar".to_string(),
                "baz".to_string(),
            ],
        );
    }

    #[test]
    fn wrap_text_preserves_blank_lines() {
        let lines = wrap_text("above\n\nbelow", 40);
        assert_eq!(
            lines,
            vec!["above".to_string(), "".to_string(), "below".to_string(),],
        );
    }

    // ── TTY drawing path tests via _to sinks ─────────────────────────────────

    #[test]
    fn setup_scroll_region_pushes_existing_content_into_scrollback_before_decstbm() {
        let mut buf: Vec<u8> = Vec::new();
        setup_scroll_region_to(&mut buf, 80, 24);
        let out = String::from_utf8_lossy(&buf);
        let decstbm_pos = out.find("\x1b[1;21r").expect("DECSTBM must be emitted");
        let newlines_before = &out[..decstbm_pos];
        let newline_count = newlines_before.chars().filter(|&c| c == '\n').count();
        assert!(
            newline_count >= 24,
            "must emit at least term_h newlines before DECSTBM to push existing content into scrollback; got {newline_count}"
        );
    }

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
    fn setup_scroll_region_emits_separator_line() {
        let mut buf: Vec<u8> = Vec::new();
        setup_scroll_region_to(&mut buf, 80, 24);
        let out = String::from_utf8_lossy(&buf);
        assert!(
            out.contains('─'),
            "separator line must use ─ characters; output: {out:?}"
        );
    }

    #[test]
    fn draw_panel_info_row_emits_moveto_and_segments() {
        let mut buf: Vec<u8> = Vec::new();
        let segs = vec!["foo".to_string(), "iter 1".to_string(), "test".to_string()];
        draw_panel_info_row_to(&mut buf, 80, 22, &segs);
        let out = String::from_utf8_lossy(&buf);
        assert!(
            out.contains("\x1b[23;1H"),
            "MoveTo(0, 22) must emit \\x1b[23;1H; output: {out:?}"
        );
        assert!(
            out.contains("foo"),
            "stage name must appear in output; output: {out:?}"
        );
        assert!(
            out.contains("iter 1"),
            "iteration must appear in output; output: {out:?}"
        );
        assert!(
            out.contains("│"),
            "pipe separators must appear in output; output: {out:?}"
        );
    }

    #[test]
    fn offset_tracker_register_and_increment() {
        let mut tracker = OffsetTracker::new();
        tracker.register("tool1", 20, 80); // 1 line
        tracker.increment_all(20, 80);
        assert_eq!(tracker.get_offset("tool1", 100), Some(2));
    }

    #[test]
    fn offset_tracker_register_wrapping_line() {
        let mut tracker = OffsetTracker::new();
        tracker.register("tool1", 100, 80); // 2 lines (wraps)
        assert_eq!(tracker.get_offset("tool1", 100), Some(2));
    }

    #[test]
    fn offset_tracker_increment_line_wrapping() {
        let mut tracker = OffsetTracker::new();
        tracker.register("tool1", 1, 80); // 1 line
        tracker.increment_all(100, 80);
        assert_eq!(tracker.get_offset("tool1", 100), Some(3));
    }

    #[test]
    fn offset_tracker_off_screen_returns_none() {
        let mut tracker = OffsetTracker::new();
        tracker.register("tool1", 40, 80); // 1 line
        for _ in 0..9 {
            tracker.increment_all(80, 80); // push it to offset 10
        }
        assert_eq!(tracker.get_offset("tool1", 5), None);
    }

    #[test]
    fn offset_tracker_on_screen_returns_some() {
        let mut tracker = OffsetTracker::new();
        tracker.register("tool1", 40, 80); // 1 line
        for _ in 0..2 {
            tracker.increment_all(80, 80); // push it to offset 3
        }
        assert_eq!(tracker.get_offset("tool1", 10), Some(3));
    }

    #[test]
    fn offset_tracker_remove_makes_get_return_none() {
        let mut tracker = OffsetTracker::new();
        tracker.register("tool1", 20, 80);
        tracker.remove("tool1");
        assert_eq!(tracker.get_offset("tool1", 100), None);
    }

    #[test]
    fn offset_tracker_multiple_concurrent_entries() {
        let mut tracker = OffsetTracker::new();
        tracker.register("tool1", 20, 80); // 1 line
        tracker.increment_all(80, 80);
        tracker.register("tool2", 20, 80); // 1 line
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
        tracker.register("tool1", 160, 80); // 2 lines at width 80
        tracker.recalculate(80, 40);
        assert_eq!(tracker.get_offset("tool1", 100), Some(4));
    }

    #[test]
    fn offset_tracker_recalculate_short_line_stays_one() {
        let mut tracker = OffsetTracker::new();
        tracker.register("tool1", 40, 80); // 40 visible chars = 1 line at width 80
        tracker.recalculate(80, 40); // resize to 40 wide
                                     // 40 chars still fits in 1 line at width 40
        assert_eq!(tracker.get_offset("tool1", 100), Some(1));
    }

    #[test]
    fn offset_tracker_recalculate_full_width_line_wraps() {
        let mut tracker = OffsetTracker::new();
        tracker.register("tool1", 80, 80); // 1 line at width 80
        tracker.recalculate(80, 40); // resize to 40 wide
                                     // 80 chars now wraps to 2 lines at width 40
        assert_eq!(tracker.get_offset("tool1", 100), Some(2));
    }

    #[test]
    fn offset_tracker_recalculate_multi_wrap_line() {
        let mut tracker = OffsetTracker::new();
        tracker.register("tool1", 200, 80); // 3 lines at width 80 (200/80 = 2.5 → 3)
        tracker.recalculate(80, 40); // resize to 40 wide
                                     // 200 chars at width 40 = 5 lines
        assert_eq!(tracker.get_offset("tool1", 100), Some(5));
    }

    #[test]
    fn offset_tracker_recalculate_with_residual_offset() {
        let mut tracker = OffsetTracker::new();
        tracker.register("tool1", 40, 80); // 1 line own
        tracker.increment_all(80, 80); // +1 line from other content
        tracker.increment_all(80, 80); // +1 line from other content
                                       // total offset = 3 (1 own + 2 other)
        assert_eq!(tracker.get_offset("tool1", 100), Some(3));
        tracker.recalculate(80, 40);
        // own: 40 chars at width 40 = 1 line
        // other: 2 lines * 80 / 40 = 4 lines (each 80-char line becomes 2 at width 40)
        assert_eq!(tracker.get_offset("tool1", 100), Some(5));
    }

    const BLINK_ANSI: &[u8] = b"\x1b[5m";
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
    fn tty_tool_call_no_nesting_prefix() {
        let mut buf: Vec<u8> = Vec::new();
        render_tty_tool_call_to(&mut buf, "Bash", "ls").unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(!out.contains("├"), "TTY call must not emit nesting prefix");
    }

    #[test]
    fn tty_tool_result_in_place_emits_cursor_up() {
        let mut buf: Vec<u8> = Vec::new();
        render_tty_tool_result_to(&mut buf, "Bash", "ls -la", true, Some(3)).unwrap();
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
        render_tty_tool_result_to(&mut buf, "Read", "foo.rs", true, Some(2)).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            contains_seq(&buf, GREEN_ANSI),
            "success result must use green dot; output: {out:?}"
        );
        assert!(
            out.contains("Read"),
            "tool name must appear; output: {out:?}"
        );
    }

    #[test]
    fn tty_tool_result_in_place_red_dot_on_failure() {
        let mut buf: Vec<u8> = Vec::new();
        render_tty_tool_result_to(&mut buf, "Write", "out.rs", false, Some(1)).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            contains_seq(&buf, RED_ANSI),
            "failure result must use red dot; output: {out:?}"
        );
    }

    #[test]
    fn tty_tool_result_off_screen_no_cursor_up() {
        let mut buf: Vec<u8> = Vec::new();
        render_tty_tool_result_to(&mut buf, "Bash", "ls", true, None).unwrap();
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
    }

    #[test]
    fn tty_tool_result_off_screen_emits_dot_and_sub_line() {
        let mut buf: Vec<u8> = Vec::new();
        render_tty_tool_result_to(&mut buf, "Grep", "pattern", true, None).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("●"), "dot must appear; output: {out:?}");
        assert!(
            out.contains("Grep"),
            "tool name must appear; output: {out:?}"
        );
        assert!(out.contains("pattern"), "args must appear; output: {out:?}");
        assert!(
            contains_seq(&buf, GREEN_ANSI),
            "green must be emitted for success; output: {out:?}"
        );
    }

    #[test]
    fn tty_tool_result_args_preserved_in_updated_line() {
        let mut buf: Vec<u8> = Vec::new();
        render_tty_tool_result_to(&mut buf, "Read", "my/special/path.rs", true, Some(5)).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            out.contains("my/special/path.rs"),
            "args must be preserved in the in-place updated line; output: {out:?}"
        );
    }

    #[test]
    fn tool_result_to_renders_done_line() {
        let mut buf: Vec<u8> = Vec::new();
        tool_result_to(&mut buf, "Bash", "ls", true).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            out.contains("Bash"),
            "must contain tool name; output: {out:?}"
        );
        assert!(
            contains_seq(&buf, GREEN_ANSI),
            "must use green for success; output: {out:?}"
        );
    }

    #[test]
    fn tool_result_to_renders_failed_line() {
        let mut buf: Vec<u8> = Vec::new();
        tool_result_to(&mut buf, "Bash", "rm -rf", false).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            out.contains("Bash"),
            "must contain tool name; output: {out:?}"
        );
        assert!(
            contains_seq(&buf, RED_ANSI),
            "must use red for failure; output: {out:?}"
        );
    }

    #[test]
    fn tty_tool_result_no_nesting_prefix() {
        let mut buf: Vec<u8> = Vec::new();
        render_tty_tool_result_to(&mut buf, "Read", "file.rs", true, None).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            !out.contains("├"),
            "TTY tool result must not emit nesting prefix"
        );
    }

    #[test]
    fn tool_call_cache_dedup_second_remove_returns_none() {
        let id = "dedup_test_unique_id_9f3a";
        tool_call_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                id.to_owned(),
                ToolCallEntry {
                    name: "TestTool".to_owned(),
                    args: "some_arg".to_owned(),
                    start_time: Instant::now(),
                },
            );

        let first = tool_call_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        assert!(first.is_some(), "first removal must return the entry");

        let second = tool_call_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        assert!(
            second.is_none(),
            "second removal must return None — duplicate result must be dropped"
        );
    }

    // AgentBuffer tests

    #[test]
    fn agent_buffer_new_starts_with_zero_tool_calls() {
        let buf = AgentBuffer::new(None);
        assert_eq!(buf.tool_call_count, 0);
    }

    #[test]
    fn agent_buffer_push_tool_call_increments_count() {
        let mut buf = AgentBuffer::new(None);
        buf.push_tool_call("A", "a");
        buf.push_tool_call("B", "b");
        buf.push_tool_call("C", "c");
        assert_eq!(buf.tool_call_count, 3);
    }

    #[test]
    fn agent_buffer_summary_line_no_tokens() {
        let buf = AgentBuffer::new(None);
        let summary = buf.summary_line(None, Duration::from_millis(1500));
        assert!(
            summary.starts_with("Done ("),
            "summary must start with Done ("
        );
        assert!(
            summary.contains("tool uses"),
            "summary must mention tool uses"
        );
        assert!(summary.contains("1.5s"), "summary must include duration");
        assert!(
            !summary.contains("tokens"),
            "summary must omit token part when no token data"
        );
    }

    #[test]
    fn agent_buffer_summary_line_with_token_delta() {
        let buf = AgentBuffer::new(Some(10_000));
        let summary = buf.summary_line(Some(15_000), Duration::from_millis(2000));
        assert!(summary.contains("5.0k tokens"), "must show 5k token delta");
        assert!(summary.contains("2.0s"), "must include duration");
    }

    #[test]
    fn agent_buffer_summary_line_tool_call_count_shown() {
        let mut buf = AgentBuffer::new(None);
        buf.push_tool_call("X", "x");
        buf.push_tool_call("Y", "y");
        let summary = buf.summary_line(None, Duration::from_millis(500));
        assert!(
            summary.contains("2 tool uses"),
            "must show count of buffered tool uses; got: {summary}"
        );
    }

    #[test]
    fn agent_summary_line_to_renders_green_dot_and_summary() {
        let mut buf: Vec<u8> = Vec::new();
        agent_summary_line_to(
            &mut buf,
            "Agent",
            "task description",
            "Done (3 tool uses · 1.2s)",
        )
        .unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("Agent"), "tool name must appear");
        assert!(
            out.contains("Done (3 tool uses"),
            "summary must appear; got: {out:?}"
        );
        assert!(out.contains("●"), "dot must appear");
        assert!(
            contains_seq(&buf, GREEN_ANSI),
            "green color must be used for summary line"
        );
    }

    #[test]
    fn agent_summary_line_to_no_cursor_up() {
        let mut buf: Vec<u8> = Vec::new();
        agent_summary_line_to(&mut buf, "Agent", "args", "Done (1 tool uses · 0.5s)").unwrap();
        assert!(
            !buf.windows(4).any(|w| w == b"\x1b[1A"),
            "non-TTY agent summary must not emit cursor-up"
        );
    }

    #[test]
    fn render_tty_agent_summary_to_off_screen_renders_single_dot() {
        let mut buf: Vec<u8> = Vec::new();
        render_tty_agent_summary_to(&mut buf, "Agent", "task", "Done (2 tool uses · 1.0s)", None)
            .unwrap();
        let out = String::from_utf8_lossy(&buf);
        let dot_count = out.matches('●').count();
        assert_eq!(
            dot_count, 1,
            "off-screen summary must render exactly one ● dot; got {dot_count} in: {out:?}"
        );
    }

    #[test]
    fn format_tokens_short_below_k() {
        assert_eq!(format_tokens_short(500), "500 tokens");
    }

    #[test]
    fn format_tokens_short_k_range() {
        assert_eq!(format_tokens_short(5_000), "5.0k tokens");
    }

    #[test]
    fn format_tokens_short_m_range() {
        assert_eq!(format_tokens_short(2_000_000), "2.0M tokens");
    }

    #[test]
    fn agent_buffer_new_has_no_live_line() {
        let buf = AgentBuffer::new(None);
        assert!(!buf.has_live_line, "new buffer must not have a live line");
        assert_eq!(buf.current_tool_name, "");
        assert_eq!(buf.current_tool_args, "");
        assert!(buf.last_result_success.is_none());
    }

    #[test]
    fn agent_buffer_push_tool_call_updates_current_name_and_args() {
        let mut buf = AgentBuffer::new(None);
        buf.push_tool_call("Read", "src/main.rs");
        assert_eq!(buf.current_tool_name, "Read");
        assert_eq!(buf.current_tool_args, "src/main.rs");
    }

    #[test]
    fn agent_buffer_push_tool_call_resets_last_result() {
        let mut buf = AgentBuffer::new(None);
        buf.push_tool_call("A", "a");
        buf.last_result_success = Some(true);
        buf.push_tool_call("B", "b");
        assert!(
            buf.last_result_success.is_none(),
            "last_result_success must be reset on new tool call"
        );
    }

    #[test]
    fn render_live_nested_line_contains_prefix_and_name() {
        let mut buf: Vec<u8> = Vec::new();
        render_live_nested_line_to(&mut buf, "Bash", "ls -la").unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("⎿"), "live line must contain ⎿ prefix");
        assert!(out.contains("●"), "live line must contain dot");
        assert!(out.contains("Bash"), "live line must contain tool name");
        assert!(out.contains("ls -la"), "live line must contain args");
        assert!(out.ends_with('\n'), "live line must end with newline");
    }

    #[test]
    fn render_live_nested_line_uses_blink_dot() {
        let mut buf: Vec<u8> = Vec::new();
        render_live_nested_line_to(&mut buf, "Read", "file.rs").unwrap();
        assert!(
            contains_seq(&buf, BLINK_ANSI),
            "live line dot must use SlowBlink"
        );
    }

    #[test]
    fn overwrite_live_nested_line_emits_cursor_up() {
        let mut buf: Vec<u8> = Vec::new();
        overwrite_live_nested_line_to(&mut buf, "Write", "out.rs", 1).unwrap();
        assert!(
            buf.windows(4).any(|w| w == b"\x1b[1A"),
            "overwrite must emit cursor-up(1)"
        );
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("Write"), "tool name must appear");
        assert!(out.contains("out.rs"), "args must appear");
        assert!(out.contains("⎿"), "prefix must appear");
    }

    #[test]
    fn overwrite_live_nested_line_uses_save_restore() {
        let mut buf: Vec<u8> = Vec::new();
        overwrite_live_nested_line_to(&mut buf, "Bash", "cmd", 2).unwrap();
        // cursor::SavePosition → ESC[s, RestorePosition → ESC[u
        assert!(
            buf.windows(2).any(|w| w == b"\x1b[s" || w == b"\x1b\x37"),
            "must emit SavePosition"
        );
    }

    #[test]
    fn update_live_nested_dot_success_emits_green() {
        let mut buf: Vec<u8> = Vec::new();
        update_live_nested_dot_to(&mut buf, "Bash", "ls", true, 1).unwrap();
        assert!(
            contains_seq(&buf, GREEN_ANSI),
            "success dot update must use green; buf: {:?}",
            String::from_utf8_lossy(&buf)
        );
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("Bash"), "tool name must appear");
        assert!(out.contains("⎿"), "prefix must appear");
    }

    #[test]
    fn update_live_nested_dot_failure_emits_red() {
        let mut buf: Vec<u8> = Vec::new();
        update_live_nested_dot_to(&mut buf, "Write", "f.rs", false, 1).unwrap();
        assert!(
            contains_seq(&buf, RED_ANSI),
            "failure dot update must use red"
        );
    }

    #[test]
    fn replace_live_line_with_summary_emits_cursor_up_and_summary() {
        let mut buf: Vec<u8> = Vec::new();
        replace_live_line_with_summary_to(&mut buf, "Done (3 tool uses · 5.0k tokens · 1.2s)", 1)
            .unwrap();
        assert!(
            buf.windows(4).any(|w| w == b"\x1b[1A"),
            "summary replace must emit cursor-up(1)"
        );
        let out = String::from_utf8_lossy(&buf);
        assert!(
            out.contains("Done (3 tool uses"),
            "summary text must appear; got: {out:?}"
        );
        assert!(out.contains("⎿"), "prefix must appear");
    }

    #[test]
    fn live_nested_visible_width_counts_correctly() {
        let name = "Read";
        let args = "src/lib.rs";
        let expected = LIVE_NESTED_PREFIX.chars().count()
            + 2
            + name.chars().count()
            + 2
            + args.chars().count();
        assert_eq!(live_nested_visible_width(name, args), expected);
    }

    #[test]
    fn agent_text_suppressed_when_parent_buffer_exists() {
        // Set up a buffer in the non-TTY cache for a known parent id.
        let pid = "toolu_suppress_test";
        agent_buffer_cache()
            .lock()
            .unwrap()
            .insert(pid.to_owned(), AgentBuffer::new(None));

        LAST_WAS_TEXT.store(false, Ordering::Relaxed);
        // This call should be suppressed — the buffer exists for pid.
        agent_text("agent said something", Some(pid));
        assert!(
            !LAST_WAS_TEXT.load(Ordering::Relaxed),
            "LAST_WAS_TEXT must stay false when text is suppressed by an active buffer"
        );

        // Clean up so other tests are not affected.
        agent_buffer_cache().lock().unwrap().remove(pid);
    }

    #[test]
    fn agent_text_not_suppressed_without_buffer() {
        let pid = "toolu_no_buffer_test";
        // Ensure no buffer exists for this id.
        agent_buffer_cache().lock().unwrap().remove(pid);

        LAST_WAS_TEXT.store(false, Ordering::Relaxed);
        // Should render normally — no buffer for pid.
        agent_text("top-level text", Some(pid));
        assert!(
            LAST_WAS_TEXT.load(Ordering::Relaxed),
            "LAST_WAS_TEXT must be true after unbuffered agent_text call"
        );

        LAST_WAS_TEXT.store(false, Ordering::Relaxed);
    }
}
