//! [`MasterRecipe`]: what a cache keys a master by beside its frames — the version of the decode
//! and of the master stack, the decode options, and the role.

use std::fmt;

use crate::calibration::master_role::MasterRole;
use crate::ingest::ingest_config::IngestConfig;
use crate::io::decoder::decode_version::DECODE_VERSION;
use crate::io::decoder::fnv1a;

/// The reference `master_snapshot` (`internals::characterization`) compares the master stack
/// with: a bias, a dark and a flat with the bias taken from each frame, stacked from mosaic FITS
/// frames. A change that moves the stack past the tolerance writes it anew, which moves every
/// [`MasterRecipe`], so no master a cache kept from the older stack is reused.
const MASTER_REFERENCE: &[u8] = include_bytes!("../../snapshots/master.ron");

/// The version every [`MasterRecipe`] carries: derived from [`DECODE_VERSION`] and
/// [`MASTER_REFERENCE`], so a change to a decode or to the master stack moves it.
const MASTER_VERSION: u64 = fnv1a::hash(
    fnv1a::hash(fnv1a::OFFSET, &DECODE_VERSION.to_le_bytes()),
    MASTER_REFERENCE,
);

/// How lumos stacks one role's master: the version of its decodes and of the master stack, the
/// decode options, and the role. Two masters of equal frames stack alike only under one recipe.
/// It displays as 16 hex digits, for a key in text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MasterRecipe(u64);

impl MasterRecipe {
    /// The recipe of `role`'s master stacked by this lumos under `ingest`, as
    /// [`CalibrationMasters::build`](crate::CalibrationMasters::build) hands it to a cache.
    pub fn of(role: MasterRole, ingest: &IngestConfig) -> Self {
        let options = ingest.decode_params().digest();
        let hash = fnv1a::hash(
            fnv1a::hash(MASTER_VERSION, &options.to_le_bytes()),
            role.extname().as_bytes(),
        );
        Self(hash)
    }
}

impl fmt::Display for MasterRecipe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use crate::calibration::master_recipe::MasterRecipe;
    use crate::calibration::master_role::MasterRole;
    use crate::ingest::ingest_config::IngestConfig;
    use crate::io::fits::fits_load_config::FitsChecksumPolicy;

    /// The recipe moves with the decode options and with the role, and with nothing else: a FITS
    /// checksum policy of `RequireValid` against the default, a flat against a dark, while the
    /// cache directory and the memory plan, which change no output, leave it.
    #[test]
    fn the_recipe_follows_the_options_and_the_role() {
        let plain = IngestConfig::default();
        let mut strict = IngestConfig::default();
        strict.fits.checksum = FitsChecksumPolicy::RequireValid;
        let mut elsewhere = IngestConfig::with_cache_dir("/elsewhere".into());
        elsewhere.memory_override = Some(1 << 30);
        let dark = MasterRecipe::of(MasterRole::Dark, &plain);
        assert_eq!(dark, MasterRecipe::of(MasterRole::Dark, &plain));
        assert_ne!(dark, MasterRecipe::of(MasterRole::Dark, &strict));
        assert_ne!(dark, MasterRecipe::of(MasterRole::Flat, &plain));
        assert_eq!(dark, MasterRecipe::of(MasterRole::Dark, &elsewhere));
        assert_eq!(dark.to_string().len(), 16);
    }
}
