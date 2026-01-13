pub trait StrExt {
    fn last_line(&self) -> &str;
    fn line_count(&self) -> usize;
}

impl StrExt for str {
    fn last_line(&self) -> &str {
        let bytes = self.as_bytes();
        let mut end = bytes.len();
        while end > 0 {
            let byte = bytes[end - 1];
            if is_newline(byte) {
                end -= 1;
            } else {
                break;
            }
        }
        if end == 0 {
            return &self[..0];
        }
        let mut idx = end;
        while idx > 0 {
            if is_newline(bytes[idx - 1]) {
                break;
            }
            idx -= 1;
        }
        &self[idx..end]
    }

    fn line_count(&self) -> usize {
        if self.is_empty() {
            return 0;
        }
        self.as_bytes().iter().filter(|&&b| is_newline(b)).count() + 1
    }
}

impl StrExt for String {
    fn last_line(&self) -> &str {
        self.as_str().last_line()
    }

    fn line_count(&self) -> usize {
        self.as_str().line_count()
    }
}

impl StrExt for &str {
    fn last_line(&self) -> &str {
        (*self).last_line()
    }

    fn line_count(&self) -> usize {
        (*self).line_count()
    }
}

fn is_newline(byte: u8) -> bool {
    byte == b'\n' || byte == b'\r'
}
