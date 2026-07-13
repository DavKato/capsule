## Problem Statement

The architectural friction or developer pain, from the developer's perspective. Which modules are involved, what risk or coupling exists, why this makes the codebase harder to navigate or maintain.

## Solution

The proposed change, from the developer's perspective. If there's a chosen interface, sketch its signature, a usage example, and what complexity it hides.

## Plan

A detailed implementation plan in plain English. Prefer breaking it into the tiniest commits possible — Martin Fowler: *"make each refactoring step as small as possible, so that you can always see the program working."* Each commit should leave the codebase in a working state.

For dependency-heavy refactors, also note the strategy:
- **In-process** — merged directly
- **Local-substitutable** — tested with [specific stand-in]
- **Ports & adapters** — port definition, production adapter, test adapter
- **Mock** — mock boundary for external services

## Decision Document

Implementation decisions made:

- Modules built / modified
- Interfaces modified
- Architectural decisions
- Schema changes, API contracts, specific interactions

Do NOT include file paths or code snippets — they go stale.

## Testing Decisions

- What good tests look like here (external behavior, not implementation details)
- Which modules will be tested
- New boundary tests to write; old tests to delete
- Prior art for similar tests in the codebase

## Out of Scope

What is explicitly not part of this refactor.

## Further Notes (optional)

Anything else worth recording.
