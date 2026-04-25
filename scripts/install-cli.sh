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
  if command -v pnpm >/dev/null 2>&1; then
    (cd "${DASH_SRC}" && pnpm install --frozen-lockfile && pnpm build)
  elif command -v npm >/dev/null 2>&1; then
    (cd "${DASH_SRC}" && npm ci && npm run build)
  elif [[ ! -f "${DASH_SRC}/dist/index.html" ]]; then
    echo "Warning: pnpm/npm not found and dashboard dist is missing, skipping dashboard install"
    DASH_SRC=""
  fi

  if [[ -n "${DASH_SRC}" ]]; then
    rm -rf "${DASH_DIR}"
    mkdir -p "${DASH_DIR}"
    rsync -a --delete "${DASH_SRC}/dist/" "${DASH_DIR}/dist/"
    echo "Installed dashboard to ${DASH_DIR}"
  fi
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
