# PROJECT

Describe your project here in 2-3 sentences. What it does, what language/framework it uses, and any key architectural constraints.

Follow all conventions in CLAUDE.md.

# ISSUES

GitHub issues are provided at start of context.

You will work on the issues that have "AFK" label only.

When all AFK tasks are complete, call `submit_verdict(status="pass", notes="Queue drained.")`.

# CONSTRAINTS

- Work on a single task per iteration. Do not start a second task.
- Never commit directly to the main branch.
- Never close or comment on issues you are not working on.
- Never delete branches.
- Never run release workflows.
- Never commit code that doesn't pass tests.
- When adding new dependencies, justify the choice in the commit message.
- Prefer small, focused changes. If the issue requires extensive work, implement the smallest shippable piece and note the remaining work in an issue comment.
- **When stuck**: If you cannot resolve a problem after two attempts, stop. Do not commit broken code. Leave a comment on the GitHub issue with: why you couldn't complete the task, what you tried, and what remains. Push any salvageable work and end the iteration.
- **Issue comments** when incomplete: Lead with *why* the task couldn't be completed, then what was implemented, and what remains.

# TASK SELECTION

Pick the next task. Prioritize tasks in this order:

1. Critical bugfixes
2. Development infrastructure
   Getting development infrastructure like tests and types and dev scripts ready is an important precursor to building features.

3. Tracer bullets for new features
   Build a tiny, end-to-end slice of the feature first, then expand it out.

4. Polish and quick wins
5. Refactors

Once you pick a task, announce the task title and working branch.

# BRANCH

Read the "Working branch" field from the issue you are working on. Check out that branch and pull the latest changes before starting work.

# IMPLEMENTATION

Read the files relevant to the issue before starting implementation.

Use /tdd for implementation and bug fixes. For documentation, config changes, or refactors already covered by existing tests, implement directly without /tdd.

# FEEDBACK LOOPS

Before committing, run the feedback loops:

- Format code (e.g. `cargo fmt`, `prettier`, `ruff format`)
- Lint (e.g. `cargo clippy -- -D warnings`, `eslint`, `ruff check`)
- Run the test suite (e.g. `cargo test`, `npm test`, `pytest`)

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
