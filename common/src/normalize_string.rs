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

#[cfg(test)]
mod tests {
    use super::*;

    // === Empty and simple cases ===

    #[test]
    fn empty_string_becomes_single_newline() {
        assert_eq!("".normalize(), "\n");
    }

    #[test]
    fn single_char_gets_trailing_newline() {
        assert_eq!("a".normalize(), "a\n");
    }

    #[test]
    fn already_normalized_unchanged() {
        assert_eq!("hello\n".normalize(), "hello\n");
    }

    #[test]
    fn multiple_lines_already_normalized() {
        assert_eq!("a\nb\nc\n".normalize(), "a\nb\nc\n");
    }

    // === Adding trailing newline ===

    #[test]
    fn adds_trailing_newline_when_missing() {
        assert_eq!("hello".normalize(), "hello\n");
    }

    #[test]
    fn adds_trailing_newline_multiline() {
        assert_eq!("a\nb\nc".normalize(), "a\nb\nc\n");
    }

    // === CRLF conversion ===

    #[test]
    fn crlf_converted_to_lf() {
        assert_eq!("hello\r\n".normalize(), "hello\n");
    }

    #[test]
    fn multiple_crlf_converted() {
        assert_eq!("a\r\nb\r\nc\r\n".normalize(), "a\nb\nc\n");
    }

    #[test]
    fn crlf_without_trailing_newline() {
        assert_eq!("a\r\nb".normalize(), "a\nb\n");
    }

    // === Standalone CR conversion ===

    #[test]
    fn standalone_cr_converted_to_lf() {
        assert_eq!("hello\r".normalize(), "hello\n");
    }

    #[test]
    fn multiple_standalone_cr_converted() {
        assert_eq!("a\rb\rc\r".normalize(), "a\nb\nc\n");
    }

    #[test]
    fn standalone_cr_without_trailing() {
        assert_eq!("a\rb".normalize(), "a\nb\n");
    }

    // === Mixed line endings ===

    #[test]
    fn mixed_lf_crlf_cr() {
        assert_eq!("a\nb\r\nc\rd".normalize(), "a\nb\nc\nd\n");
    }

    #[test]
    fn mixed_endings_with_trailing_lf() {
        assert_eq!("a\r\nb\rc\n".normalize(), "a\nb\nc\n");
    }

    // === Consecutive newlines ===

    #[test]
    fn consecutive_lf_preserved() {
        assert_eq!("a\n\n\nb".normalize(), "a\n\n\nb\n");
    }

    #[test]
    fn consecutive_crlf_converted() {
        assert_eq!("a\r\n\r\n\r\nb".normalize(), "a\n\n\nb\n");
    }

    #[test]
    fn consecutive_cr_converted() {
        assert_eq!("a\r\r\rb".normalize(), "a\n\n\nb\n");
    }

    // === Only newline characters ===

    #[test]
    fn just_lf() {
        assert_eq!("\n".normalize(), "\n");
    }

    #[test]
    fn just_crlf() {
        assert_eq!("\r\n".normalize(), "\n");
    }

    #[test]
    fn just_cr() {
        assert_eq!("\r".normalize(), "\n");
    }

    #[test]
    fn multiple_lf_only() {
        assert_eq!("\n\n\n".normalize(), "\n\n\n");
    }

    #[test]
    fn multiple_crlf_only() {
        assert_eq!("\r\n\r\n\r\n".normalize(), "\n\n\n");
    }

    #[test]
    fn multiple_cr_only() {
        assert_eq!("\r\r\r".normalize(), "\n\n\n");
    }

    // === Edge cases ===

    #[test]
    fn cr_at_start() {
        assert_eq!("\rhello".normalize(), "\nhello\n");
    }

    #[test]
    fn crlf_at_start() {
        assert_eq!("\r\nhello".normalize(), "\nhello\n");
    }

    #[test]
    fn lf_at_start() {
        assert_eq!("\nhello".normalize(), "\nhello\n");
    }

    #[test]
    fn text_between_crlf() {
        assert_eq!("\r\ntext\r\n".normalize(), "\ntext\n");
    }

    // === String and &str impls ===

    #[test]
    fn string_type_works() {
        let s = String::from("hello\r\nworld");
        assert_eq!(s.normalize(), "hello\nworld\n");
    }

    #[test]
    fn str_ref_works() {
        let s: &str = "hello\r\nworld";
        assert_eq!(s.normalize(), "hello\nworld\n");
    }

    // === Unicode preservation ===

    #[test]
    fn unicode_preserved() {
        assert_eq!("héllo\r\nwörld".normalize(), "héllo\nwörld\n");
    }

    #[test]
    fn emoji_preserved() {
        assert_eq!("hello 🎉\r\nworld 🌍".normalize(), "hello 🎉\nworld 🌍\n");
    }

    #[test]
    fn chinese_characters_preserved() {
        assert_eq!("你好\r\n世界".normalize(), "你好\n世界\n");
    }
}
