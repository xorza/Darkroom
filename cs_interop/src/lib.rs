#![allow(dead_code)]
#![deny(improper_ctypes_definitions)]

use graph::ctx::Context;
use std::mem::forget;
use std::string::FromUtf8Error;

#[repr(C)]
struct FfiBuf {
    bytes: *mut u8,
    length: u32,
    capacity: u32,
}

#[no_mangle]
extern "C" fn create_context() -> *mut u8 {
    Box::into_raw(Box::<Context>::default()) as *mut u8
}

#[no_mangle]
extern "C" fn destroy_context(ctx: *mut u8) {
    unsafe { drop(Box::<Context>::from_raw(ctx as *mut Context)) };
}

#[no_mangle]
extern "C" fn dummy(_a: FfiBuf) {}

impl FfiBuf {
    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.bytes, self.length as usize) }
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

impl<T> From<FfiBuf> for Vec<T> {
    fn from(buf: FfiBuf) -> Self {
        let t_size = std::mem::size_of::<T>();
        if buf.length as usize % t_size != 0 {
            panic!("Invalid buffer size");
        }

        let len = buf.length as usize / t_size;
        let cap = buf.capacity as usize / t_size;
        let ptr = buf.bytes as *mut T;

        forget(buf);

        unsafe { Vec::from_raw_parts(ptr, len, cap) }
    }
}

impl<T> From<Vec<T>> for FfiBuf {
    fn from(mut data: Vec<T>) -> Self {
        let t_size = std::mem::size_of::<T>();
        let len = data.len() * t_size;
        let cap = data.capacity() * t_size;
        let ptr = data.as_mut_ptr();

        forget(data);

        FfiBuf {
            bytes: ptr as *mut u8,
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
}
