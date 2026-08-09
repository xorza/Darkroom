use crate::testing::synthetic::camera::*;

fn render_one(psf: PsfModel, size: usize, flux: f32, seeing: f32) -> Vec<f32> {
    let mut pixels = Buffer2::new_filled(size, size, 0.0f32);
    let c = size as f32 / 2.0;
    psf.render(&mut pixels, c, c, flux, seeing);
    pixels.pixels().to_vec()
}

#[test]
fn gaussian_psf_conserves_flux() {
    // A wide canvas captures essentially all of the 4σ-truncated profile.
    let pixels = render_one(PsfModel::Gaussian { fwhm: 4.0 }, 81, 100.0, 1.0);
    let sum: f32 = pixels.iter().sum();
    assert!((sum - 100.0).abs() < 1.0, "sum {sum}");
}

#[test]
fn moffat_psf_conserves_flux() {
    // β=2.5, rendered to 8α: enclosed fraction 1-(1+64)^-1.5 ≈ 0.998.
    let pixels = render_one(
        PsfModel::Moffat {
            fwhm: 4.0,
            beta: 2.5,
        },
        121,
        100.0,
        1.0,
    );
    let sum: f32 = pixels.iter().sum();
    assert!((sum - 100.0).abs() < 2.0, "sum {sum}");
}

#[test]
fn elliptical_psf_conserves_flux_and_is_elongated() {
    let psf = PsfModel::Elliptical {
        fwhm: 4.0,
        eccentricity: 0.6,
        angle: 0.0,
    };
    let size = 81;
    let pixels = render_one(psf, size, 100.0, 1.0);
    let sum: f32 = pixels.iter().sum();
    assert!((sum - 100.0).abs() < 1.5, "sum {sum}");
    // Major axis horizontal (angle 0): more flux 6px right of center than 6px below.
    let c = size / 2;
    let horiz = pixels[c * size + (c + 6)];
    let vert = pixels[(c + 6) * size + c];
    assert!(horiz > vert, "horiz {horiz} vert {vert}");
}

#[test]
fn seeing_scale_widens_and_lowers_peak() {
    let psf = PsfModel::Gaussian { fwhm: 4.0 };
    let size = 81;
    let c = size / 2;
    let sharp = render_one(psf, size, 100.0, 1.0);
    let blurred = render_one(psf, size, 100.0, 2.0);
    // Same flux spread over a wider PSF → lower peak, but flux still conserved.
    assert!(blurred[c * size + c] < sharp[c * size + c]);
    let (s_sum, b_sum): (f32, f32) = (sharp.iter().sum(), blurred.iter().sum());
    assert!((s_sum - b_sum).abs() < 2.0);
}

#[test]
fn flat_field_default_is_unit() {
    let flat = FlatField::default().render(Size2us::new(8, 8), 0);
    assert!(flat.iter().all(|&f| (f - 1.0).abs() < 1e-6));
}

#[test]
fn flat_field_channel_gain_and_vignette() {
    // Per-channel gain scales the whole map.
    let ff = FlatField {
        vignette: None,
        channel_gain: [0.9, 1.0, 1.1],
    };
    assert!((ff.render(Size2us::new(4, 4), 2)[0] - 1.1).abs() < 1e-6);

    // Vignette: center brighter than corner.
    let vig = FlatField {
        vignette: Some((1.0, 0.5, 2.0)),
        channel_gain: [1.0; 3],
    };
    let map = vig.render(Size2us::new(64, 64), 0);
    let center = map[32 * 64 + 32];
    let corner = map[0];
    assert!(center > corner, "center {center} corner {corner}");
    assert!((center - 1.0).abs() < 0.05);
}

#[test]
fn psf_fwhm_accessor() {
    assert_eq!(PsfModel::Gaussian { fwhm: 3.5 }.fwhm(), 3.5);
    assert_eq!(
        PsfModel::Moffat {
            fwhm: 4.0,
            beta: 3.0
        }
        .fwhm(),
        4.0
    );
}

#[test]
fn moffat_has_heavier_wings_than_gaussian() {
    // Equal FWHM and flux: the Gaussian concentrates flux in the core, the Moffat spreads it
    // into atmospheric wings. At r=10 px the Gaussian (4σ-truncated) is gone; the Moffat is not.
    let size = 121;
    let c = size / 2;
    let g = render_one(PsfModel::Gaussian { fwhm: 4.0 }, size, 100.0, 1.0);
    let m = render_one(
        PsfModel::Moffat {
            fwhm: 4.0,
            beta: 2.5,
        },
        size,
        100.0,
        1.0,
    );
    assert!(
        g[c * size + c] > m[c * size + c],
        "Gaussian core {} should exceed Moffat core {}",
        g[c * size + c],
        m[c * size + c]
    );
    assert!(
        m[c * size + (c + 10)] > 0.005,
        "Moffat should carry real wing flux at r=10, got {}",
        m[c * size + (c + 10)]
    );
    assert!(
        g[c * size + (c + 10)] < 1e-4,
        "Gaussian wings should be negligible at r=10, got {}",
        g[c * size + (c + 10)]
    );
}

#[test]
fn moffat_beta_controls_wing_weight() {
    // Lower beta → heavier wings at fixed FWHM.
    let size = 121;
    let c = size / 2;
    let wing = |beta: f32| {
        render_one(PsfModel::Moffat { fwhm: 4.0, beta }, size, 100.0, 1.0)[c * size + (c + 10)]
    };
    let heavy = wing(2.0);
    let light = wing(6.0);
    assert!(
        heavy > light * 2.0,
        "lower beta should have heavier wings: β2 {heavy:.4} vs β6 {light:.4}"
    );
}

#[test]
fn eccentricity_controls_elongation() {
    // Larger eccentricity → more elongated profile (higher horiz/vert ratio at angle 0).
    let size = 81;
    let c = size / 2;
    let ratio = |e: f32| {
        let p = render_one(
            PsfModel::Elliptical {
                fwhm: 4.0,
                eccentricity: e,
                angle: 0.0,
            },
            size,
            100.0,
            1.0,
        );
        p[c * size + (c + 5)] / p[(c + 5) * size + c]
    };
    let low = ratio(0.3);
    let high = ratio(0.7);
    assert!(
        high > low && low > 1.0,
        "elongation must grow with eccentricity: e0.3 {low:.2}, e0.7 {high:.2}"
    );
}
