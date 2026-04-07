# Extend the base ralph image with repo-specific tooling.
# This is the right place to install language runtimes, package managers,
# database clients, or any other tools your project's setup.sh or tests need.
#
# The base image already provides: git, curl, ripgrep, nodejs, npm, github-cli,
# and Claude Code.

FROM ralph

# Example: install pnpm globally (for Node/pnpm monorepos)
RUN npm install -g pnpm && npm cache clean --force
