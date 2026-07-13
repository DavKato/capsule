# Depth is relative to the observer

A module's depth is judged from the caller's perspective, not globally. A deep module can — and should — have internal structure: sub-modules, extracted files, named concepts. These internal extractions don't make the parent module shallow, as long as they stay behind the parent's external interface.

This resolves the apparent contradiction between "keep modules deep" (from `design-principles`) and "split large files for readability." A 1600-line file can be split into internal pieces without shallowing the parent module, provided the pieces are internal organization — not new entries in the parent's external interface. The split is valid when each piece has a nameable role, a narrow coupling surface back to the rest, and no leakage into the parent's exports.

## Considered options

- **Never split deep modules.** Strict reading of "deep = keep everything together." Rejected: conflates file boundaries with module boundaries. A module is not a file. Leaving everything inlined creates formless walls of code that impair both human readability and agent navigability (agents can fail to read files above ~1500 lines in a single pass).
- **Split freely for readability.** Extract whenever a file feels large. Rejected: naive splitting creates shallow modules with wide coupling surfaces. Every new file boundary is a new seam, and seams that hide nothing are just indirection.
- **Split only when internal structure already exists (chosen).** The extracted piece must have a nameable role, a clean internal interface, and no leakage into the parent's external interface. This treats file splitting as organizing a module's internals, not as creating new modules at the parent's level.
