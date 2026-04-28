#!/bin/bash
# Runs inside the container before Claude Code starts each stage invocation.
#
# In a multi-stage pipeline this fires before every stage — implementer,
# reviewer, pr-reviewer, and documentor alike. Inject context that is useful
# across all stages: recent commits (everyone needs to know what changed) and
# open AFK issues (implementer picks work; pr-reviewer checks what remains).
#
# - Receives the container environment (from --env-file and -e flags)
# - Can modify /home/claude/prompt.txt before Claude reads it
# - Exit non-zero to abort that stage invocation with an error
#
# The workspace path is available as $CAPSULE_WORKSPACE. git, gh, ripgrep, and Claude Code are available.

set -euo pipefail

tmpfile=$(mktemp)

{
    echo "Recent commits:"
    git -C "$CAPSULE_WORKSPACE" log -n 10 --format="%h%n%ad%n%B---" --date=short 2>/dev/null \
        || echo "No commits found"
    echo ""

    echo "Open AFK issues (JSON):"
    gh issue list --state open --label AFK --json number,title,body,comments,labels
    echo ""

    cat /home/claude/prompt.txt
} > "$tmpfile"

mv "$tmpfile" /home/claude/prompt.txt
