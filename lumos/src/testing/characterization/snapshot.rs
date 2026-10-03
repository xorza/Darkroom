//! [`Snapshot`]: a bit-exact digest of a stage's output.

use blake3::Hasher;

/// The BLAKE3 digest of every value a stage produced, by bit pattern, in a fixed order. Two runs
/// agree only when every bit of every value agrees.
#[derive(Debug, Default)]
pub(crate) struct Snapshot(Hasher);

impl Snapshot {
    pub(crate) fn f32s(&mut self, values: &[f32]) -> &mut Snapshot {
        for value in values {
            self.0.update(&value.to_bits().to_le_bytes());
        }
        self
    }

    pub(crate) fn f64s(&mut self, values: &[f64]) -> &mut Snapshot {
        for value in values {
            self.0.update(&value.to_bits().to_le_bytes());
        }
        self
    }

    pub(crate) fn count(&mut self, count: usize) -> &mut Snapshot {
        self.0.update(&(count as u64).to_le_bytes());
        self
    }

    /// The digest's first 16 hex digits: 64 bits, far past any chance of two different outputs
    /// colliding, and short enough to read in a failure message.
    pub(crate) fn finish(&self) -> String {
        self.0.finalize().to_hex()[..16].to_owned()
    }
}
