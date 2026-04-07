#!/bin/bash
set -o pipefail

if [ -z "$1" ]; then
	echo "Usage: $0 <iterations>"
	exit 1
fi

IMAGE_NAME="ralph"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
RALPH_PROMPT="${SCRIPT_DIR}/prompt.md"

# ── Pre-flight checks ─────────────────────────────────────────────────────────
if ! gh repo view --json name &>/dev/null; then
	echo "❌  Not inside a GitHub repository (or 'gh' can't detect the remote)."
	exit 1
fi

# Export both names — some tools only read GITHUB_TOKEN, others only GH_TOKEN.
GH_TOKEN="${GH_TOKEN:-$(gh auth token 2>/dev/null || echo "")}"
if [ -z "$GH_TOKEN" ]; then
	echo "❌  No GitHub token found. Run 'gh auth login' on the host first."
	exit 1
fi
export GH_TOKEN
export GITHUB_TOKEN="${GH_TOKEN}"

if [ "$(id -u)" -ne 1000 ]; then
	echo "⚠️  Your UID is $(id -u), but the container user is UID 1000."
	echo "   File permissions on the bind-mounted workspace may cause errors."
	echo "   Re-build the image with a matching UID, or press Enter to continue anyway."
	read -r
fi

# Warn if the working tree is dirty — Claude will commit on top of this state.
if [ -n "$(git status --porcelain 2>/dev/null)" ]; then
	echo "⚠️  You have uncommitted changes. Claude will commit on top of them."
	echo "   Consider stashing first. Press Enter to continue anyway."
	read -r
fi

# ── Build image once ──────────────────────────────────────────────────────────
# Run with FORCE_REBUILD=1 to pull a fresh Arch snapshot and update all packages.
if [[ "${FORCE_REBUILD:-0}" == "1" ]] || ! docker image inspect "${IMAGE_NAME}" &>/dev/null; then
	echo "🔨 Building ${IMAGE_NAME} image…"
	docker build -t "${IMAGE_NAME}" - <<'DOCKERFILE'
FROM archlinux:base

# ca-certificates : TLS
# git             : commits, log, diff
# curl            : claude installer
# ripgrep         : claude code search
# nodejs, npm     : repo scripts and tooling installed via .ralph/Dockerfile
# github-cli      : issue/PR management inside the container
RUN pacman -Syu --noconfirm && \
    pacman -S --noconfirm --needed \
        git curl ca-certificates ripgrep \
        nodejs npm \
        github-cli && \
    pacman -Scc --noconfirm

# core.pager="" prevents git from opening less/more in the non-interactive container.
RUN git config --system core.pager ""

# Create a non-root user.  UID 1000 matches the typical single-user Arch host,
# so bind-mounted files are owned by the same UID.
RUN useradd -mu 1000 -s /bin/bash claude

USER claude
ENV HOME=/home/claude
ENV NPM_CONFIG_PREFIX=/home/claude/.npm-global
ENV PATH="/home/claude/.npm-global/bin:${PATH}"

RUN curl -fsSL https://claude.ai/install.sh | bash

WORKDIR /workspace

ENV PATH="/home/claude/.local/bin:/home/claude/.npm-global/bin:${PATH}"
ENV CI=true

# Entrypoint: run optional .ralph/setup.sh, then launch Claude Code.
RUN printf '#!/bin/bash\nset -e\nif [ -x /home/claude/setup.sh ]; then\n  echo "── Running setup.sh ──────────────────────────────────────────"\n  /home/claude/setup.sh\n  echo "── Setup complete ────────────────────────────────────────────"\nfi\ncat /home/claude/prompt.txt | claude --dangerously-skip-permissions --model claude-sonnet-4-6 -p --verbose --output-format stream-json\n' > /home/claude/entrypoint.sh && chmod +x /home/claude/entrypoint.sh
ENTRYPOINT ["/home/claude/entrypoint.sh"]
DOCKERFILE
	echo "✅ Image ready."
fi

# ── Per-repo image extension ─────────────────────────────────────────────────
# If the repo has .ralph/Dockerfile, build a derived image with repo-specific
# tooling (e.g. pnpm, python, postgres-client).
RUN_IMAGE="${IMAGE_NAME}"
if [ -f "$(pwd)/.ralph/Dockerfile" ]; then
	REPO_IMAGE="${IMAGE_NAME}-$(basename "$(pwd)")"
	if [[ "${FORCE_REBUILD:-0}" == "1" ]] || ! docker image inspect "${REPO_IMAGE}" &>/dev/null; then
		echo "🔨 Building repo image ${REPO_IMAGE}…"
		docker build -t "${REPO_IMAGE}" -f "$(pwd)/.ralph/Dockerfile" "$(pwd)/.ralph"
	fi
	RUN_IMAGE="${REPO_IMAGE}"
fi

# ── jq helpers ────────────────────────────────────────────────────────────────
# Display filter: plain white for assistant text, dim gray for tool calls.
# Read/Glob/Agent are filtered as noise. Non-JSON lines pass through as-is.
stream_display='
fromjson? // . |
if type == "string" then .
elif .type == "assistant" then
  .message.content[]? |
  if .type == "text" then
    "\u001b[0;37m" + .text + "\u001b[0m"
  elif .type == "tool_use" then
    if .name == "Read" or .name == "Glob" or .name == "Agent" then
      empty
    else
      "\u001b[2;37m  " + .name +
      (if .name == "Bash" then " → " + (.input.command // "" | tostring)
       elif .name == "Write" then " → " + (.input.file_path // .input.path // "" | tostring)
       elif .name == "Edit" then " → " + (.input.file_path // .input.path // "" | tostring)
       elif (.input | length) > 0 then " → " + (.input | keys[0] // "" | tostring) + ": " + (.input[.input | keys[0]] // "" | tostring | .[0:80])
       else ""
       end) + "\u001b[0m"
    end
  else empty
  end
else empty
end
'

final_result='select(.type == "result").result // empty'

# ── Temp files — declared once, cleaned up on any exit ───────────────────────
tmpfile=$(mktemp)
prompt_file=$(mktemp)
trap "rm -f ${tmpfile} ${prompt_file}" EXIT

# ── Main loop ─────────────────────────────────────────────────────────────────
for ((i = 1; i <= $1; i++)); do
	echo ""
	echo "── Iteration $i / $1 ──────────────────────────────────────────"

	commits=$(git log -n 5 --format="%H%n%ad%n%B---" --date=short 2>/dev/null || echo "No commits found")

	{
		echo "Previous commits:"
		echo "$commits"
		echo ""
		echo "Open issues (JSON):"
		gh issue list --state open --json number,title,body,comments,labels
		echo ""
		cat "$RALPH_PROMPT"
	} >"$prompt_file"

	# No -u flag — overriding the user breaks $HOME resolution inside the container.
	# Attach to the compose network if one exists so Ralph can reach services like postgres.
	COMPOSE_NETWORK=$(docker network ls --filter "name=_default" --format '{{.Name}}' | head -1)
	NETWORK_FLAG=""
	if [ -n "$COMPOSE_NETWORK" ]; then
		NETWORK_FLAG="--network ${COMPOSE_NETWORK}"
	fi

	# .ralph/.env can override values for the container context (e.g. service hostnames
	# instead of localhost).
	ENV_FLAG=""
	if [ -f "$(pwd)/.env" ]; then
		ENV_FLAG="--env-file $(pwd)/.env"
	fi
	if [ -f "$(pwd)/.ralph/.env" ]; then
		ENV_FLAG="${ENV_FLAG} --env-file $(pwd)/.ralph/.env"
	fi

	# Mount optional setup script.
	SETUP_FLAG=""
	if [ -f "$(pwd)/.ralph/setup.sh" ]; then
		SETUP_FLAG="-v $(pwd)/.ralph/setup.sh:/home/claude/setup.sh:ro"
	fi

	docker run --rm ${NETWORK_FLAG} ${ENV_FLAG} ${SETUP_FLAG} \
		-v "$(pwd):/workspace" \
		-v "${prompt_file}:/home/claude/prompt.txt:ro" \
		-v "${HOME}/.claude/.credentials.json:/home/claude/.claude/.credentials.json" \
		-v "${HOME}/.claude/settings.json:/home/claude/.claude/settings.json:ro" \
		-v "${HOME}/.claude/hooks:/home/claude/.claude/hooks:ro" \
		-v "${HOME}/.claude/plugins:/home/claude/.claude/plugins:ro" \
		-v "${HOME}/.claude/skills:/home/claude/.claude/skills:ro" \
		-e GITHUB_TOKEN="${GITHUB_TOKEN}" \
		-e GH_TOKEN="${GH_TOKEN}" \
		-e GIT_AUTHOR_NAME="$(git config user.name)" \
		-e GIT_AUTHOR_EMAIL="$(git config user.email)" \
		-e GIT_COMMITTER_NAME="$(git config user.name)" \
		-e GIT_COMMITTER_EMAIL="$(git config user.email)" \
		"${RUN_IMAGE}" |
		tee "$tmpfile" |
		jq --unbuffered -Rr "$stream_display" 2>/dev/null
	docker_exit=${PIPESTATUS[0]}

	if grep -q '"error":"authentication_failed"' "$tmpfile" 2>/dev/null; then
		echo ""
		echo "❌  Claude authentication failed. Run 'claude' on the host to refresh credentials."
		exit 1
	fi

	if [ "$docker_exit" -ne 0 ]; then
		echo ""
		echo "❌  Container exited with code ${docker_exit}. Check stderr above for details."
		exit 1
	fi

	result=$(jq -Rr "fromjson? | $final_result" "$tmpfile")

	if [[ "$result" == *"<promise>NO MORE TASKS</promise>"* ]]; then
		echo ""
		echo "✅ Ralph complete after $i iterations."
		exit 0
	fi
done

echo ""
echo "⏹  Completed all $1 iterations."
