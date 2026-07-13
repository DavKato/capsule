# Phase contract

Every skill in this repo declares a `phase:` in its frontmatter. The phase is the archetype the skill fits — what kind of input it takes, what kind of work it does, and what it leaves behind. This file is the source of truth; see also [DOMAIN.md](../DOMAIN.md) for the full domain language and [adr/0001-pure-and-integration-skills.md](./adr/0001-pure-and-integration-skills.md) for the decision behind it.

## The six phases

- **`discovery`** — gathers information and builds context. Takes no required input source; may be given a scope hint, but goes hunting beyond it. Output is what it found, in conversation.
- **`distillation`** — takes a defined input artifact (conversation context, prior skill output, a file) and refines it into a target shape. Pure transformation, no hunting.
- **`persist`** — takes a finished artifact and publishes it to a known location. The location can be external (GitHub, an API) or local (a file in the repo, an ADR, an entry in MIGRATION.md).
- **`execute`** — takes a target in project code and applies changes to it. The target is supplied by the caller.
- **`integration`** — composes other skills. Impure by construction.
- **`meta`** — self-referential: skills about the skills repo, or skills that modify the agent's own behavior for subsequent turns.

## Pure skill rule

A pure skill declares one of `discovery`, `distillation`, `persist`, `execute`, or `meta` and stays within that archetype. The test differs by phase:

- **Discovery / distillation**: produces context, no side effects.
- **Persist / execute**: the *target of the side effect* must come from the input — caller-supplied, user-specified, or read from an artifact passed in. If the skill discovers what to mutate as part of its job, it has smuggled discovery into an action skill and is impure. Such a skill belongs in `integration`, composing a discovery primitive with an action primitive.

Shape test: *if the user had to pre-specify every concrete target before running, would the skill still be useful?* If yes, pure execute/persist. If the skill's value comes from finding the targets, it's integration.

## Integration skills

An integration skill composes other skills. It can chain discovery → action, run multiple primitives in sequence, or wrap an autonomous loop. Impure by construction — composition is its purpose.

Integration skills are the natural home for "find + act" workflows where the find step has non-trivial logic. The user invokes one well-named integration skill; internally it composes a pure discovery primitive with a pure action primitive (and optionally an `auto-apply` meta skill that suppresses confirmation prompts).

## Frontmatter contract

Required field. Must be one of the six values above.

```yaml
---
name: skill-name
description: ...
phase: distillation
---
```

Externally managed skills omit the `phase:` field entirely. The absent field is the marker — these skills are exempt because we don't own their conventions and `sync-upstream-skills` would clobber local edits.

## Decision rubric

Ask: **what does this skill take as input, and what does it do?**

- Goes hunting for context with at most a scope hint → `discovery`.
- Takes a defined artifact and refines it → `distillation`.
- Takes a finished artifact and publishes it somewhere → `persist`.
- Takes a target in project code and changes it → `execute`.
- Composes other skills → `integration`.
- Is about this repo or modifies agent behavior → `meta`.
