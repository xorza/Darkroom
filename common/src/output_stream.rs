use std::sync::Arc;

use parking_lot::Mutex;

#[derive(Debug, Default, Clone)]
pub struct OutputStream(Arc<Mutex<Vec<String>>>);

impl OutputStream {
    pub fn new() -> Self {
        OutputStream(Arc::new(Mutex::new(Vec::new())))
    }

    pub fn write<S: Into<String>>(&self, s: S) {
        self.0.lock().push(s.into());
    }

    pub fn take(&self) -> Vec<String> {
        std::mem::take(&mut self.0.lock())
    }
}
