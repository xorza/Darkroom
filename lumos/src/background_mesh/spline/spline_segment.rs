//! [`SplineSegment`]: one interval of a natural cubic spline, the one evaluator the background
//! mesh and its vector kernel share.

/// One interval of a natural cubic spline between two nodes, ready to evaluate.
///
/// Standard cubic spline formula (Numerical Recipes, SEP/SExtractor), at parameter `t` from the
/// left node (0) to the right one (1):
///   `f(t) = (1−t)·f0 + t·f1 + ((1−t)³ − (1−t))·a + (t³ − t)·b`, with `a, b = h²/6 · d″`.
/// Factored — `(ct³ − ct) = −t·ct·(2 − t)` and `(t³ − t) = −t·ct·(1 + t)` — with its linear part as a
/// rise from `f0`, so equal nodes with no curvature give `f0` exactly, at any `t`:
///   `f(t) = f0 + t·(f1 − f0) − t·(1 − t)·((2 − t)·a + (1 + t)·b)`.
/// Outside [0, 1] the interval's cubic runs on past its nodes, which is how the end intervals
/// extrapolate.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SplineSegment {
    /// Value at the left node (t = 0).
    pub(crate) f0: f32,
    /// Value at the right node (t = 1).
    pub(crate) f1: f32,
    /// `h²/6 · d″` at the left node.
    pub(crate) a: f32,
    /// `h²/6 · d″` at the right node.
    pub(crate) b: f32,
}

impl SplineSegment {
    /// The interval of width `h` between values `f0`, `f1` with second derivatives `d0`, `d1`; a
    /// zero-width interval is the constant `f0`.
    pub(crate) const fn new(f0: f32, f1: f32, d0: f32, d1: f32, h: f32) -> Self {
        if h <= 0.0 {
            return Self {
                f0,
                f1: f0,
                a: 0.0,
                b: 0.0,
            };
        }
        let h2_6 = h * h / 6.0;
        Self {
            f0,
            f1,
            a: h2_6 * d0,
            b: h2_6 * d1,
        }
    }

    #[inline]
    pub(crate) fn eval(self, t: f32) -> f32 {
        let ct = 1.0 - t;
        let t_ct = t * ct;
        self.f0 + t * (self.f1 - self.f0) - t_ct * ((2.0 - t) * self.a + (1.0 + t) * self.b)
    }
}
