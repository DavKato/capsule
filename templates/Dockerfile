FROM archlinux:base

# ca-certificates : TLS
# git             : commits, log, diff
# curl            : claude installer
# ripgrep         : claude code search
# nodejs, npm     : repo scripts and tooling installed via .capsule/Dockerfile
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

# Entrypoint: configure git credentials from GH_TOKEN and git identity from
# GIT_AUTHOR_* env vars, run optional before-each.sh, then launch Claude Code.
RUN printf '#!/bin/bash\nset -e\nif [ -n "${GH_TOKEN}" ]; then\n  git config --global credential.helper store\n  echo "https://oauth2:${GH_TOKEN}@github.com" > "${HOME}/.git-credentials"\n  chmod 600 "${HOME}/.git-credentials"\nfi\n_name="${GIT_AUTHOR_NAME:-Capsule}"\n_email="${GIT_AUTHOR_EMAIL:-capsule@localhost}"\ngit config --global user.name "${_name}"\ngit config --global user.email "${_email}"\nif [ -x /home/claude/before-each.sh ]; then\n  echo "── Running before-each.sh ────────────────────────────────────"\n  /home/claude/before-each.sh\n  echo "── before-each.sh complete ───────────────────────────────────"\nfi\ncat /home/claude/prompt.txt | claude --dangerously-skip-permissions --model claude-sonnet-4-6 -p --verbose --output-format stream-json\n' > /home/claude/entrypoint.sh && chmod +x /home/claude/entrypoint.sh
ENTRYPOINT ["/home/claude/entrypoint.sh"]
