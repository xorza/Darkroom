//! [`DecodeConfig`]: how a file decodes, the one statement every decode and every cache keyed on
//! one reads.

use common::SerdeFormat;
use serde::Serialize;

use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::io::decoder::fnv1a;
use crate::io::fits::fits_load_config::FitsLoadConfig;
use crate::io::null_policy::NullPolicy;

/// How a file decodes: every option a decode's output depends on.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct DecodeConfig {
    /// How FITS frames are read; ignored for every other format.
    pub fits: FitsLoadConfig,
    /// What a null sample does to a linear decode: a FITS null, or a floating-point raster's NaN
    /// or infinity.
    pub nulls: NullPolicy,
    /// How many passes an X-Trans demosaic makes; ignored for every other mosaic.
    pub xtrans_passes: MarkesteijnPasses,
}

impl DecodeConfig {
    /// FNV-1a of the whole configuration as bitcode, the checksum policy too: a frame decoded
    /// without a check would otherwise pass for one that a required checksum admitted. It
    /// serializes `self`, so no field is left out.
    pub(crate) fn digest(&self) -> u64 {
        let bytes = SerdeFormat::Bitcode
            .serialize(self)
            .expect("decode options of plain enums and scalars always serialize");
        fnv1a::hash(fnv1a::OFFSET, &bytes)
    }
}

#[cfg(test)]
mod tests {
    use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
    use crate::io::decode_config::DecodeConfig;
    use crate::io::fits::fits_load_config::{FitsCubeInterpretation, FitsLoadConfig};
    use crate::io::null_policy::NullPolicy;

    /// Each field moves the digest: two configurations that differ in one field alone digest
    /// apart, and an equal one digests the same.
    #[test]
    fn each_field_moves_the_digest() {
        let base = DecodeConfig::default();
        assert_eq!(base.digest(), DecodeConfig::default().digest());
        let variants = [
            DecodeConfig {
                fits: FitsLoadConfig {
                    cube: FitsCubeInterpretation::Rgb,
                    ..FitsLoadConfig::default()
                },
                ..DecodeConfig::default()
            },
            DecodeConfig {
                nulls: NullPolicy::Reject,
                ..DecodeConfig::default()
            },
            DecodeConfig {
                xtrans_passes: MarkesteijnPasses::Three,
                ..DecodeConfig::default()
            },
        ];
        for variant in &variants {
            assert_ne!(*variant, base);
            assert_ne!(variant.digest(), base.digest(), "{variant:?}");
        }
    }
}
