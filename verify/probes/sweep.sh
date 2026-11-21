#!/usr/bin/env bash
# Run a fresh ngspice process for each point of a one-variable sweep.
# Provenance: original Sansho verification driver.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
out="$here/../out/probes"
mkdir -p "$out"
if (( $# == 0 )); then
    set -- "$here"/*.cir
fi
failures=()
runner=$(command -v "${NGSPICE:-ngspice}") || { echo "ngspice executable unavailable" >&2; exit 1; }
runner=$(cd "$(dirname "$runner")" && pwd -P)/$(basename "$runner")
limit=${SANSHO_PROBE_TIMEOUT:-300}
if [[ ! $limit =~ ^[1-9][0-9]*$ ]]; then
    echo "SANSHO_PROBE_TIMEOUT must be a positive integer in seconds" >&2
    exit 2
fi

# A timed read of the exit status also works on hosts without GNU timeout.
# The simulator and its children share one process group. The Docker wrapper
# registers its unique container name before launch so cleanup survives the CLI.
run_setting() (
    cd "$1" || exit 1
    shift
    local scratch job="" status
    scratch=$(mktemp -d "$out/.sweep.XXXXXXXX") || exit 1
    export SANSHO_PROBE_DOCKER_MARKER="$scratch/container"
    cleanup() {
        if [[ -n $job ]]; then
            kill -KILL -- "-$job" 2>/dev/null || :
            wait "$job" 2>/dev/null || :
            if [[ -s $SANSHO_PROBE_DOCKER_MARKER ]]; then
                local container
                read -r container < "$SANSHO_PROBE_DOCKER_MARKER"
                docker rm -f "$container" >&2
            fi
        fi
        rm -rf "$scratch"
    }
    trap cleanup EXIT
    trap 'exit 129' HUP
    trap 'exit 130' INT
    trap 'exit 143' TERM
    mkfifo "$scratch/status" || exit 1
    set -m
    (
        set +m
        "$runner" "$@" >&2 && echo 0 || echo "$?"
    ) > "$scratch/status" &
    job=$!
    if read -r -t "$limit" status < "$scratch/status"; then
        wait "$job"
        job=""
        exit "$status"
    elif (( $? > 128 )); then
        echo "PROBE-TIMEOUT after $limit seconds" >&2
        exit 124
    fi
    echo "Runner exited without reporting its status" >&2
    exit 1
)
for deck in "$@"; do
    stem=$(basename "$deck" .cir)
    read -ra values <<< "$(sed -n 's/^\* sweep: //p' "$deck")"
    # New decks name the swept model parameter before its numeric settings.
    if [[ ${values[0]:-} =~ ^[[:alpha:]_][[:alnum:]_]*$ ]]; then
        values=("${values[@]:1}")
    fi
    if [[ -z ${values[0]:-} ]]; then
        echo "missing sweep values in $deck" >&2
        failures+=("$stem (missing sweep values)")
        continue
    fi
    rejected=$(sed -n 's/^\* expect-unknown-instance: \([[:alnum:]_]\{1,\}\)$/\1/p' "$deck")
    for value in "${values[@]}"; do
        base="$out/${stem}_$value"
        failed=0
        sed -e "s/^\.param probe=.*/.param probe=$value/" \
            -e "s/^set numdgt=.*/set numdgt=$value/" \
            -e "s/^\.temp .*/.temp $value/" "$deck" > "$base.cir"
        if [[ $stem == r14_control_numdgt ]]; then
            # Keep the wrdata produced by each precision setting beside its log.
            mkdir -p "$base.control"
            rm -f "$base.control/fields.dat"
            run_setting "$base.control" -b "$base.cir" > "$base.log" 2>&1 || failed=1
            [[ -s "$base.control/fields.dat" ]] || failed=1
        else
            rm -f "$base.raw"
            SPICE_ASCIIRAWFILE=1 run_setting "$PWD" -b "$base.cir" -r "$base.raw" > "$base.log" 2>&1 || failed=1
            if ! grep -Eiq '^[[:space:]]*\.control([[:space:]]|$)' "$deck"; then
                [[ -s "$base.raw" ]] || failed=1
            fi
        fi
        if [[ -n $rejected ]]; then
            if [[ -s $base.raw ]] || grep -q '^PROBE-TIMEOUT ' "$base.log" ||
                ! grep -Fqi "unknown parameter ($rejected)" "$base.log"; then
                failures+=("$stem $value (expected rejection of $rejected)")
            fi
            continue
        fi
        if (( failed )) || grep -Eiq '^error([ :]|$)|run simulation\(s\) aborted|simulation interrupted due to error|^doAnalyses:.*(failed|timestep too small)' "$base.log"; then
            failures+=("$stem $value ($base.log)")
        fi
    done
done
if [[ -n ${failures[0]:-} ]]; then
    printf 'Failed probe settings (%d)\n' "${#failures[@]}" >&2
    printf '  %s\n' "${failures[@]}" >&2
    exit 1
fi
