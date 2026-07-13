# Three-tier skill visibility

Many skills were authored for a personal project workflow. After a job change, they clutter the model's skill list and trigger false-positive auto-invocations during daily work.

We split skills into three visibility tiers:

1. **Disabled** (`disable-model-invocation: true`) — project-specific skills not used in any autonomous pipeline. The model cannot invoke them at all. Trade-off: due to claude-code#50075 (still open as of 2026-05-29), interactive `/skill` invocation is also blocked. Acceptable because these skills aren't needed day-to-day.

2. **Silent** (no `when_to_use` frontmatter) — skills used by the capsule autonomous pipeline (directly or as composed dependencies). They remain callable by pipeline agents via `/skill-name` in their prompts, but the model won't auto-trigger them from conversational context.

3. **Active** (full frontmatter) — universally useful skills that should auto-trigger when context matches: review-diff, tdd, diagnose, socrates, design-principles, etc.

## Considered Options

- **Disable everything not needed interactively.** Rejected: breaks the capsule pipeline, which invokes skills like file-slices → create-issue → draft-slices via the Skill tool.
- **Remove `when_to_use` from everything.** Rejected: genuinely useful skills (review-diff, diagnose, tdd) would stop firing when appropriate, forcing explicit invocation every time.
- **Move unused skills out of the directory.** Rejected: harder to maintain, no git tracking of the decision, and the pipeline expects them at known paths.
