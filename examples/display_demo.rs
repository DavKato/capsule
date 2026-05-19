use std::thread;
use std::time::Duration;

use capsule::display;
use capsule::pipeline::RetryInfo;
use capsule::verdict::{Verdict, VerdictStatus};

fn pause(ms: u64) {
    thread::sleep(Duration::from_millis(ms));
}

fn section(label: &str) {
    eprintln!("\n\x1b[1;35m── {label} ──\x1b[0m\n");
    pause(600);
}

fn main() {
    display::init();

    // --- capsule_info / warning / info ---
    section("Messages");
    display::capsule_info("building image capsule-demo:latest …");
    pause(300);
    display::warning("token expires in 12 minutes");
    pause(300);
    display::info("container started");

    // --- update output: already up to date ---
    section("Update — already up to date");
    display::dim_info("Current version: 2.1.137");
    pause(300);
    display::dim_info("Checking for updates...");
    pause(600);
    display::info("Already up to date (2.1.137)");

    // --- update output: new version available ---
    section("Update — new version");
    display::dim_info("Current version: 2.1.137");
    pause(300);
    display::dim_info("Checking for updates...");
    pause(600);
    display::dim_info("Updating 2.1.137 → 2.1.138...");
    pause(800);
    display::info("Successfully updated to 2.1.138");

    // --- notice_box ---
    section("Notice box");
    display::notice_box(&[
        "capsule v0.8.0".into(),
        "New: scroll-region display architecture".into(),
        "Run `capsule update` to upgrade".into(),
    ]);

    // --- stage header (no retry) ---
    section("Stage header — first run");
    display::stage_header("implementer", 1, "claude-sonnet-4-6", None);

    // --- set_stage starts the live timer in the panel ---
    display::set_stage("implementer", 1, "claude-sonnet-4-6");

    // --- agent text: short lines ---
    pause(400);
    display::agent_text("I'll start by reading the project structure…");
    display::agent_text("The codebase uses a pipeline model with stages.");
    pause(200);

    // --- agent text: long line that wraps ---
    section("Agent text — wrapping");
    display::agent_text("This is a much longer line of text that should demonstrate the word-wrapping behavior. When the terminal is narrower than the content, the display layer breaks it at word boundaries and indents continuation lines so everything stays aligned under the bullet point.");
    pause(400);

    // --- agent text: preserves newlines (e.g. code / structured output) ---
    section("Agent text — newlines preserved");
    display::agent_text("Here's what I found:\n\nsrc/main.rs  — entry point\nsrc/display.rs — terminal rendering\nsrc/pipeline.rs — stage orchestration");
    pause(400);

    // --- tool call states ---
    section("Tool call states");
    display::tool_call("Read", "src/main.rs", "s-1", None);
    display::tool_call("Read", "src/lib.rs", "s-2", None);
    display::tool_result("s-1", true);
    display::tool_result("s-2", true);
    display::tool_call("Bash", "cargo test", "s-3", None);
    display::tool_result("s-3", false);

    // --- tool calls (overlapping) ---
    section("Tool calls — overlapping");
    display::tool_call("Read", "src/main.rs", "tc-1", None);
    pause(500);
    display::tool_call("Bash", "cargo test --lib", "tc-2", None);
    pause(800);
    display::tool_result("tc-1", true);
    pause(400);
    display::tool_call("Edit", "src/display.rs  old_string=…", "tc-3", None);
    pause(600);
    display::tool_result("tc-2", false);
    pause(400);
    display::tool_result("tc-3", true);

    // --- agent text after tools ---
    pause(300);
    display::agent_text("The test failure was expected — fixing now.");

    // --- usage counter in panel ---
    section("Usage counter");
    display::set_usage(12_400);
    pause(800);
    display::set_usage(48_700);
    pause(800);
    display::set_usage(128_000);
    pause(800);

    // --- token warning ---
    section("Token warning");
    display::set_token_warning(Some("token lifetime < 10 min — consider refreshing"));
    pause(1500);
    display::set_token_warning(None);

    // --- status bar standalone ---
    section("Status bar — styled segments");
    display::set_stage("chatter", 1, "claude-haiku-4-5-20251001");
    pause(1500);
    display::set_usage(2_100);
    pause(1500);
    display::set_usage(18_400);
    pause(1500);
    display::set_token_warning(Some("context > 80% — approaching limit"));
    pause(2000);
    display::set_token_warning(None);

    // --- session footer: pass ---
    section("Session footer — pass");
    display::clear_stage();
    display::session_footer(
        "implementer",
        1,
        Some(&Verdict {
            status: VerdictStatus::Pass,
            notes: Some("all tests green".into()),
        }),
        Duration::from_secs(247),
        Some("sess_abc123def456"),
        Some("33.3k (16.6%) used"),
    );

    // --- stage header with retry ---
    section("Stage header — retry 2/3");
    display::stage_header(
        "reviewer",
        2,
        "claude-opus-4-6",
        Some(&RetryInfo { current: 2, max: 3 }),
    );
    pause(800);

    // --- session footer: fail ---
    section("Session footer — fail");
    display::clear_stage();
    display::session_footer(
        "reviewer",
        2,
        Some(&Verdict {
            status: VerdictStatus::Fail,
            notes: Some("found 2 issues in error handling".into()),
        }),
        Duration::from_secs(83),
        Some("sess_xyz789"),
        Some("12.1k (6.1%) used"),
    );

    // --- stage header with retry ---
    section("Stage header — retry");
    display::stage_header(
        "implementer",
        3,
        "claude-sonnet-4-6",
        Some(&RetryInfo { current: 1, max: 5 }),
    );
    pause(800);

    // --- session footer: done ---
    section("Session footer — done (no notes)");
    display::clear_stage();
    display::session_footer(
        "implementer",
        3,
        Some(&Verdict {
            status: VerdictStatus::Done,
            notes: None,
        }),
        Duration::from_secs(412),
        None,
        None,
    );

    // --- session footer: implicit fail (no verdict) ---
    section("Session footer — implicit fail (no verdict)");
    display::session_footer("crasher", 1, None, Duration::from_secs(5), None, None);

    pause(500);
    display::teardown();
    eprintln!("\n\x1b[1;32mDone.\x1b[0m");
}
