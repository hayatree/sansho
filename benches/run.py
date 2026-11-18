#!/usr/bin/env python3
"""Time fresh spx processes on the five PSP decks, in milliseconds.

Provenance: original benchmark driver using Python's monotonic clock.
"""
import argparse
import os
from pathlib import Path
from statistics import median
import subprocess
from tempfile import TemporaryDirectory
from time import perf_counter_ns

ROOT = Path(__file__).resolve().parents[1]
DECKS = [ROOT / "verify/va-decks" / (name + ".cir") for name in (
    "psp_nmos_idvg", "psp_inverter_dc", "psp_inverter_tran", "psp_ring5"
)] + [ROOT / "benches/psp_ring21.cir"]


def sample(binary, deck, jobs, cache, profile):
    env = dict(os.environ, SPX_CACHE_DIR=cache, SPX_CACHE="1", SPX_VA_JIT="1")
    if profile:
        env["SPX_PROFILE"] = "1"
    else:
        env.pop("SPX_PROFILE", None)
    begin = perf_counter_ns()
    run = subprocess.run([binary, "run", *jobs, deck], env=env,
                         stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    wall = (perf_counter_ns() - begin) / 1e6
    if run.returncode:
        raise RuntimeError(run.stderr)
    times = {"wall": wall}
    for line in run.stderr.splitlines():
        if line.startswith("spx-phase "):
            _, name, value = line.split()
            times[name] = times.get(name, 0) + float(value)
        elif line.startswith("spx-code "):
            times["code_bytes"] = times.get("code_bytes", 0) + int(line.split()[-1])
    if profile:
        if "analysis" not in times or "I-VA-JIT" in run.stderr:
            raise RuntimeError(f"Incomplete native profile for {deck}: {run.stderr}")
        phases = ("read", "netlist_pre", "flatten", "netlist_parse", "va_pre", "va_parse",
                  "va_sema_lower", "va_cache_read", "va_cache_write", "analysis", "output")
        # Older serial preparation timed the entire binding loop including codegen.
        preparation = times.get("bindings_inclusive", times.get("bind", 0) + times.get("jit_wall", 0))
        times["other"] = wall - preparation - sum(times.get(k, 0) for k in phases)
    return times


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/spx")
    parser.add_argument("--wall-binary", type=Path, help="optional uninstrumented baseline executable")
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--profile", action="store_true")
    args = parser.parse_args()
    if args.runs < 5:
        parser.error("--runs must be at least five")
    print("deck jobs cache analysis_ms wall_ms")
    for deck in DECKS:
        for jobs in (["-j1"], []):
            samples = {"cold": [], "warm": []}
            for _ in range(args.runs):
                # Independent empty private caches for external wall time and phase probes.
                with TemporaryDirectory() as timed, TemporaryDirectory() as profiled:
                    for rows in samples.values():
                        normal = sample(args.wall_binary or args.binary, deck, jobs, timed, False)
                        phases = sample(args.binary, deck, jobs, profiled, True)
                        phases["wall"] = normal["wall"]
                        rows.append(phases)
            for label, rows in samples.items():
                values = {k: median(r.get(k, 0.) for r in rows) for k in set().union(*rows)}
                print(deck.stem, jobs[0] if jobs else "default", label,
                      f"{values['analysis']:.2f}", f"{values['wall']:.2f}", flush=True)
                if args.profile:
                    print(" ", " ".join(f"{k}={v:.3f}" for k, v in sorted(values.items())
                                       if k not in ("analysis", "wall")), flush=True)


if __name__ == "__main__":
    main()
