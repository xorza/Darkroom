use std::mem::forget;
use std::string::FromUtf8Error;

use graph::graph::Graph;
use imaginarium::color_format::ColorFormat;
use imaginarium::image::{Image, ImageDesc};

#[repr(C)]
pub struct FfiBuf {
    bytes: *mut u8,
    length: u32,
    capacity: u32,
}

#[no_mangle]
pub extern "C" fn test3() -> FfiBuf {
    "Hello from Rust!".into()
}

#[no_mangle]
pub extern "C" fn add(left: u32, right: u32) -> u32 {
    left + right
}

#[no_mangle]
pub extern "C" fn test1() {
    let _img = Image::new_empty(ImageDesc::new(10, 10, ColorFormat::RGB_U8));
    let _graph = Graph::default();
}

impl FfiBuf {
    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.bytes, self.length as usize) }
    }
}
impl From<&str> for FfiBuf {
    fn from(data: &str) -> Self {
        data.to_string().into()
    }
}
impl From<String> for FfiBuf {
    fn from(data: String) -> Self {
        data.into_bytes().into()
    }
}
impl TryFrom<FfiBuf> for String {
    type Error = FromUtf8Error;

    fn try_from(buf: FfiBuf) -> Result<Self, Self::Error> {
        String::from_utf8(buf.into())
    }
}
impl From<FfiBuf> for Vec<u8> {
    fn from(buf: FfiBuf) -> Self {
        let len = buf.length as usize;
        let cap = buf.capacity as usize;
        let ptr = buf.bytes;

        forget(buf);

        unsafe { Vec::from_raw_parts(ptr, len, cap) }
    }
}
impl From<Vec<u8>> for FfiBuf {
    fn from(mut data: Vec<u8>) -> Self {
        let len = data.len();
        let cap = data.capacity();
        let ptr = data.as_mut_ptr();

        forget(data);

        FfiBuf {
            bytes: ptr,
            length: len as u32,
            capacity: cap as u32,
        }
    }
}
impl Drop for FfiBuf {
    fn drop(&mut self) {
        if self.bytes.is_null() {
            return;
        }

        let len = self.length as usize;
        let cap = self.capacity as usize;
        let ptr = self.bytes;

        unsafe {
            let _ = Vec::from_raw_parts(ptr, len, cap);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let buf: FfiBuf = "Hello from Rust!".into();
        let data: String = buf.try_into().unwrap();
        assert_eq!(data, "Hello from Rust!");
    }
}
