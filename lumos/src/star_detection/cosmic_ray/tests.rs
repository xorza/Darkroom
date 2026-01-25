//! Tests for cosmic ray detection.

use super::*;

#[test]
fn test_detect_cosmic_rays_sharp_peak() {
    // Single sharp pixel should be detected as cosmic ray
    // Use larger image for proper fine structure calculation
    let size = 15;
    let center = 7;
    let mut pixels = vec![0.1f32; size * size];
    pixels[center * size + center] = 1.0; // Sharp cosmic ray

    let background = vec![0.1f32; size * size];
    let noise = vec![0.01f32; size * size];

    let config = LACosmicConfig::default();
    let result = detect_cosmic_rays(&pixels, size, size, &background, &noise, &config);

    assert!(
        result.cosmic_ray_mask[center * size + center],
        "Sharp peak should be detected as cosmic ray"
    );
}

#[test]
fn test_detect_cosmic_rays_gaussian_star() {
    // Gaussian star with larger sigma should NOT be detected as cosmic ray
    // Use larger image and wider Gaussian to simulate a real star
    let size = 15;
    let center = 7;
    let sigma = 2.0f32; // Wider Gaussian simulating real seeing
    let mut pixels = vec![0.1f32; size * size];

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - center as f32;
            let dy = y as f32 - center as f32;
            let r2 = dx * dx + dy * dy;
            let value = 0.8 * (-r2 / (2.0 * sigma * sigma)).exp();
            if value > 0.001 {
                pixels[y * size + x] = 0.1 + value;
            }
        }
    }

    let background = vec![0.1f32; size * size];
    let noise = vec![0.01f32; size * size];

    let config = LACosmicConfig::default();
    let result = detect_cosmic_rays(&pixels, size, size, &background, &noise, &config);

    // Gaussian star should not be flagged at center
    assert!(
        !result.cosmic_ray_mask[center * size + center],
        "Gaussian star should NOT be detected as cosmic ray"
    );
}

#[test]
fn test_grow_mask() {
    let mut mask = vec![false; 25];
    mask[12] = true; // Center pixel (2*5+2)

    let pixels = vec![0.5f32; 25]; // All elevated

    let grown = grow_mask(&mask, 5, 5, 1, &pixels);

    // All 8 neighbors plus center should be true
    assert!(grown[6]); // 1*5+1
    assert!(grown[7]); // 1*5+2
    assert!(grown[8]); // 1*5+3
    assert!(grown[11]); // 2*5+1
    assert!(grown[12]); // 2*5+2
    assert!(grown[13]); // 2*5+3
    assert!(grown[16]); // 3*5+1
    assert!(grown[17]); // 3*5+2
    assert!(grown[18]); // 3*5+3
}
