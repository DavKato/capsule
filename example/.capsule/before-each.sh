#!/bin/bash
# Runs inside the container before Claude Code starts each iteration.
#
# Use this to inject dynamic context into the prompt or to install dependencies.
#
# - Receives the container environment (from --env-file and -e flags)
# - Can modify /home/claude/prompt.txt before Claude reads it
# - Exit non-zero to abort that iteration with an error
#
# The workspace is at /workspace. git, gh, ripgrep, and Claude Code are available.

set -euo pipefail

# Example: prepend recent commits and open GitHub issues to the prompt.

tmpfile=$(mktemp)

{
    echo "Previous commits:"
    git -C /workspace log -n 5 --format="%H%n%ad%n%B---" --date=short 2>/dev/null \
        || echo "No commits found"
    echo ""

    echo "Open issues (JSON):"
    gh issue list --state open --json number,title,body,comments,labels 2>/dev/null \
        || echo "[]"
    echo ""

    cat /home/claude/prompt.txt
} > "$tmpfile"

mv "$tmpfile" /home/claude/prompt.txt
