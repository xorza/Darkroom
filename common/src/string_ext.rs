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
        let bytes = self.as_bytes();
        let mut count = 0;
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'\r' {
                count += 1;
                // Skip \n if it follows \r (treat \r\n as single line ending)
                if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    i += 1;
                }
            } else if bytes[i] == b'\n' {
                count += 1;
            }
            i += 1;
        }
        count + 1
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

#[cfg(test)]
mod tests {
    use super::*;

    // === line_count tests ===

    #[test]
    fn line_count_empty_string() {
        assert_eq!("".line_count(), 0);
    }

    #[test]
    fn line_count_single_line_no_newline() {
        assert_eq!("hello".line_count(), 1);
    }

    #[test]
    fn line_count_single_line_with_lf() {
        assert_eq!("hello\n".line_count(), 2);
    }

    #[test]
    fn line_count_single_line_with_crlf() {
        assert_eq!("hello\r\n".line_count(), 2);
    }

    #[test]
    fn line_count_single_line_with_cr() {
        assert_eq!("hello\r".line_count(), 2);
    }

    #[test]
    fn line_count_multiple_lines_lf() {
        assert_eq!("a\nb\nc".line_count(), 3);
    }

    #[test]
    fn line_count_multiple_lines_crlf() {
        assert_eq!("a\r\nb\r\nc".line_count(), 3);
    }

    #[test]
    fn line_count_multiple_lines_cr() {
        assert_eq!("a\rb\rc".line_count(), 3);
    }

    #[test]
    fn line_count_mixed_line_endings() {
        assert_eq!("a\nb\r\nc\rd".line_count(), 4);
    }

    #[test]
    fn line_count_consecutive_lf() {
        assert_eq!("\n\n\n".line_count(), 4);
    }

    #[test]
    fn line_count_consecutive_crlf() {
        assert_eq!("\r\n\r\n".line_count(), 3);
    }

    #[test]
    fn line_count_just_lf() {
        assert_eq!("\n".line_count(), 2);
    }

    #[test]
    fn line_count_just_crlf() {
        assert_eq!("\r\n".line_count(), 2);
    }

    #[test]
    fn line_count_just_cr() {
        assert_eq!("\r".line_count(), 2);
    }

    #[test]
    fn line_count_string_type() {
        let s = String::from("a\nb\nc");
        assert_eq!(s.line_count(), 3);
    }

    #[test]
    fn line_count_str_ref() {
        let s: &str = "a\nb";
        assert_eq!(s.line_count(), 2);
    }

    // === last_line tests ===

    #[test]
    fn last_line_empty_string() {
        assert_eq!("".last_line(), "");
    }

    #[test]
    fn last_line_single_line_no_newline() {
        assert_eq!("hello".last_line(), "hello");
    }

    #[test]
    fn last_line_single_line_with_lf() {
        assert_eq!("hello\n".last_line(), "hello");
    }

    #[test]
    fn last_line_single_line_with_crlf() {
        assert_eq!("hello\r\n".last_line(), "hello");
    }

    #[test]
    fn last_line_single_line_with_cr() {
        assert_eq!("hello\r".last_line(), "hello");
    }

    #[test]
    fn last_line_multiple_lines_lf() {
        assert_eq!("first\nsecond\nthird".last_line(), "third");
    }

    #[test]
    fn last_line_multiple_lines_crlf() {
        assert_eq!("first\r\nsecond\r\nthird".last_line(), "third");
    }

    #[test]
    fn last_line_multiple_lines_cr() {
        assert_eq!("first\rsecond\rthird".last_line(), "third");
    }

    #[test]
    fn last_line_trailing_newline_lf() {
        assert_eq!("first\nsecond\n".last_line(), "second");
    }

    #[test]
    fn last_line_trailing_newline_crlf() {
        assert_eq!("first\r\nsecond\r\n".last_line(), "second");
    }

    #[test]
    fn last_line_multiple_trailing_newlines() {
        assert_eq!("first\nsecond\n\n\n".last_line(), "second");
    }

    #[test]
    fn last_line_only_newlines() {
        assert_eq!("\n\n\n".last_line(), "");
    }

    #[test]
    fn last_line_just_lf() {
        assert_eq!("\n".last_line(), "");
    }

    #[test]
    fn last_line_just_crlf() {
        assert_eq!("\r\n".last_line(), "");
    }

    #[test]
    fn last_line_mixed_line_endings() {
        assert_eq!("a\nb\r\nc\rd".last_line(), "d");
    }

    #[test]
    fn last_line_string_type() {
        let s = String::from("a\nb\nc");
        assert_eq!(s.last_line(), "c");
    }

    #[test]
    fn last_line_str_ref() {
        let s: &str = "a\nb";
        assert_eq!(s.last_line(), "b");
    }
}
