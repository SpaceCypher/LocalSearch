#!/bin/bash
# Build everything (Rust engine + Swift app), package it, and launch it.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"

"$REPO/scripts/package.sh"

# Quit a running copy so the new build is the one that opens.
# -x matches the process name exactly; -f would also kill shells and editors
# whose command line merely mentions a LocalSearch path.
pkill -x LocalSearch || true
# It saves its index on the way out; opening before it has exited fails (-600)
for _ in $(seq 1 100); do
    pgrep -x LocalSearch > /dev/null || break
    sleep 0.1
done

echo "Launching..."
open "$REPO/dist/LocalSearch.app"
