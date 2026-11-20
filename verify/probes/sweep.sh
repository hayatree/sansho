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
for deck in "$@"; do
    stem=$(basename "$deck" .cir)
    read -ra values <<< "$(sed -n 's/^\* sweep: //p' "$deck")"
    # New decks name the swept model parameter before its numeric settings.
    if [[ ${values[0]:-} =~ ^[[:alpha:]_][[:alnum:]_]*$ ]]; then
        values=("${values[@]:1}")
    fi
    if (( ${#values[@]} == 0 )); then
        echo "missing sweep values in $deck" >&2
        exit 1
    fi
    for value in "${values[@]}"; do
        base="$out/${stem}_$value"
        sed -e "s/^\.param probe=.*/.param probe=$value/" \
            -e "s/^\.temp .*/.temp $value/" "$deck" > "$base.cir"
        SPICE_ASCIIRAWFILE=1 "${NGSPICE:-ngspice}" -b "$base.cir" -r "$base.raw" > "$base.log" 2>&1
    done
done
