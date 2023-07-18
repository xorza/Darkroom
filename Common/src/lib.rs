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

pub trait Apply<T> {
    fn apply(&mut self, f: fn(&mut T));
}
impl<T> Apply<T> for Option<T> {
    fn apply(&mut self, f: fn(&mut T)) {
        if let Some(v) = self.as_mut() {
            f(v);
        }
    }
}
