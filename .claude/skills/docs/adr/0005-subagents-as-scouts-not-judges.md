# Subagents as scouts, not judges

In `review-diff`, subagents were producing structured findings (severity, recommended fix) from their scoped slice of a PR diff. A subagent scoped to deleted test files made a false "missing test coverage" finding because it lacked cross-scope context — the absorption target belonged to another subagent that correctly found no gap (issue #4). We restructured so subagents report observations (tagged with checklist checkpoints) and open questions, while the orchestrator synthesizes all reports into the final findings list.

## Considered options

- **Subagents produce findings, orchestrator merges (status quo).** Each subagent evaluates checkpoints and returns severity-rated findings. Orchestrator deduplicates. Rejected: subagents make judgment calls without cross-scope context, producing false findings when the answer to their question lives in another subagent's scope.
- **Subagents produce findings, orchestrator verifies.** Keep subagent findings but add a verification pass. Rejected: verification requires the orchestrator to re-investigate each finding — duplicating the subagent's work while still paying for the subagent's initial (potentially wrong) conclusion.
- **Subagents report observations, orchestrator synthesizes (chosen).** Subagents are scouts: they explore and report what they see, tagged with the relevant checkpoint, plus open questions for things they couldn't determine. The orchestrator has all observations and cross-references them to produce findings. Judgment happens where full context is available.

## Consequences

- Step 4 (orchestrator synthesis) becomes heavier — it produces every finding, not just merges.
- Review scouts are a custom agent type (`.claude/agents/review-scout.md`) restricted to `Bash, Read` — enforcing "scout, not actor" at the tool level. Their behavioral contract (observations + open questions, scope boundary, checkpoint tagging) lives in the agent definition, not in per-invocation prompts.
- Test files are excluded from scope assignment — they're unscoped reference material any scout can read. This prevents deleted tests from creating unanswerable questions in a scoped subagent.
- Diff splitting is handled by a standalone shell script (`review-diff/scripts/split-diff.sh`), not an LLM agent. The orchestrator passes the PR number and group-to-files mapping as positional args; the script fetches the diff, splits it, writes per-group temp files, and prints the paths. This replaced both the broken `gh pr diff -- <files>` scoping and a short-lived filter subagent that spent ~17K tokens on a purely mechanical string operation (issue #7).
- Scouts must not read implementation files outside their assigned scope.
