# Generalized Hyperbolic Stretch (GHS)

The math behind the `Ghs` stretch method, from the ghsastro.co.uk reference (Cranfield, Knagg-Baugh,
Richard — the GHS / Siril authors). A *designer* tone curve: unlike the auto STF/asinh stretches, GHS
gives explicit control over **where** in the tonal range contrast is added and how sharply, while
protecting shadows and highlights. `b ≈ −1.4` reproduces an arcsinh stretch; the family generalizes
it.

See `docs/image-stretching.md` for the STF/asinh curves this sits beside.

## Parameters (5)

| Param | Range | Meaning |
|---|---|---|
| **D** | ≥ 0 | Stretch **strength** (the hyperbolic coefficient). `D = 0` is the identity. (GHS UIs sometimes expose the slider as `ln(D+1)`, i.e. `D = e^{sf} − 1`.) |
| **b** | any (UI −5…15) | **Local intensity / curve family.** `b = 0` exponential, `b < 0` logarithmic-like (`b ≈ −1.4` ≈ asinh), `b > 0` hyperbolic. Larger `|b|` concentrates contrast near SP. |
| **SP** | `[0, 1]` | **Symmetry point** — the value where the stretch is most intense. Set near the faint-signal level. |
| **LP** | `[0, SP]` | **Shadow protection** — below LP the curve is linear (preserves shadow detail / noise floor). |
| **HP** | `[SP, 1]` | **Highlight protection** — above HP the curve is linear (prevents star-core bloat). |

## The transform

### Base hyperbolic function `T` and its derivative `T'`

Defined for `u ≥ 0`, with `T(0) = 0` in every case (this is what makes the mirrored curve continuous
at SP). Selected on `b`:

| `b` | `T(u)` | `T'(u)` |
|---|---|---|
| `b = −1` | `ln(1 + D·u)` | `D / (1 + D·u)` |
| `b < 0, b ≠ −1` | `(1 − (1 − b·D·u)^{(b+1)/b}) / (D·(b+1))` | `(1 − b·D·u)^{1/b}` |
| `b = 0` | `1 − e^{−D·u}` | `D·e^{−D·u}` |
| `b > 0` | `1 − (1 + b·D·u)^{−1/b}` | `D·(1 + b·D·u)^{−(1+b)/b}` |

(`b = −1` and `b = 0` are genuine special cases — the general forms divide by `b` or `b+1`. `b = 1`
is *not* special: the `b > 0` form already gives `1 − (1 + D·u)^{−1}`. For `b < 0` the base `1 − b·D·u
= 1 + |b|·D·u > 0`, so all powers are of positive bases.)

### Piecewise curve (mirrored about SP, linear tails)

```
T1(x) = T'(SP−LP)·(x − LP) − T(SP−LP)      for  0  ≤ x < LP     (linear, tangent to T2 at LP)
T2(x) = −T(SP − x)                          for  LP ≤ x < SP     (mirror of the base below SP)
T3(x) =  T(x − SP)                          for  SP ≤ x < HP     (base above SP)
T4(x) = T'(HP−SP)·(x − HP) + T(HP−SP)       for  HP ≤ x ≤ 1      (linear, tangent to T3 at HP)
```

The mirror `T2 = −T(SP−x)` plus `T3 = T(x−SP)` makes the curve **antisymmetric about SP** (equal
contrast added below and above it). Because `T(0) = 0`, `T2` and `T3` meet at SP; the linear tails are
tangents at LP/HP, so the whole curve is **C¹-continuous and monotonic increasing**.

### Normalization → `[0,1]`

Map the raw piecewise value through `(0,0)`–`(1,1)`:
```
f(x) = (T_i(x) − t0) / (t1 − t0)            clamped to [0, 1]
   t0 = T1(0) = −LP·T'(SP−LP) − T(SP−LP)    (raw output at x=0)
   t1 = T4(1) = (1−HP)·T'(HP−SP) + T(HP−SP) (raw output at x=1)
```
Only **four** base evaluations are needed up front — `T` and `T'` at `SP−LP` and at `HP−SP` — giving
`t0`, `t1`, and the two linear-tail slopes; the rest is two base evaluations per pixel.

## Properties

- **C¹ and monotonic** → no banding, no inversion.
- **No added noise / repeatable** — the GHS authors note the transforms "will not add noise and will
  not create non-repairable artifacts with multiple applications" (it's a smooth monotone remap).
- **Generalizes asinh** — `b ≈ −1.4` closely matches `asinh`; `b = 0` is a pure exponential; `b > 0`
  pushes a hyperbolic (sharper-shouldered) curve. One knob spans the useful stretch family.
- **Shadow/highlight protection** is built in via the linear LP/HP tails — the reason GHS bloats stars
  less than a global MTF/asinh at equal shadow lift.

## Placement & usage in lumos

A **display-domain** curve, applied by `stretch()` exactly like STF/asinh. It's an **explicit**
method (the user designs the curve), so it needs no image statistics — handled alongside
`Asinh { beta }`, not the auto methods.

**Linear-input caveat:** lumos applies the stretch to *linear* data (background ≈ 0, stars > 1), so SP
is in linear-input units — for a raw linear stack the faint signal sits at small values, so SP is
small (and stars above HP/1 ride the linear tail and clamp to white via the color-preserving cap).
GHS shines most as a *designer* follow-up; an auto-SP-from-background variant (deriving SP from the
median like `AutoAsinh` does for `β`) is the natural future extension.

Color handling is shared: `ColorPreserving` runs GHS on the intensity `I=(r+g+b)/3` and scales
channels by `f(I)/I` (hue preserved); `PerChannel` runs it independently per channel.

## References

- **ghsastro.co.uk** — GeneralizedHyperbolicStretch reference (the equations above):
  `ghsastro.co.uk/doc/tools/GeneralizedHyperbolicStretch/`
- **Siril** — GHS tutorial & docs: `siril.org/tutorials/ghs/`,
  `siril.readthedocs.io/en/stable/processing/stretching.html`
- GHS is by Mike Cranfield, Adrian Knagg-Baugh, and Cyril Richard (PixInsight script + Siril).
