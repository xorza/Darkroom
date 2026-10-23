//! [`FixedFile`]: a file read as long as it was when it was opened.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

/// A file opened for reading, which ends where it ended when it was opened: what it is read for
/// was admitted by that length, and bytes written to it since are not read. A file cut shorter
/// since ends where it ends now.
#[derive(Debug)]
pub struct FixedFile {
    file: File,
    len: u64,
    pos: u64,
}

impl FixedFile {
    /// The file at `path`, its length taken as it opens.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        Ok(Self { file, len, pos: 0 })
    }

    /// The bytes the file held when it was opened.
    pub const fn len(&self) -> u64 {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The bytes left between the position and the end.
    const fn left(&self) -> u64 {
        self.len.saturating_sub(self.pos)
    }
}

impl Read for FixedFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let room = usize::try_from(self.left()).map_or(buf.len(), |left| left.min(buf.len()));
        let count = self.file.read(&mut buf[..room])?;
        self.pos += u64::try_from(count).expect("a read count fits u64");
        Ok(count)
    }

    /// Reserves exactly the bytes left, as `File` does by its length, so a whole read holds the
    /// file and no more.
    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        let left = self.left();
        buf.try_reserve_exact(usize::try_from(left).map_err(|_| io::ErrorKind::OutOfMemory)?)?;
        let count = (&mut self.file).take(left).read_to_end(buf)?;
        self.pos += u64::try_from(count).expect("a read count fits u64");
        Ok(count)
    }
}

impl Seek for FixedFile {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let pos = match pos {
            SeekFrom::End(offset) => SeekFrom::Start(
                self.len
                    .checked_add_signed(offset)
                    .ok_or(io::ErrorKind::InvalidInput)?,
            ),
            pos => pos,
        };
        self.pos = self.file.seek(pos)?;
        Ok(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Seek, SeekFrom, Write};

    use crate::TempDir;
    use crate::fs;
    use crate::fs::fixed_file::FixedFile;

    /// A file of 10 bytes, opened, then grown by 5: it reads its 10 bytes whole, ends at 10 for a
    /// seek from its end, and reads 2 bytes from 8 before its end; cut to 4, it reads those 4.
    #[test]
    fn a_fixed_file_ends_where_it_ended_at_open() {
        let dir = TempDir::new("fixed-file");
        let path = dir.join("grows");
        fs::write(&path, b"0123456789").unwrap();
        let mut file = FixedFile::open(&path).unwrap();
        fs::open_write(&path)
            .unwrap()
            .write_all(b"0123456789abcde")
            .unwrap();
        assert_eq!(file.len(), 10);

        let mut whole = Vec::new();
        assert_eq!(file.read_to_end(&mut whole).unwrap(), 10);
        assert_eq!(whole, b"0123456789");
        assert_eq!(whole.capacity(), 10, "reserved exactly the bytes left");

        assert_eq!(file.seek(SeekFrom::End(-2)).unwrap(), 8);
        let mut tail = [0; 8];
        assert_eq!(file.read(&mut tail).unwrap(), 2);
        assert_eq!(&tail[..2], b"89");
        assert_eq!(file.read(&mut tail).unwrap(), 0);

        fs::write(&path, b"0123").unwrap();
        file.rewind().unwrap();
        let mut cut = Vec::new();
        assert_eq!(file.read_to_end(&mut cut).unwrap(), 4);
        assert_eq!(cut, b"0123");
    }
}
