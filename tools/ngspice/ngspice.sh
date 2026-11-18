#!/usr/bin/env bash
# Run the black-box reference tools with identical host and container paths.
# Provenance: original verification wrapper.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd -P)"
image="${SPX_NGSPICE_IMAGE:-spx-ngspice:latest}"
if [[ "${1:-}" != --compile-va ]] && ! docker image inspect "$image" >/dev/null 2>&1; then
  docker build -q -t "$image" "$root/tools/ngspice" >/dev/null
fi
# A deck directory may carry its own .spiceinit (e.g. `set ngbehavior=hsa` for PDK
# libraries); ngspice reads it from the working directory, so run there.
workdir="$PWD"
prev=""
for arg in "$@"; do
  if [[ "$prev" == -b && -f "$(dirname "$arg")/.spiceinit" ]]; then
    workdir="$(cd "$(dirname "$arg")" && pwd -P)"
  fi
  prev="$arg"
done
args=(--rm --network none -e SPICE_ASCIIRAWFILE=1 -u "$(id -u):$(id -g)"
      -v "$root:$root" -w "$workdir")
if [[ -n "${SPX_VERIFY_OUT:-}" && "$SPX_VERIFY_OUT" != "$root/"* ]]; then
  args+=(-v "$SPX_VERIFY_OUT:$SPX_VERIFY_OUT")
fi
if [[ "${1:-}" == --compile-va ]]; then
  shift
  exec docker run --pull=never "${args[@]}" --entrypoint /usr/local/bin/openvaf-r "$image" "$@"
fi
exec docker run "${args[@]}" "$image" "$@"
