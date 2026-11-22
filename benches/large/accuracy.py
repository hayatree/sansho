#!/usr/bin/env python3
"""Compare multiplier transient raw files with the cached gold waveform.

Provenance: original benchmark using linear interpolation and threshold crossings.
"""
import argparse
from bisect import bisect_right
from itertools import islice
from math import fsum, isclose, isfinite
from pathlib import Path
from struct import Struct

CACHE = Path(__file__).resolve().parents[2] / "verify/out/large"
SAMPLES = 4000


def read(path):
    with path.open("rb") as raw:
        fields = {}
        for line in raw:
            key, _, value = line.decode().strip().partition(":")
            if key == "Variables":
                break
            fields[key] = value.strip()
        if fields.get("Flags") != "real" or fields.get("Plotname", "").lower() != "transient analysis":
            raise ValueError(f"{path}: expected one real transient plot")
        width = int(fields["No. Variables"])
        count = int(fields["No. Points"])
        names = [raw.readline().split()[1].decode().lower() for _ in range(width)]
        indices = [names.index("time"), *(names.index(f"v(p{i})") for i in range(32))]
        columns = [[] for _ in indices]
        kind = raw.readline().strip()
        if kind == b"Binary:":
            row = Struct(f"={width}d")
            for _ in range(count):
                data = raw.read(row.size)
                if len(data) != row.size:
                    raise ValueError(f"{path}: incomplete binary point")
                values = row.unpack(data)
                for column, j in zip(columns, indices):
                    column.append(values[j])
            extra = raw.read(1)
        elif kind == b"Values:":
            words = (word for line in raw for word in line.split())
            for i in range(count):
                point = list(islice(words, width + 1))
                if len(point) != width + 1 or int(point[0]) != i:
                    raise ValueError(f"{path}: incomplete or misnumbered point {i}")
                for column, j in zip(columns, indices):
                    column.append(float(point[j + 1]))
            extra = next(words, None)
        else:
            raise ValueError(f"{path}: unknown raw data format")
        if count < 2 or extra:
            raise ValueError(f"{path}: expected a single complete transient")
    times = columns[0]
    if any(b <= a for a, b in zip(times, times[1:])) or not all(
            isfinite(v) for column in columns for v in column):
        raise ValueError(f"{path}: invalid waveform")
    return times, columns[1:]


def sample(times, signals, grid):
    result = []
    for t in grid:
        j = min(max(1, bisect_right(times, t)), len(times) - 1)
        w = min(1., max(0., (t - times[j - 1]) / (times[j] - times[j - 1])))
        result.extend(y[j - 1] + w * (y[j] - y[j - 1]) for y in signals)
    return result


def crossings(signals):
    return [sum((a >= 0.9) != (b >= 0.9) for a, b in zip(y, y[1:])) for y in signals]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("deck", choices=("mult16_mos1", "mult16_bsim4"))
    parser.add_argument("candidate", type=Path, nargs="*")
    args = parser.parse_args()
    times, signals = read(CACHE / f"{args.deck}.gold.raw")
    grid = [times[0] + (times[-1] - times[0]) * i / (SAMPLES - 1) for i in range(SAMPLES)]
    gold = sample(times, signals, grid)
    counts = crossings(signals)
    print("Run  Points  Mean |dV| (mV)  Crossings  Crossing diff")
    print(f"ngspice gold  {len(times)}  0  {sum(counts)}  0")
    reference = CACHE / f"{args.deck}.ref.raw"
    candidates = args.candidate or [CACHE / f"{args.deck}.sansho.raw"]
    for path in [reference, *candidates]:
        t, y = read(path)
        if not (isclose(t[0], times[0]) and isclose(t[-1], times[-1])):
            raise ValueError(f"{path}: interval differs from gold")
        volts = sample(t, y, grid)
        error = fsum(abs(a - b) for a, b in zip(volts, gold)) / len(gold)
        observed = crossings(y)
        diff = sum(abs(a - b) for a, b in zip(counts, observed))
        label = "ngspice default" if path == reference else path.name
        print(f"{label}  {len(t)}  {1000 * error:.3f}  {sum(observed)}  {diff}")


if __name__ == "__main__":
    main()
