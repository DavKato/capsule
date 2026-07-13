---
name: audit-comments
description: Audits source files for low-value comments and emits a findings list, keeping only those that explain *why*. Use to identify comment noise without applying changes.
phase: discovery
---

# audit-comments

Surveys source files in a repo and emits a findings list of low-value comments. Does not modify anything — that's a downstream decision.

## What to flag

- **restate**: comments that restate what the code does (`// increment i` above `i++`)
- **dead-code**: commented-out source lines
- **todo**: TODO / FIXME / HACK / XXX notes
- **boilerplate**: auto-generated comment scaffolding
- **private-doc**: doc comments (`///`, `/** */`, `"""`) on private/internal items
- **module-obvious**: module-level / file-level doc comments that only describe what the module name and exports already make obvious

## What to keep (do not flag)

- Comments explaining *why*: business logic, non-obvious constraints, tradeoffs, workarounds for external bugs
- Safety/correctness warnings not captured by the type system (`// SAFETY:`, ordering constraints, caller contracts)
- Doc comments on *public* API items (functions, types, modules exposed to callers)
- Module-level comments that explain something non-derivable from the module name, its exports, and its location in the tree

## Workflow

### 1 — Determine scope

If the user specified a scope (file, directory, glob, or file type), use it directly. Otherwise:

1. Count source files in the repo (exclude `target/`, `node_modules/`, `.git/`, build artifacts).
2. If **< 50 source files**: scan all source files automatically.
3. If **≥ 50 source files**: ask the user which files, directories, or file types to target.

### 2 — Detect comment syntax per file

- `//` and `/* */` — Rust, Go, JS/TS, Java, C/C++
- `#` — Python, Ruby, Shell, TOML, YAML
- `"""` / `'''` — Python docstrings
- `--` — SQL, Lua
- `<!-- -->` — HTML, XML

Skip non-source files (lock files, generated files, binaries, `.md` docs).

### 3 — Emit findings

Print a numbered list. For each finding:

```
N. [category] file:line — short label
   Quote: "<exact comment text>"
   Recommendation: remove | partial-remove (state what to keep)
```

End with a one-line summary: total findings, breakdown by category.

Do not modify any file.

## Edge cases

- **Mixed blocks**: a comment block mixing a why-comment with a restatement gets a `partial-remove` recommendation stating which part to keep.
- **Ambiguous**: if it's unclear whether a comment qualifies as a why-comment, do not flag it. False negatives (leaving noise) are better than false positives (deleting intent).
