#!/usr/bin/env bash
# Run the black-box reference tools with identical host and container paths.
# Provenance: original verification wrapper.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd -P)"
image="${SANSHO_NGSPICE_IMAGE:-sansho-ngspice:latest}"
if [[ "${1:-}" != --compile-va ]] && ! docker image inspect "$image" >/dev/null 2>&1; then
  if [[ "$image" != sansho-ngspice:latest ]]; then
    # Never auto-build a non-default oracle (e.g. sansho-ngspice-pss): its recipe
    # fetches source and must be built by a person via its own build.sh.
    echo "ngspice.sh: image $image not found; build it first (see tools/ngspice-pss/README.md)" >&2
    exit 2
  fi
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
if [[ -n "${SANSHO_VERIFY_OUT:-}" && "$SANSHO_VERIFY_OUT" != "$root/"* ]]; then
  args+=(-v "$SANSHO_VERIFY_OUT:$SANSHO_VERIFY_OUT")
fi
if [[ -n "${SANSHO_PROBE_DOCKER_MARKER:-}" ]]; then
  name="${SANSHO_PROBE_DOCKER_MARKER%/container}"
  name="sansho-probe-${name##*/}"
  printf '%s\n' "$name" > "$SANSHO_PROBE_DOCKER_MARKER"
  args+=(--name "$name")
fi
if [[ "${1:-}" == --compile-va ]]; then
  shift
  exec docker run --pull=never "${args[@]}" --entrypoint /usr/local/bin/openvaf-r "$image" "$@"
fi
exec docker run "${args[@]}" "$image" "$@"
