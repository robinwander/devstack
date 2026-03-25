#!/usr/bin/env bash
set -euo pipefail

binary=$1
shift

if [[ "$binary" != */deps/* ]]; then
  exec "$binary" "$@"
fi

for arg in "$@"; do
  case "$arg" in
    --help|-h|--list|--nocapture|--show-output|--format=*)
      exec "$binary" "$@"
      ;;
  esac
done

output=$(mktemp)
cleanup() {
  rm -f "$output"
}
trap cleanup EXIT

if "$binary" "$@" >"$output" 2>&1; then
  exit 0
else
  status=$?
  cat "$output" >&2
  exit "$status"
fi
