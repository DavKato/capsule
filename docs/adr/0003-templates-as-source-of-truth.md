# Templates as the source of truth for user-facing scaffolds

Capsule ships a small set of pre-built `.capsule/` skeletons (`single-iter`,
`ralph-loop`, etc.) under `templates/` in the repo. `capsule init --template
<name>` copies the chosen template byte-for-byte into the user's repo — no
substitution, no scaffolding logic, no template engine. Templates replace the
former `examples/` directory: a single artifact serves as both the documented
reference shape and the thing users actually install. CI runs `capsule check`
against every template, so what users see in the repo and what `init` produces
are provably valid and never drift.

## Considered options

**Examples + separate hardcoded init skeleton** — keep `examples/` for reading
and a hardcoded minimal skeleton baked into the `init` command. Rejected: two
sources of truth, guaranteed drift, and the hardcoded skeleton is harder to
inspect than a directory of files.

**Parameterized templates with substitution** — let templates carry placeholders
(project name, language, model) substituted at init time. Rejected: every
real customization happens *after* init when the agent edits prompts to fit
the project, so pre-substitution saves nothing while introducing a template
engine to design, document, and debug.

**Many narrow templates over few broad ones** (chosen direction) — when a
distinction matters (e.g., `ralph-loop-rust` vs `ralph-loop-node`), ship two
templates rather than one parameterized one. Forces clarity over cleverness
and keeps each template trivially CI-validatable.
