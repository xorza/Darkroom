//! [`HduSections`]: sections of one image HDU, read into scratch the reader and this keep.

use std::fs::File;
use std::ops::Range;

use fits_well::image::Borrowed as FitsBorrowed;
use fits_well::io::{ChecksumReport, DataChecksum, StreamReader};

/// One image HDU of an open file, read a section at a time into a borrowed view: plain samples
/// byte-swapped into `words`, compressed ones decoded there, so no section allocates. With a
/// `sum`, each section's stored bytes feed the HDU's data checksum as they are read, so the
/// checksum costs no second pass over the unit.
#[derive(Debug)]
pub(super) struct HduSections<'r> {
    reader: &'r mut StreamReader<File>,
    idx: usize,
    words: Vec<u64>,
    sum: Option<DataChecksum>,
}

impl<'r> HduSections<'r> {
    pub(super) const fn new(reader: &'r mut StreamReader<File>, idx: usize) -> Self {
        Self {
            reader,
            idx,
            words: Vec::new(),
            sum: None,
        }
    }

    /// Sections of HDU `idx` that feed its data checksum.
    pub(super) fn summed(
        reader: &'r mut StreamReader<File>,
        idx: usize,
    ) -> fits_well::Result<Self> {
        let sum = reader.begin_data_checksum(idx)?;
        Ok(Self {
            sum: Some(sum),
            ..Self::new(reader, idx)
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

    /// The samples of `ranges`, one per axis, fastest first.
    pub(super) fn read_section(
        &mut self,
        ranges: &[Range<usize>],
    ) -> fits_well::Result<FitsBorrowed<'_>> {
        match &mut self.sum {
            Some(sum) => {
                self.reader
                    .read_image_section_view_summed(self.idx, ranges, &mut self.words, sum)
            }
            None => self
                .reader
                .read_image_section_view(self.idx, ranges, &mut self.words),
        }
    }
}
