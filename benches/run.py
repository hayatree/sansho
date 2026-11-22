#!/usr/bin/env python3
"""Time fresh Sansho processes on the five PSP decks, in milliseconds.

Provenance: original benchmark driver using Python's monotonic clock.
"""
import argparse
from collections import Counter
import os
import re
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
    env = dict(os.environ, SANSHO_CACHE_DIR=cache, SANSHO_CACHE="1", SANSHO_VA_JIT="1")
    env.pop("SANSHO_PROFILE_DEVICES", None)
    if profile:
        env["SANSHO_PROFILE"] = "1"
    else:
        env.pop("SANSHO_PROFILE", None)
    begin = perf_counter_ns()
    run = subprocess.run([binary, "run", *jobs, deck], env=env,
                         stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    wall = (perf_counter_ns() - begin) / 1e6
    if run.returncode:
        raise RuntimeError(run.stderr)
    times = {"wall": wall}
    for line in run.stderr.splitlines():
        if line.startswith("sansho-phase "):
            _, name, value = line.split()
            times[name] = times.get(name, 0) + float(value)
        elif line.startswith("sansho-code "):
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


def device_sample(binary, deck, cache, jobs):
    env = dict(os.environ, SANSHO_CACHE_DIR=cache, SANSHO_CACHE="1", SANSHO_VA_JIT="1",
               SANSHO_PROFILE_DEVICES="1", SANSHO_PROFILE="1")
    run = subprocess.run([binary, "run", *jobs, deck], env=env, check=True,
                         capture_output=True, text=True)
    tran = re.search(r"^Transient: (\d+) accepted steps, (\d+) rejected, (\d+) Newton iterations",
                    run.stdout, re.MULTILINE)
    accepted, rejected, iterations = map(int, tran.groups()) if tran else ("-", "-", "-")
    native = interpreted = reused = elapsed = lu = factor = steps = 0
    attempted = finished = workers = group_ns = assemblies = 0
    analysis = 0.0
    counts = Counter()
    for line in run.stderr.splitlines():
        if line.startswith("sansho-phase analysis "):
            analysis += float(line.split()[-1])
        elif line.startswith("sansho-device "):
            n, i, r, ns = map(int, line.split()[1:])
            native += n
            interpreted += i
            reused += r
            elapsed += ns
        elif line.startswith("sansho-pair "):
            a, f, _ = map(int, line.split()[1:])
            attempted += a
            finished += f
        elif line.startswith("sansho-groups "):
            workers, n, ns = map(int, line.split()[1:])
            assemblies += n
            group_ns += ns
        elif line.startswith("sansho-factor "):
            steps, factor = map(int, line.split()[1:])
        elif line.startswith("sansho-newton "):
            _, t, step_iterations, total_lu = line.split()
            if float(t) > 0 or deck in DECKS[:2]:
                counts[int(step_iterations)] += 1
            lu = int(total_lu)
    if deck.resolve() in DECKS and not native:
        raise RuntimeError(f"no native evaluations counted for {deck}")
    return ((native + interpreted + reused, native, interpreted, reused,
             dict(sorted(counts.items())), finished, attempted, steps, workers, assemblies,
             accepted, rejected, iterations),
            (elapsed / native if native else 0, elapsed / 1e6, lu / 1e6,
             factor / (1000 * steps) if steps else float("nan"),
             group_ns / 1e6 if workers else float("nan"), analysis,
             (group_ns - elapsed) / (native + interpreted + reused)
             if workers == 1 and native else float("nan")))


def device_counts(binary, runs, baseline, decks, jobs):
    print("deck build visits native interpreted reused Newton_histogram paired attempted "
          "factors workers assemblies accepted rejected Newton_iterations JIT_ns/eval "
          "JIT_ms LU_ms factor_us group_ms analysis_ms group_other_ns/visit")
    for deck in decks:
        with TemporaryDirectory() as a, TemporaryDirectory() as b:
            builds = ([("HEAD", baseline, a)] if baseline else []) + [("new", binary, b)]
            samples = {name: [] for name, _, _ in builds}
            for idx in range(runs + 1):
                for name, exe, cache in builds:
                    counts, timings = device_sample(exe, deck, cache, jobs)
                    if idx:
                        samples[name].append(timings)
                    if idx == runs:
                        print(deck.stem, name, *counts,
                              *(f"{median(column):.3f}" for column in zip(*samples[name])),
                              flush=True)


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


def warm_timing(baseline, binary, runs, decks, jobs, metric):
    print(f"deck HEAD_{metric}_ms new_{metric}_ms speedup")
    profile = metric == "analysis"
    for deck in decks:
        with TemporaryDirectory() as before, TemporaryDirectory() as after:
            builds = ((baseline, before), (binary, after))
            for exe, cache in builds:
                sample(exe, deck, jobs, cache, profile)
            rows = [[sample(exe, deck, jobs, cache, profile)[metric] for exe, cache in builds]
                    for _ in range(runs)]
            a, b = map(median, zip(*rows))
            print(deck.stem, f"{a:.3f}", f"{b:.3f}", f"{a / b:.3f}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/sansho")
    parser.add_argument("--wall-binary", type=Path, help="optional uninstrumented baseline executable")
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--profile", action="store_true")
    parser.add_argument("--baseline", type=Path,
                        help="interleave this executable with --binary using private warm caches")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--warm-analysis", action="store_true",
                      help="repeat only warm interleaved analysis measurements against --baseline")
    mode.add_argument("--warm-wall", action="store_true",
                      help="repeat only uninstrumented warm process timings against --baseline")
    mode.add_argument("--devices", action="store_true",
                        help="profile device evaluations, group wall time and Newton steps")
    parser.add_argument("--deck", type=Path, action="append",
                        help="select a deck for --devices, --warm-analysis or --warm-wall (repeatable)")
    parser.add_argument("--jobs", type=str, default="1",
                        help="worker count for device or warm analysis measurements (default is one)")
    args = parser.parse_args()
    if args.jobs != "default" and not (args.jobs.isdecimal() and int(args.jobs) > 0):
        parser.error("--jobs requires a positive integer or 'default'")
    if args.deck and not (args.devices or args.warm_analysis or args.warm_wall):
        parser.error("--deck requires --devices, --warm-wall or --warm-analysis")
    if args.baseline and args.runs < 7:
        parser.error("interleaved measurements require at least seven pairs")
    if args.warm_analysis or args.warm_wall:
        if not args.baseline:
            parser.error("warm interleaved timings require --baseline")
        warm_timing(args.baseline, args.binary, args.runs, args.deck or DECKS,
                    [] if args.jobs == "default" else ["-j", args.jobs],
                    "wall" if args.warm_wall else "analysis")
        return
    if args.devices:
        if args.runs < 5:
            parser.error("--runs must be at least five")
        device_counts(args.binary, args.runs, args.baseline, args.deck or DECKS,
                      [] if args.jobs == "default" else ["-j", args.jobs])
        return
    if args.baseline:
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
