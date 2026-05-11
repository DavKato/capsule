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

    // --- agent text ---
    pause(400);
    display::agent_text("I'll start by reading the project structure…");
    display::agent_text("The codebase uses a pipeline model with stages.");
    pause(200);

    // --- tool calls (overlapping) ---
    section("Tool calls");
    display::tool_call("Read", "src/main.rs", "tc-1");
    pause(500);
    display::tool_call("Bash", "cargo test --lib", "tc-2");
    pause(800);
    display::tool_result("tc-1", true);
    pause(400);
    display::tool_call("Edit", "src/display.rs  old_string=…", "tc-3");
    pause(600);
    display::tool_result("tc-2", false);
    pause(400);
    display::tool_result("tc-3", true);

    // --- more agent text ---
    pause(300);
    display::agent_text("The test failure was expected — fixing now.");

    // --- token warning ---
    section("Token warning");
    display::set_token_warning(Some("token lifetime < 10 min — consider refreshing"));
    pause(1500);
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
    );

    // --- stage header with retry ---
    section("Stage header — retry 2/3");
    display::stage_header(
        "reviewer",
        2,
        "claude-opus-4-6",
        Some(&RetryInfo {
            current: 2,
            max: Some(3),
        }),
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
    );

    // --- stage header with unlimited retry ---
    section("Stage header — unlimited retry");
    display::stage_header(
        "implementer",
        3,
        "claude-sonnet-4-6",
        Some(&RetryInfo {
            current: 1,
            max: None,
        }),
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
    );

    // --- session footer: implicit fail (no verdict) ---
    section("Session footer — implicit fail (no verdict)");
    display::session_footer("crasher", 1, None, Duration::from_secs(5), None);

    pause(500);
    display::teardown();
    eprintln!("\n\x1b[1;32mDone.\x1b[0m");
}
