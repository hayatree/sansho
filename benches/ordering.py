#!/usr/bin/env python3
"""Compare threshold Markowitz pivots with the first real Newton factorization.

Provenance: original experiment based on Markowitz (1957) and sparse Gaussian
elimination. Input comes from the local simulator's numeric matrix diagnostic.
"""
import argparse
import ast
import os
import re
import struct
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def markowitz(columns, rhs, threshold):
    rows = set(range(len(rhs)))
    remaining = set(rows)
    factors = []
    work = rhs.copy()
    nnz = 0
    while remaining:
        degrees = {i: 0 for i in rows}
        for j in remaining:
            for i in columns[j]:
                degrees[i] += 1
        candidates = []
        for j in remaining:
            col = columns[j]
            limit = threshold * max(map(abs, col.values()), default=0.)
            for i, value in col.items():
                if value and abs(value) >= limit:
                    cost = (degrees[i] - 1) * (len(col) - 1)
                    candidates.append((cost, i != j, -abs(value), j, i))
        _, _, _, j, i = min(candidates)
        remaining.remove(j)
        rows.remove(i)
        diag = columns[j].pop(i)
        inv = 1. / diag
        lower = [(r, v * inv) for r, v in columns[j].items()]
        upper = [(k, columns[k].pop(i)) for k in sorted(remaining) if i in columns[k]]
        nnz += len(lower) + 1 + len(upper)
        for r, v in lower:
            work[r] -= v * work[i]
        for k, u in upper:
            col = columns[k]
            for r, v in lower:
                col[r] = col.get(r, 0.) - v * u
        factors.append((j, i, inv, upper))
    result = [0.] * len(rhs)
    for j, i, inv, upper in reversed(factors):
        value = work[i]
        for k, u in upper:
            value -= u * result[k]
        result[j] = value * inv
    return nnz, result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/spx")
    parser.add_argument("decks", nargs="*", type=Path, default=[
        ROOT / "verify/va-decks/psp_ring5.cir", ROOT / "benches/psp_ring21.cir"])
    args = parser.parse_args()
    for deck in args.decks:
        proc = subprocess.run([args.binary, "run", "-j1", deck], check=True,
                              env=dict(os.environ, SPX_PROFILE_MATRIX="entries"),
                              stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
        lines = proc.stderr.splitlines()
        def first(prefix):
            return next(line.removeprefix(prefix) for line in lines if line.startswith(prefix))
        ptr, indices, values = map(ast.literal_eval, re.findall(r"\[[^\]]*\]", first("spx-csc ")))
        rhs = ast.literal_eval(first("spx-first-rhs "))
        baseline = ast.literal_eval(first("spx-first-solution "))
        print(deck.stem, first("spx-matrix "))
        for active in (False, True):
            cols = [{indices[k]: values[k] for k in range(begin, end)
                     if not active or values[k] != 0.} for begin, end in zip(ptr, ptr[1:])]
            nnz = sum(map(len, cols))
            lu, x = markowitz(cols, rhs, 0.1)
            changed = sum(struct.pack("d", a) != struct.pack("d", b) for a, b in zip(x, baseline))
            error = max(abs(a - b) for a, b in zip(x, baseline))
            print(f"  Markowitz {'numeric' if active else 'structural'} A={nnz} LU={lu} "
                  f"fill={lu-nnz} differing_components={changed} max_abs={error:.4g}")


if __name__ == "__main__":
    main()
