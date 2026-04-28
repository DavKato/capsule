#!/bin/bash
# Runs once on the host before the first container starts.
#
# Use this for pre-flight checks that must pass before any iteration runs.
# Exit non-zero to abort the entire capsule run.
#
# - Receives the full host environment, including variables from .capsule/.env
# - Runs as the host user, with access to host tools (docker, gh, etc.)
# - stdout and stderr are shown to the user

set -euo pipefail

# Example: verify a PostgreSQL container is up before Claude tries to use it.
# Replace "postgres" with your actual container name.
#
# if ! docker exec postgres pg_isready -q 2>/dev/null; then
#     echo "❌ PostgreSQL is not running. Start it first:" >&2
#     echo "   docker compose up -d postgres" >&2
#     exit 1
# fi
# echo "✅ PostgreSQL is ready."

echo "✅ before-all.sh: pre-flight checks passed."
