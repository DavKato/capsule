#!/bin/bash
set -e
if [ -n "${GH_TOKEN}" ]; then
  git config --global credential.helper store
  echo "https://oauth2:${GH_TOKEN}@github.com" > "${HOME}/.git-credentials"
  chmod 600 "${HOME}/.git-credentials"
fi
_name="${GIT_AUTHOR_NAME:-Capsule}"
_email="${GIT_AUTHOR_EMAIL:-capsule@localhost}"
git config --global user.name "${_name}"
git config --global user.email "${_email}"
if [ -f /home/claude/before-each.sh ]; then
  bash /home/claude/before-each.sh
fi
if [ -n "${CAPSULE_RESUME_SESSION}" ]; then
  claude --dangerously-skip-permissions --model "${CAPSULE_MODEL:-claude-sonnet-4-6}" --resume "${CAPSULE_RESUME_SESSION}" -p "Continue where you left off." --verbose --output-format stream-json --mcp-config /home/claude/.mcp.json
else
  cat /home/claude/prompt.txt | claude --dangerously-skip-permissions --model "${CAPSULE_MODEL:-claude-sonnet-4-6}" -p --verbose --output-format stream-json --mcp-config /home/claude/.mcp.json
fi
