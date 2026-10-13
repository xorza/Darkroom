//! [`HduList`]: a FITS file's header-data units, and the path its refusals name.

use std::path::Path;

use fits_well::io::Hdu;

/// A FITS file's header-data units, and the path every refusal of them names.
#[derive(Debug, Clone, Copy)]
pub(crate) struct HduList<'a> {
    pub(crate) path: &'a Path,
    pub(crate) hdus: &'a [Hdu],
}
