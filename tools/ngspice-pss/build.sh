#!/usr/bin/env bash
# Build the PSS-enabled ngspice oracle image without exposing its source.
# Provenance: original verification helper.
#
# Run this yourself (not inside an agent session). Only a status line is shown.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd -P)"
image="${SANSHO_NGSPICE_PSS_IMAGE:-sansho-ngspice-pss:latest}"
if out="$(docker build --progress=plain -t "$image" "$here" 2>&1)"; then
  echo "built $image"
  docker run --rm --entrypoint cat "$image" /opt/ngspice-pss/BUILDINFO
else
  # Show only the recipe's own status line and download/apt errors; never
  # configure, compiler or make output.
  printf '%s\n' "$out" | grep -E 'ngspice-pss build:|curl: \([0-9]+\)|gzip:|^#[0-9]+ [0-9.]+ E: ' | sed -E 's/^#[0-9]+ [0-9.]+ //' >&2 || true
  echo "build failed (other output withheld by policy)" >&2
  exit 1
fi
