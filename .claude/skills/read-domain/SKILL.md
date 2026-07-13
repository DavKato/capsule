---
name: read-domain
description: Load the project's domain glossary (DOMAIN.md, DOMAIN-MAP.md) into conversation context. Use when a skill or workflow needs domain awareness before proceeding.
phase: discovery
model: claude-haiku-4-5
---

# read-domain

Load the project's domain glossary into conversation context so subsequent work uses the right language and respects recorded decisions.

## Step 0 — Check for prior load

If domain terms from DOMAIN.md already appear in recent conversation context (e.g. from an earlier skill invocation), skip the load and proceed with what's available.

## Step 1 — Load the glossary

Detect the structure:
- If `DOMAIN-MAP.md` exists at the repo root, read it to find all contexts. Then read each context's `DOMAIN.md`.
- If only a root `DOMAIN.md` exists, read it. Single context.
- If neither exists, proceed silently. Don't flag the absence or suggest creating one.

## Step 2 — Surface available ADRs (don't read them yet)

List the filenames in `docs/adr/` (and context-scoped `src/<context>/docs/adr/` in multi-context repos). Don't read the contents.

Use the filenames to identify which ADRs are relevant to the current task. Read only those. If none are relevant, skip entirely.

## Format reference

The expected structure of DOMAIN.md is defined in [domain-format.md](../docs/domain-format.md). Use it to validate what you're loading.

## Use the glossary's vocabulary

When your output names a domain concept, use the term as defined in `DOMAIN.md`. Don't drift to synonyms the glossary explicitly avoids.

## Flag ADR conflicts

If your output contradicts an existing ADR, surface it explicitly rather than silently overriding:

> _Contradicts ADR-0007 (event-sourced orders) — but worth reopening because…_
