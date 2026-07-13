# PR Review Checklist

Apply these checkpoints in order. Security is highest priority — flag and stop elaborating lower-priority items if a Critical security issue is found.

---

## 1. Security (Critical priority)

- **Injection** — any user-controlled input passed to shell commands, SQL, template engines, or `eval`-like constructs without sanitisation?
- **Authentication / authorisation bypass** — does new code skip `protectedProcedure`, omit `householdId` filtering, or expose endpoints without auth?
- **Secrets in source** — hardcoded API keys, tokens, passwords, or connection strings? Even in tests.
- **Insecure defaults** — CORS wildcard `*`, `allowAllOrigins`, debug flags left on, overly permissive IAM policies beyond what the change requires?
- **Dependency risk** — new third-party packages added? Check for known vulnerabilities or unnecessarily broad permissions.
- **OWASP Top 10** — XSS, CSRF, SSRF, path traversal, broken object-level authorisation.

---

## 2. Anti-patterns

- **God objects / fat controllers** — business logic leaking into routers, components, or lambdas instead of services?
- **Service layer bypass** — routers or UI code querying the database directly (violates CLAUDE.md rule)?
- **Prop drilling / unnecessary coupling** — data passed through many layers when a better boundary exists?
- **Premature abstraction** — a helper or utility created for a single call site?
- **Magic values** — unexplained numeric literals, hardcoded strings that belong in config or constants?
- **Mutable shared state** — module-level variables mutated across requests?

---

## 3. Best practices

- **Single responsibility** — does each function / class / module do one thing?
- **Error handling at boundaries** — are errors caught at system entry points (API handlers, lambda handler)? Are internal errors allowed to propagate as unhandled rejections?
- **Type safety** — are there any `any`, unsafe casts, or `!` non-null assertions that could be avoided?
- **Idempotency** — mutations (DB writes, API calls) safe to retry?
- **Logging** — are errors and significant events logged with enough context to diagnose?

---

## 4. Lint / test failures

- Run (or check CI) for lint errors **and warnings** — no warnings are allowed.
- Check that the existing test suite still passes against the diff (look for test files changed or tests deleted).
- New code that removes or skips existing tests must be flagged.

---

## 5. Test quality and coverage

Load `design-principles` before evaluating this section.

- **Behaviour coverage** — are new behaviours tested through public interfaces, not implementation details?
- **No implementation-coupled tests** — tests should survive an internal refactor. Mocks of internal collaborators are a warning sign.
- **Missing edge cases** — error paths, empty inputs, concurrent mutations.
- **Test organisation** — are tests co-located with the code they test? Do test names describe behaviours ("user cannot checkout with empty cart") rather than functions ("test handleCheckout")?
- **No untested happy paths** — the main success path for every new feature must have at least one test.

---

## 6. Clean implementation (ease of change)

The best codebase is one where future changes are small and localised.

- **Tight coupling** — if this feature changes tomorrow, how many files need to touch? Can that number be reduced?
- **Deep modules** — does the public interface hide complexity, or does it expose internal structure?
- **Duplication** — copy-pasted logic that will diverge silently over time?
- **Hard-coded assumptions** — values or structure that will require a widespread change if requirements shift?
- **Encapsulation leaks** — does the caller need to know too much about the callee's internals to use it correctly?

---

## 7. Verbose comments

Comments should only exist where the code **cannot** speak for itself.

Flag:
- Comments that restate what the code does (`// increment counter` above `counter++`)
- Commented-out code left in the diff
- TODO/FIXME comments without a linked issue or owner
- JSDoc on trivial getters/setters

Keep:
- Comments explaining **why** a non-obvious decision was made
- Comments referencing external specs, RFCs, or known quirks
- Warnings about non-obvious side effects
