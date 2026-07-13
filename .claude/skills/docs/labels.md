# Labels

Canonical label set for GitHub issues. Source of truth for any workflow that touches issue labels.

Every issue gets at most one **category** label and exactly one **state** label, except:
- `prd` issues get only the "prd" label (no state label).
- `research` issues get only the "research" label (no state label).

## Category

| Label         | Color     | Description                                                              |
| ------------- | --------- | ------------------------------------------------------------------------ |
| `bug`         | `d73a4a`  | A user-facing behavior is wrong.                                         |
| `enhancement` | `a2eeef`  | New feature or non-bug UX/UI/quality improvement.                        |
| `refactor`    | `c5def5`  | Internal code change without behavioral effect.                          |
| `prd`         | `5319e7`  | Product requirements doc — parent for a chain of slices.                 |
| `feedback`    | `c5def5`  | Session feedback for workflow improvements.                              |
| `research`    | `b088f5`  | Open-ended investigation or spike — no implementation expected.          |

Slices (vertical implementation units under a `prd` parent) carry no category label — their relationship to the parent is captured by the GitHub sub-issues API, not a label.

## State

| Label              | Color     | Description                                                                  |
| ------------------ | --------- | ---------------------------------------------------------------------------- |
| `needs-triage`     | `fbca04`  | Maintainer needs to evaluate.                                                |
| `needs-info`       | `f9d0c4`  | Waiting on reporter for more information.                                    |
| `ready-for-agent`  | `0e8a16`  | Fully specified, ready for an AFK agent to pick up.                          |
| `ready-for-human`  | `1d76db`  | Fully specified, requires human implementation.                              |
| `wontfix`          | `cccccc`  | Will not be actioned. Closed.                                                |

## State transitions

Every issue carries exactly one state label. If multiple state labels are present, flag it and ask the maintainer before proceeding.

```
(unlabeled) → needs-triage → ready-for-agent
                            → ready-for-human
                            → needs-info → needs-triage (on reporter reply)
                            → wontfix (close)
```

The maintainer can override any transition. Flag unusual jumps (e.g. unlabeled straight to `ready-for-agent`) and ask before proceeding.

Assume labels exist. If a `gh` command fails because a label is missing, ask the user to initialize the project's labels.
