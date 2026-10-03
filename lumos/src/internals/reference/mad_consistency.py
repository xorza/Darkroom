"""Small-sample consistency factors of the MAD, for `Spread::MAD_CONSISTENCY`.

b_n makes b_n * 1.4826 * MAD an unbiased estimate of sigma for n Gaussian samples. The MAD here is
the one `Spread` computes: the median is the mean of the two middle values for an even n, and so is
the median of the absolute deviations. Croux & Rousseeuw (1992) tabulate b_n for n <= 9 and give
n / (n - 0.8) above; their table does not fit the midpoint median at even n (n = 6: 1.200 against
1.191), so the factors are measured again for this estimator.

Run: python3 mad_consistency.py
"""

import numpy as np

MAD_TO_SIGMA = 1.482602218505602
TRIALS = 10_000_000
CHUNK = 250_000


def factor(n, rng):
    total = 0.0
    done = 0
    while done < TRIALS:
        count = min(CHUNK, TRIALS - done)
        x = np.sort(rng.standard_normal((count, n)), axis=1)
        lo, hi = (n - 1) // 2, n // 2
        median = 0.5 * (x[:, lo : lo + 1] + x[:, hi : hi + 1])
        deviations = np.sort(np.abs(x - median), axis=1)
        mad = 0.5 * (deviations[:, lo] + deviations[:, hi])
        total += mad.sum()
        done += count
    return 1.0 / (MAD_TO_SIGMA * total / TRIALS)


def main():
    rng = np.random.default_rng(20261003)
    for n in range(2, 21):
        print(f"{n}: {factor(n, rng):.4f}")


if __name__ == "__main__":
    main()
