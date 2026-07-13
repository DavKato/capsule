# PRD promotion creates a new issue

When triage enrichment produces multiple slices, a new PRD issue is created and the original issue is closed as superseded — rather than mutating the original issue body in place.

The alternative (updating the original with `draft-prd` output) is simpler — one fewer issue, no close/link ceremony — but it destroys the raw motivation that led to the PRD. Keeping the original intact means the PRD can reference it, and future readers can trace back to what triggered the work.
