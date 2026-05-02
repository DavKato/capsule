use crate::verdict::VerdictStatus;

/// Formats a previous-stage block for injection into a stage prompt.
/// Returns `None` when `notes` is `None` or empty.
pub fn format(stage_name: &str, status: &VerdictStatus, notes: Option<&str>) -> Option<String> {
    let notes = notes.filter(|n| !n.is_empty())?;
    let status_str = match status {
        VerdictStatus::Pass => "pass",
        VerdictStatus::Fail => "fail",
        VerdictStatus::Done => "done",
    };
    Some(format!(
        "<previous-stage>\nStage: {stage_name}\nStatus: {status_str}\nNotes: {notes}\n</previous-stage>"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_with_notes_returns_block() {
        let out = format("review", &VerdictStatus::Pass, Some("all checks passed"));
        let block = out.unwrap();
        assert!(block.contains("<previous-stage>"));
        assert!(block.contains("Stage: review"));
        assert!(block.contains("Status: pass"));
        assert!(block.contains("Notes: all checks passed"));
        assert!(block.contains("</previous-stage>"));
    }

    #[test]
    fn format_without_notes_returns_none() {
        assert!(format("review", &VerdictStatus::Pass, None).is_none());
    }

    #[test]
    fn format_with_empty_notes_returns_none() {
        assert!(format("review", &VerdictStatus::Fail, Some("")).is_none());
    }

    #[test]
    fn format_done_status_serializes_correctly() {
        let out = format("impl", &VerdictStatus::Done, Some("loop exited")).unwrap();
        assert!(out.contains("Status: done"));
    }

    #[test]
    fn format_fail_status_serializes_correctly() {
        let out = format("test", &VerdictStatus::Fail, Some("tests broke")).unwrap();
        assert!(out.contains("Status: fail"));
    }
}
