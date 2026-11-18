#!/usr/bin/env python3
"""Compare every IHP result vector from two release CLI binaries.

Provenance: original regression driver for spx's round-trip JSON representation.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import struct
import subprocess
from tempfile import NamedTemporaryFile, TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]


def bits(token):
    value = float(token)
    if not math.isfinite(value):
        raise ValueError(f"nonfinite value {token}")
    return struct.pack(">d", value)


def results(binary, path, jobs, env):
    data = subprocess.check_output([binary, "run", "--json", *jobs, path], env=env)
    result = json.loads(data, parse_float=bits)["results"]
    for analysis in result["analyses"]:
        # JSON encodes nonfinite floats as null, which cannot establish bit identity.
        def check(v):
            if isinstance(v, list):
                for x in v:
                    check(x)
            elif not isinstance(v, bytes):
                raise ValueError(f"{path}: expected a finite floating-point sample")
        check(analysis["data"]["columns"])
        check(analysis["scale_values"])
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline", type=Path)
    parser.add_argument("candidate", nargs="?", type=Path, default=ROOT / "target/release/spx")
    args = parser.parse_args()
    decks = sorted((ROOT / "verify/va-decks").glob("*.cir")) + [ROOT / "benches/psp_ring21.cir"]
    for deck in decks:
        with NamedTemporaryFile(mode="w", suffix=".spx-input", dir=deck.parent) as full, TemporaryDirectory() as cache:
            # Keep relative include resolution and strip the decks' save projection.
            full.writelines(line for line in deck.read_text().splitlines(keepends=True)
                            if line.strip().lower().split()[:1] != [".save"])
            full.flush()
            env = dict(os.environ, SPX_CACHE_DIR=cache, SPX_CACHE="1", SPX_VA_JIT="1")
            before = results(args.baseline, full.name, ["-j1"], dict(env, SPX_CACHE="0"))
            for mode, jobs in (("cold", ["-j1"]), ("warm", []), ("warm", ["-j1"])):
                if before != results(args.candidate, full.name, jobs, env):
                    raise SystemExit(f"{deck.name} differs ({mode}, {jobs or 'default'})")
            with TemporaryDirectory() as parallel:
                if results(args.candidate, full.name, [], dict(env, SPX_CACHE_DIR=parallel)) != before:
                    raise SystemExit(f"{deck.name} differs (cold, default)")
                def artifacts(directory):
                    return {p.name: hashlib.sha256(p.read_bytes()).digest()
                            for p in Path(directory).iterdir()}
                serial = artifacts(cache)
                if not serial or serial != artifacts(parallel):
                    raise SystemExit(f"{deck.name}: worker counts produced different cache artifacts")
            print(f"{deck.name}: all vectors identical", flush=True)


if __name__ == "__main__":
    main()
