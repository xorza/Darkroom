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

/// How a floating-point FITS image's sample scale is decided.
///
/// An integer `BITPIX` fixes what one sample is worth — the decoder divides by
/// `|BSCALE| × (2^bits − 1)` and the frame lands in the pipeline's `[0, 1]` domain. `BITPIX = -32`
/// and `-64` declare no such span, so a float HDU is the one input whose scale the header may not
/// settle, and it is the caller who knows.
///
/// The reference readers split three ways on this: PixInsight assumes `[0, 1]` with a configurable
/// input range, Siril scans the pixel data, and astropy does not normalize at all. Scanning is the
/// one option ruled out here — a divisor read off each frame's own extrema differs frame to frame,
/// which is what [`crate::StackError::SampleDomainMismatch`] exists to reject.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FitsFloatScale {
    /// Decide from `DATAMAX`: a declared saturation level well above unity means the samples are
    /// ADU and they are divided by the 16-bit full scale; anything else — a `DATAMAX` of about 1,
    /// or none at all — is taken as already normalized.
    Auto,
    /// Take the samples as already normalized, whatever `DATAMAX` says. For a file whose declared
    /// saturation level describes the sensor rather than the samples.
    Normalized,
    /// Divide by an explicit full scale: what one normalized unit is worth in the file's own units.
    /// The answer for an ADU frame carrying no `DATAMAX`, which no header rule can identify.
    FullScale(f32),
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
///
/// Not `Eq`: [`FitsFloatScale::FullScale`] carries an `f32`.
#[derive(Debug, Clone, PartialEq)]
pub struct FitsLoadOptions {
    /// Image HDU selection policy.
    pub hdu: FitsHduSelector,
    /// Three-plane cube interpretation.
    pub cube: FitsCubeInterpretation,
    /// Checksum validation policy.
    pub checksum: FitsChecksumPolicy,
    /// How a floating-point HDU's sample scale is decided.
    pub float_scale: FitsFloatScale,
}

impl Default for FitsLoadOptions {
    fn default() -> Self {
        Self {
            hdu: FitsHduSelector::Auto,
            cube: FitsCubeInterpretation::Reject,
            checksum: FitsChecksumPolicy::VerifyIfPresent,
            float_scale: FitsFloatScale::Auto,
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
