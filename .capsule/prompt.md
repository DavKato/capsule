# PROJECT

Capsule is a Rust CLI tool that runs AI agents (Claude Code) inside isolated Docker containers, looping autonomously through GitHub issues. You are working on capsule's own codebase.

Follow all conventions in CLAUDE.md.

# ISSUES

GitHub issues are provided at start of context.

You will work on the issues that have "AFK" label only.

If all AFK tasks are complete, output <promise>AFK_COMPLETE</promise>.

# CONSTRAINTS

- Work on a single task per iteration. Do not start a second task.
- Never commit directly to the main branch.
- Never close or comment on issues you are not working on.
- Never delete branches.
- Never run release workflows (e.g., `cargo release`, creating GitHub releases).
- Never commit code that doesn't pass `cargo test`.
- When adding new dependencies, justify the choice in the commit message.
- Prefer small, focused changes. If the issue requires extensive work, implement the smallest shippable piece and note the remaining work in an issue comment.
- **When stuck**: If you cannot get tests passing or resolve a problem after two attempts, stop. Do not commit broken code. Leave a comment on the GitHub issue with: why you couldn't complete the task, what you tried, and what remains. Push any salvageable work and end the iteration.
- **Issue comments** when incomplete: Lead with _why_ the task couldn't be completed, then what was implemented, and what remains.

# TASK SELECTION

Pick the next task. Prioritize tasks in this order:

1. Critical bugfixes
2. Development infrastructure
   Getting development infrastructure like tests and types and dev scripts ready is an important precursor to building features.

3. Tracer bullets for new features
   Tracer bullets are small slices of functionality that go through all layers of the system, allowing you to test and validate your approach early. This helps in identifying potential issues and ensures that the overall architecture is sound before investing significant time in development.
   TL;DR - build a tiny, end-to-end slice of the feature first, then expand it out.

4. Polish and quick wins
5. Refactors

Once you pick a task, announce the task title and working branch.

# BRANCH

Read the "Working branch" field from the issue you are working on. Check out that branch and pull the latest changes before starting work.

# IMPLEMENTATION

Read the files relevant to the issue before starting implementation.

Use /tdd whenever you write non-trivial logic that isn't already exercised by existing tests — including features, bug fixes, and any new code paths introduced during a refactor. For documentation, config changes, or pure refactors where no new logic is introduced, implement directly without /tdd.

If you add non-trivial logic without a test, explain why in the commit message.

Before adding a test outside of /tdd, state explicitly what behavior it protects and how it would fail if that behavior broke. If you can't answer that, don't add the test.

# COMMIT

Make a git commit. The commit message must:

1. Include what was done
2. Include key decisions made
3. Blockers or notes for next iteration

# ISSUE HANDLING

Push the branch.
If there is no PR from this branch to the base branch, create one using `gh pr create`. Make sure to associate the PRD issue number in the body.

If the task is complete, close the issue.
If the task is not complete, leave a comment on the GitHub issue describing: why it couldn't be completed, what was done, and what remains.
