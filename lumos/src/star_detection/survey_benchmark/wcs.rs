//! WCS (World Coordinate System) support for FITS images.
//!
//! Implements the TAN (tangent plane / gnomonic) projection for converting
//! between pixel coordinates and sky coordinates (RA/Dec).

/// World Coordinate System for a FITS image.
///
/// Supports the TAN (tangent plane) projection which is the most common
/// for optical astronomical images.
#[derive(Debug, Clone)]
#[allow(clippy::upper_case_acronyms)]
pub struct WCS {
    /// Reference pixel X (1-indexed in FITS, we store 0-indexed)
    pub crpix1: f64,
    /// Reference pixel Y (1-indexed in FITS, we store 0-indexed)
    pub crpix2: f64,
    /// Reference RA at reference pixel (degrees)
    pub crval1: f64,
    /// Reference Dec at reference pixel (degrees)
    pub crval2: f64,
    /// CD matrix element (1,1) - degrees per pixel
    pub cd1_1: f64,
    /// CD matrix element (1,2)
    pub cd1_2: f64,
    /// CD matrix element (2,1)
    pub cd2_1: f64,
    /// CD matrix element (2,2)
    pub cd2_2: f64,
}

/// Sky coordinate bounds (RA/Dec box).
#[derive(Debug, Clone)]
pub struct SkyBounds {
    /// Minimum RA (degrees)
    pub ra_min: f64,
    /// Maximum RA (degrees)
    pub ra_max: f64,
    /// Minimum Dec (degrees)
    pub dec_min: f64,
    /// Maximum Dec (degrees)
    pub dec_max: f64,
}

impl SkyBounds {
    /// Center RA of the bounds.
    pub fn ra_center(&self) -> f64 {
        (self.ra_min + self.ra_max) / 2.0
    }

    /// Center Dec of the bounds.
    pub fn dec_center(&self) -> f64 {
        (self.dec_min + self.dec_max) / 2.0
    }

    /// Radius in degrees that encompasses the entire region.
    pub fn radius(&self) -> f64 {
        let dra = (self.ra_max - self.ra_min) / 2.0;
        let ddec = (self.dec_max - self.dec_min) / 2.0;
        (dra * dra + ddec * ddec).sqrt()
    }

    /// Check if a point is within bounds.
    pub fn contains(&self, ra: f64, dec: f64) -> bool {
        ra >= self.ra_min && ra <= self.ra_max && dec >= self.dec_min && dec <= self.dec_max
    }
}

impl WCS {
    /// Create WCS from FITS header keywords.
    ///
    /// Expects standard WCS keywords: CRPIX1, CRPIX2, CRVAL1, CRVAL2,
    /// and either CD matrix (CD1_1, CD1_2, CD2_1, CD2_2) or
    /// CDELT/CROTA convention.
    pub fn from_header<F>(mut get_keyword: F) -> Option<Self>
    where
        F: FnMut(&str) -> Option<f64>,
    {
        // Required keywords
        let crpix1 = get_keyword("CRPIX1")? - 1.0; // Convert to 0-indexed
        let crpix2 = get_keyword("CRPIX2")? - 1.0;
        let crval1 = get_keyword("CRVAL1")?;
        let crval2 = get_keyword("CRVAL2")?;

        // Try CD matrix first
        let (cd1_1, cd1_2, cd2_1, cd2_2) =
            if let (Some(cd1_1), Some(cd1_2), Some(cd2_1), Some(cd2_2)) = (
                get_keyword("CD1_1"),
                get_keyword("CD1_2"),
                get_keyword("CD2_1"),
                get_keyword("CD2_2"),
            ) {
                (cd1_1, cd1_2, cd2_1, cd2_2)
            } else {
                // Fall back to CDELT + CROTA convention
                let cdelt1 = get_keyword("CDELT1").unwrap_or(-1.0 / 3600.0); // Default 1 arcsec/pix
                let cdelt2 = get_keyword("CDELT2").unwrap_or(1.0 / 3600.0);
                let crota2 = get_keyword("CROTA2").unwrap_or(0.0).to_radians();

                let cos_r = crota2.cos();
                let sin_r = crota2.sin();

                (
                    cdelt1 * cos_r,
                    -cdelt2 * sin_r,
                    cdelt1 * sin_r,
                    cdelt2 * cos_r,
                )
            };

        Some(Self {
            crpix1,
            crpix2,
            crval1,
            crval2,
            cd1_1,
            cd1_2,
            cd2_1,
            cd2_2,
        })
    }

    /// Convert pixel coordinates to sky coordinates (RA, Dec in degrees).
    ///
    /// Uses the TAN (gnomonic) projection.
    pub fn pixel_to_sky(&self, x: f64, y: f64) -> (f64, f64) {
        // Offset from reference pixel
        let dx = x - self.crpix1;
        let dy = y - self.crpix2;

        // Intermediate world coordinates (degrees)
        let xi = self.cd1_1 * dx + self.cd1_2 * dy;
        let eta = self.cd2_1 * dx + self.cd2_2 * dy;

        // Convert to radians for trig
        let xi_rad = xi.to_radians();
        let eta_rad = eta.to_radians();
        let ra0_rad = self.crval1.to_radians();
        let dec0_rad = self.crval2.to_radians();

        // TAN (gnomonic) deprojection
        let rho = (xi_rad * xi_rad + eta_rad * eta_rad).sqrt();

        let (ra, dec) = if rho < 1e-10 {
            // At reference point
            (self.crval1, self.crval2)
        } else {
            let c = rho.atan();
            let sin_c = c.sin();
            let cos_c = c.cos();

            let dec_rad = (cos_c * dec0_rad.sin() + eta_rad * sin_c * dec0_rad.cos() / rho).asin();

            let ra_rad = ra0_rad
                + (xi_rad * sin_c)
                    .atan2(rho * dec0_rad.cos() * cos_c - eta_rad * dec0_rad.sin() * sin_c);

            (ra_rad.to_degrees(), dec_rad.to_degrees())
        };

        // Normalize RA to [0, 360)
        let ra_norm = if ra < 0.0 { ra + 360.0 } else { ra % 360.0 };

        (ra_norm, dec)
    }

    /// Convert sky coordinates (RA, Dec in degrees) to pixel coordinates.
    ///
    /// Uses the TAN (gnomonic) projection.
    pub fn sky_to_pixel(&self, ra: f64, dec: f64) -> (f64, f64) {
        let ra_rad = ra.to_radians();
        let dec_rad = dec.to_radians();
        let ra0_rad = self.crval1.to_radians();
        let dec0_rad = self.crval2.to_radians();

        // TAN projection
        let cos_dec = dec_rad.cos();
        let sin_dec = dec_rad.sin();
        let cos_dec0 = dec0_rad.cos();
        let sin_dec0 = dec0_rad.sin();
        let cos_dra = (ra_rad - ra0_rad).cos();
        let sin_dra = (ra_rad - ra0_rad).sin();

        let denom = sin_dec * sin_dec0 + cos_dec * cos_dec0 * cos_dra;

        // Intermediate world coordinates (radians)
        let xi_rad = cos_dec * sin_dra / denom;
        let eta_rad = (sin_dec * cos_dec0 - cos_dec * sin_dec0 * cos_dra) / denom;

        // Convert to degrees
        let xi = xi_rad.to_degrees();
        let eta = eta_rad.to_degrees();

        // Invert CD matrix to get pixel offsets
        let det = self.cd1_1 * self.cd2_2 - self.cd1_2 * self.cd2_1;
        assert!(det.abs() > 1e-15, "CD matrix is singular");

        let dx = (self.cd2_2 * xi - self.cd1_2 * eta) / det;
        let dy = (-self.cd2_1 * xi + self.cd1_1 * eta) / det;

        (self.crpix1 + dx, self.crpix2 + dy)
    }

    /// Get the sky coordinate bounds for an image.
    pub fn image_bounds_sky(&self, width: usize, height: usize) -> SkyBounds {
        // Sample corners and edges
        let corners = [
            self.pixel_to_sky(0.0, 0.0),
            self.pixel_to_sky(width as f64, 0.0),
            self.pixel_to_sky(0.0, height as f64),
            self.pixel_to_sky(width as f64, height as f64),
        ];

        let mut ra_min = f64::MAX;
        let mut ra_max = f64::MIN;
        let mut dec_min = f64::MAX;
        let mut dec_max = f64::MIN;

        for (ra, dec) in corners {
            ra_min = ra_min.min(ra);
            ra_max = ra_max.max(ra);
            dec_min = dec_min.min(dec);
            dec_max = dec_max.max(dec);
        }

        SkyBounds {
            ra_min,
            ra_max,
            dec_min,
            dec_max,
        }
    }

    /// Get the approximate pixel scale in arcseconds per pixel.
    pub fn pixel_scale_arcsec(&self) -> f64 {
        // Average of the diagonal elements, converted to arcsec
        let scale = ((self.cd1_1 * self.cd1_1 + self.cd2_1 * self.cd2_1).sqrt()
            + (self.cd1_2 * self.cd1_2 + self.cd2_2 * self.cd2_2).sqrt())
            / 2.0;
        scale * 3600.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_simple_wcs() -> WCS {
        // Simple WCS: 1 arcsec/pixel, no rotation, centered at RA=180, Dec=45
        WCS {
            crpix1: 500.0,
            crpix2: 500.0,
            crval1: 180.0,
            crval2: 45.0,
            cd1_1: -1.0 / 3600.0, // -1 arcsec/pixel in RA direction
            cd1_2: 0.0,
            cd2_1: 0.0,
            cd2_2: 1.0 / 3600.0, // +1 arcsec/pixel in Dec direction
        }
    }

    #[test]
    fn test_reference_pixel() {
        let wcs = make_simple_wcs();
        let (ra, dec) = wcs.pixel_to_sky(500.0, 500.0);

        assert!((ra - 180.0).abs() < 1e-10);
        assert!((dec - 45.0).abs() < 1e-10);
    }

    #[test]
    fn test_roundtrip() {
        let wcs = make_simple_wcs();

        // Test several positions
        for (x, y) in [(100.0, 100.0), (500.0, 500.0), (900.0, 900.0)] {
            let (ra, dec) = wcs.pixel_to_sky(x, y);
            let (x2, y2) = wcs.sky_to_pixel(ra, dec);

            assert!(
                (x - x2).abs() < 1e-6,
                "X roundtrip failed: {} -> {} -> {}",
                x,
                ra,
                x2
            );
            assert!(
                (y - y2).abs() < 1e-6,
                "Y roundtrip failed: {} -> {} -> {}",
                y,
                dec,
                y2
            );
        }
    }

    #[test]
    fn test_pixel_offset() {
        let wcs = make_simple_wcs();

        // 100 pixels right should be ~-100 arcsec in RA (because CD1_1 is negative)
        let (ra, dec) = wcs.pixel_to_sky(600.0, 500.0);

        // The RA offset depends on declination due to spherical geometry
        // At dec=45, cos(45) ≈ 0.707, so RA change is larger
        // Dec should be very close to reference (within ~1 arcsec due to projection)
        assert!(
            (dec - 45.0).abs() < 0.001,
            "Dec changed too much: {}",
            dec - 45.0
        );

        // RA should have decreased (moved west)
        assert!(ra < 180.0, "RA should decrease when moving right: {}", ra);
    }

    #[test]
    fn test_image_bounds() {
        let wcs = make_simple_wcs();
        let bounds = wcs.image_bounds_sky(1000, 1000);

        // Should span about 1000 arcsec = 0.278 degrees in each direction
        assert!(bounds.ra_max > bounds.ra_min);
        assert!(bounds.dec_max > bounds.dec_min);
        assert!((bounds.dec_max - bounds.dec_min - 1000.0 / 3600.0).abs() < 0.01);
    }

    #[test]
    fn test_from_header() {
        let wcs = WCS::from_header(|key| match key {
            "CRPIX1" => Some(501.0), // 1-indexed
            "CRPIX2" => Some(501.0),
            "CRVAL1" => Some(180.0),
            "CRVAL2" => Some(45.0),
            "CD1_1" => Some(-1.0 / 3600.0),
            "CD1_2" => Some(0.0),
            "CD2_1" => Some(0.0),
            "CD2_2" => Some(1.0 / 3600.0),
            _ => None,
        });

        let wcs = wcs.expect("Should parse WCS");
        assert!((wcs.crpix1 - 500.0).abs() < 1e-10); // Should be 0-indexed
    }

    #[test]
    fn test_pixel_scale() {
        let wcs = make_simple_wcs();
        let scale = wcs.pixel_scale_arcsec();

        assert!((scale - 1.0).abs() < 0.01); // Should be ~1 arcsec/pixel
    }
}
