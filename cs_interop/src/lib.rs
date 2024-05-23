#![allow(dead_code)]
#![deny(improper_ctypes_definitions)]

use std::mem::forget;
use std::str::Utf8Error;

use bytes::{Buf, BufMut};

use graph::ctx::Context;
use graph::elements::basic_invoker::BasicInvoker;
use graph::elements::timers_invoker::TimersInvoker;
use graph::invoke::Invoker;

mod func_lib_api;
mod graph_api;

#[repr(C)]
#[derive(Debug)]
struct FfiBuf {
    bytes: *mut u8,
    length: u32,
    capacity: u32,
}

#[repr(C)]
#[derive(Debug)]
struct Id(FfiBuf);

#[repr(C)]
#[derive(Default, Debug)]
struct FfiStr(FfiBuf);

#[repr(C)]
#[derive(Default, Debug)]
struct FfiStrVec(FfiBuf);

#[no_mangle]
extern "C" fn create_context() -> *mut u8 {
    let mut context = Box::<Context>::default();
    context.invoker.merge(BasicInvoker::default());
    context.invoker.merge(TimersInvoker::default());
    context.func_lib.merge(context.invoker.get_func_lib());

    Box::into_raw(context) as *mut u8
}

#[no_mangle]
extern "C" fn destroy_context(ctx: *mut u8) {
    unsafe { drop(Box::<Context>::from_raw(ctx as *mut Context)) };
}

fn get_context<'a>(ctx: *mut u8) -> &'a mut Context {
    unsafe { &mut *(ctx as *mut Context) }
}

#[no_mangle]
extern "C" fn dummy(_a: FfiBuf, _b: FfiStr, _c: FfiStrVec, _d: Id) {}

impl FfiBuf {
    pub fn is_null(&self) -> bool {
        self.bytes.is_null()
    }

    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.bytes, self.length as usize) }
    }

    pub fn as_str(&self) -> Result<&str, Utf8Error> {
        std::str::from_utf8(self.as_slice())
    }
}

impl Default for FfiBuf {
    fn default() -> Self {
        FfiBuf {
            bytes: std::ptr::null_mut(),
            length: 0,
            capacity: 0,
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
            bytes,
            length,
            capacity,
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
        let length = (data.len() * t_size) as u32;
        let capacity = (data.capacity() * t_size) as u32;
        let bytes = data.as_mut_ptr() as *mut u8;

        forget(data);

        FfiBuf {
            bytes,
            length,
            capacity,
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

impl FfiStr {
    pub fn is_null(&self) -> bool {
        self.0.is_null()
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str().unwrap()
    }
}

impl From<&str> for FfiStr {
    fn from(data: &str) -> Self {
        FfiStr(data.as_bytes().into())
    }
}

impl From<String> for FfiStr {
    fn from(data: String) -> Self {
        FfiStr(data.into_bytes().into())
    }
}

impl From<FfiStr> for String {
    fn from(buf: FfiStr) -> Self {
        buf.0.try_into().unwrap()
    }
}

impl FfiStrVec {
    pub fn is_null(&self) -> bool {
        self.0.is_null()
    }
}

impl FromIterator<String> for FfiStrVec {
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = String>,
    {
        let data: Vec<String> = iter.into_iter().collect();
        let mut bytes = Vec::<u8>::new();

        bytes.put_u32_ne(data.len() as u32);
        for s in data {
            let s = s.as_bytes();
            bytes.put_u32_ne(s.len() as u32);
            bytes.put_slice(s);
        }

        FfiStrVec(bytes.into())
    }
}

struct FfiStrVecIter {
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

impl IntoIterator for FfiStrVec {
    type Item = String;
    type IntoIter = FfiStrVecIter;

    fn into_iter(self) -> Self::IntoIter {
        if self.0.is_null() {
            return FfiStrVecIter {
                data: Vec::new(),
                count: 0,
                idx: 0,
                offset: 0,
            };
        }

        let data: Vec<u8> = self.0.into();
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
        let buf: FfiStrVec = FfiStrVec::from_iter(vec![
            "Hello".to_string(),
            "from".to_string(),
            "Rust!".to_string(),
        ]);
        let data: Vec<String> = buf.into_iter().collect();
        assert_eq!(data, vec!["Hello", "from", "Rust!"]);
    }
}
