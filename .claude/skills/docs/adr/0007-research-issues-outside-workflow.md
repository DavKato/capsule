# Research issues live outside the workflow graph

Research issues (`research` label) persist pre-PRD thinking that doesn't crystallize in a single session. They use the issue tracker for structure and searchability but opt out of the workflow machinery entirely: no state labels, no sweep, no agent pickup, no working branch. When research crystallizes into a PRD, a fresh PRD issue is filed (linking back to the research issue) and the research issue is closed.

## Considered options

- **GitHub Discussions.** Would separate research from actionable work cleanly, but Discussions live outside the issues API — no structured templates, no label filtering alongside issues, no sub-issues integration if research ever needs to become trackable.
- **Research as workflow-integrated issues (with state labels, sweep, agent pickup).** Rejected: research is HITL by nature — it feeds into the PRD creation step which is a human decision. Routing it through `sweep-issues` and `ready-for-agent` would force-fit a lifecycle designed for implementable work onto exploratory thinking.
- **Research as issues, outside the workflow (chosen).** Gets the benefits of the issue tracker (templates, labels, search, cross-referencing) without polluting the workflow graph. The `research` label is the opt-out signal.

## Consequences

- `sweep-issues` must skip issues with the `research` label — they have no state labels to triage.
- `create-issue` with the `research` template applies only the `research` category label, no state label.
- A research issue is never a parent or sub-issue. It has no children and no working branch.
- The research template's Conclusion section records the outcome: "became PRD #N" or "not pursued, reason: X."
