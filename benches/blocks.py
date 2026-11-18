#!/usr/bin/env python3
"""Sample native VA basic blocks on Linux x86-64 and AArch64.

Provenance: original sampler using POSIX process CPU time and Cranelift block maps.
"""
import argparse
import bisect
from collections import Counter
import os
from pathlib import Path
import struct
import subprocess
from tempfile import TemporaryDirectory, TemporaryFile

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("deck", type=Path)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/spx")
    parser.add_argument("--runs", type=int, default=12)
    args = parser.parse_args()
    counts = Counter()
    with TemporaryDirectory() as tmp:
        library = Path(tmp) / "sample.so"
        subprocess.run(["cc", "-shared", "-fPIC", "-O2", "-o", library,
                        ROOT / "benches/sample_pc.c"], check=True)
        for _ in range(args.runs):
            with TemporaryFile() as stream:
                fd = stream.fileno()
                env = dict(os.environ, SPX_PROFILE_VA_BLOCKS="1", SPX_SAMPLE_FD=str(fd),
                           LD_PRELOAD=str(library), SPX_CACHE_DIR=tmp, SPX_VA_JIT="1")
                run = subprocess.run([args.binary, "run", "-j1", args.deck], env=env,
                                     pass_fds=(fd,), check=True, stdout=subprocess.DEVNULL,
                                     stderr=subprocess.PIPE, text=True)
                regions = []
                for line in run.stderr.splitlines():
                    if line.startswith("spx-va-block "):
                        _, address, size, name = line.split()
                        regions.append((int(address, 16), int(size, 16), name))
                regions.sort()
                starts = [address for address, _, _ in regions]
                stream.seek(0)
                for (ip,) in struct.iter_unpack("P", stream.read()):
                    i = bisect.bisect_right(starts, ip) - 1
                    if i >= 0 and ip < starts[i] + regions[i][1]:
                        counts[regions[i][2]] += 1
    if not counts:
        parser.exit(1, "No samples landed in native VA code.\n")
    print(f"{sum(counts.values())} native VA samples")
    for name, hits in counts.most_common(12):
        print(f"{name:30} {hits:5} {hits / sum(counts.values()):6.1%}")


if __name__ == "__main__":
    main()
