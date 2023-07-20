use std::path::Path;

#[macro_use]
pub mod macros;

pub const EPSILON: f64 = 1e-10;

pub fn get_file_extension(filename: &str) -> anyhow::Result<&str> {
    let extension = Path::new(filename)
        .extension()
        .and_then(|os_str| os_str.to_str())
        .ok_or(anyhow::anyhow!("Failed to get file extension"))?;

    Ok(extension)
}

pub fn is_debug() -> bool {
    cfg!(debug_assertions)
}


pub trait Apply<T, F>
where
    F: FnOnce(&T),
{
    fn apply(&self, f: F);
}
impl<T, F> Apply<T, F> for Option<&T>
where
    F: FnOnce(&T),
{
    fn apply(&self, f: F) {
        if let Some(v) = self {
            f(*v);
        }
    }
}

pub trait ApplyMut<T, F>
where
    F: FnOnce(&mut T),
{
    fn apply_mut(&mut self, f: F);
}
impl<T, F> ApplyMut<T, F> for Option<&mut T>
where
    F: FnOnce(&mut T),
{
    fn apply_mut(&mut self, f: F) {
        if let Some(v) = self {
            f(*v);
        }
    }
}

pub trait TakeWith<T, F>
where
    F: FnOnce(T),
{
    fn take_with(&mut self, f: F);
}
impl<T, F> TakeWith<T, F> for Option<T>
where
    F: FnOnce(T),
{
    fn take_with(&mut self, f: F) {
        if let Some(v) = self.take() {
            f(v);
        }
    }
}