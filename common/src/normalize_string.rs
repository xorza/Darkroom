pub trait NormalizeString {
    /// Normalizes line endings by stripping `\r` and guarantees a trailing `\n`.
    fn normalize(&self) -> String;
}

impl NormalizeString for str {
    fn normalize(&self) -> String {
        let bytes = self.as_bytes();
        let mut out = String::new();
        let mut last = 0;
        let mut idx = 0;
        let mut changed = false;

        while idx < bytes.len() {
            if bytes[idx] == b'\r' {
                if !changed {
                    out = String::with_capacity(self.len());
                    changed = true;
                }
                out.push_str(&self[last..idx]);
                if idx + 1 < bytes.len() && bytes[idx + 1] == b'\n' {
                    idx += 1;
                }
                out.push('\n');
                idx += 1;
                last = idx;
            } else {
                idx += 1;
            }
        }

        if !changed {
            if self.ends_with('\n') {
                return self.to_string();
            }

            let mut out = String::with_capacity(self.len() + 1);
            out.push_str(self);
            out.push('\n');
            return out;
        }

        out.push_str(&self[last..]);
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out
    }
}

impl NormalizeString for String {
    fn normalize(&self) -> String {
        self.as_str().normalize()
    }
}

impl NormalizeString for &str {
    fn normalize(&self) -> String {
        (*self).normalize()
    }
}
