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

# Git wrapper: re-forces capsule's configured identity on every git call,
# preventing agents from overriding commit_as with arbitrary values.
COPY --chmod=755 git-wrapper.sh /usr/local/bin/git

# Create a non-root user.  UID 1000 is for build-time installation only;
# runtime uid comes from --user and may differ from 1000.
RUN useradd -mu 1000 -s /bin/bash claude

USER claude
ENV HOME=/home/claude
ENV NPM_CONFIG_PREFIX=/home/claude/.npm-global
ENV PATH="/home/claude/.npm-global/bin:${PATH}"

RUN curl -fsSL https://claude.ai/install.sh | bash

# Make home dir world-accessible so the container can run as any uid via --user.
RUN chmod -R o+rwX /home/claude

ENV PATH="/home/claude/.local/bin:/home/claude/.npm-global/bin:${PATH}"
ENV CI=true

# Entrypoint: configure git credentials from GH_TOKEN and git identity from
# GIT_AUTHOR_* env vars, run optional CAPSULE_STAGE_SETUP command, then launch Claude Code.
COPY --chmod=755 entrypoint.sh /home/claude/entrypoint.sh
ENTRYPOINT ["/home/claude/entrypoint.sh"]
