# Drop flat-form config

Flat-form config (`iterations:` + `prompt:` without `stages:`) was the original single-stage shorthand, desugared internally into a one-stage loop. With multi-stage pipelines as the primary model, flat-form added a second config grammar to parse, validate, and explain — for a convenience that a one-stage `stages:` block already covers. We're removing it entirely: configs without a `stages:` key produce a hard error with migration guidance. The user base is small enough that a deprecation period isn't warranted.

## Considered Options

- **Deprecation with warning** — keep flat-form working but emit a warning for N releases. Rejected: maintains two code paths for a small user base, and the migration is trivial (wrap in `stages:`).
- **Auto-desugar silently** — detect flat-form and convert internally forever. Rejected: keeps the flat-form parsing code alive indefinitely and hides the migration from users.
