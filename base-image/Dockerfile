FROM ubuntu:24.04

# Ubuntu (not Arch) because it publishes native amd64 *and* arm64 images, so the
# base builds on Apple Silicon as well as x86 Linux/CI. glibc also keeps the
# `claude.ai/install.sh` native binary working (musl/Alpine would not). Ubuntu's
# default repos also carry every tool below — including gh — so install is a
# single apt-get with no third-party apt repositories, and user-supplied
# `.capsule/Dockerfile` snippets match the Ubuntu install docs most vendors ship.
#
# ca-certificates : TLS
# git             : commits, log, diff
# curl            : claude installer
# ripgrep         : claude code search
# nodejs, npm     : repo scripts and tooling installed via .capsule/Dockerfile
# gh              : issue/PR management inside the container
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        ca-certificates git curl ripgrep \
        nodejs npm \
        gh && \
    apt-get clean && \
    rm -rf /var/lib/apt/lists/*

# core.pager="" prevents git from opening less/more in the non-interactive container.
RUN git config --system core.pager ""

# Git wrapper: re-forces capsule's configured identity on every git call,
# preventing agents from overriding commit_as with arbitrary values.
COPY --chmod=755 git-wrapper.sh /usr/local/bin/git

# Create a non-root user.  UID 1000 is for build-time installation only;
# runtime uid comes from --user and may differ from 1000. Keeping it at 1000
# (the common Linux host UID) means a uid-1000 host resolves via /etc/passwd to
# this prepared, world-writable /home/claude rather than an unprepared home.
# Ubuntu 24.04 ships a stock `ubuntu` user at UID 1000 — remove it so the id is
# free for `claude` (`|| true` keeps this portable to bases without that user).
RUN userdel -r ubuntu 2>/dev/null || true && \
    useradd -mu 1000 -s /bin/bash claude

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
