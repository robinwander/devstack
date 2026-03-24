#!/usr/bin/env bash
set -euo pipefail

path="$1"
value="${2:-x}"
mkdir -p "$(dirname "$path")"
printf '%s\n' "$value" >> "$path"
