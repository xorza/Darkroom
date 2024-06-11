use std::fmt::{Display, Formatter};
use std::mem::forget;
use std::str::Utf8Error;
use bytes::{Buf, BufMut};

#[repr(C)]
#[derive(Debug)]
pub struct FfiBuf {
    data: *mut u8,
    len: u32,
    cap: u32,
}

impl FfiBuf {
    pub fn is_null(&self) -> bool {
        self.data.is_null()
    }

    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.data, self.len as usize) }
    }

    pub fn as_str(&self) -> Result<&str, Utf8Error> {
        std::str::from_utf8(self.as_slice())
    }

    pub fn to_uuid(&self) -> uuid::Uuid {
        self.as_str().unwrap().parse().unwrap()
    }
    
}

impl Display for FfiBuf {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str().unwrap())
    }
}

impl Default for FfiBuf {
    fn default() -> Self {
        FfiBuf {
            data: std::ptr::null_mut(),
            len: 0,
            cap: 0,
        }
    }
}

impl Drop for FfiBuf {
    fn drop(&mut self) {
        if self.data.is_null() {
            return;
        }

        let len = self.len as usize;
        let cap = self.cap as usize;
        let ptr = self.data;

        unsafe {
            drop(Vec::from_raw_parts(ptr, len, cap));
        }
    }
}

impl<const N: usize, T> From<[T; N]> for FfiBuf
    where
        T: Clone,
{
    fn from(data: [T; N]) -> Self {
        data.to_vec().into()
    }
}

impl<T> From<&[T]> for FfiBuf
    where
        T: Clone,
{
    fn from(data: &[T]) -> Self {
        data.to_vec().into()
    }
}

impl From<&str> for FfiBuf {
    fn from(data: &str) -> Self {
        data.as_bytes().into()
    }
}

impl From<String> for FfiBuf {
    fn from(mut data: String) -> Self {
        let length = data.len() as u32;
        let capacity = data.capacity() as u32;
        let bytes = data.as_mut_ptr();

        forget(data);

        FfiBuf {
            data: bytes,
            len: length,
            cap: capacity,
        }
    }
}

impl TryFrom<FfiBuf> for String {
    type Error = Utf8Error;

    fn try_from(buf: FfiBuf) -> Result<Self, Self::Error> {
        Ok(buf.as_str()?.to_string())
    }
}

impl<T> From<FfiBuf> for Vec<T> {
    fn from(buf: FfiBuf) -> Self {
        let t_size = std::mem::size_of::<T>();
        if buf.len as usize % t_size != 0 {
            panic!("Invalid buffer size");
        }

        let len = buf.len as usize / t_size;
        let cap = buf.cap as usize / t_size;
        let ptr = buf.data as *mut T;

        forget(buf);

        unsafe { Vec::from_raw_parts(ptr, len, cap) }
    }
}

impl<T> From<Vec<T>> for FfiBuf {
    fn from(mut data: Vec<T>) -> Self {
        let t_size = std::mem::size_of::<T>();
        let length = (data.len() * t_size) as u32;
        let capacity = (data.capacity() * t_size) as u32;
        let bytes = data.as_mut_ptr() as *mut u8;

        forget(data);

        FfiBuf {
            data: bytes,
            len: length,
            cap: capacity,
        }
    }
}

impl FromIterator<String> for FfiBuf {
    fn from_iter<I>(iter: I) -> Self
        where
            I: IntoIterator<Item=String>,
    {
        let data: Vec<String> = iter.into_iter().collect();
        let mut bytes = Vec::<u8>::new();

        bytes.put_u32_ne(data.len() as u32);
        for s in data {
            let s = s.as_bytes();
            bytes.put_u32_ne(s.len() as u32);
            bytes.put_slice(s);
        }

        FfiBuf::from(bytes)
    }
}

pub struct FfiStrVecIter {
    data: Vec<u8>,
    count: u32,
    idx: u32,
    offset: usize,
}

impl Iterator for FfiStrVecIter {
    type Item = String;

    fn next(&mut self) -> Option<Self::Item> {
        if self.idx >= self.count {
            return None;
        }

        let mut reader = &self.data[self.offset..];
        let length = reader.get_u32_ne();
        let str = &reader[0..length as usize];

        self.offset += 4 + length as usize;
        self.idx += 1;

        Some(std::str::from_utf8(str).unwrap().to_string())
    }
}

impl IntoIterator for FfiBuf {
    type Item = String;
    type IntoIter = FfiStrVecIter;

    fn into_iter(self) -> Self::IntoIter {
        if self.is_null() {
            return FfiStrVecIter {
                data: Vec::new(),
                count: 0,
                idx: 0,
                offset: 0,
            };
        }

        let data: Vec<u8> = self.into();
        let mut reader = data.as_slice();
        let count = reader.get_u32_ne();

        FfiStrVecIter {
            data,
            count,
            idx: 0,
            offset: 4,
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffi_buf_works() {
        let buf: FfiBuf = "Hello from Rust!".into();
        let data: String = buf.try_into().unwrap();
        assert_eq!(data, "Hello from Rust!");

        let buf: FfiBuf = vec![1u8, 2, 3, 4, 5].into();
        let data: Vec<u8> = buf.into();
        assert_eq!(data, vec![1u8, 2, 3, 4, 5]);

        let buf: FfiBuf = vec![1u32, 2, 3, 4, 5].into();
        let data: Vec<u32> = buf.into();
        assert_eq!(data, vec![1u32, 2, 3, 4, 5]);

        let buf: FfiBuf = [1u32, 2, 3, 4, 5].into();
        let data: Vec<u32> = buf.into();
        assert_eq!(data, vec![1u32, 2, 3, 4, 5]);
    }

    #[test]
    fn ffi_bufstrvec_works() {
        let buf: FfiBuf = FfiBuf::from_iter(vec![
            "Hello".to_string(),
            "from".to_string(),
            "Rust!".to_string(),
        ]);
        let data: Vec<String> = buf.into_iter().collect();
        assert_eq!(data, vec!["Hello", "from", "Rust!"]);
    }
}
