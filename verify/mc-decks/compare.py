"""Compare independent spx and ngspice Monte Carlo measurement samples.

Provenance: original implementation of the two-sample mean and F variance
checks, using the regularized incomplete beta continued fraction.
"""

import argparse
import json
import math
from pathlib import Path
from statistics import mean, stdev


def beta_cdf(x, a):
    """CDF of Beta(a,a) at x <= 1/2 by modified Lentz continued fraction."""
    if x == 0:
        return 0.0
    if x == 0.5:
        return 0.5
    log_scale = math.lgamma(2 * a) - 2 * math.lgamma(a) + a * math.log(x * (1 - x))
    floor = 1e-300
    c = 1.0
    d = 1 / (1 - 2 * a * x / (a + 1))
    fraction = d
    for k in range(1, 10000):
        t = a + 2 * k
        for coef in (k * (a - k) * x / ((t - 1) * t),
                     -(a + k) * (2 * a + k) * x / (t * (t + 1))):
            d = 1 + coef * d
            c = 1 + coef / c
            d = 1 / (d if abs(d) > floor else floor)
            c = c if abs(c) > floor else floor
            factor = c * d
            fraction *= factor
        if abs(factor - 1) < 2e-14:
            return math.exp(log_scale) * fraction / a
    raise ArithmeticError("incomplete beta did not converge")


def std_bounds(n):
    """Central 99.9% interval for the ratio of two normal sample SDs."""
    lo, hi = 0.0, 0.5
    for _ in range(70):
        x = (lo + hi) / 2
        if beta_cdf(x, (n - 1) / 2) < 0.0005:
            lo = x
        else:
            hi = x
    lower = math.sqrt(hi / (1 - hi))
    return lower, 1 / lower


def compare(batch, reference, model_rtol):
    lines = reference.read_text().splitlines()
    if not lines:
        raise ValueError("empty reference")
    names = lines[0].split()
    rows = [list(map(float, line.split())) for line in lines[1:]]
    runs = batch["runs"]
    n = len(runs)
    if n < 2 or len(rows) != n or any(len(row) != len(names) for row in rows):
        raise ValueError("need one complete reference row per run and at least two runs")
    if any(not run["success"] for run in runs):
        raise ValueError("batch contains failed runs")
    if not names or any([m["name"] for m in run["measures"]] != names for run in runs):
        raise ValueError("ordered measurement names differ")
    lower, upper = std_bounds(n)
    ok = True
    for i, name in enumerate(names):
        x = [r["measures"][i]["value"] for r in runs]
        y = [r[i] for r in rows]
        if any(v is None or not math.isfinite(v) for v in x + y):
            raise ValueError(f"{name}: missing or nonfinite measurement")
        sx, sy = stdev(x), stdev(y)
        mx, my = mean(x), mean(y)
        bound = 4 * math.hypot(sx, sy) / math.sqrt(n) + model_rtol * abs(my)
        if sy == 0:
            raise ValueError(f"{name}: zero reference spread; check oracle resampling")
        ratio = sx / sy
        passed = abs(mx - my) <= bound and lower <= ratio <= upper
        print(f"{name}  {'PASS' if passed else 'FAIL'}  N={n}  means={mx:.8g}/{my:.8g}"
              f"  |delta|<={bound:.4g}  SDs={sx:.8g}/{sy:.8g}"
              f"  ratio={ratio:.5g}  band=[{lower:.5g},{upper:.5g}]")
        ok &= passed
    return ok


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("batch", type=Path)
    parser.add_argument("reference", type=Path)
    parser.add_argument("--model-rtol", type=float, default=0.0)
    args = parser.parse_args()
    if not math.isfinite(args.model_rtol) or args.model_rtol < 0:
        parser.error("model tolerance must be finite and nonnegative")
    try:
        return 0 if compare(json.loads(args.batch.read_text()), args.reference,
                            args.model_rtol) else 1
    except (OSError, KeyError, TypeError, ValueError) as err:
        parser.exit(2, f"{err}\n")


if __name__ == "__main__":
    raise SystemExit(main())
