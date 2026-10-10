"""Clean-data rejection share of sigma clipping and winsorized clipping, for the test
`sigma_clip_and_winsorized_reject_clean_data_at_their_reference_rates`.

A reference implementation of both under the rejection driver, standard library only:

- Sigma clip, k = 2.5, three passes: the first scales a band about the median by
  sigma = b_n * 1.4826 * MAD; each later one by the slope of the kept samples, sorted, against their
  Blom scores among all n samples, about the median of the kept samples.
- Winsorized, k = 3: one Huber estimate of every sample (clamp at 1.5 sigma, sigma = 1.1333926 times
  the standard deviation of the clamped copy, until sigma moves by at most 0.05%), then one clip.

Every sigma is raised to the floor, 0 or the noise model's sigma of 1. A pass that proposes fewer
than three samples is not reached on clean data at these k.

Run: python3 clip_rates.py
"""

import math
import random
from statistics import NormalDist

NORMAL = NormalDist()
# Spread::MAD_CONSISTENCY, n = 2 ..= 20.
CONSISTENCY = [1.1955, 1.4869, 1.3604, 1.2170, 1.1895, 1.1378, 1.1274, 1.1011, 1.0958, 1.0799,
               1.0765, 1.0661, 1.0638, 1.0563, 1.0548, 1.0491, 1.0478, 1.0435, 1.0425]
MAD_TO_SIGMA = 1.482602218505602
WINSORIZED_CORRECTION = 1.133392655462487
MIN_SURVIVORS = 3
SCORES = {}


def consistency(n):
    return CONSISTENCY[n - 2] if n <= 20 else n / (n - 0.8)


def blom(n):
    if n not in SCORES:
        SCORES[n] = [NORMAL.inv_cdf((i + 1 - 0.375) / (n + 0.25)) for i in range(n)]
    return SCORES[n]


def median(values):
    n = len(values)
    return values[n // 2] if n % 2 else 0.5 * (values[n // 2 - 1] + values[n // 2])


def mad_spread(values):
    centre = median(values)
    deviations = sorted(abs(v - centre) for v in values)
    return centre, median(deviations) * MAD_TO_SIGMA * consistency(len(values))


def keep(values, lo, hi, centre, sigma, k):
    low, high = centre - k * sigma, centre + k * sigma
    window = values[lo:hi]
    return lo + sum(1 for v in window if v < low), lo + sum(1 for v in window if v <= high)


def sigma_clip(values, k, floor):
    lo, hi = 0, len(values)
    scores = blom(len(values))
    for index in range(3):
        if hi - lo <= MIN_SURVIVORS:
            break
        window = values[lo:hi]
        if index == 0:
            centre, sigma = mad_spread(window)
        else:
            centre = median(window)
            z = scores[lo:hi]
            z_mean, v_mean = sum(z) / len(z), sum(window) / len(window)
            sigma = (sum((a - z_mean) * (b - v_mean) for a, b in zip(z, window))
                     / sum((a - z_mean) ** 2 for a in z))
        proposal = keep(values, lo, hi, centre, max(sigma, floor), k)
        if proposal == (lo, hi):
            break
        lo, hi = proposal
    return hi - lo


def winsorized(values, k, floor):
    centre, sigma = mad_spread(values)
    sigma = max(sigma, floor)
    clamped = list(values)
    for _ in range(50):
        reach = 1.5 * sigma
        clamped = [min(max(v, centre - reach), centre + reach) for v in clamped]
        mean = sum(clamped) / len(clamped)
        squares = sum((v - mean) ** 2 for v in clamped)
        next_sigma = max(WINSORIZED_CORRECTION * math.sqrt(squares / (len(clamped) - 1)), floor)
        centre = mean
        converged = abs(next_sigma - sigma) <= sigma * 0.0005
        sigma = next_sigma
        if converged:
            break
    lo, hi = keep(values, 0, len(values), centre, sigma, k)
    return hi - lo


def share(method, k, floor, count, trials):
    rejected = []
    for _ in range(trials):
        values = sorted(random.gauss(0.0, 1.0) for _ in range(count))
        rejected.append(count - method(values, k, floor))
    mean = sum(rejected) / trials
    sd = math.sqrt(sum((r - mean) ** 2 for r in rejected) / (trials - 1))
    return mean / count, sd


random.seed(20_251_010)
for name, method, k in (("sigma clip", sigma_clip, 2.5), ("winsorized", winsorized, 3.0)):
    for floor in (0.0, 1.0):
        for count, trials in ((10, 200_000), (20, 100_000), (50, 40_000)):
            rate, sd = share(method, k, floor, count, trials)
            print(f"{name:10s} k={k} floor={floor} n={count:2d} trials={trials}: "
                  f"share {rate:.6f}, per-trial sd {sd:.4f}")
