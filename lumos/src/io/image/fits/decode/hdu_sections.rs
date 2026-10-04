//! [`HduSections`]: sections of one image HDU, read into scratch the reader and this keep.

use std::fs::File;
use std::ops::Range;

use fits_well::image::BorrowedImage;
use fits_well::io::{SliceReader, StreamReader};

/// One image HDU of an open file, read a section at a time into a borrowed view: plain samples
/// byte-swapped into `words`, compressed ones decoded there, so no section allocates.
#[derive(Debug)]
pub(super) struct HduSections<'r, R> {
    reader: &'r mut R,
    index: usize,
    words: Vec<u64>,
}

impl<'r, R> HduSections<'r, R> {
    pub(super) const fn new(reader: &'r mut R, index: usize) -> Self {
        Self {
            reader,
            index,
            words: Vec::new(),
        }
    }
}

/// A source of an image HDU's sections, as [`HduSections`] reads them from either kind of reader.
pub(super) trait SectionRead {
    /// The samples of `ranges`, one per axis, fastest first.
    fn read_section(&mut self, ranges: &[Range<usize>]) -> fits_well::Result<BorrowedImage<'_>>;
}

impl SectionRead for HduSections<'_, StreamReader<File>> {
    fn read_section(&mut self, ranges: &[Range<usize>]) -> fits_well::Result<BorrowedImage<'_>> {
        self.reader
            .read_image_section_view(self.index, ranges, &mut self.words)
    }
}

impl SectionRead for HduSections<'_, SliceReader<'_>> {
    fn read_section(&mut self, ranges: &[Range<usize>]) -> fits_well::Result<BorrowedImage<'_>> {
        self.reader
            .read_image_section_view(self.index, ranges, &mut self.words)
    }
}
