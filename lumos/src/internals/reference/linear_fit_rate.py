"""Clean-data rejection share of the linear-fit clip, for the test
`linear_fit_rejects_clean_data_at_a_rate_that_falls_with_the_count`.

A reference implementation of `LinearFitClipConfig` under the rejection driver: a sigma clip about
the median with sigma = b_n * 1.4826 * MAD, then up to three passes that fit the kept samples, sorted,
against their Blom scores among all n samples and keep those within k sigma of the intercept. A pass
that proposes fewer than three samples is not reached on clean data at k = 3.

Run: python3 linear_fit_rate.py
"""

import math

import numpy as np
from scipy.stats import norm

# Spread::MAD_CONSISTENCY, n = 2 ..= 20.
CONSISTENCY = [1.1955, 1.4869, 1.3604, 1.2170, 1.1895, 1.1378, 1.1274, 1.1011, 1.0958, 1.0799,
               1.0765, 1.0661, 1.0638, 1.0563, 1.0548, 1.0491, 1.0478, 1.0435, 1.0425]
MAD_TO_SIGMA = 1.482602218505602
K = 3.0
FITTED_PASSES = 3
MIN_SURVIVORS = 3


def consistency(n):
    return CONSISTENCY[n - 2] if n <= 20 else n / (n - 0.8)


def median(sorted_values):
    n = len(sorted_values)
    return sorted_values[n // 2] if n % 2 else 0.5 * (sorted_values[n // 2 - 1] + sorted_values[n // 2])


def keep(sorted_values, lo, hi, centre, sigma):
    window = sorted_values[lo:hi]
    return (lo + int(np.searchsorted(window, centre - K * sigma, "left")),
            lo + int(np.searchsorted(window, centre + K * sigma, "right")))


def rejected(samples, scores):
    s = np.sort(samples)
    n = len(s)
    centre = median(s)
    sigma = MAD_TO_SIGMA * median(np.sort(np.abs(s - centre))) * consistency(n)
    lo, hi = keep(s, 0, n, centre, sigma)
    for _ in range(FITTED_PASSES):
        if hi - lo <= MIN_SURVIVORS:
            break
        z, y = scores[lo:hi], s[lo:hi]
        slope = ((z - z.mean()) * (y - y.mean())).sum() / ((z - z.mean()) ** 2).sum()
        new_lo, new_hi = keep(s, lo, hi, y.mean() - slope * z.mean(), slope)
        if (new_lo, new_hi) == (lo, hi):
            break
        lo, hi = new_lo, new_hi
    return n - (hi - lo)


def main():
    rng = np.random.default_rng(99)
    for n, trials in [(20, 1_000_000), (50, 400_000), (200, 100_000)]:
        scores = norm.ppf((np.arange(1, n + 1) - 0.375) / (n + 0.25))
        counts = np.array([rejected(rng.standard_normal(n), scores) for _ in range(trials)], float)
        print(f"{n}: share {counts.mean() / n:.6f}, per-trial sd {counts.std():.3f}, "
              f"standard error {counts.std() / math.sqrt(trials) / n:.2e}")


if __name__ == "__main__":
    main()
