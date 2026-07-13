#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SPLIT="$SCRIPT_DIR/split-diff.sh"
failures=0

fail() { echo "FAIL: $1"; ((failures++)); }
pass() { echo "PASS: $1"; }

# --- Fixture: a unified diff with 3 files across 2 directories ---
DIFF='diff --git a/src/api/handler.go b/src/api/handler.go
index abc1234..def5678 100644
--- a/src/api/handler.go
+++ b/src/api/handler.go
@@ -10,3 +10,5 @@ func Handle() {
+    log.Println("new line")
+    return nil
 }
diff --git a/src/worker/process.go b/src/worker/process.go
index 111aaaa..222bbbb 100644
--- a/src/worker/process.go
+++ b/src/worker/process.go
@@ -5,2 +5,3 @@ func Process() {
+    doWork()
 }
diff --git a/src/api/routes.go b/src/api/routes.go
index ccc3333..ddd4444 100644
--- a/src/api/routes.go
+++ b/src/api/routes.go
@@ -1,2 +1,3 @@ func Routes() {
+    r.Handle("/new", handler)
 }'

# --- Test 1: basic two-group split ---
paths=$(printf '%s\n' "$DIFF" | "$SPLIT" - \
  "api:src/api/handler.go,src/api/routes.go" \
  "worker:src/worker/process.go")

api_path=$(echo "$paths" | sed -n '1p')
worker_path=$(echo "$paths" | sed -n '2p')

if [[ $(echo "$paths" | wc -l) -eq 2 ]]; then
  pass "two groups produce two paths"
else
  fail "expected 2 paths, got $(echo "$paths" | wc -l)"
fi

if grep -q "handler.go" "$api_path" && grep -q "routes.go" "$api_path"; then
  pass "api group contains both api files"
else
  fail "api group missing expected files"
fi

if grep -q "process.go" "$worker_path" && ! grep -q "handler.go" "$worker_path"; then
  pass "worker group contains only worker files"
else
  fail "worker group has wrong content"
fi

# --- Test 2: file not in any group is silently dropped ---
paths=$(printf '%s\n' "$DIFF" | "$SPLIT" - \
  "api:src/api/handler.go")

if [[ $(echo "$paths" | wc -l) -eq 1 ]]; then
  pass "unmapped files are silently dropped"
else
  fail "expected 1 path for partial mapping"
fi

if ! grep -q "process.go" "$(echo "$paths" | head -1)"; then
  pass "dropped file not in output"
else
  fail "dropped file leaked into output"
fi

# --- Test 3: group with no matching files produces no output path ---
paths=$(printf '%s\n' "$DIFF" | "$SPLIT" - \
  "api:src/api/handler.go" \
  "empty:src/nonexistent/file.go")

if [[ $(echo "$paths" | wc -l) -eq 1 ]]; then
  pass "group with no matching files produces no path"
else
  fail "expected 1 path, got $(echo "$paths" | wc -l)"
fi

# --- Test 4: diff headers are preserved correctly ---
api_path=$(printf '%s\n' "$DIFF" | "$SPLIT" - "api:src/api/handler.go" | head -1)
first_line=$(head -1 "$api_path")

if [[ "$first_line" == "diff --git a/src/api/handler.go b/src/api/handler.go" ]]; then
  pass "diff header preserved exactly"
else
  fail "diff header mangled: $first_line"
fi

# --- Test 5: no groups argument exits with error ---
if printf '%s\n' "$DIFF" | "$SPLIT" - 2>/dev/null; then
  fail "should exit non-zero with no groups"
else
  pass "exits non-zero with no groups"
fi

# --- Test 6: rename diff (a/ and b/ paths differ) uses b/ path ---
RENAME_DIFF='diff --git a/src/old/name.go b/src/new/name.go
similarity index 90%
rename from src/old/name.go
rename to src/new/name.go
--- a/src/old/name.go
+++ b/src/new/name.go
@@ -1,3 +1,3 @@
-old content
+new content'

paths=$(printf '%s\n' "$RENAME_DIFF" | "$SPLIT" - "moved:src/new/name.go")

if [[ $(echo "$paths" | wc -l) -eq 1 ]] && grep -q "new content" "$(echo "$paths" | head -1)"; then
  pass "rename diff matched by b/ (destination) path"
else
  fail "rename diff not matched by destination path"
fi

# --- Summary ---
echo ""
if [[ $failures -eq 0 ]]; then
  echo "All tests passed."
else
  echo "$failures test(s) failed."
  exit 1
fi
