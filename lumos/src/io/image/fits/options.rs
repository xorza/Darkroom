/// Selects the image HDU decoded from a FITS container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FitsHduSelector {
    /// Accept the file only when it contains exactly one image-bearing HDU.
    Auto,
    /// Select a zero-based HDU index.
    Index(usize),
    /// Select an image extension by case-insensitive `EXTNAME` and optional `EXTVER`.
    Name {
        /// Extension name to match.
        extname: String,
        /// Extension version to match; a missing FITS `EXTVER` is version 1.
        extver: Option<i64>,
    },
}

/// Declares how a three-plane FITS image is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FitsCubeInterpretation {
    /// Reject three-plane data because shape alone does not establish color semantics.
    Reject,
    /// Interpret planes 0, 1, and 2 as red, green, and blue.
    Rgb,
}

/// Controls checksum validation for the selected FITS HDU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FitsChecksumPolicy {
    /// Do not compute `DATASUM` or `CHECKSUM`.
    Ignore,
    /// Verify either checksum when present and reject invalid values.
    VerifyIfPresent,
    /// Require both `DATASUM` and `CHECKSUM` to be present and valid.
    RequireValid,
}

/// FITS-specific selection and validation policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FitsLoadOptions {
    /// Image HDU selection policy.
    pub hdu: FitsHduSelector,
    /// Three-plane cube interpretation.
    pub cube: FitsCubeInterpretation,
    /// Checksum validation policy.
    pub checksum: FitsChecksumPolicy,
}

impl Default for FitsLoadOptions {
    fn default() -> Self {
        Self {
            hdu: FitsHduSelector::Auto,
            cube: FitsCubeInterpretation::Reject,
            checksum: FitsChecksumPolicy::VerifyIfPresent,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::io::image::fits::options::{
        FitsChecksumPolicy, FitsCubeInterpretation, FitsHduSelector, FitsLoadOptions,
    };

    #[test]
    fn scientific_fits_defaults_require_unambiguous_image_semantics() {
        let options = FitsLoadOptions::default();
        assert_eq!(options.hdu, FitsHduSelector::Auto);
        assert_eq!(options.cube, FitsCubeInterpretation::Reject);
        assert_eq!(options.checksum, FitsChecksumPolicy::VerifyIfPresent);
    }
}
