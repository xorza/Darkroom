use super::error::{Result, ScnError};
use super::value::ScnValue;

// ===========================================================================
// Tokenizer
// ===========================================================================

#[derive(Debug, PartialEq)]
enum Token {
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Colon,
    Comma,
    Null,
    True,
    False,
    Int(i128),
    Uint(u128),
    Float(f64),
    String(String),
    /// Identifier that is not a keyword — used for variant tags and bare keys.
    Ident(String),
    Eof,
}

#[derive(Debug)]
struct Lexer<'a> {
    input: &'a [u8],
    pos: usize,
    line: usize,
    col: usize,
}

impl<'a> Lexer<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input: input.as_bytes(),
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    fn error(&self, msg: impl Into<String>) -> ScnError {
        ScnError::Parse {
            line: self.line,
            col: self.col,
            message: msg.into(),
        }
    }

    fn peek_byte(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn advance(&mut self) -> u8 {
        let b = self.input[self.pos];
        self.pos += 1;
        if b == b'\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        b
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            // Skip whitespace
            while self.pos < self.input.len()
                && matches!(self.input[self.pos], b' ' | b'\t' | b'\r' | b'\n')
            {
                self.advance();
            }
            // Skip // comments
            if self.pos + 1 < self.input.len()
                && self.input[self.pos] == b'/'
                && self.input[self.pos + 1] == b'/'
            {
                while self.pos < self.input.len() && self.input[self.pos] != b'\n' {
                    self.advance();
                }
                continue;
            }
            break;
        }
    }

    fn next_token(&mut self) -> Result<Token> {
        self.skip_whitespace_and_comments();

        let Some(b) = self.peek_byte() else {
            return Ok(Token::Eof);
        };

        match b {
            b'{' => {
                self.advance();
                Ok(Token::LBrace)
            }
            b'}' => {
                self.advance();
                Ok(Token::RBrace)
            }
            b'[' => {
                self.advance();
                Ok(Token::LBracket)
            }
            b']' => {
                self.advance();
                Ok(Token::RBracket)
            }
            b':' => {
                self.advance();
                Ok(Token::Colon)
            }
            b',' => {
                self.advance();
                Ok(Token::Comma)
            }
            b'"' => self.read_string(),
            b'-' => self.read_number(),
            b'0'..=b'9' => self.read_number(),
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => self.read_ident_or_keyword(),
            _ => Err(self.error(format!("unexpected character: {:?}", b as char))),
        }
    }

    fn read_string(&mut self) -> Result<Token> {
        assert_eq!(self.input[self.pos], b'"');
        self.advance(); // consume opening "

        // Check for triple-quote
        if self.pos + 1 < self.input.len()
            && self.input[self.pos] == b'"'
            && self.input[self.pos + 1] == b'"'
        {
            self.advance(); // second "
            self.advance(); // third "
            return self.read_triple_quoted_string();
        }

        // Fast path: scan for closing quote without escapes or non-ASCII.
        // This avoids per-char String::push for the common case.
        let scan_start = self.pos;
        loop {
            if self.pos >= self.input.len() {
                // Unterminated — fall through to slow path for proper error
                break;
            }
            let b = self.input[self.pos];
            if b == b'"' {
                // Found closing quote — return slice directly
                let s = std::str::from_utf8(&self.input[scan_start..self.pos])
                    .map_err(|_| self.error("invalid UTF-8 in string"))?;
                let result = s.to_string();
                self.col += self.pos - scan_start + 1; // +1 for closing "
                self.pos += 1; // skip closing "
                return Ok(Token::String(result));
            }
            if b == b'\\' || !(0x20..0x80).contains(&b) {
                // Needs slow path for escapes, non-ASCII, or control chars
                break;
            }
            self.pos += 1;
        }

        // Rewind and use slow path (handles escapes, non-ASCII, errors)
        self.pos = scan_start;
        self.read_string_slow()
    }

    fn read_string_slow(&mut self) -> Result<Token> {
        let mut result = String::new();
        loop {
            if self.pos >= self.input.len() {
                return Err(self.error("unterminated string"));
            }
            let b = self.advance();
            match b {
                b'"' => return Ok(Token::String(result)),
                b'\\' => {
                    if self.pos >= self.input.len() {
                        return Err(self.error("unterminated escape sequence"));
                    }
                    let esc = self.advance();
                    match esc {
                        b'\\' => result.push('\\'),
                        b'"' => result.push('"'),
                        b'n' => result.push('\n'),
                        b'r' => result.push('\r'),
                        b't' => result.push('\t'),
                        b'0' => result.push('\0'),
                        b'u' => {
                            let ch = self.read_unicode_escape()?;
                            result.push(ch);
                        }
                        _ => {
                            return Err(
                                self.error(format!("unknown escape sequence: \\{}", esc as char))
                            );
                        }
                    }
                }
                _ => {
                    if b < 0x80 {
                        result.push(b as char);
                    } else {
                        // Multi-byte UTF-8: decode from leading byte pattern.
                        // Rewind the byte we already consumed via advance().
                        self.pos -= 1;
                        self.col -= 1;
                        let ch = self.decode_utf8_char()?;
                        result.push(ch);
                    }
                }
            }
        }
    }

    fn read_triple_quoted_string(&mut self) -> Result<Token> {
        // We've consumed the opening """. Now read until closing """.
        // Skip the first newline after opening """ if present.
        if self.pos < self.input.len() && self.input[self.pos] == b'\n' {
            self.advance();
        } else if self.pos + 1 < self.input.len()
            && self.input[self.pos] == b'\r'
            && self.input[self.pos + 1] == b'\n'
        {
            self.advance();
            self.advance();
        }

        let content_start = self.pos;
        // Find closing """
        loop {
            if self.pos + 2 >= self.input.len() {
                return Err(self.error("unterminated triple-quoted string"));
            }
            if self.input[self.pos] == b'"'
                && self.input[self.pos + 1] == b'"'
                && self.input[self.pos + 2] == b'"'
            {
                let content_end = self.pos;
                self.advance(); // "
                self.advance(); // "
                self.advance(); // "

                let raw = std::str::from_utf8(&self.input[content_start..content_end])
                    .map_err(|_| self.error("invalid UTF-8 in triple-quoted string"))?;

                // Determine indent from closing """ line
                let closing_indent = self.find_closing_indent(raw);
                let result = strip_leading_indent(raw, closing_indent);
                return Ok(Token::String(result));
            }
            self.advance();
        }
    }

    fn find_closing_indent(&self, raw: &str) -> usize {
        // The closing """ indent is determined by the whitespace on the last line
        if let Some(last_newline) = raw.rfind('\n') {
            let last_line = &raw[last_newline + 1..];
            last_line.len() - last_line.trim_start().len()
        } else {
            0
        }
    }

    fn read_unicode_escape(&mut self) -> Result<char> {
        if self.pos >= self.input.len() || self.input[self.pos] != b'{' {
            return Err(self.error("expected '{' after \\u"));
        }
        self.advance(); // consume {

        let start = self.pos;
        while self.pos < self.input.len() && self.input[self.pos] != b'}' {
            self.advance();
        }
        if self.pos >= self.input.len() {
            return Err(self.error("unterminated unicode escape"));
        }
        let hex = std::str::from_utf8(&self.input[start..self.pos])
            .map_err(|_| self.error("invalid UTF-8 in unicode escape"))?;
        self.advance(); // consume }

        let codepoint = u32::from_str_radix(hex, 16)
            .map_err(|_| self.error(format!("invalid hex in unicode escape: {hex}")))?;
        char::from_u32(codepoint)
            .ok_or_else(|| self.error(format!("invalid unicode codepoint: U+{codepoint:X}")))
    }

    /// Decode a single UTF-8 character at the current position.
    /// O(1) per character — decodes from leading byte pattern directly.
    /// Note: error paths are unreachable from the public API since `parse()` takes `&str`
    /// (guaranteed valid UTF-8). The checks are defensive for correctness.
    fn decode_utf8_char(&mut self) -> Result<char> {
        let b0 = self.input[self.pos];
        let (len, min_cp) = match b0 {
            0xC0..=0xDF => (2, 0x80u32),
            0xE0..=0xEF => (3, 0x800),
            0xF0..=0xF7 => (4, 0x1_0000),
            _ => return Err(self.error("invalid UTF-8 byte")),
        };
        if self.pos + len > self.input.len() {
            return Err(self.error("truncated UTF-8 sequence"));
        }
        // Decode: extract payload bits from leading byte, then continuation bytes
        let mut cp = (b0 as u32) & (0x7F >> len);
        for i in 1..len {
            let b = self.input[self.pos + i];
            if b & 0xC0 != 0x80 {
                return Err(self.error("invalid UTF-8 continuation byte"));
            }
            cp = (cp << 6) | (b as u32 & 0x3F);
        }
        // Reject overlong encodings and surrogates
        if cp < min_cp || (0xD800..=0xDFFF).contains(&cp) {
            return Err(self.error("invalid UTF-8 encoding"));
        }
        let ch = char::from_u32(cp).ok_or_else(|| self.error("invalid UTF-8 codepoint"))?;
        for _ in 0..len {
            self.advance();
        }
        Ok(ch)
    }

    fn read_number(&mut self) -> Result<Token> {
        let start = self.pos;
        let negative = self.input[self.pos] == b'-';
        if negative {
            self.advance();
            // Check for -inf / -nan
            if self.input[self.pos..].starts_with(b"inf") && !self.is_ident_byte_at(self.pos + 3) {
                self.advance();
                self.advance();
                self.advance();
                return Ok(Token::Float(f64::NEG_INFINITY));
            }
            if self.input[self.pos..].starts_with(b"nan") && !self.is_ident_byte_at(self.pos + 3) {
                self.advance();
                self.advance();
                self.advance();
                return Ok(Token::Float(f64::NAN));
            }
        }

        // Must have at least one digit
        if self.pos >= self.input.len() || !self.input[self.pos].is_ascii_digit() {
            return Err(self.error("expected digit after '-'"));
        }

        let first_digit = self.input[self.pos];
        self.advance();

        // Check for hex/octal/binary prefix after leading 0
        if first_digit == b'0' && self.pos < self.input.len() {
            match self.input[self.pos] {
                b'x' | b'X' => {
                    self.advance();
                    return self.read_prefixed_integer(negative, 16, |d| d.is_ascii_hexdigit());
                }
                b'o' | b'O' => {
                    self.advance();
                    return self.read_prefixed_integer(negative, 8, |d| matches!(d, b'0'..=b'7'));
                }
                b'b' | b'B' => {
                    self.advance();
                    return self.read_prefixed_integer(negative, 2, |d| matches!(d, b'0' | b'1'));
                }
                _ => {}
            }
        }

        // Reject leading zeros (except 0 itself, 0.x, 0eN)
        if first_digit == b'0'
            && self.pos < self.input.len()
            && (self.input[self.pos].is_ascii_digit() || self.input[self.pos] == b'_')
        {
            return Err(self.error("leading zeros are not allowed"));
        }

        // Remaining integer digits with underscores
        self.scan_digits_with_underscores()?;

        let mut is_float = false;

        // Fractional part
        if self.pos < self.input.len() && self.input[self.pos] == b'.' {
            is_float = true;
            self.advance();
            if self.pos >= self.input.len() || !self.input[self.pos].is_ascii_digit() {
                return Err(self.error("expected digit after '.'"));
            }
            self.advance();
            self.scan_digits_with_underscores()?;
        }

        // Exponent
        if self.pos < self.input.len() && matches!(self.input[self.pos], b'e' | b'E') {
            is_float = true;
            self.advance();
            if self.pos < self.input.len() && matches!(self.input[self.pos], b'+' | b'-') {
                self.advance();
            }
            if self.pos >= self.input.len() || !self.input[self.pos].is_ascii_digit() {
                return Err(self.error("expected digit in exponent"));
            }
            self.advance();
            self.scan_digits_with_underscores()?;
        }

        let raw = std::str::from_utf8(&self.input[start..self.pos]).unwrap();

        // Strip underscores for parsing
        let owned;
        let text = if raw.contains('_') {
            owned = raw.replace('_', "");
            owned.as_str()
        } else {
            raw
        };

        if is_float {
            let f: f64 = text
                .parse()
                .map_err(|_| self.error(format!("invalid float: {raw}")))?;
            Ok(Token::Float(f))
        } else if negative {
            let i: i128 = text
                .parse()
                .map_err(|_| self.error(format!("invalid integer: {raw}")))?;
            Ok(Token::Int(i))
        } else {
            // Try u128 first for large positive numbers, fall back to i128
            if let Ok(u) = text.parse::<u128>() {
                if u > i128::MAX as u128 {
                    Ok(Token::Uint(u))
                } else {
                    Ok(Token::Int(u as i128))
                }
            } else {
                Err(self.error(format!("invalid integer: {raw}")))
            }
        }
    }

    fn read_ident_or_keyword(&mut self) -> Result<Token> {
        let start = self.pos;
        while self.pos < self.input.len()
            && matches!(self.input[self.pos], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_')
        {
            self.advance();
        }
        let ident = std::str::from_utf8(&self.input[start..self.pos]).unwrap();
        match ident {
            "null" => Ok(Token::Null),
            "true" => Ok(Token::True),
            "false" => Ok(Token::False),
            "nan" => Ok(Token::Float(f64::NAN)),
            "inf" => Ok(Token::Float(f64::INFINITY)),
            _ => Ok(Token::Ident(ident.to_string())),
        }
    }

    fn is_ident_byte_at(&self, pos: usize) -> bool {
        pos < self.input.len()
            && matches!(self.input[pos], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_')
    }

    /// Scan additional digits and underscores after an already-consumed digit.
    /// Validates no trailing underscore and no consecutive underscores.
    fn scan_digits_with_underscores(&mut self) -> Result<()> {
        let mut prev_underscore = false;
        while self.pos < self.input.len() {
            let b = self.input[self.pos];
            if b.is_ascii_digit() {
                prev_underscore = false;
                self.advance();
            } else if b == b'_' {
                if prev_underscore {
                    return Err(self.error("consecutive underscores in number"));
                }
                prev_underscore = true;
                self.advance();
            } else {
                break;
            }
        }
        if prev_underscore {
            return Err(self.error("trailing underscore in number"));
        }
        Ok(())
    }

    /// Read a hex/octal/binary integer after the `0x`/`0o`/`0b` prefix has been consumed.
    fn read_prefixed_integer(
        &mut self,
        negative: bool,
        radix: u32,
        is_valid_digit: fn(u8) -> bool,
    ) -> Result<Token> {
        if self.pos >= self.input.len() || !is_valid_digit(self.input[self.pos]) {
            return Err(self.error(format!("expected base-{radix} digit after prefix")));
        }

        let digit_start = self.pos;
        let mut prev_underscore = false;
        while self.pos < self.input.len() {
            let b = self.input[self.pos];
            if is_valid_digit(b) {
                prev_underscore = false;
                self.advance();
            } else if b == b'_' {
                if prev_underscore {
                    return Err(self.error("consecutive underscores in number"));
                }
                prev_underscore = true;
                self.advance();
            } else {
                break;
            }
        }

        if prev_underscore {
            return Err(self.error("trailing underscore in number"));
        }

        let raw = std::str::from_utf8(&self.input[digit_start..self.pos]).unwrap();
        let owned;
        let digits = if raw.contains('_') {
            owned = raw.replace('_', "");
            owned.as_str()
        } else {
            raw
        };

        let value = u128::from_str_radix(digits, radix)
            .map_err(|_| self.error(format!("invalid base-{radix} integer")))?;

        self.make_integer_token(negative, value)
    }

    fn make_integer_token(&self, negative: bool, value: u128) -> Result<Token> {
        if negative {
            if value <= i128::MAX as u128 {
                Ok(Token::Int(-(value as i128)))
            } else if value == i128::MAX as u128 + 1 {
                Ok(Token::Int(i128::MIN))
            } else {
                Err(self.error("integer overflow: value too large for signed integer"))
            }
        } else if value > i128::MAX as u128 {
            Ok(Token::Uint(value))
        } else {
            Ok(Token::Int(value as i128))
        }
    }
}

// ===========================================================================
// Parser: tokens → ScnValue
// ===========================================================================

const MAX_DEPTH: usize = 128;

struct Parser<'a> {
    lexer: Lexer<'a>,
    /// Peeked token (single lookahead).
    peeked: Option<Token>,
    /// Current nesting depth (arrays, maps, variants).
    depth: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            lexer: Lexer::new(input),
            peeked: None,
            depth: 0,
        }
    }

    fn enter_nested(&mut self) -> Result<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            Err(self
                .lexer
                .error(format!("exceeded maximum nesting depth of {MAX_DEPTH}")))
        } else {
            Ok(())
        }
    }

    fn leave_nested(&mut self) {
        self.depth -= 1;
    }

    fn peek(&mut self) -> Result<&Token> {
        if self.peeked.is_none() {
            self.peeked = Some(self.lexer.next_token()?);
        }
        Ok(self.peeked.as_ref().unwrap())
    }

    fn next(&mut self) -> Result<Token> {
        if let Some(tok) = self.peeked.take() {
            Ok(tok)
        } else {
            self.lexer.next_token()
        }
    }

    fn expect(&mut self, expected: &Token) -> Result<()> {
        let tok = self.next()?;
        if std::mem::discriminant(&tok) != std::mem::discriminant(expected) {
            return Err(self
                .lexer
                .error(format!("expected {expected:?}, got {tok:?}")));
        }
        Ok(())
    }

    fn parse_value(&mut self) -> Result<ScnValue> {
        // Peek to determine production, but only consume via next() to avoid cloning
        // heap-allocated String/Ident tokens.
        match self.peek()? {
            Token::Null => {
                self.next()?;
                Ok(ScnValue::Null)
            }
            Token::True => {
                self.next()?;
                Ok(ScnValue::Bool(true))
            }
            Token::False => {
                self.next()?;
                Ok(ScnValue::Bool(false))
            }
            Token::Int(_)
            | Token::Uint(_)
            | Token::Float(_)
            | Token::String(_)
            | Token::Ident(_) => {
                let tok = self.next()?;
                match tok {
                    Token::Int(i) => Ok(ScnValue::Int(i)),
                    Token::Uint(u) => Ok(ScnValue::Uint(u)),
                    Token::Float(f) => Ok(ScnValue::Float(f)),
                    Token::String(s) => Ok(ScnValue::String(s)),
                    Token::Ident(tag) => self.parse_variant(tag),
                    _ => unreachable!(),
                }
            }
            Token::LBrace => self.parse_map(),
            Token::LBracket => self.parse_array(),
            _ => {
                let tok = self.next()?;
                Err(self.lexer.error(format!("unexpected token: {tok:?}")))
            }
        }
    }

    fn parse_array(&mut self) -> Result<ScnValue> {
        self.expect(&Token::LBracket)?;
        self.enter_nested()?;
        let mut items = Vec::new();

        loop {
            if matches!(self.peek()?, Token::RBracket) {
                self.next()?;
                self.leave_nested();
                return Ok(ScnValue::Array(items));
            }
            items.push(self.parse_value()?);
            // Optional comma after item (separator or trailing)
            if matches!(self.peek()?, Token::Comma) {
                self.next()?;
            }
        }
    }

    fn parse_map(&mut self) -> Result<ScnValue> {
        self.expect(&Token::LBrace)?;
        self.enter_nested()?;
        let mut entries = Vec::new();

        loop {
            if matches!(self.peek()?, Token::RBrace) {
                self.next()?;
                self.leave_nested();
                return Ok(ScnValue::Map(entries));
            }

            let key = self.parse_key()?;
            if entries.iter().any(|(k, _)| k == &key) {
                return Err(self.lexer.error(format!("duplicate map key: {key:?}")));
            }
            self.expect(&Token::Colon)?;
            let value = self.parse_value()?;
            entries.push((key, value));

            // Optional comma after entry (separator or trailing)
            if matches!(self.peek()?, Token::Comma) {
                self.next()?;
            }
        }
    }

    fn parse_key(&mut self) -> Result<String> {
        let tok = self.next()?;
        match tok {
            Token::Ident(s) => Ok(s),
            Token::String(s) => Ok(s),
            // Allow keywords as keys (common in data)
            Token::True => Ok("true".to_string()),
            Token::False => Ok("false".to_string()),
            Token::Null => Ok("null".to_string()),
            Token::Float(f) if f.is_nan() => Ok("nan".to_string()),
            Token::Float(f) if f == f64::INFINITY => Ok("inf".to_string()),
            other => Err(self.lexer.error(format!("expected key, got {other:?}"))),
        }
    }

    fn parse_variant(&mut self, tag: String) -> Result<ScnValue> {
        // After consuming an identifier, check if next token starts a value.
        // If so, it's a newtype or struct variant. Otherwise, unit variant.
        let next = self.peek()?;
        match next {
            // These tokens can start a value → this is a variant with payload
            Token::LBrace
            | Token::LBracket
            | Token::Null
            | Token::True
            | Token::False
            | Token::Int(_)
            | Token::Uint(_)
            | Token::Float(_)
            | Token::String(_)
            | Token::Ident(_) => {
                self.enter_nested()?;
                let payload = self.parse_value()?;
                self.leave_nested();
                Ok(ScnValue::Variant(tag, Some(Box::new(payload))))
            }
            // Anything else (comma, colon, rbrace, rbracket, eof) → unit variant
            _ => Ok(ScnValue::Variant(tag, None)),
        }
    }
}

// ===========================================================================
// Helpers
// ===========================================================================

fn strip_leading_indent(raw: &str, indent: usize) -> String {
    let mut result = String::new();
    for (i, line) in raw.lines().enumerate() {
        if i > 0 {
            result.push('\n');
        }
        if line.len() >= indent
            && line.as_bytes()[..indent]
                .iter()
                .all(|&b| b == b' ' || b == b'\t')
        {
            result.push_str(&line[indent..]);
        } else {
            result.push_str(line.trim_start());
        }
    }
    // Remove trailing empty line (from the line before closing """)
    if result.ends_with('\n') {
        result.pop();
    }
    result
}

// ===========================================================================
// Public API
// ===========================================================================

pub fn parse(input: &str) -> Result<ScnValue> {
    let mut parser = Parser::new(input);
    let value = parser.parse_value()?;
    // Verify we consumed everything
    let tok = parser.next()?;
    if tok != Token::Eof {
        return Err(parser
            .lexer
            .error(format!("unexpected trailing token: {tok:?}")));
    }
    Ok(value)
}
