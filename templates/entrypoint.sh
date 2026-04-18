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
if [ -x /home/claude/before-each.sh ]; then
  echo "── Running before-each.sh ────────────────────────────────────"
  /home/claude/before-each.sh
  echo "── before-each.sh complete ───────────────────────────────────"
fi
cat /home/claude/prompt.txt | claude --dangerously-skip-permissions --model "${CAPSULE_MODEL:-claude-sonnet-4-6}" -p --verbose --output-format stream-json
