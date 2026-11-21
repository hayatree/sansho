#!/usr/bin/env bash
# Build the PSS-enabled ngspice oracle image without exposing its source.
# Provenance: original verification helper.
#
# Run this yourself (not inside an agent session). Only a status line is shown.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd -P)"
image="${SANSHO_NGSPICE_PSS_IMAGE:-sansho-ngspice-pss:latest}"
if docker build -q -t "$image" "$here" >/dev/null 2>&1; then
  echo "built $image"
  docker run --rm --entrypoint cat "$image" /opt/ngspice-pss/BUILDINFO
else
  echo "build failed (output withheld by policy); rerun later or report the failure only" >&2
  exit 1
fi
