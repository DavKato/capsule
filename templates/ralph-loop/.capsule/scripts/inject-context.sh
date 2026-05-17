#!/bin/bash
set -euo pipefail

tmpfile=$(mktemp)

{
    echo "Recent commits:"
    git -C "$CAPSULE_WORKSPACE" log -n 10 --format="%h %ad %s" --date=short 2>/dev/null \
        || echo "No commits found"
    echo ""

    echo "Open issues (JSON):"
    gh issue list --state open --json number,title,body,labels
    echo ""

    cat /home/claude/prompt.txt
} > "$tmpfile"

mv "$tmpfile" /home/claude/prompt.txt
