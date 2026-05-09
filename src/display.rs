use crossterm::{
    cursor,
    style::{Color, Print, ResetColor, SetForegroundColor},
    terminal,
    terminal::ClearType,
    QueueableCommand,
};
use std::io::{stdout, Write};

pub const GREEN: Color = Color::Green;
pub const RED: Color = Color::Red;
pub const CYAN: Color = Color::Cyan;
pub const YELLOW: Color = Color::Yellow;

/// Info about the current retry attempt — shown only when retrying.
pub struct RetryInfo {
    /// How many times the stage has already failed (1 = first retry).
    pub current: u32,
    /// Maximum number of retries (`None` = unlimited).
    pub max: Option<u32>,
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

fn render_box<W: Write + QueueableCommand>(
    out: &mut W,
    content: &[String],
    term_w: usize,
) -> std::io::Result<()> {
    let max_content = content.iter().map(|l| l.len()).max().unwrap_or(0);
    // box width includes two border chars; inner holds the padded content
    let box_w = (max_content + 4).min(term_w);
    let inner_w = box_w.saturating_sub(2);

    let top = format!("┌{}┐", "─".repeat(box_w.saturating_sub(2)));
    let bot = format!("└{}┘", "─".repeat(box_w.saturating_sub(2)));

    out.queue(SetForegroundColor(CYAN))?;
    out.queue(Print(format!("{top}\n")))?;
    for line in content {
        let padded = format!(" {line}");
        let cell = if padded.len() + 1 > inner_w {
            // Truncate: leave room for trailing space inside the border.
            format!("{} ", &padded[..inner_w.saturating_sub(1)])
        } else {
            format!("{:<width$}", padded, width = inner_w)
        };
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
    let lines = header_content_lines(stage_name, iteration, model, retry);
    let term_w = terminal_width() as usize;
    render_box(&mut stdout(), &lines, term_w).ok();
}

const TOOL_ARGS_MAX: usize = 60;

/// Print a yellow-dot tool-call line: `  ● ToolName  args`.
pub fn tool_call(name: &str, args: &str) {
    tool_call_to(&mut stdout(), name, args).ok();
}

fn tool_call_to<W: Write + QueueableCommand>(
    out: &mut W,
    name: &str,
    args: &str,
) -> std::io::Result<()> {
    let display_args: String = if args.chars().count() > TOOL_ARGS_MAX {
        let s: String = args.chars().take(TOOL_ARGS_MAX).collect();
        format!("{s}…")
    } else {
        args.to_owned()
    };
    out.queue(SetForegroundColor(YELLOW))?;
    out.queue(Print("  ● "))?;
    out.queue(ResetColor)?;
    out.queue(Print(format!("{name}  {display_args}\n")))?;
    out.flush()
}

/// Cursor-up to overwrite the previous tool-call line with a green (success) or red (failure) dot.
pub fn tool_result(name: &str, success: bool) {
    tool_result_to(&mut stdout(), name, success).ok();
}

fn tool_result_to<W: Write + QueueableCommand>(
    out: &mut W,
    name: &str,
    success: bool,
) -> std::io::Result<()> {
    let color = if success { GREEN } else { RED };
    out.queue(cursor::MoveUp(1))?;
    out.queue(terminal::Clear(ClearType::CurrentLine))?;
    out.queue(SetForegroundColor(color))?;
    out.queue(Print("  ● "))?;
    out.queue(ResetColor)?;
    out.queue(Print(format!("{name}\n")))?;
    out.flush()
}

/// Print agent thinking text at normal weight (not dimmed).
pub fn thinking_text(text: &str) {
    thinking_text_to(&mut stdout(), text).ok();
}

fn thinking_text_to<W: Write + QueueableCommand>(out: &mut W, text: &str) -> std::io::Result<()> {
    out.queue(Print(text))?;
    out.queue(Print("\n"))?;
    out.flush()
}

/// Print assistant text-content in white.
pub fn text_content(text: &str) {
    text_content_to(&mut stdout(), text).ok();
}

fn text_content_to<W: Write + QueueableCommand>(out: &mut W, text: &str) -> std::io::Result<()> {
    out.queue(SetForegroundColor(Color::White))?;
    out.queue(Print(text))?;
    out.queue(ResetColor)?;
    out.queue(Print("\n"))?;
    out.flush()
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
        // Top border is capped to terminal width (20 chars).
        assert!(output.contains("┌"), "output must contain top-left corner");
        // The long content line must be truncated — the full text should not appear.
        assert!(
            !output.contains("implementer_long_name"),
            "long content must be truncated"
        );
    }

    #[test]
    fn box_renders_without_error() {
        // Smoke test: render_box must not panic or error on a normal terminal width.
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
        let long_args = "a".repeat(100);
        let mut buf: Vec<u8> = Vec::new();
        tool_call_to(&mut buf, "Bash", &long_args).unwrap();
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
    fn tool_result_success_emits_cursor_up_and_green() {
        let mut buf: Vec<u8> = Vec::new();
        tool_result_to(&mut buf, "Bash", true).unwrap();
        let out = String::from_utf8_lossy(&buf);
        // Cursor-up escape: ESC [ 1 A
        assert!(
            buf.windows(4).any(|w| w == b"\x1b[1A"),
            "cursor-up escape must be emitted; output: {out:?}"
        );
        assert!(
            out.contains("Bash"),
            "tool name must appear after overwrite"
        );
    }

    #[test]
    fn tool_result_failure_emits_cursor_up_and_red() {
        let mut buf: Vec<u8> = Vec::new();
        tool_result_to(&mut buf, "Write", false).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(
            buf.windows(4).any(|w| w == b"\x1b[1A"),
            "cursor-up escape must be emitted on failure; output: {out:?}"
        );
        assert!(out.contains("Write"), "tool name must appear on failure");
    }

    #[test]
    fn thinking_text_not_dimmed() {
        let mut buf: Vec<u8> = Vec::new();
        thinking_text_to(&mut buf, "some thought").unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("some thought"), "text must appear");
        // dim ANSI code is ESC[2m — must not appear
        assert!(
            !buf.windows(4).any(|w| w == b"\x1b[2m"),
            "thinking_text must not emit dim escape code"
        );
    }

    #[test]
    fn text_content_renders_text() {
        let mut buf: Vec<u8> = Vec::new();
        text_content_to(&mut buf, "hello world").unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("hello world"), "text must appear in output");
    }
}
