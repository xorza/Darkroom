//! The fits-well calls that decode or encode tiles on rayon's pool, each behind the check
//! `common::par` runs at its own entry points: fits-well cannot see `common::par`, so a loop's body
//! that read a compressed image would start parallel work unrefused. The check runs for a plain
//! image too, whose read is serial, so a test on a plain file refuses what a compressed one would.

use std::fs::File;

use common::par;
use fits_well::image::{Borrowed, Region};
use fits_well::io::{HduSelector, StreamReader, StreamSource};
use fits_well::table::Table;
use fits_well::{FitsError, ImageReader};

/// [`ImageReader::view`].
///
/// # Panics
/// In a parallel loop's body.
#[expect(clippy::disallowed_methods, reason = "the checked route to the call")]
pub(crate) fn view<'a, 'g>(
    image: &'a mut ImageReader<'_, StreamSource<File>>,
    region: impl Into<Region<'g>>,
    words: &'a mut Vec<u64>,
) -> Result<Borrowed<'a>, FitsError> {
    par::enter();
    image.view(region, words)
}

/// [`FitsReader::read_table`](fits_well::FitsReader::read_table).
///
/// # Panics
/// In a parallel loop's body.
#[expect(clippy::disallowed_methods, reason = "the checked route to the call")]
pub(crate) fn read_table<'h>(
    reader: &mut StreamReader<File>,
    hdu: impl Into<HduSelector<'h>>,
) -> Result<Table, FitsError> {
    par::enter();
    reader.read_table(hdu)
}

#[cfg(test)]
mod tests {
    use common::{TempDir, fs, par};
    use fits_well::FitsReader;
    use fits_well::image::{Image, Region, Scaling, View};
    use rayon::iter::ParallelIterator;

    use crate::internals::fits::write_fits;
    use crate::io::fits::tile_parallel;

    /// A plain image's read is serial, yet a loop's body that reads one is refused, as one that
    /// read a compressed image would be. At top level the same read returns the samples.
    #[test]
    #[should_panic(expected = "a parallel loop's body starts no parallel work")]
    fn a_body_that_reads_a_plain_image_panics() {
        let dir = TempDir::new("lumos-tile-parallel");
        let path = dir.join("plain.fits");
        let image = Image::new([2, 1], vec![1.0_f32, 2.0], Scaling::IDENTITY).unwrap();
        write_fits(&path, image.view(), None);
        let read = || {
            let mut reader = FitsReader::open(fs::open(&path).unwrap()).unwrap();
            let mut image = reader.image(0).unwrap();
            let mut words = Vec::new();
            let view = tile_parallel::view(&mut image, Region::All, &mut words).unwrap();
            assert_eq!(view.samples(), View::F32(&[1.0, 2.0]));
        };
        read();
        par::into_iter(0..2).for_each(|_| read());
    }
}
