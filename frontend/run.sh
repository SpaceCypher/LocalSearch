#!/bin/bash
# Build everything (Rust engine + Swift app), package it, and launch it.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"

"$REPO/scripts/package.sh"

# Quit a running copy so the new build is the one that opens.
# -x matches the process name exactly; -f would also kill shells and editors
# whose command line merely mentions a LocalSearch path.
pkill -x LocalSearch || true

echo "Launching..."
open "$REPO/dist/LocalSearch.app"
