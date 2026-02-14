//! Deterministic FNV-1a hasher.
//!
//! `DefaultHasher` uses random seeds, producing different hashes across process
//! invocations. FNV-1a is deterministic and fast for short keys (file paths, etc.).

use std::hash::Hasher;

/// FNV-1a 64-bit hasher with fixed seed.
#[derive(Debug)]
pub struct FnvHasher(u64);

impl FnvHasher {
    pub fn new() -> Self {
        Self(0xcbf29ce484222325)
    }
}

impl Default for FnvHasher {
    fn default() -> Self {
        Self::new()
    }
}

impl Hasher for FnvHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= b as u64;
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
}
