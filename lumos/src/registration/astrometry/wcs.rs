//! World Coordinate System (WCS) implementation.
//!
//! Provides transformations between pixel coordinates and celestial coordinates
//! following the FITS WCS standard.

/// A pixel position paired with its corresponding sky position (RA, Dec in degrees).
pub type PixelSkyMatch = ((f64, f64), (f64, f64));

/// World Coordinate System solution.
///
/// Represents the astrometric calibration of an image, allowing conversion
/// between pixel coordinates and sky coordinates (RA/Dec).
///
/// # WCS Model
///
/// The transformation uses the gnomonic (tangent plane) projection:
///
/// 1. Pixel to intermediate: `(u, v) = CD × (x - CRPIX1, y - CRPIX2)`
/// 2. Intermediate to sky: de-project from tangent plane
///
/// Where CD is a 2x2 matrix encoding scale, rotation, and any shear.
#[derive(Debug, Clone)]
pub struct Wcs {
    /// Reference pixel coordinates (CRPIX1, CRPIX2)
    pub crpix: (f64, f64),

    /// Reference sky coordinates in degrees (CRVAL1=RA, CRVAL2=Dec)
    pub crval: (f64, f64),

    /// CD matrix: transformation from pixel offset to intermediate coordinates (degrees)
    /// [[CD1_1, CD1_2], [CD2_1, CD2_2]]
    pub cd: [[f64; 2]; 2],

    /// Coordinate types (e.g., "RA---TAN", "DEC--TAN")
    pub ctype: (String, String),

    /// Image dimensions (width, height) in pixels
    pub naxis: (u32, u32),
}

impl Wcs {
    /// Create a new WCS with the given parameters.
    pub fn new(crpix: (f64, f64), crval: (f64, f64), cd: [[f64; 2]; 2], naxis: (u32, u32)) -> Self {
        Self {
            crpix,
            crval,
            cd,
            ctype: ("RA---TAN".to_string(), "DEC--TAN".to_string()),
            naxis,
        }
    }

    /// Create a WCS builder for fluent construction.
    pub fn builder() -> WcsBuilder {
        WcsBuilder::default()
    }

    /// Convert pixel coordinates to sky coordinates (RA, Dec in degrees).
    ///
    /// Uses the gnomonic (tangent plane) projection.
    pub fn pixel_to_sky(&self, x: f64, y: f64) -> (f64, f64) {
        // Pixel offset from reference point
        let dx = x - self.crpix.0;
        let dy = y - self.crpix.1;

        // Apply CD matrix to get intermediate coordinates (degrees)
        let xi = self.cd[0][0] * dx + self.cd[0][1] * dy;
        let eta = self.cd[1][0] * dx + self.cd[1][1] * dy;

        // Convert to radians
        let xi_rad = xi.to_radians();
        let eta_rad = eta.to_radians();

        // Reference point in radians
        let ra0 = self.crval.0.to_radians();
        let dec0 = self.crval.1.to_radians();

        // De-project from tangent plane (gnomonic projection inverse)
        let (sin_dec0, cos_dec0) = dec0.sin_cos();
        let denom = cos_dec0 - eta_rad * sin_dec0;

        let ra = ra0 + xi_rad.atan2(denom);
        let dec = (sin_dec0 + eta_rad * cos_dec0).atan2((xi_rad.powi(2) + denom.powi(2)).sqrt());

        // Convert back to degrees and normalize RA to [0, 360)
        let mut ra_deg = ra.to_degrees();
        if ra_deg < 0.0 {
            ra_deg += 360.0;
        } else if ra_deg >= 360.0 {
            ra_deg -= 360.0;
        }

        (ra_deg, dec.to_degrees())
    }

    /// Convert sky coordinates (RA, Dec in degrees) to pixel coordinates.
    ///
    /// Uses the gnomonic (tangent plane) projection.
    pub fn sky_to_pixel(&self, ra: f64, dec: f64) -> (f64, f64) {
        // Convert to radians
        let ra_rad = ra.to_radians();
        let dec_rad = dec.to_radians();
        let ra0 = self.crval.0.to_radians();
        let dec0 = self.crval.1.to_radians();

        // Compute intermediate coordinates (tangent plane projection)
        let (sin_dec, cos_dec) = dec_rad.sin_cos();
        let (sin_dec0, cos_dec0) = dec0.sin_cos();
        let delta_ra = ra_rad - ra0;
        let (sin_dra, cos_dra) = delta_ra.sin_cos();

        // Gnomonic projection denominator
        let d = sin_dec * sin_dec0 + cos_dec * cos_dec0 * cos_dra;

        // Standard coordinates (radians)
        let xi_rad = cos_dec * sin_dra / d;
        let eta_rad = (sin_dec * cos_dec0 - cos_dec * sin_dec0 * cos_dra) / d;

        // Convert to degrees
        let xi = xi_rad.to_degrees();
        let eta = eta_rad.to_degrees();

        // Invert CD matrix: solve CD * (dx, dy) = (xi, eta)
        let det = self.cd[0][0] * self.cd[1][1] - self.cd[0][1] * self.cd[1][0];
        debug_assert!(det.abs() > 1e-15, "CD matrix is singular (det = {})", det);

        let dx = (self.cd[1][1] * xi - self.cd[0][1] * eta) / det;
        let dy = (-self.cd[1][0] * xi + self.cd[0][0] * eta) / det;

        (self.crpix.0 + dx, self.crpix.1 + dy)
    }

    /// Compute the pixel scale in arcseconds per pixel.
    ///
    /// Returns the average of X and Y scales.
    pub fn pixel_scale_arcsec(&self) -> f64 {
        let scale_x = (self.cd[0][0].powi(2) + self.cd[1][0].powi(2)).sqrt();
        let scale_y = (self.cd[0][1].powi(2) + self.cd[1][1].powi(2)).sqrt();
        ((scale_x + scale_y) / 2.0) * 3600.0 // Convert degrees to arcsec
    }

    /// Compute the rotation angle in degrees (position angle of Y axis).
    ///
    /// Returns angle measured from North through East.
    pub fn rotation_degrees(&self) -> f64 {
        // Position angle: rotation from North towards East
        // For CD matrix created by from_scale_rotation: cd[1][0] = sin_r, cd[1][1] = cos_r
        self.cd[1][0].atan2(self.cd[1][1]).to_degrees()
    }

    /// Check if the image is mirrored (flipped).
    ///
    /// Returns true if the CD matrix has negative determinant.
    pub fn is_mirrored(&self) -> bool {
        let det = self.cd[0][0] * self.cd[1][1] - self.cd[0][1] * self.cd[1][0];
        det < 0.0
    }

    /// Get the field of view in degrees (width, height).
    pub fn field_of_view(&self) -> (f64, f64) {
        let scale = self.pixel_scale_arcsec() / 3600.0; // Back to degrees
        (scale * self.naxis.0 as f64, scale * self.naxis.1 as f64)
    }

    /// Compute the sky coordinates of the image center.
    pub fn center(&self) -> (f64, f64) {
        self.pixel_to_sky(self.naxis.0 as f64 / 2.0, self.naxis.1 as f64 / 2.0)
    }

    /// Compute the sky coordinates of the four corners.
    ///
    /// Returns [(bottom-left), (bottom-right), (top-right), (top-left)].
    pub fn corners(&self) -> [(f64, f64); 4] {
        let w = self.naxis.0 as f64;
        let h = self.naxis.1 as f64;
        [
            self.pixel_to_sky(0.0, 0.0),
            self.pixel_to_sky(w, 0.0),
            self.pixel_to_sky(w, h),
            self.pixel_to_sky(0.0, h),
        ]
    }

    /// Create WCS from pixel scale and rotation.
    ///
    /// This is a convenience constructor for simple cases where the transformation
    /// is just scale and rotation with no shear.
    ///
    /// # Arguments
    /// * `crpix` - Reference pixel (usually image center)
    /// * `crval` - Reference sky coordinates (RA, Dec in degrees)
    /// * `pixel_scale` - Pixel scale in arcseconds/pixel
    /// * `rotation` - Position angle in degrees (North through East)
    /// * `naxis` - Image dimensions (width, height)
    /// * `mirrored` - Whether the image is mirrored (e.g., from a Newtonian)
    pub fn from_scale_rotation(
        crpix: (f64, f64),
        crval: (f64, f64),
        pixel_scale: f64,
        rotation: f64,
        naxis: (u32, u32),
        mirrored: bool,
    ) -> Self {
        let scale_deg = pixel_scale / 3600.0; // Convert arcsec to degrees
        let rot_rad = rotation.to_radians();
        let (sin_r, cos_r) = rot_rad.sin_cos();

        // CD matrix for scale and rotation
        // If mirrored, flip the X axis
        let sign = if mirrored { -1.0 } else { 1.0 };

        let cd = [
            [sign * scale_deg * cos_r, -scale_deg * sin_r],
            [sign * scale_deg * sin_r, scale_deg * cos_r],
        ];

        Self::new(crpix, crval, cd, naxis)
    }

    /// Compute residuals for a set of matched stars.
    ///
    /// Returns the RMS error in arcseconds.
    pub fn compute_residuals(&self, matches: &[PixelSkyMatch]) -> f64 {
        if matches.is_empty() {
            return 0.0;
        }

        let mut sum_sq = 0.0;
        for &((px, py), (ra, dec)) in matches {
            let (pred_ra, pred_dec) = self.pixel_to_sky(px, py);

            // Angular separation in arcseconds
            let delta_ra = (pred_ra - ra) * dec.to_radians().cos();
            let delta_dec = pred_dec - dec;
            let sep_arcsec = ((delta_ra.powi(2) + delta_dec.powi(2)).sqrt()) * 3600.0;

            sum_sq += sep_arcsec.powi(2);
        }

        (sum_sq / matches.len() as f64).sqrt()
    }
}

impl Default for Wcs {
    fn default() -> Self {
        Self {
            crpix: (0.0, 0.0),
            crval: (0.0, 0.0),
            cd: [[1.0 / 3600.0, 0.0], [0.0, 1.0 / 3600.0]], // 1 arcsec/pixel
            ctype: ("RA---TAN".to_string(), "DEC--TAN".to_string()),
            naxis: (1, 1),
        }
    }
}

/// Builder for constructing WCS solutions.
#[derive(Debug, Default)]
pub struct WcsBuilder {
    crpix: Option<(f64, f64)>,
    crval: Option<(f64, f64)>,
    cd: Option<[[f64; 2]; 2]>,
    naxis: Option<(u32, u32)>,
    pixel_scale: Option<f64>,
    rotation: Option<f64>,
    mirrored: bool,
}

impl WcsBuilder {
    /// Set the reference pixel coordinates.
    pub fn crpix(mut self, x: f64, y: f64) -> Self {
        self.crpix = Some((x, y));
        self
    }

    /// Set the reference sky coordinates (RA, Dec in degrees).
    pub fn crval(mut self, ra: f64, dec: f64) -> Self {
        self.crval = Some((ra, dec));
        self
    }

    /// Set the CD matrix directly.
    pub fn cd(mut self, matrix: [[f64; 2]; 2]) -> Self {
        self.cd = Some(matrix);
        self
    }

    /// Set the image dimensions.
    pub fn naxis(mut self, width: u32, height: u32) -> Self {
        self.naxis = Some((width, height));
        self
    }

    /// Set the pixel scale in arcseconds/pixel.
    pub fn pixel_scale(mut self, scale: f64) -> Self {
        self.pixel_scale = Some(scale);
        self
    }

    /// Set the rotation angle in degrees.
    pub fn rotation(mut self, angle: f64) -> Self {
        self.rotation = Some(angle);
        self
    }

    /// Set whether the image is mirrored.
    pub fn mirrored(mut self, mirrored: bool) -> Self {
        self.mirrored = mirrored;
        self
    }

    /// Build the WCS.
    ///
    /// # Panics
    /// Panics if required fields (crpix, crval, naxis) are not set,
    /// or if neither CD matrix nor pixel_scale is provided.
    pub fn build(self) -> Wcs {
        let crpix = self.crpix.expect("crpix is required");
        let crval = self.crval.expect("crval is required");
        let naxis = self.naxis.expect("naxis is required");

        if let Some(cd) = self.cd {
            Wcs::new(crpix, crval, cd, naxis)
        } else if let Some(scale) = self.pixel_scale {
            let rotation = self.rotation.unwrap_or(0.0);
            Wcs::from_scale_rotation(crpix, crval, scale, rotation, naxis, self.mirrored)
        } else {
            panic!("Either cd matrix or pixel_scale is required");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOLERANCE: f64 = 1e-6;

    #[test]
    fn test_pixel_to_sky_identity() {
        // Simple WCS: center at (180, 45) deg, 1 arcsec/pixel
        let wcs =
            Wcs::from_scale_rotation((512.0, 512.0), (180.0, 45.0), 1.0, 0.0, (1024, 1024), false);

        // Reference pixel should map to reference coordinates
        let (ra, dec) = wcs.pixel_to_sky(512.0, 512.0);
        assert!((ra - 180.0).abs() < TOLERANCE);
        assert!((dec - 45.0).abs() < TOLERANCE);
    }

    #[test]
    fn test_sky_to_pixel_roundtrip() {
        let wcs = Wcs::from_scale_rotation(
            (512.0, 512.0),
            (180.0, 45.0),
            2.0,
            30.0, // 30 degree rotation
            (1024, 1024),
            false,
        );

        // Test several points
        let test_pixels = [(100.0, 100.0), (512.0, 512.0), (900.0, 700.0)];

        for (x, y) in test_pixels {
            let (ra, dec) = wcs.pixel_to_sky(x, y);
            let (x2, y2) = wcs.sky_to_pixel(ra, dec);
            assert!((x - x2).abs() < 1e-9, "X mismatch: {} vs {}", x, x2);
            assert!((y - y2).abs() < 1e-9, "Y mismatch: {} vs {}", y, y2);
        }
    }

    #[test]
    fn test_pixel_scale() {
        let wcs = Wcs::from_scale_rotation(
            (512.0, 512.0),
            (180.0, 45.0),
            1.5, // 1.5 arcsec/pixel
            0.0,
            (1024, 1024),
            false,
        );

        let scale = wcs.pixel_scale_arcsec();
        assert!((scale - 1.5).abs() < 1e-10, "Scale mismatch: {}", scale);
    }

    #[test]
    fn test_rotation() {
        let wcs = Wcs::from_scale_rotation(
            (512.0, 512.0),
            (180.0, 45.0),
            1.0,
            45.0, // 45 degree rotation
            (1024, 1024),
            false,
        );

        let rotation = wcs.rotation_degrees();
        assert!(
            (rotation - 45.0).abs() < 1e-10,
            "Rotation mismatch: {}",
            rotation
        );
    }

    #[test]
    fn test_mirrored() {
        let wcs_normal =
            Wcs::from_scale_rotation((512.0, 512.0), (180.0, 45.0), 1.0, 0.0, (1024, 1024), false);

        let wcs_mirrored =
            Wcs::from_scale_rotation((512.0, 512.0), (180.0, 45.0), 1.0, 0.0, (1024, 1024), true);

        assert!(!wcs_normal.is_mirrored());
        assert!(wcs_mirrored.is_mirrored());
    }

    #[test]
    fn test_field_of_view() {
        let wcs = Wcs::from_scale_rotation(
            (512.0, 512.0),
            (180.0, 45.0),
            1.0, // 1 arcsec/pixel
            0.0,
            (3600, 3600), // 1 degree FOV
            false,
        );

        let (fov_x, fov_y) = wcs.field_of_view();
        assert!((fov_x - 1.0).abs() < 1e-10, "FOV X mismatch: {}", fov_x);
        assert!((fov_y - 1.0).abs() < 1e-10, "FOV Y mismatch: {}", fov_y);
    }

    #[test]
    fn test_builder() {
        let wcs = Wcs::builder()
            .crpix(512.0, 512.0)
            .crval(180.0, 45.0)
            .naxis(1024, 1024)
            .pixel_scale(1.0)
            .rotation(30.0)
            .build();

        assert_eq!(wcs.crpix, (512.0, 512.0));
        assert_eq!(wcs.crval, (180.0, 45.0));
        assert_eq!(wcs.naxis, (1024, 1024));
        assert!((wcs.pixel_scale_arcsec() - 1.0).abs() < 1e-10);
        assert!((wcs.rotation_degrees() - 30.0).abs() < 1e-10);
    }

    #[test]
    fn test_compute_residuals() {
        let wcs =
            Wcs::from_scale_rotation((512.0, 512.0), (180.0, 45.0), 1.0, 0.0, (1024, 1024), false);

        // Perfect matches should have zero residuals
        let matches: Vec<_> = [(100.0, 100.0), (512.0, 512.0), (900.0, 700.0)]
            .iter()
            .map(|&(x, y)| ((x, y), wcs.pixel_to_sky(x, y)))
            .collect();

        let rms = wcs.compute_residuals(&matches);
        assert!(rms < 1e-10, "RMS should be ~0 for perfect matches: {}", rms);
    }

    #[test]
    fn test_corners() {
        let wcs =
            Wcs::from_scale_rotation((512.0, 512.0), (180.0, 45.0), 1.0, 0.0, (1024, 1024), false);

        let corners = wcs.corners();
        assert_eq!(corners.len(), 4);

        // Verify corners are distinct and reasonable
        for i in 0..4 {
            for j in (i + 1)..4 {
                let (ra1, dec1) = corners[i];
                let (ra2, dec2) = corners[j];
                assert!(
                    (ra1 - ra2).abs() > 0.001 || (dec1 - dec2).abs() > 0.001,
                    "Corners {} and {} are too close",
                    i,
                    j
                );
            }
        }
    }
}
