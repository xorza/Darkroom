pub trait LastLine {
    fn last_line(&self) -> &str;
}

impl LastLine for str {
    fn last_line(&self) -> &str {
        let bytes = self.as_bytes();
        let mut end = bytes.len();
        while end > 0 {
            let byte = bytes[end - 1];
            if byte == b'\n' || byte == b'\r' {
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
            if bytes[idx - 1] == b'\n' {
                break;
            }
            idx -= 1;
        }
        &self[idx..end]
    }
}

impl LastLine for String {
    fn last_line(&self) -> &str {
        self.as_str().last_line()
    }
}

impl LastLine for &str {
    fn last_line(&self) -> &str {
        (*self).last_line()
    }
}
