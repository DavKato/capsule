use crossterm::{
    style::{Color, Print, ResetColor, SetForegroundColor},
    terminal, QueueableCommand,
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
}
