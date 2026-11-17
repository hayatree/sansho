#!/usr/bin/env bash
# Run the black-box reference tools with identical host and container paths.
# Provenance: original verification wrapper.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd -P)"
image="${SPX_NGSPICE_IMAGE:-spx-ngspice:latest}"
if [[ "${1:-}" != --compile-va ]] && ! docker image inspect "$image" >/dev/null 2>&1; then
  docker build -q -t "$image" "$root/tools/ngspice" >/dev/null
fi
args=(--rm --network none -e SPICE_ASCIIRAWFILE=1 -u "$(id -u):$(id -g)"
      -v "$root:$root" -w "$PWD")
if [[ -n "${SPX_VERIFY_OUT:-}" && "$SPX_VERIFY_OUT" != "$root/"* ]]; then
  args+=(-v "$SPX_VERIFY_OUT:$SPX_VERIFY_OUT")
fi
if [[ "${1:-}" == --compile-va ]]; then
  shift
  exec docker run --pull=never "${args[@]}" --entrypoint /usr/local/bin/openvaf-r "$image" "$@"
fi
exec docker run "${args[@]}" "$image" "$@"
