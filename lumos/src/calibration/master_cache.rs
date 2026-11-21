//! [`MasterCache`]: where a master build keeps a role's master between runs, and the
//! [`MasterKey`] it keeps it under.

use std::fmt;
use std::path::Path;

use common::FileIdentity;

use crate::calibration::master_recipe::MasterRecipe;
use crate::calibration::master_role::MasterRole;
use crate::image::cfa::cfa_image::CfaImage;

/// What one role's master is stacked from, as a digest: each frame's name and identity, how lumos
/// stacks it, and the keys of the masters taken from each frame — a flat's bias and flat-dark. A
/// master stacked before its subtractor changed, by an older lumos, under other decode options, or
/// from a frame since rewritten has another key, and is stacked again.
///
/// The frames' identities are read before the stack reads them, so a frame changed while it was
/// stacked keys its old content, and the next build stacks the role again rather than reuse the
/// master. It displays as 64 hex digits, for a key in text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MasterKey([u8; 32]);

impl MasterKey {
    /// The key of a master of `recipe` stacked from `frames`, each first losing the masters of
    /// `subtracted`; `None` when a frame's identity cannot be read, whose stack then reports why.
    pub(crate) fn of(frames: &[&Path], recipe: MasterRecipe, subtracted: &[Self]) -> Option<Self> {
        let mut hasher = blake3::Hasher::new();
        for frame in frames {
            let name = frame.file_name()?.as_encoded_bytes();
            let identity = FileIdentity::of(frame).ok()?;
            hasher.update(
                &u64::try_from(name.len())
                    .expect("a file name's length fits u64")
                    .to_le_bytes(),
            );
            hasher.update(name);
            identity.write_to(|bytes| {
                hasher.update(bytes);
            });
        }
        hasher.update(&recipe.to_bytes());
        for key in subtracted {
            hasher.update(&key.0);
        }
        Some(Self(*hasher.finalize().as_bytes()))
    }
}

impl fmt::Display for MasterKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

/// One role's master as a cache sees it: the role, the frames it is stacked from, and the key it
/// is kept under.
#[derive(Debug, Clone, Copy)]
pub struct MasterSlot<'a> {
    pub role: MasterRole,
    pub frames: &'a [&'a Path],
    pub key: MasterKey,
}

/// Where [`CalibrationMasters::build`](crate::CalibrationMasters::build) keeps a role's master
/// between runs: read before the role is stacked, written once it is. A cache never fails a build:
/// a miss or an entry that does not read is `None`, and the role is stacked; a store that cannot
/// keep the master is the implementor's to report, as the build has its master either way.
pub trait MasterCache: Sync + fmt::Debug {
    /// The master kept for `slot`, or `None`.
    fn load(&self, slot: &MasterSlot<'_>) -> Option<CfaImage>;

    /// Keep `master`, stacked for `slot`, for a later build.
    fn store(&self, slot: &MasterSlot<'_>, master: &CfaImage);
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use common::{TempDir, fs};

    use crate::calibration::master_cache::MasterKey;
    use crate::calibration::master_recipe::MasterRecipe;
    use crate::calibration::master_role::MasterRole;
    use crate::ingest::ingest_config::IngestConfig;

    /// A key moves with each thing a master is stacked from: a frame rewritten to another length,
    /// the frames' order, the recipe's role, and a subtracted master's key; and it holds for the
    /// frames as they are. A frame that cannot be read has no key.
    #[test]
    fn a_key_moves_with_what_a_master_is_stacked_from() {
        let dir = TempDir::new("lumos-master-key");
        let (a, b) = (dir.join("a.fits"), dir.join("b.fits"));
        fs::write(&a, b"one").unwrap();
        fs::write(&b, b"two").unwrap();
        let ingest = IngestConfig::default();
        let dark = MasterRecipe::of(MasterRole::Dark, &ingest);
        let key = |frames: &[&Path], recipe, subtracted: &[MasterKey]| {
            MasterKey::of(frames, recipe, subtracted).unwrap()
        };
        let base = key(&[&a, &b], dark, &[]);
        assert_eq!(key(&[&a, &b], dark, &[]), base);
        assert_ne!(key(&[&b, &a], dark, &[]), base);
        assert_ne!(
            key(&[&a, &b], MasterRecipe::of(MasterRole::Flat, &ingest), &[]),
            base
        );
        assert_ne!(key(&[&a, &b], dark, &[base]), base);
        fs::write(&b, b"three").unwrap();
        assert_ne!(key(&[&a, &b], dark, &[]), base);
        assert_eq!(base.to_string().len(), 64);
        assert!(MasterKey::of(&[dir.join("absent.fits").as_path()], dark, &[]).is_none());
    }
}
