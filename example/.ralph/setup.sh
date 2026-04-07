#!/bin/bash
# Runs inside the container before Claude Code starts.
# Use this for project-specific setup: installing dependencies, running
# migrations, seeding test data, etc.
#
# - stdout is shown to the user (wrapped in "Running setup.sh" / "Setup complete" banners)
# - The script runs with set -e in the entrypoint, so any non-zero exit aborts the run
# - CI=true is set in the environment, so tools that check for a TTY behave correctly

# Example: install dependencies in a pnpm monorepo.
pnpm config set store-dir /home/claude/.pnpm-store
pnpm install --frozen-lockfile || pnpm install
