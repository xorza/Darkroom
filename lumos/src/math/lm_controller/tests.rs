use crate::math::lm_controller::{LmController, LmProblem, NormalEquations};

/// Residuals `r` and their Jacobian rows `J` (of the model, so `r = data − model` and the gradient
/// is `Jᵀr`), gathered into the normal equations.
fn equations<const N: usize>(rows: impl Iterator<Item = (f64, [f64; N])>) -> NormalEquations<N> {
    let mut equations = NormalEquations {
        hessian: [[0.0; N]; N],
        gradient: [0.0; N],
        chi2: 0.0,
    };
    for (residual, jacobian) in rows {
        for i in 0..N {
            for j in i..N {
                equations.hessian[i][j] += jacobian[i] * jacobian[j];
            }
            equations.gradient[i] += jacobian[i] * residual;
        }
        equations.chi2 += residual * residual;
    }
    equations.mirror_lower_triangle();
    equations
}

/// `y = a + b·x`.
#[derive(Debug)]
struct Line {
    points: Vec<(f64, f64)>,
}

impl LmProblem<2> for Line {
    fn normal_equations(&self, params: &[f64; 2]) -> NormalEquations<2> {
        equations(
            self.points
                .iter()
                .map(|&(x, y)| (y - params[0] - params[1] * x, [1.0, x])),
        )
    }

    fn chi2(&self, params: &[f64; 2]) -> f64 {
        self.normal_equations(params).chi2
    }

    fn constrain(&self, _params: &mut [f64; 2]) {}
}

/// `y = a·e^(−b·x)`.
#[derive(Debug)]
struct Decay {
    points: Vec<(f64, f64)>,
}

impl LmProblem<2> for Decay {
    fn normal_equations(&self, params: &[f64; 2]) -> NormalEquations<2> {
        let [a, b] = *params;
        equations(self.points.iter().map(|&(x, y)| {
            let e = (-b * x).exp();
            (y - a * e, [e, -a * x * e])
        }))
    }

    fn chi2(&self, params: &[f64; 2]) -> f64 {
        self.normal_equations(params).chi2
    }

    fn constrain(&self, _params: &mut [f64; 2]) {}
}

/// Rosenbrock's function as least squares: `r₁ = 10·(x₂ − x₁²)`, `r₂ = 1 − x₁`.
#[derive(Debug)]
struct Rosenbrock;

impl LmProblem<2> for Rosenbrock {
    fn normal_equations(&self, params: &[f64; 2]) -> NormalEquations<2> {
        let [x1, x2] = *params;
        equations(
            [
                (10.0 * (x2 - x1 * x1), [20.0 * x1, -10.0]),
                (1.0 - x1, [1.0, 0.0]),
            ]
            .into_iter(),
        )
    }

    fn chi2(&self, params: &[f64; 2]) -> f64 {
        self.normal_equations(params).chi2
    }

    fn constrain(&self, _params: &mut [f64; 2]) {}
}

/// The line through (0, 1), (1, 3), (2, 5), (3, 7) is `1 + 2x` exactly, and the fit lands on it
/// to rounding from (0, 0). `JᵀJ = [[4, 6], [6, 14]]`, of determinant 20, so `(JᵀJ)⁻¹` has the
/// diagonal 14/20 and 4/20.
#[test]
fn a_line_fits_exactly_with_its_inverse_hessian() {
    let line = Line {
        points: vec![(0.0, 1.0), (1.0, 3.0), (2.0, 5.0), (3.0, 7.0)],
    };
    let fit = LmController::STANDARD.fit(&line, [0.0, 0.0]).unwrap();
    assert!((fit.params[0] - 1.0).abs() < 1e-12, "{:?}", fit.params);
    assert!((fit.params[1] - 2.0).abs() < 1e-12, "{:?}", fit.params);
    let inverse = fit.inverse_hessian_diagonal.unwrap();
    assert!((inverse[0] - 0.7).abs() < 1e-12, "{inverse:?}");
    assert!((inverse[1] - 0.2).abs() < 1e-12, "{inverse:?}");
}

/// Rosenbrock's valley from Madsen, Nielsen and Tingleff's start (−1.2, 1) reaches its minimum at
/// (1, 1).
#[test]
fn rosenbrock_reaches_its_minimum() {
    let fit = LmController::STANDARD
        .fit(&Rosenbrock, [-1.2, 1.0])
        .unwrap();
    assert!((fit.params[0] - 1.0).abs() < 1e-8, "{:?}", fit.params);
    assert!((fit.params[1] - 1.0).abs() < 1e-8, "{:?}", fit.params);
}

/// The fit does not depend on the data's scale. `2·e^(−x/2)` at x = 0..9 from
/// (1, 1), and the same data and start scaled by 2⁻⁴⁰: every Hessian entry, gradient and χ² scales
/// by a power of two, the scaled system and both stop tests not at all, so the decay rate is the
/// same bit for bit and the amplitude scaled exactly. An absolute pivot of 1e-15 called the scaled
/// fit singular.
#[test]
fn a_fit_does_not_depend_on_the_data_scale() {
    let scale = 2.0f64.powi(-40);
    let decay = |amplitude: f64| Decay {
        points: (0..10)
            .map(|x| (f64::from(x), amplitude * (-0.5 * f64::from(x)).exp()))
            .collect(),
    };
    let native = LmController::STANDARD.fit(&decay(2.0), [1.0, 1.0]).unwrap();
    let scaled = LmController::STANDARD
        .fit(&decay(2.0 * scale), [scale, 1.0])
        .unwrap();
    assert!((native.params[0] - 2.0).abs() < 1e-12);
    assert!((native.params[1] - 0.5).abs() < 1e-12);
    assert_eq!(scaled.params[1].to_bits(), native.params[1].to_bits());
    assert_eq!(
        scaled.params[0].to_bits(),
        (native.params[0] * scale).to_bits()
    );
    assert_eq!(scaled.iterations, native.iterations);
}

/// A parameter the data do not constrain has no curvature, so the scaled system cannot be built
/// and the fit reports no solution: `y = a + 0·b`.
#[test]
fn an_unconstrained_parameter_is_no_fit() {
    #[derive(Debug)]
    struct Flat;
    impl LmProblem<2> for Flat {
        fn normal_equations(&self, params: &[f64; 2]) -> NormalEquations<2> {
            equations([(1.0 - params[0], [1.0, 0.0])].into_iter())
        }
        fn chi2(&self, params: &[f64; 2]) -> f64 {
            self.normal_equations(params).chi2
        }
        fn constrain(&self, _params: &mut [f64; 2]) {}
    }
    assert!(LmController::STANDARD.fit(&Flat, [0.0, 0.0]).is_none());
}
