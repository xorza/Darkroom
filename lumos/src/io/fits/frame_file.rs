//! The file a frame writes: its primary HDU, then the extensions of what it carries beside its
//! samples, under one checksum policy, for both sample layouts.

use std::io::{Seek, Write};

use fits_well::FitsWriter;

use crate::image::frame::{Frame, Samples};
use crate::io::error::WriteError;
use crate::io::fits::flags_extension;
use crate::io::fits::gain_extension;
use crate::io::fits::plane_hdu::{FloatPlanes, PlaneHdu};

/// `primary`, the HDU of `frame`'s samples, then `frame`'s flags and its flat gain when it has
/// them, to `out` as one checksummed FITS file.
pub(crate) fn write(
    primary: &PlaneHdu<FloatPlanes<'_>>,
    frame: &Frame<impl Samples>,
    out: impl Write + Seek,
) -> Result<(), WriteError> {
    let flags = flags_extension::encode(frame.flags(), None, 1)?;
    let gain = gain_extension::encode(frame.flat_gain().map(|gain| &**gain), None)?;
    let mut writer = FitsWriter::new(out).with_checksums();
    primary.write(&mut writer)?;
    if let Some(flags) = &flags {
        flags.write(&mut writer)?;
    }
    if let Some(gain) = &gain {
        gain.write(&mut writer)?;
    }
    Ok(())
}
