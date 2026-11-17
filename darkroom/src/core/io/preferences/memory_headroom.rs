//! [`MemoryHeadroom`]: the memory the editor leaves free for everything else.

use serde::{Deserialize, Serialize};

/// How much of the memory the process can allocate the editor leaves free: after each run, the
/// node outputs kept while memory allows are dropped, the least recently used first, while what
/// remains allocatable is under it. One setting of the editor, as the machine it runs on is.
///
/// - **Auto is ComfyUI's default**: a tenth of the memory, between 2 and 10 GiB
///   ([startup flags](https://docs.comfy.org/development/comfyui-server/startup-flags)), here of
///   the memory allocatable at launch, which a container's limit already bounds. Refused: a fixed
///   size of cache, Nuke's comp cache share, which ignores what the other programs take.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MemoryHeadroom {
    /// A tenth of the memory allocatable at launch, between 2 and 10 GiB.
    #[default]
    Auto,
    /// This many GiB.
    Gib(u32),
}

/// One GiB in bytes.
const GIB: u64 = 1 << 30;

impl MemoryHeadroom {
    /// The headroom in bytes, where `available` bytes were allocatable at launch.
    pub(crate) const fn bytes(self, available: u64) -> u64 {
        match self {
            Self::Auto => {
                let tenth = available / 10;
                if tenth < 2 * GIB {
                    2 * GIB
                } else if tenth > 10 * GIB {
                    10 * GIB
                } else {
                    tenth
                }
            }
            Self::Gib(gib) => gib as u64 * GIB,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::core::io::preferences::memory_headroom::{GIB, MemoryHeadroom};

    /// Auto is a tenth of what was allocatable, held to 2 GiB under 20 and to 10 GiB over 100:
    /// 64 GiB gives 6.4 GiB; 8 GiB gives 2; 256 GiB gives 10. A set size is its GiB whatever the
    /// memory: 3 GiB is 3 × 2³⁰ bytes.
    #[test]
    fn auto_is_a_tenth_of_the_memory_between_two_and_ten_gib() {
        let auto = |available_gib: u64| MemoryHeadroom::Auto.bytes(available_gib * GIB);
        assert_eq!(auto(64), 64 * GIB / 10);
        assert_eq!(auto(8), 2 * GIB);
        assert_eq!(auto(256), 10 * GIB);
        assert_eq!(MemoryHeadroom::Gib(3).bytes(64 * GIB), 3 * GIB);
        assert_eq!(MemoryHeadroom::Gib(3).bytes(0), 3 * GIB);
    }
}
