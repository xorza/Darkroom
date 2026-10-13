//! [`LumosExtension`]: the image extensions Lumos writes beside an image to describe it.

use std::fmt::{self, Display, Formatter};

use crate::io::fits::flags_extension::{FLAGS_EXTNAME, FLAGS_FORMAT, FLAGS_VERSION};
use crate::io::fits::gain_extension::{GAIN_EXTNAME, GAIN_FORMAT, GAIN_VERSION};

/// An image extension that describes another image of its file rather than being one; it shows
/// as its `EXTNAME`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LumosExtension {
    /// `LUMFLAGS`: the image's [`PixelFlags`](crate::PixelFlags).
    Flags,
    /// `LUMGAIN`: the image's [`FlatGain`](crate::FlatGain).
    Gain,
}

impl LumosExtension {
    pub(crate) const ALL: [Self; 2] = [Self::Flags, Self::Gain];

    pub(crate) const fn extname(self) -> &'static str {
        match self {
            Self::Flags => FLAGS_EXTNAME,
            Self::Gain => GAIN_EXTNAME,
        }
    }

    /// The `LUMOSFMT` this reader takes.
    pub(crate) const fn format(self) -> &'static str {
        match self {
            Self::Flags => FLAGS_FORMAT,
            Self::Gain => GAIN_FORMAT,
        }
    }

    /// The `LUMOSVER` this reader takes.
    pub(crate) const fn version(self) -> i64 {
        match self {
            Self::Flags => FLAGS_VERSION,
            Self::Gain => GAIN_VERSION,
        }
    }
}

impl Display for LumosExtension {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.extname())
    }
}
