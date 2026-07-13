# Sub-issues for grouping

GitHub sub-issues are the grouping primitive for related work. The **Working branch** is declared via a comment on the **Parent issue** (not in the body); **Sub-issues** inherit it via parent lookup. The branch name is decided at filing time but the branch itself is created at pickup time from the declared base (see ADR-0004). Parent attachment is **eager** (declared at file-time when known via `create-an-issue`'s `parent:` parameter) with an **orphan default** (filed as self-parented when no parent is known) — orphans are adopted later by the `group-issues` integration skill.

We chose this over a `working-branch:` field smeared across every issue body because the parent-child relationship is a hard graph edge in GitHub (queryable via the sub-issues API) and a single source of truth on the parent eliminates drift when grouping decisions change. We chose eager-with-orphan-default over a late-writer planner skill because eager removes a class of bookkeeping bugs (forgotten branch assignments) and the orphan path is small enough to handle as routine housekeeping in `sweep-issues` rather than a separate planning step.

## Considered options

- **`working-branch:` field on every issue body, smeared at file-time.** Rejected: drift when grouping changes, no native graph in GitHub, every filing path has to remember to set it.
- **Late writer + planner skill.** Issues file with no grouping; a `plan-branch` skill assigns groups later. Rejected: bookkeeping debt, branch ambiguity at pickup time, redundant once sub-issues exist as a primitive.
- **Eager-at-file with recommendation in `create-an-issue`.** Filer is prompted "is this related to #42?" at every filing. Rejected: smuggles editorial logic into a primitive, intrusive for genuinely standalone filings, new failure modes if the recommendation is wrong.
- **Sub-issues + eager + orphan default + `group-issues` for adoption (chosen).** Single source of truth, native GitHub graph, primitive stays primitive, editorial work has a dedicated home.

## Consequences

- The **Drain signal** (zero open sub-issues under a parent) becomes one `gh api` call — clean trigger for any downstream skill that should run only after all child work is complete.
- Mid-loop findings (filed by `review-pr` during step 3) automatically join the parent's working branch by being filed as sub-issues — no late writer needed.
- `create-an-issue` gains a `parent:` parameter and uses the sub-issues API (`POST /repos/{owner}/{repo}/issues/{parent}/sub_issues`). The API takes the sub-issue's internal database id, not its issue number — this gotcha is documented in `sub-issues.md`.
- Orphans (filed by `file-improvement` or standalone `/create-an-issue`) accumulate until adopted; `sweep-issues` surfaces them daily for `group-issues` to handle.
