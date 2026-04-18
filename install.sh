#!/usr/bin/env bash
set -euo pipefail

REPO="DavKato/capsule"
BIN_DIR="${HOME}/.local/bin"
BIN="${BIN_DIR}/capsule"
SHELL_NAME="$(basename "${SHELL:-}")"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
BOLD='\033[1m'
RESET='\033[0m'

log()  { printf "${CYAN}[capsule]${RESET} %s\n" "$*"; }
ok()   { printf "${GREEN}[capsule]${RESET} %s\n" "$*"; }
die()  { printf "${RED}[capsule] error:${RESET} %s\n" "$*" >&2; exit 1; }

# ── 1. Detect platform ──────────────────────────────────────────────────────

OS="$(uname -s)"
ARCH="$(uname -m)"

case "${OS}" in
  Linux)
    case "${ARCH}" in
      x86_64)  TRIPLE="x86_64-unknown-linux-gnu" ;;
      aarch64) TRIPLE="aarch64-unknown-linux-gnu" ;;
      *) die "Unsupported Linux architecture: ${ARCH}" ;;
    esac
    ;;
  Darwin)
    case "${ARCH}" in
      x86_64)        TRIPLE="x86_64-apple-darwin" ;;
      arm64|aarch64) TRIPLE="aarch64-apple-darwin" ;;
      *) die "Unsupported macOS architecture: ${ARCH}" ;;
    esac
    ;;
  *) die "Unsupported OS: ${OS}" ;;
esac

log "Detected platform: ${OS}/${ARCH} → ${TRIPLE}"

# ── 2. Verify dependencies ──────────────────────────────────────────────────

command -v curl >/dev/null 2>&1 || die "'curl' is required but not found"
command -v tar  >/dev/null 2>&1 || die "'tar' is required but not found"

# ── 3. Download and install binary ──────────────────────────────────────────

URL="https://github.com/${REPO}/releases/latest/download/capsule-${TRIPLE}.tar.gz"
log "Downloading ${URL}"

mkdir -p "${BIN_DIR}"

CAPSULE_TMPDIR="$(mktemp -d)"
trap 'rm -rf "${CAPSULE_TMPDIR}"' EXIT

ARCHIVE="${CAPSULE_TMPDIR}/capsule.tar.gz"
curl --silent --show-error --fail --location "${URL}" --output "${ARCHIVE}"
tar -xz -C "${CAPSULE_TMPDIR}" -f "${ARCHIVE}"
mv "${CAPSULE_TMPDIR}/capsule" "${BIN}"
chmod +x "${BIN}"

"${BIN}" --help >/dev/null 2>&1 || die "Downloaded binary does not run — possible corrupt download or wrong architecture"

ok "Binary installed to ${BIN}"

# ── 4. Ensure ~/.local/bin is on PATH ───────────────────────────────────────

RC_MODIFIED=""

_ensure_path() {
  local rc_file="$1"
  local export_line='export PATH="${HOME}/.local/bin:${PATH}"'

  if [[ ! -f "${rc_file}" ]]; then
    return
  fi

  if grep -qF '.local/bin' "${rc_file}" 2>/dev/null; then
    return
  fi

  printf '\n# Added by capsule installer\n%s\n' "${export_line}" >> "${rc_file}"
  RC_MODIFIED="${rc_file}"
  log "Added ~/.local/bin to PATH in ${rc_file}"
}

case ":${PATH}:" in
  *":${BIN_DIR}:"*)
    ;;
  *)
    log "~/.local/bin is not on PATH — updating shell rc file"
    case "${SHELL_NAME}" in
      bash) _ensure_path "${HOME}/.bashrc" ;;
      zsh)  _ensure_path "${HOME}/.zshrc" ;;
      fish) ;;
      *)    _ensure_path "${HOME}/.profile" ;;
    esac
    ;;
esac

# ── 5. Install shell completions ─────────────────────────────────────────────

COMPLETIONS_INSTALLED=""

case "${SHELL_NAME}" in
  bash)
    COMP_DIR="${HOME}/.local/share/bash-completion/completions"
    COMP_FILE="${COMP_DIR}/capsule"
    mkdir -p "${COMP_DIR}"
    "${BIN}" completion bash > "${COMP_FILE}"
    COMPLETIONS_INSTALLED="${COMP_FILE}"
    ok "Bash completions installed to ${COMP_FILE}"
    ;;

  zsh)
    ZSH_COMP_DIR="${HOME}/.zsh/completions"
    ZSH_COMP_FILE="${ZSH_COMP_DIR}/_capsule"
    mkdir -p "${ZSH_COMP_DIR}"
    "${BIN}" completion zsh > "${ZSH_COMP_FILE}"
    COMPLETIONS_INSTALLED="${ZSH_COMP_FILE}"
    ok "Zsh completions installed to ${ZSH_COMP_FILE}"

    ZSHRC="${HOME}/.zshrc"
    if [[ -f "${ZSHRC}" ]] && ! grep -qF '.zsh/completions' "${ZSHRC}" 2>/dev/null; then
      printf '\n# Added by capsule installer\nfpath=(${HOME}/.zsh/completions $fpath)\n' >> "${ZSHRC}"
      # Only add compinit if not already present — calling it twice causes warnings
      if ! grep -qF 'compinit' "${ZSHRC}" 2>/dev/null; then
        printf 'autoload -Uz compinit && compinit\n' >> "${ZSHRC}"
      fi
      RC_MODIFIED="${ZSHRC}"
      log "Added completions dir to fpath in ${ZSHRC}"
    fi
    ;;

  fish)
    FISH_COMP_DIR="${HOME}/.config/fish/completions"
    FISH_COMP_FILE="${FISH_COMP_DIR}/capsule.fish"
    mkdir -p "${FISH_COMP_DIR}"
    "${BIN}" completion fish > "${FISH_COMP_FILE}"
    COMPLETIONS_INSTALLED="${FISH_COMP_FILE}"
    ok "Fish completions installed to ${FISH_COMP_FILE}"

    FISH_CONFIG="${HOME}/.config/fish/config.fish"
    mkdir -p "$(dirname "${FISH_CONFIG}")"
    if ! grep -qF '.local/bin' "${FISH_CONFIG}" 2>/dev/null; then
      printf '\n# Added by capsule installer\nfish_add_path "%s"\n' "${BIN_DIR}" >> "${FISH_CONFIG}"
      RC_MODIFIED="${FISH_CONFIG}"
      log "Added ~/.local/bin to fish PATH in ${FISH_CONFIG}"
    fi
    ;;

  *)
    log "Shell '${SHELL_NAME}' not recognized — skipping completion setup"
    log "Run 'capsule completion <shell>' manually to generate completions"
    ;;
esac

# ── 6. Summary ───────────────────────────────────────────────────────────────

printf '\n%b' "${BOLD}"
printf '─%.0s' {1..50}
printf '\n capsule installed successfully!\n'
printf '─%.0s' {1..50}
printf '%b\n\n' "${RESET}"

printf '  Binary:      %s\n' "${BIN}"
[[ -n "${COMPLETIONS_INSTALLED}" ]] && printf '  Completions: %s\n' "${COMPLETIONS_INSTALLED}"
[[ -n "${RC_MODIFIED}" ]] && printf '  RC updated:  %s\n' "${RC_MODIFIED}"

if [[ -n "${RC_MODIFIED}" ]]; then
  printf '\n  Restart your shell or run:\n'
  printf '    source %s\n' "${RC_MODIFIED}"
fi

printf '\n  Get started:\n'
printf '    capsule --help\n\n'
