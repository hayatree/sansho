"""Compare the R15 VBIC sweeps against cached ASCII raw files, offline.

Provenance: original Sansho comparison of independently generated waveforms.
Run cargo build --release -p sansho-cli --features jit before invoking.
"""

import argparse
import bisect
from collections import Counter
import math
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parent
CACHE = ROOT / "out/probes"
OUT = ROOT / "out/r15b"


def plots(path):
    result = {}
    for block in path.read_text().split("Title:")[1:]:
        header, data = block.split("\nValues:\n")
        fields, variables = header.split("\nVariables:\n")
        fields = dict(line.split(": ", 1) for line in fields.splitlines() if ": " in line)
        names = [line.split()[1].lower() for line in variables.splitlines()]
        samples = iter(data.split())
        columns = {name: [] for name in names}
        for index in range(int(fields["No. Points"])):
            if int(next(samples)) != index:
                raise ValueError(f"{path}: unexpected sample index")
            for name in names:
                parts = next(samples).split(",")
                columns[name].append(complex(*map(float, parts)))
        if next(samples, None) is not None:
            raise ValueError(f"{path}: trailing samples")
        result[fields["Plotname"].lower()] = columns
    if not result:
        raise ValueError(f"{path}: empty raw")
    return result


def compare(reference, native):
    for kind, columns in reference.items():
        ours = native[kind]
        integrated = kind == "integrated noise"
        names = list(columns)
        if not integrated:
            axis = names.pop(0)
            x = [z.real for z in columns[axis]]
            y = [z.real for z in next(iter(ours.values()))]
        for name in names:
            signal = name.removeprefix("v(").removesuffix(")") if integrated else name
            ref, sim = columns[name], ours[signal]
            if integrated:
                paired = sim
            elif "transient" in kind:
                if x[0] < y[0] or x[-1] > y[-1] * (1 + 1e-12):
                    raise ValueError("native transient does not cover reference")
                paired = []
                for t in x:
                    i = min(max(1, bisect.bisect_left(y, t)), len(y) - 1)
                    f = (t - y[i - 1]) / (y[i] - y[i - 1])
                    paired.append(sim[i - 1] + f * (sim[i] - sim[i - 1]))
            else:
                # 44.2 emits only the first point of the two-point linear AC sweep.
                if len(y) != len(x) and not (kind == "ac analysis" and len(x) == 1 and len(y) == 2):
                    raise ValueError("different sweep lengths")
                if any(not math.isclose(a, b, abs_tol=1e-12, rel_tol=1e-9) for a, b in zip(x, y)):
                    raise ValueError("different sweep grids")
                paired = sim[:len(x)]
            errors = [abs(a - b) for a, b in zip(ref, paired, strict=True)]
            if not errors or not all(map(math.isfinite, errors)):
                raise ValueError("empty or non-finite comparison")
            delta, peak = max(errors), max(map(abs, ref))
            current = name.startswith("i(")
            floor = 1e-9 if current else 1e-6
            rel = 0.002
            if "noise" in kind:
                floor = 1e-30
            if "transient" in kind:
                rel = 0.03 if current else 0.01
            yield signal, delta, peak, delta / (floor + rel * peak), len(ref)


def check(deck):
    stem = deck.stem
    raw = deck.with_suffix(".raw")
    log = deck.with_suffix(".log").read_text()
    target = OUT / (stem + ".raw")
    run = subprocess.run(
        [ROOT.parent / "target/release/sansho", "run", "-j", "1", "-r", target, deck],
        capture_output=True, text=True,
    )
    if not raw.exists() or not raw.stat().st_size:
        # Lack of a cached plot alone does not establish a parameter rejection.
        if re.search(r"^error[: ]", log, re.IGNORECASE | re.MULTILINE):
            status = "BOTH-REJECT" if "E-ELAB-PARAM" in run.stderr else "FAIL"
            return status, ["44.2 rejected", run.stderr.strip() or "native accepted"]
        raise ValueError(f"{raw}: missing reference")
    if run.returncode:
        return "FAIL", [run.stderr.strip()]

    keys = re.findall(r"unrecognized parameter \(([^)]+)\)", log.lower())
    native_warnings = [s for s in run.stderr.splitlines() if "warning[" in s]
    status = "PASS"
    detail = []
    if bool(keys) != bool(native_warnings) or any(
        not any(key in warning and "W-MODEL-IGNORED" in warning for warning in native_warnings)
        for key in keys
    ):
        detail.append("warning mismatch")
        status = "FAIL"
    for signal, delta, peak, score, n in compare(plots(raw), plots(target)):
        detail.append(f"{signal}={score:.4g}({delta:.4g}/{peak:.4g},{n}pts)")
        if score > 1:
            status = "FAIL"
    if "simulation(s) aborted" in log:
        status = "PARTIAL-REFERENCE"
    return status, detail


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pattern", default="r15_vbic_*", help="probe template stem glob")
    args = parser.parse_args()
    OUT.mkdir(exist_ok=True, parents=True)
    counts = Counter()
    templates = sorted((ROOT / "probes").glob(args.pattern + ".cir"))
    if not templates:
        parser.error("no matching probe templates")
    for template in templates:
        line = next(s for s in template.read_text().splitlines() if s.startswith("* sweep:"))
        for value in line.split()[3:]:
            stem = template.stem + "_" + value
            try:
                status, detail = check(CACHE / (stem + ".cir"))
            except (OSError, ValueError, KeyError) as error:
                status, detail = "FAIL", [str(error)]
            print(status, stem, *detail, flush=True)
            counts[status] += 1
    print(dict(counts))
    return int(bool(counts["FAIL"]))


if __name__ == "__main__":
    raise SystemExit(main())
