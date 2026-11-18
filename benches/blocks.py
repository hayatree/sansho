#!/usr/bin/env python3
"""Sample native VA basic blocks on Linux x86-64 and AArch64.

Provenance: original sampler using POSIX process CPU time and Cranelift block maps.
"""
import argparse
import bisect
from collections import Counter
import os
from pathlib import Path
import platform
import re
import struct
import subprocess
from tempfile import TemporaryDirectory, TemporaryFile

ROOT = Path(__file__).resolve().parents[1]


def stack_accesses(binary, deck, directory, names):
    """Count static SIMD stack reads and writes in the selected x86-64 blocks."""
    env = dict(os.environ, SPX_PROFILE_VA_BLOCKS="1", SPX_CACHE_DIR=directory, SPX_VA_JIT="1")
    images = {}
    with subprocess.Popen([binary, "run", "-j1", deck], env=env, stdout=subprocess.DEVNULL,
                          stderr=subprocess.PIPE, text=True) as process:
        with open(f"/proc/{process.pid}/mem", "rb", buffering=0) as memory:
            for line in process.stderr:
                if line.startswith("spx-va-block "):
                    _, address, size, name = line.split()
                    if name in names:
                        length = int(size, 16)
                        code = os.pread(memory.fileno(), length, int(address, 16))
                        if len(code) != length:
                            raise RuntimeError(f"short block read for {name}")
                        images.setdefault(name, set()).add(code)
        if process.wait():
            raise RuntimeError(f"block capture failed for {deck}")
    for name, variants in sorted(images.items()):
        for code in sorted(variants):
            path = Path(directory) / "block.bin"
            path.write_bytes(code)
            asm = subprocess.check_output(["objdump", "-D", "-b", "binary", "-m", "i386:x86-64",
                                           "-M", "intel", "--no-show-raw-insn", path], text=True)
            reads = writes = 0
            for line in asm.splitlines():
                if re.search(r"\bxmm\d+", line):
                    dest, _, sources = line.partition(",")
                    writes += bool(re.search(r"\[(?:rsp|rbp)\b", dest))
                    reads += bool(re.search(r"\[(?:rsp|rbp)\b", sources))
            print(f"{name:30} {len(code):7} {reads:6} {writes:6}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("deck", type=Path)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/spx")
    parser.add_argument("--runs", type=int, default=12)
    parser.add_argument("--spills", action="store_true",
                        help="disassemble hot packed blocks and count SIMD stack operands (Linux x86-64)")
    args = parser.parse_args()
    if args.spills and (platform.system(), platform.machine()) != ("Linux", "x86_64"):
        parser.error("--spills requires Linux x86-64")
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
    if args.spills:
        hot = [name for name, _ in counts.most_common(12) if name.startswith("pair-")]
        print("block                            bytes  reads writes")
        with TemporaryDirectory() as tmp:
            stack_accesses(args.binary, args.deck, tmp, hot)


if __name__ == "__main__":
    main()
