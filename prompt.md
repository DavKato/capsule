# ISSUES

GitHub issues are provided at start of context. Parse it to get open issues with their bodies and comments.

You will work on the AFK issues only, not the HITL ones.

If all AFK tasks are complete, output <promise>NO MORE TASKS</promise>.

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

In case "working branch" is specified in the issue, make sure you are in the latest working branch.

# EXPLORATION

Explore the repo.

# IMPLEMENTATION

Invoke /tdd skill and complete the task.

# FEEDBACK LOOPS

Before committing, run the feedback loops:

- `pnpm test` to run the tests
- `pnpm lint:fix` to run the linter

# COMMIT

Make a git commit. The commit message must:

1. Include key decisions made
2. Include files changed
3. Blockers or notes for next iteration
4. If the task is complete, tag the issue with "Closes" keyword to automatically close the issue.

# THE ISSUE HANDLING

Push the branch.
If there is no PR from this branch to the base branch, create one using `gh pr create`. Make sure to associate the PRD issue number in the body.

If the task is not complete, leave a comment on the GitHub issue with what was done.

# FINAL RULES

ONLY WORK ON A SINGLE TASK.
