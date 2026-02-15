//! Star field generator with comprehensive configuration.
//!
//! Generates synthetic star fields with various properties for testing
//! the star detection algorithm under different conditions.

use super::artifacts::{BayerPattern, add_bayer_pattern, add_cosmic_rays};
use super::backgrounds::{
    NebulaConfig, add_gradient_background, add_nebula_background, add_uniform_background,
    add_vignette_background,
};
use super::star_profiles::{
    fwhm_to_moffat_alpha, fwhm_to_sigma, render_elliptical_star, render_gaussian_star,
    render_moffat_star, render_saturated_star,
};
use crate::common::Buffer2;
use glam::DVec2;
use std::f32::consts::PI;

/// Ground truth star information for validation.
#[derive(Debug, Clone)]
pub struct GroundTruthStar {
    /// Position (sub-pixel)
    pub pos: DVec2,
    /// Flux (integrated brightness)
    pub flux: f32,
    /// FWHM in pixels
    pub fwhm: f32,
    /// Eccentricity (0 = circular)
    pub eccentricity: f32,
    /// Whether this star is saturated
    pub is_saturated: bool,
    /// Orientation angle (for elliptical stars)
    pub angle: f32,
}

/// Type of star crowding distribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrowdingType {
    /// Uniformly distributed stars
    Uniform,
    /// Dense cluster in center with sparse halo
    Clustered,
    /// Density gradient across the field
    Gradient,
}

/// Type of star elongation (tracking errors).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElongationType {
    /// Circular stars
    None,
    /// All stars have same elongation and angle
    Uniform,
    /// Random elongation and angles
    Varying,
    /// Elongation increases with distance from field center (field rotation)
    FieldRotation,
}

/// Configuration for star field generation.
#[derive(Debug, Clone)]
pub struct StarFieldConfig {
    /// Image width in pixels
    pub width: usize,
    /// Image height in pixels
    pub height: usize,
    /// Number of stars to generate
    pub num_stars: usize,
    /// FWHM range (min, max) in pixels
    pub fwhm_range: (f32, f32),
    /// Magnitude range (min=brightest, max=faintest)
    /// Converted to flux using flux = 10^((mag_zero - mag) / 2.5)
    pub magnitude_range: (f32, f32),
    /// Zero-point magnitude (flux = 1.0 at this magnitude)
    pub mag_zero_point: f32,
    /// Background level (0.0-1.0)
    pub background_level: f32,
    /// Gaussian noise sigma
    pub noise_sigma: f32,
    /// Star crowding type
    pub crowding: CrowdingType,
    /// Star elongation type
    pub elongation: ElongationType,
    /// For Uniform elongation: eccentricity and angle
    pub uniform_elongation: (f32, f32),
    /// For Varying elongation: eccentricity range
    pub eccentricity_range: (f32, f32),
    /// Fraction of stars that are saturated (0.0-1.0)
    pub saturation_fraction: f32,
    /// Saturation level
    pub saturation_level: f32,
    /// Number of cosmic ray hits to add
    pub cosmic_ray_count: usize,
    /// Whether to add Bayer pattern artifacts
    pub add_bayer: bool,
    /// Bayer pattern strength
    pub bayer_strength: f32,
    /// Whether to use Moffat profile instead of Gaussian
    pub use_moffat: bool,
    /// Moffat beta parameter (if using Moffat)
    pub moffat_beta: f32,
    /// Background gradient (start_level, end_level, angle)
    pub gradient: Option<(f32, f32, f32)>,
    /// Vignette parameters (center_level, edge_level, falloff)
    pub vignette: Option<(f32, f32, f32)>,
    /// Nebula configuration
    pub nebula: Option<NebulaConfig>,
    /// Random seed for reproducibility
    pub seed: u64,
    /// Edge margin - stars won't be placed within this distance of edges
    pub edge_margin: usize,
}

impl Default for StarFieldConfig {
    fn default() -> Self {
        Self {
            width: 256,
            height: 256,
            num_stars: 30,
            fwhm_range: (3.0, 4.0),
            magnitude_range: (10.0, 14.0),
            mag_zero_point: 14.5,
            background_level: 0.1,
            noise_sigma: 0.02,
            crowding: CrowdingType::Uniform,
            elongation: ElongationType::None,
            uniform_elongation: (0.4, 0.0),
            eccentricity_range: (0.2, 0.6),
            saturation_fraction: 0.0,
            saturation_level: 0.95,
            cosmic_ray_count: 0,
            add_bayer: false,
            bayer_strength: 0.05,
            use_moffat: false,
            moffat_beta: 2.5,
            gradient: None,
            vignette: None,
            nebula: None,
            seed: 42,
            edge_margin: 15,
        }
    }
}

/// Generate a synthetic star field.
///
/// Returns the pixel data and ground truth star information.
pub fn generate_star_field(config: &StarFieldConfig) -> (Buffer2<f32>, Vec<GroundTruthStar>) {
    let mut pixels = vec![0.0f32; config.width * config.height];
    let mut ground_truth = Vec::with_capacity(config.num_stars);

    let mut rng = crate::testing::TestRng::new(config.seed);

    // Add background
    match (&config.gradient, &config.vignette) {
        (Some((start, end, angle)), _) => {
            add_gradient_background(
                &mut pixels,
                config.width,
                config.height,
                *start,
                *end,
                *angle,
            );
        }
        (_, Some((center, edge, falloff))) => {
            add_vignette_background(
                &mut pixels,
                config.width,
                config.height,
                *center,
                *edge,
                *falloff,
            );
        }
        _ => {
            add_uniform_background(&mut pixels, config.background_level);
        }
    }

    // Add nebula if configured
    if let Some(ref nebula_config) = config.nebula {
        add_nebula_background(&mut pixels, config.width, config.height, nebula_config);
    }

    // Generate star positions based on crowding type
    let positions = generate_star_positions(config, &mut rng);

    // Generate stars
    for (x, y) in positions.iter() {
        // Random magnitude -> flux
        let mag = config.magnitude_range.0
            + rng.next_f32() * (config.magnitude_range.1 - config.magnitude_range.0);
        let flux = 10.0f32.powf((config.mag_zero_point - mag) / 2.5);

        // Random FWHM
        let fwhm =
            config.fwhm_range.0 + rng.next_f32() * (config.fwhm_range.1 - config.fwhm_range.0);

        // Determine if saturated
        let is_saturated =
            config.saturation_fraction > 0.0 && rng.next_f32() < config.saturation_fraction;

        // Determine elongation
        let (eccentricity, angle) = match config.elongation {
            ElongationType::None => (0.0, 0.0),
            ElongationType::Uniform => config.uniform_elongation,
            ElongationType::Varying => {
                let ecc = config.eccentricity_range.0
                    + rng.next_f32() * (config.eccentricity_range.1 - config.eccentricity_range.0);
                let ang = rng.next_f32() * PI;
                (ecc, ang)
            }
            ElongationType::FieldRotation => {
                let cx = config.width as f32 / 2.0;
                let cy = config.height as f32 / 2.0;
                let dx = *x - cx;
                let dy = *y - cy;
                let dist = (dx * dx + dy * dy).sqrt();
                let max_dist = (cx * cx + cy * cy).sqrt();

                // Eccentricity increases with distance from center
                let ecc = (dist / max_dist) * 0.5;
                // Angle is tangent to circle around center
                let ang = dy.atan2(dx) + PI / 2.0;
                (ecc, ang)
            }
        };

        // Render star
        let sigma = fwhm_to_sigma(fwhm);

        if is_saturated {
            // Render saturated star (higher amplitude, clipped)
            let sat_amplitude = flux * 2.0;
            render_saturated_star(
                &mut pixels,
                config.width,
                *x,
                *y,
                sigma,
                sat_amplitude,
                config.saturation_level,
            );
        } else if eccentricity > 0.01 {
            // Render elliptical star
            let sigma_major = sigma / (1.0 - eccentricity * eccentricity).sqrt().sqrt();
            let sigma_minor = sigma_major * (1.0 - eccentricity * eccentricity).sqrt();
            let amplitude = flux / (2.0 * PI * sigma_major * sigma_minor);

            render_elliptical_star(
                &mut pixels,
                config.width,
                *x,
                *y,
                sigma_major,
                sigma_minor,
                angle,
                amplitude,
            );
        } else if config.use_moffat {
            // Render Moffat profile
            let alpha = fwhm_to_moffat_alpha(fwhm, config.moffat_beta);
            let amplitude = flux * (config.moffat_beta - 1.0) / (PI * alpha * alpha);

            render_moffat_star(
                &mut pixels,
                config.width,
                *x,
                *y,
                alpha,
                config.moffat_beta,
                amplitude,
            );
        } else {
            // Render circular Gaussian
            let amplitude = flux / (2.0 * PI * sigma * sigma);
            render_gaussian_star(&mut pixels, config.width, *x, *y, sigma, amplitude);
        }

        ground_truth.push(GroundTruthStar {
            pos: DVec2::new(*x as f64, *y as f64),
            flux,
            fwhm,
            eccentricity,
            is_saturated,
            angle,
        });
    }

    // Add cosmic rays
    if config.cosmic_ray_count > 0 {
        add_cosmic_rays(
            &mut pixels,
            config.width,
            config.cosmic_ray_count,
            (0.5, 1.0),
            config.seed + 1000,
        );
    }

    // Add Bayer pattern
    if config.add_bayer {
        add_bayer_pattern(
            &mut pixels,
            config.width,
            config.bayer_strength,
            BayerPattern::RGGB,
        );
    }

    // Add noise
    if config.noise_sigma > 0.0 {
        super::patterns::add_gaussian_noise(&mut pixels, config.noise_sigma, config.seed + 2000);
    }

    // Clamp to valid range
    for p in &mut pixels {
        *p = p.clamp(0.0, 1.0);
    }

    (
        Buffer2::new(config.width, config.height, pixels),
        ground_truth,
    )
}

/// Generate star positions based on crowding type.
fn generate_star_positions(
    config: &StarFieldConfig,
    rng: &mut crate::testing::TestRng,
) -> Vec<(f32, f32)> {
    let mut positions = Vec::with_capacity(config.num_stars);

    let margin = config.edge_margin as f32;
    let x_range = config.width as f32 - 2.0 * margin;
    let y_range = config.height as f32 - 2.0 * margin;

    match config.crowding {
        CrowdingType::Uniform => {
            for _ in 0..config.num_stars {
                let x = margin + rng.next_f32() * x_range;
                let y = margin + rng.next_f32() * y_range;
                positions.push((x, y));
            }
        }
        CrowdingType::Clustered => {
            // 70% of stars in central cluster, 30% in halo
            let cx = config.width as f32 / 2.0;
            let cy = config.height as f32 / 2.0;
            let cluster_radius = (config.width.min(config.height) as f32) * 0.15;
            let cluster_count = (config.num_stars as f32 * 0.7) as usize;

            // Central cluster (Gaussian distribution)
            for _ in 0..cluster_count {
                loop {
                    let x = cx + rng.next_gaussian_f32() * cluster_radius;
                    let y = cy + rng.next_gaussian_f32() * cluster_radius;

                    if x >= margin
                        && x < config.width as f32 - margin
                        && y >= margin
                        && y < config.height as f32 - margin
                    {
                        positions.push((x, y));
                        break;
                    }
                }
            }

            // Sparse halo (uniform)
            let halo_count = config.num_stars - cluster_count;
            for _ in 0..halo_count {
                let x = margin + rng.next_f32() * x_range;
                let y = margin + rng.next_f32() * y_range;
                positions.push((x, y));
            }
        }
        CrowdingType::Gradient => {
            // More stars on left side than right
            for _ in 0..config.num_stars {
                // Use inverse transform sampling for gradient distribution
                let u = rng.next_f32();
                // Quadratic CDF: P(X < x) = x^2, so x = sqrt(u)
                // This gives more stars at higher x values, invert for left-heavy
                let x = margin + (1.0 - u.sqrt()) * x_range;
                let y = margin + rng.next_f32() * y_range;
                positions.push((x, y));
            }
        }
    }

    positions
}

/// Generate a specific test configuration: sparse field with well-separated stars.
pub fn sparse_field_config() -> StarFieldConfig {
    StarFieldConfig {
        width: 256,
        height: 256,
        num_stars: 15,
        fwhm_range: (3.0, 4.0),
        magnitude_range: (12.5, 13.5),
        mag_zero_point: 14.8,
        background_level: 0.1,
        noise_sigma: 0.02,
        ..Default::default()
    }
}

/// Generate a specific test configuration: dense field with crowding.
pub fn dense_field_config() -> StarFieldConfig {
    StarFieldConfig {
        width: 256,
        height: 256,
        num_stars: 80,
        fwhm_range: (3.0, 4.0),
        magnitude_range: (12.0, 14.0),
        mag_zero_point: 14.8,
        background_level: 0.1,
        noise_sigma: 0.02,
        crowding: CrowdingType::Uniform,
        ..Default::default()
    }
}

/// Generate a specific test configuration: crowded cluster.
pub fn crowded_cluster_config() -> StarFieldConfig {
    StarFieldConfig {
        width: 256,
        height: 256,
        num_stars: 150,
        fwhm_range: (3.0, 4.0),
        magnitude_range: (12.0, 14.5),
        mag_zero_point: 14.8,
        background_level: 0.1,
        noise_sigma: 0.02,
        crowding: CrowdingType::Clustered,
        ..Default::default()
    }
}

/// Generate a specific test configuration: elliptical stars (tracking error).
pub fn elliptical_stars_config() -> StarFieldConfig {
    StarFieldConfig {
        width: 256,
        height: 256,
        num_stars: 30,
        fwhm_range: (3.0, 4.0),
        magnitude_range: (12.5, 13.5),
        mag_zero_point: 14.8,
        background_level: 0.1,
        noise_sigma: 0.02,
        elongation: ElongationType::Uniform,
        uniform_elongation: (0.4, 0.5),
        ..Default::default()
    }
}

/// Generate a specific test configuration: faint stars in high noise.
pub fn faint_stars_config() -> StarFieldConfig {
    StarFieldConfig {
        width: 256,
        height: 256,
        num_stars: 20,
        fwhm_range: (3.0, 4.0),
        magnitude_range: (13.5, 14.5),
        mag_zero_point: 14.8,
        background_level: 0.15,
        noise_sigma: 0.04,
        ..Default::default()
    }
}

/// Generate a globular cluster-like dense core region.
///
/// Creates an extremely dense star field with:
/// - A bright concentrated core with exponentially increasing star density
/// - Stars getting brighter toward the center
/// - Many overlapping/blended stars forming complex connected components
/// - A diffuse glow from unresolved stars in the core
///
/// This is useful for stress-testing deblending algorithms with realistic
/// crowded field conditions similar to globular clusters or galactic bulge regions.
///
/// # Arguments
/// * `width` - Image width in pixels
/// * `height` - Image height in pixels
/// * `num_stars` - Maximum number of stars to attempt (typically 20000-100000).
///   Actual count may be lower because stars outside the image or far
///   from center are skipped.
/// * `seed` - Random seed for reproducibility
///
/// # Returns
/// Buffer with pixel values in range 0.0-1.0
pub fn generate_globular_cluster(
    width: usize,
    height: usize,
    num_stars: usize,
    seed: u64,
) -> Buffer2<f32> {
    let background = 0.02f32;
    let mut pixels = vec![background; width * height];

    let mut rng = crate::testing::TestRng::new(seed);

    let center_x = width as f64 / 2.0;
    let center_y = height as f64 / 2.0;
    let core_radius = (width.min(height) as f64) * 0.05; // Dense core
    let halo_radius = (width.min(height) as f64) * 0.35; // Extended halo

    // Generate stars with radial distribution using exponential profile
    // More stars near center, density falls off exponentially
    for _ in 0..num_stars {
        // Exponential radial distribution: r = -scale * ln(u)
        // This gives high density at center
        let u1 = rng.next_f64().max(1e-10);
        let u2 = rng.next_f64();

        let scale = core_radius * 2.5;
        let r = -scale * u1.ln();

        // Skip stars too far out (they'll be outside the image anyway)
        // No hard cutoff - just skip if way outside
        if r > halo_radius * 1.5 {
            continue;
        }

        // Random angle (full circle)
        let theta = u2 * 2.0 * std::f64::consts::PI;

        let cx = center_x + r * theta.cos();
        let cy = center_y + r * theta.sin();

        // Skip if outside image bounds
        if cx < 10.0 || cx >= (width - 10) as f64 || cy < 10.0 || cy >= (height - 10) as f64 {
            continue;
        }

        // Brightness increases toward center (brighter giants in core)
        let dist_from_center = ((cx - center_x).powi(2) + (cy - center_y).powi(2)).sqrt();
        let brightness_boost = 1.0 + 2.0 * (1.0 - (dist_from_center / halo_radius).min(1.0));
        let base_brightness = 0.15 + rng.next_f64() * 0.6;
        let brightness = (base_brightness * brightness_boost).min(1.0) as f32;

        // Smaller sigma for tighter stars, slight variation
        let sigma = 1.8 + rng.next_f32() * 0.8;
        let two_sigma_sq = 2.0 * sigma * sigma;

        let radius = (sigma * 4.0).ceil() as i32;
        let cx_i = cx as i32;
        let cy_i = cy as i32;

        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let x = (cx_i + dx) as usize;
                let y = (cy_i + dy) as usize;
                if x < width && y < height {
                    let fx = x as f32 - cx as f32;
                    let fy = y as f32 - cy as f32;
                    let r2 = fx * fx + fy * fy;
                    let value = brightness * (-r2 / two_sigma_sq).exp();
                    pixels[y * width + x] += value;
                }
            }
        }
    }

    // Add faint diffuse glow in the core (unresolved stars)
    let glow_sigma = core_radius as f32 * 2.0;
    let two_glow_sigma_sq = 2.0 * glow_sigma * glow_sigma;
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - center_x as f32;
            let dy = y as f32 - center_y as f32;
            let r2 = dx * dx + dy * dy;
            let glow = 0.1 * (-r2 / two_glow_sigma_sq).exp();
            pixels[y * width + x] += glow;
        }
    }

    // Clamp values
    for p in &mut pixels {
        *p = p.min(1.0);
    }

    Buffer2::new(width, height, pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_star_field_basic() {
        let config = sparse_field_config();
        let (pixels, ground_truth) = generate_star_field(&config);

        assert_eq!(pixels.len(), config.width * config.height);
        assert_eq!(ground_truth.len(), config.num_stars);

        // Check all pixels are in valid range
        for p in pixels.iter() {
            assert!((0.0..=1.0).contains(p));
        }
    }

    #[test]
    fn test_generate_clustered_field() {
        let config = crowded_cluster_config();
        let (_pixels, ground_truth) = generate_star_field(&config);

        assert_eq!(ground_truth.len(), config.num_stars);

        // Check that stars are more concentrated near center
        let cx = config.width as f64 / 2.0;
        let cy = config.height as f64 / 2.0;
        let center_radius = (config.width as f64) * 0.25;

        let center_count = ground_truth
            .iter()
            .filter(|s| {
                let dx = s.pos.x - cx;
                let dy = s.pos.y - cy;
                (dx * dx + dy * dy).sqrt() < center_radius
            })
            .count();

        // Should have more than uniform distribution near center
        let expected_uniform = config.num_stars as f64
            * (std::f64::consts::PI * center_radius * center_radius)
            / (config.width * config.height) as f64;
        assert!(
            center_count as f64 > expected_uniform * 1.5,
            "Center count {} should exceed uniform expectation {}",
            center_count,
            expected_uniform
        );
    }

    #[test]
    fn test_elliptical_stars() {
        let config = elliptical_stars_config();
        let (_, ground_truth) = generate_star_field(&config);

        // All stars should have non-zero eccentricity
        for star in &ground_truth {
            assert!(
                star.eccentricity > 0.3,
                "Star eccentricity {} should be > 0.3",
                star.eccentricity
            );
        }
    }

    #[test]
    fn test_reproducibility() {
        let config = sparse_field_config();

        let (pixels1, truth1) = generate_star_field(&config);
        let (pixels2, truth2) = generate_star_field(&config);

        // Same seed should produce identical results
        assert_eq!(pixels1.len(), pixels2.len());
        for (p1, p2) in pixels1.iter().zip(pixels2.iter()) {
            assert!((p1 - p2).abs() < 1e-6);
        }

        assert_eq!(truth1.len(), truth2.len());
        for (t1, t2) in truth1.iter().zip(truth2.iter()) {
            assert!((t1.pos.x - t2.pos.x).abs() < 1e-6);
            assert!((t1.pos.y - t2.pos.y).abs() < 1e-6);
        }
    }
}
