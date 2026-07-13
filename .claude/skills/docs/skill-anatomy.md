# Skill anatomy

The shape of a skill in this repo. Read this when authoring a new skill or refactoring an existing one. The phase contract lives in [phases.md](./phases.md); this file covers everything else.

## Naming

Skill directory and `name:` use imperative verbs without articles. `refactor-skill`, not `refactor-a-skill`. `create-issue`, not `create-an-issue`.

## Style

Terse. Don't restate what's already in conversation context, linked docs, or elsewhere in the skill.

## Skill references

When a workflow step should **invoke** a composed skill, use `/skill-name` syntax (e.g. `Invoke /read-domain`) — this triggers the Skill tool. For non-invocation mentions, use plain backticks (`` `read-domain` ``).

## File references

Use `${CLAUDE_SKILL_DIR}` for file references within skills. The harness resolves it to an absolute path at load time. Same-directory: `${CLAUDE_SKILL_DIR}/CHECKLIST.md`. Cross-directory: `${CLAUDE_SKILL_DIR}/../docs/phases.md`. Never use relative markdown links (`[text](../path)`) — agents can't resolve them reliably.

## File structure

```
skill-name/
├── SKILL.md           # Main instructions (required)
├── REFERENCE.md       # Spillover for content past the line cap
├── EXAMPLES.md        # Usage examples (if needed)
└── scripts/           # Utility scripts (if needed)
```

`SKILL.md` is the only required file. Everything else is on-demand.

## Frontmatter

Required fields: `name`, `description`, `phase`. Everything else is optional.

| Field                      | Purpose                                                                                                                                                     |
| -------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `name`                     | Slash command name. Lowercase + hyphens, max 64 chars.                                                                                                      |
| `description`              | What + when, in 200 chars. The model uses this to decide whether to load the skill.                                                                         |
| `phase`                    | One of `discovery`, `distillation`, `persist`, `execute`, `integration`, `meta`. See [phases.md](./phases.md). Skills synced from upstream omit this field. |
| `when_to_use`              | Extra triggers/example requests. Appended to `description` in the listing.                                                                                  |
| `argument-hint`            | Autocomplete hint, e.g. `[issue-number]`.                                                                                                                   |
| `arguments`                | Named positional args for `$name` substitution. Space-separated string or YAML list.                                                                        |
| `disable-model-invocation` | `true` = manual invoke only; description not loaded into context.                                                                                           |
| `user-invocable`           | `false` = Claude-only, hidden from `/` menu.                                                                                                                |
| `allowed-tools`            | Tools pre-approved while the skill is active.                                                                                                               |
| `model` / `effort`         | Override model or effort level for this skill's turn.                                                                                                       |
| `context: fork` + `agent`  | Run skill in a forked subagent (e.g. `Explore`, `Plan`).                                                                                                    |
| `hooks`                    | Skill-scoped lifecycle hooks.                                                                                                                               |
| `paths`                    | Glob patterns gating auto-activation.                                                                                                                       |

## Description rules

`description` cap: 200 chars. Format:

- First sentence: what it does.
- Second sentence: `Use when [canonical trigger]` — only for model-routed skills. User-invoked skills can omit it.
- Third person.

Trigger phrase variants and example requests go in **`when_to_use`**, not `description`. The model sees both at routing time (they're concatenated in the skill listing), so splitting them keeps `description` scannable while `when_to_use` carries the noise. User-invoked skills should skip `when_to_use` entirely.

**`disable-model-invocation: true` skills** are shown only in the `/` menu. Skip `when_to_use`; keep a brief `Use when ...` in the description only if the human needs the hint.

Good:

```yaml
description: Extract text and tables from PDF files, fill forms, merge documents. Use when working with PDFs.
when_to_use: User mentions PDFs, forms, tables, or document extraction; asks to "pull data from a PDF" or "fill a form".
```

Bad: `Helps with documents.` — gives the model nothing to discriminate on.

## String substitutions

Available inside SKILL.md content:

| Variable               | Description                                           |
| ---------------------- | ----------------------------------------------------- |
| `$ARGUMENTS`           | Full argument string.                                 |
| `$ARGUMENTS[N]` / `$N` | Argument by 0-based index.                            |
| `$name`                | Named argument from the `arguments` frontmatter list. |
| `${CLAUDE_SESSION_ID}` | Current session ID.                                   |
| `${CLAUDE_EFFORT}`     | Active effort level.                                  |
| `${CLAUDE_SKILL_DIR}`  | Directory containing this skill's SKILL.md.           |

Inline shell injection: `` !`<cmd>` `` runs before the skill is sent to the model and inlines the output. Multi-line: open a fenced block with ` ```! `.

## Integration skill workflow

Integration skills compose other skills. Name the primitives composed and the order. If a confirmation step is needed, encode it as a workflow step.

Pure `persist` and `execute` skills are the inverse: their SKILL.md states what targets they accept and what change they produce, with no discovery or recommendation logic.

Skill body template:

```md
# integration-name

<One-line summary of the end-to-end path.>

## Workflow

1. **<Step label>.** <What happens, including early-exit or reuse conditions.>

2. **<Step label>.** Run /Y. <One-line description of the side effect.>

3. **<Final output>.** <What the user sees at the end.>

<Optional notes: consent model, when to skip a step, etc.>
```

## Line cap

SKILL.md cap: 250 lines. Past that, split:

- Detailed reference → `REFERENCE.md`
- Long examples → `EXAMPLES.md`
- Distinct domains (e.g. finance vs. sales schemas) → separate files

Reference the supporting files from SKILL.md so the model knows what each contains.

## Scripts

Add a script under `scripts/` when:

- Operation is deterministic (validation, formatting).
- Same code would be regenerated each invocation.
- Errors need explicit handling the model shouldn't reinvent.

Scripts save tokens and improve reliability vs. generated code.

## Review checklist

- [ ] `name`, `description`, `phase` declared
- [ ] Description ≤200 chars
- [ ] `phase` matches archetype (input + motion + output; see [phases.md](./phases.md))
- [ ] Integration skill body opens with a one-line summary
- [ ] Skill references use plain backticks, not markdown links
- [ ] SKILL.md ≤250 lines; spillover in REFERENCE.md
- [ ] No time-sensitive info
- [ ] Consistent terminology (see [DOMAIN.md](../DOMAIN.md))
- [ ] Concrete examples included
- [ ] References one level deep

```

```
