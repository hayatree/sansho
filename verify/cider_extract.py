"""Recover effective silicon parameters from the cached R20 terminal measurements.

Provenance: original inverse measurements using the Boltzmann, Einstein and
finite-volume equations described in docs/cider.md. Run from the checkout.
"""

import argparse
import math
from pathlib import Path
import re
import tempfile

from compare_vbic import plots, simulate


ROOT = Path(__file__).resolve().parent
T0 = 300.15
EPS0 = 8.8541878128e-14
# Units conversion for the forward simulator. The slope experiment varies U
# by changing its temperature and independently holding the specified ni fixed.
K_OVER_Q = 8.617087e-5
NC, NV = 2.8e19, 1.04e19


def columns(path):
    (result,) = plots(path).values()
    return result


def pair(measure, x, y):
    """Invert two independent positive observables using relative perturbations."""
    step = 1e-4
    for _ in range(12):
        r, s = measure(x, y)
        if max(abs(r), abs(s)) < 1e-9:
            return x, y
        rx, sx = measure(x * (1 + step), y)
        ry, sy = measure(x, y * (1 + step))
        a, b = (rx - r) / step, (ry - r) / step
        c, d = (sx - s) / step, (sy - s) / step
        det = a * d - b * c
        x *= 1 + (b * s - d * r) / det
        y *= 1 + (c * r - a * s) / det
    raise ValueError("two-observable inversion did not converge")


def measure(binary, work, source, ni, eps=11.7, electrons=1340, holes=500, kelvin=T0):
    # Choose explicit DOS and calculate the equivalent gap. This removes the
    # native intrinsic-density and mobility defaults from the inverse problem.
    gap = 2 * K_OVER_Q * kelvin * math.log(
        math.sqrt(NC * NV) * (kelvin / T0) ** 1.5 / ni
    )
    source = re.sub(r"^\.temp .*\n", "", source, flags=re.MULTILINE)
    source = source.replace(".options itl1", f".temp {kelvin - 273.15}\n.options itl1")
    source = source.replace(
        "+ material number=1 silicon",
        f"+ material number=1 silicon nc={NC} nv={NV} eg={gap} deg.dt=0 permittivity={eps * EPS0}",
    )
    source = source.replace(
        "+ end", f"+ mobility mat=1 electron mumax={electrons}\n"
        f"+ mobility mat=1 hole mumax={holes}\n+ end"
    )
    deck, raw = work / "explicit.cir", work / "native.raw"
    deck.write_text(source)
    result = simulate(deck, raw, binary)
    if result.returncode:
        raise ValueError(result.stderr)
    return columns(raw)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT.parent / "target/release/sansho")
    args = parser.parse_args()
    cache = ROOT / "out"
    cv = (ROOT / "decks/r20_diode_cv.cir").read_text()
    cv_ref = columns(cache / "r20_diode_cv.ref.raw")
    srh_deck = cache / "probes/r20_srh_1.cir"
    srh, srh_ref = srh_deck.read_text(), columns(srh_deck.with_suffix(".raw"))
    hot_deck = cache / "probes/r20_tempmob_85.cir"
    hot, hot_ref = hot_deck.read_text(), columns(hot_deck.with_suffix(".raw"))

    with tempfile.TemporaryDirectory(dir=ROOT.parent / "target", prefix="r20b-extract-") as scratch:
        def replay(source, ni, **params):
            return measure(args.binary.resolve(), Path(scratch), source, ni, **params)

        def capacitances(ni, eps):
            sim = replay(cv, ni, eps=eps)
            return [sim[s][0].imag / cv_ref[s][0].imag - 1 for s in ("i(v1)", "i(v3)")]

        ni, eps = pair(capacitances, 1.1e10, 11.7)
        print(f"0 and -0.5 V capacitance: ni={ni:.5g} cm^-3, epsilon_r={eps:.6f}")
        middle = replay(cv, ni, eps=eps)["i(v2)"][0].imag / cv_ref["i(v2)"][0].imag
        print(f"withheld -0.2 V capacitance ratio: {middle:.9f}")

        def injection(electrons, holes):
            sim = replay(srh, ni, eps=eps, electrons=electrons, holes=holes)
            return [sim[s][-1].real / srh_ref[s][-1].real - 1 for s in ("i(vref)", "i(vtest)")]

        electrons, holes = pair(injection, 1350, 480)
        print(f"0.68 V, SRH off/on: electron/hole mobility={electrons:.2f}/{holes:.2f} cm^2/(V s)")

        def slope(kelvin, density):
            sim = replay(srh, density, kelvin=kelvin)
            return [sim["i(vref)"][i].real / srh_ref["i(vref)"][i].real - 1 for i in (7, 9)]

        kelvin, density = pair(slope, T0, ni)
        print(f"0.48 and 0.56 V ideal branch: k/q={K_OVER_Q * kelvin / T0:.8g} V/K, ni={density:.5g}")

        def hot_density_ratio(density):
            sim = replay(hot, density, kelvin=358.15)
            return sim["i(vref)"][7].real / hot_ref["i(vref)"][7].real

        low, high = 3e11, 9e11
        if not hot_density_ratio(low) < 1 < hot_density_ratio(high):
            raise ValueError("hot density outside extraction interval")
        for _ in range(30):
            midpoint = (low + high) / 2
            if hot_density_ratio(midpoint) < 1:
                low = midpoint
            else:
                high = midpoint
        print(f"85 C, 0.48 V, TempMob off: ni={(low + high) / 2:.5g} cm^-3")


if __name__ == "__main__":
    main()
