//! [`GainKey`]: what a flat gain is known by, before its nodes are read.

use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};

use crate::image::flat_gain::FlatGain;

/// The nodes a key's hash takes at once, as little-endian bytes.
const LE_CHUNK: usize = 4096;

/// What a [`FlatGain`] is known by: the blake3 digest of the image size it covers and each
/// channel's nodes, in turn, every value little-endian, so a file states the same key on every
/// machine. Two gains of one key are the same gain, so a run holds one of them, and a file that
/// states its key is taken without its nodes being read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct GainKey(#[serde(with = "common::hex_bytes")] [u8; 32]);

impl GainKey {
    /// The key of `gain`, which [`FlatGain::key`] takes once.
    pub(super) fn of(gain: &FlatGain) -> Self {
        let size = gain.size();
        let mut hasher = blake3::Hasher::new();
        for dimension in [size.width, size.height, gain.channels()] {
            hasher.update(&(dimension as u64).to_le_bytes());
        }
        let mut bytes = [0; LE_CHUNK * size_of::<f32>()];
        for plane in gain.planes() {
            for chunk in plane.chunks(LE_CHUNK) {
                let le = &mut bytes[..size_of_val(chunk)];
                for (out, node) in le
                    .as_chunks_mut::<{ size_of::<f32>() }>()
                    .0
                    .iter_mut()
                    .zip(chunk)
                {
                    *out = node.to_le_bytes();
                }
                hasher.update(le);
            }
        }
        Self(hasher.finalize().into())
    }

    /// The key a hex digest of 64 digits spells; `None` for any other text.
    pub(crate) fn from_hex(hex: &str) -> Option<Self> {
        blake3::Hash::from_hex(hex)
            .ok()
            .map(|hash| Self(hash.into()))
    }
}

/// The digest as 64 lowercase hex digits.
impl Display for GainKey {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(blake3::Hash::from(self.0).to_hex().as_str())
    }
}
