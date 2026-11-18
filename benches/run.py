#!/usr/bin/env python3
"""Time fresh spx processes on the five PSP decks, in milliseconds.

Provenance: original benchmark driver using Python's monotonic clock.
"""
import argparse
import os
from pathlib import Path
from statistics import median
from collections import Counter
import subprocess
from tempfile import TemporaryDirectory
from time import perf_counter_ns

ROOT = Path(__file__).resolve().parents[1]
DECKS = [ROOT / "verify/va-decks" / (name + ".cir") for name in (
    "psp_nmos_idvg", "psp_inverter_dc", "psp_inverter_tran", "psp_ring5"
)] + [ROOT / "benches/psp_ring21.cir"]


def sample(binary, deck, jobs, cache, profile):
    env = dict(os.environ, SPX_CACHE_DIR=cache, SPX_CACHE="1", SPX_VA_JIT="1")
    env.pop("SPX_PROFILE_DEVICES", None)
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


def device_counts(binary, runs):
    print("deck visits native interpreted reused JIT_ns/eval Newton_histogram JIT_ms LU_ms analysis_ms")
    for deck in DECKS:
        samples = []
        with TemporaryDirectory() as cache:
            env = dict(os.environ, SPX_CACHE_DIR=cache, SPX_CACHE="1", SPX_VA_JIT="1",
                       SPX_PROFILE_DEVICES="1", SPX_PROFILE="1")
            for _ in range(runs + 1):
                run = subprocess.run([binary, "run", "-j1", deck], env=env, check=True,
                                     stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
                native = interpreted = reused = elapsed = lu = 0
                analysis = 0.0
                counts = Counter()
                for line in run.stderr.splitlines():
                    if line.startswith("spx-phase analysis "):
                        analysis += float(line.split()[-1])
                    if line.startswith("spx-device "):
                        n, i, r, ns = map(int, line.split()[1:])
                        native += n
                        interpreted += i
                        reused += r
                        elapsed += ns
                    if line.startswith("spx-newton "):
                        _, t, iterations, total_lu = line.split()
                        # Omit the time-zero operating point in transient histograms.
                        if float(t) > 0 or deck in DECKS[:2]:
                            counts[int(iterations)] += 1
                        lu = int(total_lu)
                if not native:
                    raise RuntimeError(f"no native evaluations counted for {deck}")
                samples.append((elapsed / native, elapsed / 1e6, lu / 1e6, analysis))
        print(deck.stem, native + interpreted + reused, native, interpreted, reused,
              round(median(row[0] for row in samples[1:])), dict(sorted(counts.items())),
              *(f"{median(row[k] for row in samples[1:]):.3f}" for k in range(1, 4)), flush=True)


def paired(baseline, candidate, runs):
    print("deck jobs cache HEAD_analysis_ms new_analysis_ms HEAD_wall_ms new_wall_ms HEAD_j1_anchor_wall_ms")
    for deck in DECKS:
        for jobs in (["-j1"], []):
            rows = {key: [] for key in ("cold", "warm")}
            for _ in range(runs):
                with (TemporaryDirectory() as a, TemporaryDirectory() as b,
                      TemporaryDirectory() as pa, TemporaryDirectory() as pb):
                    pair = [(baseline, a, pa), (candidate, b, pb)]
                    for label in rows:
                        phases = [sample(exe, deck, jobs, profile_cache, True)
                                  for exe, _, profile_cache in pair]
                        for result, (exe, cache, _) in zip(phases, pair):
                            result["wall"] = sample(exe, deck, jobs, cache, False)["wall"]
                        anchor = phases[0]["wall"]
                        if not jobs and label == "warm":
                            anchor = sample(baseline, deck, ["-j1"], a, False)["wall"]
                        phases[0]["anchor"] = anchor if jobs or label == "warm" else float("nan")
                        rows[label].append(phases)
            for label, pairs in rows.items():
                print(deck.stem, jobs[0] if jobs else "default", label,
                      *(f"{median(pair[i][k] for pair in pairs):.3f}"
                        for k in ("analysis", "wall") for i in range(2)),
                      f"{median(pair[0]['anchor'] for pair in pairs):.3f}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/spx")
    parser.add_argument("--wall-binary", type=Path, help="optional uninstrumented baseline executable")
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--profile", action="store_true")
    parser.add_argument("--baseline", type=Path,
                        help="interleave this executable with --binary using private warm caches")
    parser.add_argument("--devices", action="store_true",
                        help="profile individual PSP evaluations and Newton steps on one worker")
    args = parser.parse_args()
    if args.devices:
        if args.runs < 5:
            parser.error("--runs must be at least five")
        device_counts(args.binary, args.runs)
        return
    if args.baseline:
        if args.runs < 7:
            parser.error("interleaved measurements require at least seven pairs")
        paired(args.baseline, args.binary, args.runs)
        return
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
