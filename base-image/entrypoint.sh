#!/bin/bash
set -e
# Docker's --user doesn't set HOME; pin it so all tools find the right home dir.
export HOME=/home/claude
if [ -n "${GH_TOKEN}" ]; then
  git config --global credential.helper store
  echo "https://oauth2:${GH_TOKEN}@github.com" > "${HOME}/.git-credentials"
  chmod 600 "${HOME}/.git-credentials"
fi
_name="${GIT_AUTHOR_NAME:-Capsule}"
_email="${GIT_AUTHOR_EMAIL:-capsule@localhost}"
cat > /tmp/.capsule-git-identity <<IDENTITY
export GIT_AUTHOR_NAME="${_name}"
export GIT_AUTHOR_EMAIL="${_email}"
export GIT_COMMITTER_NAME="${_name}"
export GIT_COMMITTER_EMAIL="${_email}"
IDENTITY
# Export directly for processes that bypass the git wrapper
export GIT_AUTHOR_NAME="${_name}"
export GIT_AUTHOR_EMAIL="${_email}"
export GIT_COMMITTER_NAME="${_name}"
export GIT_COMMITTER_EMAIL="${_email}"
if [ -n "${CAPSULE_STAGE_SETUP}" ]; then
  if [ -f "${CAPSULE_STAGE_SETUP}" ]; then
    bash "${CAPSULE_STAGE_SETUP}"
  else
    bash -c "${CAPSULE_STAGE_SETUP}"
  fi
fi
MODEL_FLAG=""
if [ -n "${CAPSULE_MODEL}" ]; then
  MODEL_FLAG="--model ${CAPSULE_MODEL}"
fi
if [ -n "${CAPSULE_RESUME_SESSION}" ]; then
  claude --dangerously-skip-permissions ${MODEL_FLAG} --resume "${CAPSULE_RESUME_SESSION}" -p "Continue where you left off." --verbose --output-format stream-json --mcp-config /home/claude/.mcp.json
else
  cat /home/claude/prompt.txt | claude --dangerously-skip-permissions ${MODEL_FLAG} -p --verbose --output-format stream-json --mcp-config /home/claude/.mcp.json
fi
