//! [`HduSections`]: sections of one image HDU, read into scratch the reader and this keep.

use std::fs::File;
use std::ops::Range;

use fits_well::image::BorrowedImage;
use fits_well::io::{ChecksumReport, DataChecksum, SliceReader, StreamReader};

/// One image HDU of an open file, read a section at a time into a borrowed view: plain samples
/// byte-swapped into `words`, compressed ones decoded there, so no section allocates. With a
/// `sum`, each section's stored bytes feed the HDU's data checksum as they are read, so the
/// checksum costs no second pass over the unit.
#[derive(Debug)]
pub(super) struct HduSections<'r, R> {
    reader: &'r mut R,
    index: usize,
    words: Vec<u64>,
    sum: Option<DataChecksum>,
}

impl<'r, R> HduSections<'r, R> {
    pub(super) const fn new(reader: &'r mut R, index: usize) -> Self {
        Self {
            reader,
            index,
            words: Vec::new(),
            sum: None,
        }
    }
}

impl<'r> HduSections<'r, StreamReader<File>> {
    /// Sections of HDU `index` that feed its data checksum.
    pub(super) fn summed(
        reader: &'r mut StreamReader<File>,
        index: usize,
    ) -> fits_well::Result<Self> {
        let sum = reader.begin_data_checksum(index)?;
        Ok(Self {
            sum: Some(sum),
            ..Self::new(reader, index)
        })
    }

    /// The checksum report of what the sections fed, the rest of the unit read to finish it.
    ///
    /// # Panics
    ///
    /// If these sections sum nothing, or their sum was finished already.
    pub(super) fn finish_checksum(&mut self) -> fits_well::Result<ChecksumReport> {
        let sum = self.sum.take().expect("sections that sum, finished once");
        self.reader.finish_data_checksum(sum)
    }
}

/// A source of an image HDU's sections, as [`HduSections`] reads them from either kind of reader.
pub(super) trait SectionRead {
    /// The samples of `ranges`, one per axis, fastest first.
    fn read_section(&mut self, ranges: &[Range<usize>]) -> fits_well::Result<BorrowedImage<'_>>;
}

impl SectionRead for HduSections<'_, StreamReader<File>> {
    fn read_section(&mut self, ranges: &[Range<usize>]) -> fits_well::Result<BorrowedImage<'_>> {
        match &mut self.sum {
            Some(sum) => {
                self.reader
                    .read_image_section_view_summed(self.index, ranges, &mut self.words, sum)
            }
            None => self
                .reader
                .read_image_section_view(self.index, ranges, &mut self.words),
        }
    }
}

impl SectionRead for HduSections<'_, SliceReader<'_>> {
    fn read_section(&mut self, ranges: &[Range<usize>]) -> fits_well::Result<BorrowedImage<'_>> {
        self.reader
            .read_image_section_view(self.index, ranges, &mut self.words)
    }
}
