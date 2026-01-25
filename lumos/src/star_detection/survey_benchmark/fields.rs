//! Pre-defined test fields for benchmarking.
//!
//! Curated set of astronomical fields with known characteristics for
//! testing star detection accuracy.

use super::catalog::CatalogSource;
use std::ops::Range;

/// Difficulty level of a test field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Difficulty {
    /// Well-separated stars, low background
    Sparse,
    /// Moderate star density
    Medium,
    /// Crowded field with overlapping stars
    Dense,
    /// Challenging conditions (faint, crowded, variable background)
    Challenging,
}

/// SDSS image identifiers.
#[derive(Debug, Clone, Copy)]
pub struct SdssField {
    /// Run number
    pub run: u32,
    /// Camera column (1-6)
    pub camcol: u8,
    /// Field number
    pub field: u16,
    /// Rerun number (typically 301)
    pub rerun: u32,
}

/// A pre-defined test field.
#[derive(Debug, Clone)]
pub struct TestField {
    /// Short name for the field
    pub name: &'static str,
    /// Description of the field characteristics
    pub description: &'static str,
    /// Center RA (degrees)
    pub ra: f64,
    /// Center Dec (degrees)
    pub dec: f64,
    /// Preferred catalog source
    pub source: CatalogSource,
    /// SDSS field identifiers (if applicable)
    pub sdss: Option<SdssField>,
    /// Expected number of detectable stars
    pub expected_star_count: Range<usize>,
    /// Difficulty level
    pub difficulty: Difficulty,
    /// Suggested magnitude limit for catalog queries
    pub mag_limit: f32,
    /// Approximate field FWHM in arcsec
    pub typical_fwhm_arcsec: f32,
}

impl TestField {
    /// Get the expected FWHM in pixels for a given plate scale.
    pub fn expected_fwhm_pixels(&self, arcsec_per_pixel: f32) -> f32 {
        self.typical_fwhm_arcsec / arcsec_per_pixel
    }
}

/// Sparse field with well-separated stars.
///
/// Good for validating basic detection accuracy.
pub fn sparse_field() -> TestField {
    TestField {
        name: "sparse_north",
        description: "Sparse high-galactic-latitude field with well-separated stars",
        ra: 177.25, // From SDSS run 2505
        dec: 0.03,
        source: CatalogSource::Sdss,
        sdss: Some(SdssField {
            run: 2505,
            camcol: 1,
            field: 32,
            rerun: 301,
        }),
        expected_star_count: 20..100,
        difficulty: Difficulty::Sparse,
        mag_limit: 20.0,
        typical_fwhm_arcsec: 1.4,
    }
}

/// Medium density field.
///
/// Typical extragalactic field conditions.
pub fn medium_field() -> TestField {
    TestField {
        name: "medium_density",
        description: "Medium density field typical of extragalactic observations",
        ra: 150.0,
        dec: 30.0,
        source: CatalogSource::Sdss,
        sdss: Some(SdssField {
            run: 3836,
            camcol: 2,
            field: 100,
            rerun: 301,
        }),
        expected_star_count: 100..300,
        difficulty: Difficulty::Medium,
        mag_limit: 21.0,
        typical_fwhm_arcsec: 1.3,
    }
}

/// Dense star field near the galactic plane.
///
/// Tests crowded field handling and deblending.
pub fn dense_field() -> TestField {
    TestField {
        name: "galactic_dense",
        description: "Dense field near galactic plane with crowding",
        ra: 270.0, // Near galactic center direction
        dec: -20.0,
        source: CatalogSource::GaiaDr3,
        sdss: None, // SDSS doesn't cover this well
        expected_star_count: 500..2000,
        difficulty: Difficulty::Dense,
        mag_limit: 18.0,
        typical_fwhm_arcsec: 1.5,
    }
}

/// Faint star field for testing detection limits.
///
/// Stars near the detection threshold.
pub fn faint_field() -> TestField {
    TestField {
        name: "faint_stars",
        description: "Field with predominantly faint stars near detection limit",
        ra: 200.0,
        dec: 50.0,
        source: CatalogSource::Sdss,
        sdss: Some(SdssField {
            run: 4192,
            camcol: 4,
            field: 200,
            rerun: 301,
        }),
        expected_star_count: 50..200,
        difficulty: Difficulty::Challenging,
        mag_limit: 22.0,
        typical_fwhm_arcsec: 1.4,
    }
}

/// Open cluster field (M67).
///
/// Well-studied cluster with accurate positions.
/// Note: M67 itself (RA=132.825) is not well covered by SDSS, so we use
/// a different field from SDSS run 3836 which has good Gaia coverage.
pub fn cluster_m67() -> TestField {
    // The SDSS field 3836/3/89 is actually centered at RA~161.78, Dec~11.4
    // We use Gaia catalog for this region instead of M67 proper
    TestField {
        name: "m67_cluster",
        description: "SDSS field with Gaia catalog - tests cross-survey matching",
        ra: 161.78, // Actual SDSS field center (not M67)
        dec: 11.4,
        source: CatalogSource::GaiaDr3,
        sdss: Some(SdssField {
            run: 3836,
            camcol: 3,
            field: 89,
            rerun: 301,
        }),
        expected_star_count: 50..200,
        difficulty: Difficulty::Medium,
        mag_limit: 18.0,
        typical_fwhm_arcsec: 1.3,
    }
}

/// Standard star field (Landolt SA95).
///
/// Photometric standard field with well-calibrated stars.
pub fn standard_sa95() -> TestField {
    TestField {
        name: "landolt_sa95",
        description: "Landolt standard field SA95 with photometric standards",
        ra: 58.75, // SA95 field center
        dec: 0.0,
        source: CatalogSource::Sdss,
        sdss: Some(SdssField {
            run: 752,
            camcol: 1,
            field: 100,
            rerun: 301,
        }),
        expected_star_count: 30..100,
        difficulty: Difficulty::Sparse,
        mag_limit: 19.0,
        typical_fwhm_arcsec: 1.4,
    }
}

/// Get all pre-defined test fields.
pub fn all_test_fields() -> Vec<TestField> {
    vec![
        sparse_field(),
        medium_field(),
        dense_field(),
        faint_field(),
        cluster_m67(),
        standard_sa95(),
    ]
}

/// Get test fields by difficulty level.
pub fn fields_by_difficulty(difficulty: Difficulty) -> Vec<TestField> {
    all_test_fields()
        .into_iter()
        .filter(|f| f.difficulty == difficulty)
        .collect()
}

/// Get test fields that have SDSS coverage.
pub fn sdss_fields() -> Vec<TestField> {
    all_test_fields()
        .into_iter()
        .filter(|f| f.sdss.is_some())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_fields_valid() {
        let fields = all_test_fields();
        assert!(!fields.is_empty());

        for field in fields {
            // Validate coordinates
            assert!(field.ra >= 0.0 && field.ra < 360.0);
            assert!(field.dec >= -90.0 && field.dec <= 90.0);

            // Validate ranges
            assert!(field.expected_star_count.start < field.expected_star_count.end);
            assert!(field.mag_limit > 0.0);
            assert!(field.typical_fwhm_arcsec > 0.0);
        }
    }

    #[test]
    fn test_sparse_field() {
        let field = sparse_field();
        assert_eq!(field.difficulty, Difficulty::Sparse);
        assert!(field.expected_star_count.end < 200);
    }

    #[test]
    fn test_dense_field() {
        let field = dense_field();
        assert_eq!(field.difficulty, Difficulty::Dense);
        assert!(field.expected_star_count.start >= 500);
    }

    #[test]
    fn test_fwhm_conversion() {
        let field = sparse_field();
        // SDSS has ~0.396 arcsec/pixel
        let fwhm_pix = field.expected_fwhm_pixels(0.396);
        assert!(fwhm_pix > 3.0 && fwhm_pix < 5.0);
    }

    #[test]
    fn test_fields_by_difficulty() {
        let sparse = fields_by_difficulty(Difficulty::Sparse);
        assert!(sparse.iter().all(|f| f.difficulty == Difficulty::Sparse));
    }
}
