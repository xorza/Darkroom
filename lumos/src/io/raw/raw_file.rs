//! [`RawFile`]: a camera RAW file read whole.

/// A camera RAW file read whole, as LibRaw parses it in place: what a header read hands the load,
/// which then reads no byte of the file again.
pub(crate) struct RawFile {
    pub(super) bytes: Vec<u8>,
}

impl RawFile {
    /// The file's size.
    pub(crate) const fn len(&self) -> usize {
        self.bytes.len()
    }
}

impl std::fmt::Debug for RawFile {
    /// The file's size, not its bytes.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawFile")
            .field("len", &self.bytes.len())
            .finish()
    }
}
