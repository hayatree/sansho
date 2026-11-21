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
runner=$(realpath "$runner")
for deck in "$@"; do
    stem=$(basename "$deck" .cir)
    read -ra values <<< "$(sed -n 's/^\* sweep: //p' "$deck")"
    # New decks name the swept model parameter before its numeric settings.
    if [[ ${values[0]:-} =~ ^[[:alpha:]_][[:alnum:]_]*$ ]]; then
        values=("${values[@]:1}")
    fi
    if (( ${#values[@]} == 0 )); then
        echo "missing sweep values in $deck" >&2
        failures+=("$stem (missing sweep values)")
        continue
    fi
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
            (cd "$base.control" && "$runner" -b "$base.cir") > "$base.log" 2>&1 || failed=1
            [[ -s "$base.control/fields.dat" ]] || failed=1
        else
            rm -f "$base.raw"
            SPICE_ASCIIRAWFILE=1 "$runner" -b "$base.cir" -r "$base.raw" > "$base.log" 2>&1 || failed=1
            if ! grep -Eiq '^[[:space:]]*\.control([[:space:]]|$)' "$deck"; then
                [[ -s "$base.raw" ]] || failed=1
            fi
        fi
        if (( failed )) || grep -Eiq '^error([ :]|$)|run simulation\(s\) aborted|simulation interrupted due to error|^doAnalyses:.*(failed|timestep too small)' "$base.log"; then
            failures+=("$stem $value ($base.log)")
        fi
    done
done
if (( ${#failures[@]} )); then
    printf 'Failed probe settings (%d)\n' "${#failures[@]}" >&2
    printf '  %s\n' "${failures[@]}" >&2
    exit 1
fi
