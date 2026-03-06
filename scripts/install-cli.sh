#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
BIN_DIR="${HOME}/.local/bin"

command -v cargo >/dev/null 2>&1 || {
  echo "cargo is required" >&2
  exit 1
}

cargo build --release --manifest-path "${ROOT_DIR}/Cargo.toml"

install -d "${BIN_DIR}"
install -m 755 "${ROOT_DIR}/target/release/devstack" "${BIN_DIR}/devstack"

echo "Installed devstack to ${BIN_DIR}/devstack"
if ! echo "$PATH" | tr ':' '\n' | grep -qx "${BIN_DIR}"; then
  echo "Note: ${BIN_DIR} is not in PATH. Add it to your shell profile."
fi

# Install dashboard
if [[ "$(uname -s)" == "Darwin" ]]; then
  DASH_DIR="${HOME}/Library/Application Support/devstack/dashboard"
else
  DASH_DIR="${HOME}/.local/share/devstack/dashboard"
fi
DASH_SRC="${ROOT_DIR}/devstack-dash"

if [[ -d "${DASH_SRC}" ]]; then
  mkdir -p "${DASH_DIR}"
  rsync -a --delete "${DASH_SRC}/" "${DASH_DIR}/"

  if command -v pnpm >/dev/null 2>&1; then
    (cd "${DASH_DIR}" && pnpm install --frozen-lockfile)
  elif command -v npm >/dev/null 2>&1; then
    (cd "${DASH_DIR}" && npm ci)
  else
    echo "Warning: pnpm/npm not found, skipping dashboard dependency install"
  fi
  echo "Installed dashboard to ${DASH_DIR}"
fi

if command -v systemctl >/dev/null 2>&1; then
  if systemctl --user status devstack.service >/dev/null 2>&1; then
    systemctl --user restart devstack.service || true
    echo "Restarted devstack daemon (systemd user service)"
  fi
fi

if [[ "$(uname -s)" == "Darwin" ]] && command -v launchctl >/dev/null 2>&1; then
  "${BIN_DIR}/devstack" install
  echo "Restarted devstack daemon (LaunchAgent)"
fi
