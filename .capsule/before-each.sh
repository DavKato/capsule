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

tmpfile=$(mktemp)

{
	echo "Previous commits:"
	git -C /workspace log -n 5 --format="%h%n%ad%n%B---" --date=short 2>/dev/null ||
		echo "No commits found"
	echo ""

	echo "Open AFK issues (JSON):"
	gh issue list --state open --label AFK --json number,title,body,comments,labels
	echo ""

	cat /home/claude/prompt.txt
} >"$tmpfile"

mv "$tmpfile" /home/claude/prompt.txt
