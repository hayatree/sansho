"""Compare model and distributed-line sweeps against cached ASCII raw files, offline.

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
import tempfile
from contextlib import ExitStack

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


def device_signal(name):
    """ASCII raw writers can attach a current or voltage wrapper to @ vectors."""
    match = re.fullmatch(r"[vi]\((@[^()]+)\)", name)
    return match.group(1) if match else name


def compare(reference, native, soi=False, unavailable=()):
    for kind, columns in reference.items():
        ours = {device_signal(name): data for name, data in native[kind].items()}
        integrated = kind == "integrated noise"
        single_point = integrated or kind == "operating point"
        names = list(columns)
        if not single_point:
            axis = names.pop(0)
            x = [z.real for z in columns[axis]]
            y = [z.real for z in next(iter(ours.values()))]
        for name in names:
            signal = name.removeprefix("v(").removesuffix(")") if integrated else name
            signal = device_signal(signal)
            if signal in unavailable:
                continue
            ref, sim = columns[name], ours[signal]
            if single_point:
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
            if soi and current:
                floor = 1e-16
            if soi and signal.startswith("@m"):
                quantity = signal.split("[")[-1].rstrip("]")
                if quantity.startswith(("q", "c")):
                    floor = 1e-21
                elif quantity.startswith(("g", "i")):
                    floor = 1e-16
            rel = 0.002
            if signal.startswith("@") and signal.endswith("[tf]"):
                floor = 1e-18  # Transit time is in seconds.
            if "noise" in kind:
                floor = 1e-30
            if "transient" in kind:
                rel = 0.03 if current else 0.01
            yield signal, delta, peak, delta / (floor + rel * peak), len(ref)


def simulate(deck, target, binary):
    return subprocess.run(
        [binary, "run", "-j", "1", "-r", target, deck],
        capture_output=True, text=True,
    )


def expand(template, value):
    body, changes = re.subn(r"^\.param probe=.*$", ".param probe=" + value,
                            template.read_text(), flags=re.MULTILINE)
    if changes != 1:
        raise ValueError(f"{template}: expected one probe assignment")
    return body


def rejected_parameter(body):
    marker = re.search(r"^\* expect-unknown-instance: (\w+)$", body, re.MULTILINE)
    return marker.group(1) if marker else None


def native_rejected(run, name):
    return run.returncode and "E-ELAB-PARAM" in run.stderr and name in run.stderr.lower()


def native_check(template, value, work, binary):
    # The oracle sweep script replaces precisely this assignment in R15/R16.
    deck = work / (template.stem + "_" + value + ".cir")
    body = expand(template, value)
    deck.write_text(body)
    target = deck.with_suffix(".raw")
    run = simulate(deck, target, binary)
    name = rejected_parameter(body)
    if name:
        return ("NATIVE-REJECT" if native_rejected(run, name) else "FAIL"), [run.stderr.strip()]
    if run.returncode:
        return "FAIL", [run.stderr.strip()]
    return "NATIVE", [finite_plots(target), run.stderr.strip()]


def finite_plots(target):
    waveforms = plots(target)
    for columns in waveforms.values():
        for signal, samples in columns.items():
            if not samples or any(not math.isfinite(abs(z)) for z in samples):
                raise ValueError(f"{target}: empty or nonfinite {signal}")
    return ", ".join(waveforms)


def check(deck, out, binary, expected, value):
    if not deck.exists():
        return "MISSING-REFERENCE", ["collect the new setting with sweep.sh"]

    def statements(text):
        return [s.strip() for s in text.splitlines()[1:] if s.strip() and not s.lstrip().startswith("*")]

    if statements(deck.read_text()) != statements(expected):
        return "STALE-REFERENCE", ["cached circuit differs from current template"]
    stem = deck.stem
    raw = deck.with_suffix(".raw")
    log = deck.with_suffix(".log").read_text()
    target = out / (stem + ".raw")
    run = simulate(deck, target, binary)
    timed_out = any(line.startswith("PROBE-TIMEOUT ") for line in log.splitlines())
    if stem == "r16b_pnp_load_1k" and (timed_out or (
        "Transient op started" in log and raw.exists()
        and raw.read_bytes().rstrip().endswith(b"No. Points:")
    )):
        # The maintainer killed this 44.2 run after nine hours, before row 0.
        # Still require a successful, finite native sweep and operating point.
        if run.returncode:
            return "FAIL", [run.stderr.strip()]
        native = plots(target)
        for kind, points in [("operating point", 1), ("dc transfer characteristic", 25)]:
            for name in ("v(b)", "v(drive)", "i(vdrive)", "i(vc)"):
                samples = native[kind][name]
                if len(samples) != points or not all(math.isfinite(abs(x)) for x in samples):
                    return "FAIL", [f"incomplete native {kind} {name}"]
        return "ORACLE-NONTERMINATION", ["44.2 produced no usable reference"]
    name = rejected_parameter(expected)
    if name:
        rejected = (not timed_out and f"unknown parameter ({name})" in log.lower()
                    and not (raw.exists() and raw.stat().st_size) and native_rejected(run, name))
        return ("BOTH-REJECT" if rejected else "FAIL"), [f"unknown instance parameter {name}", run.stderr.strip()]
    if timed_out:
        return "FAIL", ["oracle setting timed out"]
    for settings, diagnostic in re.findall(r"^\* reference-error: (.+) \| (.+)$", expected, re.MULTILINE):
        if value in settings.split() and diagnostic in log.splitlines():
            if run.returncode or "fatal error in ngspice, exit(1)" not in log:
                return "FAIL", [diagnostic, run.stderr.strip()]
            if raw.exists() and re.search(r"^No. Points:\s*[1-9]", raw.read_text(), re.MULTILINE):
                return "FAIL", ["expected no reference waveform", diagnostic]
            return "ORACLE-REJECTED", [diagnostic, finite_plots(target)]
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
    # Compare triggered ratings once per rating. The simulators can report
    # the same crossing at different accepted points in a transient.
    rating = re.compile(r"\bv(?:be|bc|ce|cs)_max\b", re.IGNORECASE)
    ref_soa = {name.lower() for line in log.splitlines() if "exceed" in line.lower()
               for name in rating.findall(line)}
    native_soa = {name.lower() for line in native_warnings if "W-HICUM-SOA" in line
                  for name in rating.findall(line)}
    native_warnings = [s for s in native_warnings if "W-HICUM-SOA" not in s]
    status = "PASS"
    detail = []
    if ref_soa != native_soa or bool(keys) != bool(native_warnings) or any(
        not any(key in warning and "W-MODEL-IGNORED" in warning for warning in native_warnings)
        for key in keys
    ):
        detail.append("warning mismatch")
        status = "FAIL"
    known = set()
    for settings, signals in re.findall(r"^\* known-difference: (.+) \| (.+)$", expected, re.MULTILINE):
        if value in settings.split():
            known.update(signals.split())
    # 44.2 can write a zero-filled column even after rejecting its name.
    unavailable = set(re.findall(r"^Warning: unrecognized variable - (\S+)\s*$", log, re.MULTILINE))
    if unavailable:
        detail.append("unavailable in reference: " + ", ".join(sorted(unavailable)))
    measured = list(compare(plots(raw), plots(target), soi=stem.startswith(("r17_soi_", "r17b_")),
                            unavailable=unavailable))
    if not measured and status == "PASS":
        status = "PARTIAL-REFERENCE"
    for signal, delta, peak, score, n in measured:
        detail.append(f"{signal}={score:.4g}({delta:.4g}/{peak:.4g},{n}pts)")
        if score > 1:
            if signal not in known:
                status = "FAIL"
            elif status == "PASS":
                status = "KNOWN-DIFFERENCE"
    if "simulation(s) aborted" in log:
        detail.append(f"available comparison: {status}")
        status = "PARTIAL-REFERENCE"
    return status, detail


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pattern", default="r15_vbic_*", help="probe template stem glob")
    parser.add_argument("--native-only", action="store_true", help="run each setting without a reference")
    parser.add_argument("--binary", type=Path, default=ROOT.parent / "target/release/sansho")
    parser.add_argument("--out", type=Path, help="directory for native comparison raws")
    args = parser.parse_args()
    round_name = args.pattern[:3]
    out = args.out or (ROOT / "out" / round_name if round_name in {"r16", "r17", "r18"} else OUT)
    counts = Counter()
    templates = sorted((ROOT / "probes").glob(args.pattern + ".cir"))
    if not templates:
        parser.error("no matching probe templates")
    with ExitStack() as stack:
        if args.native_only:
            scratch = ROOT.parent / "target"
            scratch.mkdir(exist_ok=True)
            work = Path(stack.enter_context(tempfile.TemporaryDirectory(dir=scratch, prefix="probe-")))
        else:
            out.mkdir(exist_ok=True, parents=True)
        for template in templates:
            line = next(s for s in template.read_text().splitlines() if s.startswith("* sweep:"))
            for value in line.split()[3:]:
                stem = template.stem + "_" + value
                try:
                    if args.native_only:
                        status, detail = native_check(template, value, work, args.binary.resolve())
                    else:
                        status, detail = check(CACHE / (stem + ".cir"), out, args.binary.resolve(), expand(template, value), value)
                except (OSError, ValueError, KeyError) as error:
                    status, detail = "FAIL", [str(error)]
                print(status, stem, *detail, flush=True)
                counts[status] += 1
    print(dict(counts))
    return int(any(counts[k] for k in ("FAIL", "MISSING-REFERENCE", "STALE-REFERENCE")))


if __name__ == "__main__":
    raise SystemExit(main())
