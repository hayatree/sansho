#!/usr/bin/env bash
# Run ngspice inside Docker with the current directory mounted at /work.
# Usage: tools/ngspice/ngspice.sh -b deck.cir -r out.raw
set -euo pipefail
IMAGE="${SPX_NGSPICE_IMAGE:-spx-ngspice:latest}"
if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
  docker build -q -t "$IMAGE" "$(dirname "$0")" >/dev/null
fi
exec docker run --rm --network none -e SPICE_ASCIIRAWFILE=1 -u "$(id -u):$(id -g)" \
  -v "$PWD:/work" -w /work "$IMAGE" "$@"
