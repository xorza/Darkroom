# serde_scn — Implementation Notes

Custom human-readable serialization format (SCN) with serde integration. JSON-like simplicity,
first-class Rust enum/variant support, comments, trailing commas, unquoted keys.

## Architecture

Two-phase approach: `T -> ScnValue -> text` (serialize) and `text -> ScnValue -> T` (deserialize).

| File | Role |
|------|------|
| `mod.rs` | Public API: `to_string`, `to_writer`, `from_str`, `from_slice`, `from_reader` |
| `value.rs` | `ScnValue` enum (IR) + `ValueSerializer` (T -> ScnValue) |
| `parse.rs` | Lexer + recursive-descent parser (text -> ScnValue) |
| `emit.rs` | Pretty-printer (ScnValue -> text) |
| `de.rs` | Serde `Deserializer` impl (ScnValue -> T) |
| `error.rs` | `ScnError` with thiserror |
| `tests.rs` | Roundtrip + spec coverage tests |
| `SPEC.md` | Format specification |

---

## Known Issues

### BOM handling not implemented

UTF-8 BOM (0xEF 0xBB 0xBF) at the start of a file causes a parse error. Windows editors
(Notepad, VS Code with certain settings) may add BOM automatically. TOML and YAML strip it;
serde_json also fails on BOM.

**Fix**: Skip 3-byte BOM at position 0 in `parse()`. Trivial: `if input.starts_with("\u{FEFF}")
{ input = &input[3..]; }`.

### i128/u128 broken with `#[serde(flatten)]` and `#[serde(untagged)]`

Serde's internal `Content` enum (used for buffering in flatten, untagged, and internally tagged
enums) has no `Content::I128` / `Content::U128` variants. Values exceeding i64/u64 range will fail
when deserialized through these serde features. This is a serde-core bug, not an SCN bug.

Tracked: serde issues #2576, #2230. No workaround; values must fit in i64/u64 when used with
flatten/untagged.

### Emitter does not emit trailing commas

The emitter omits trailing commas after the last item in arrays and maps. Industry best practice
(rustfmt, Prettier) is to always emit trailing commas in multiline mode for cleaner diffs and
easier appending. Since the parser now requires commas between items, trailing commas on the last
item are optional but recommended for consistency.

### Emitter never emits triple-quoted strings

The emitter always uses regular quoted strings with escape sequences, even for multiline content.
Strings containing `\n` would be more readable as `"""..."""`. Heuristic: use triple-quoted when
the string contains literal newlines and does not contain `"""`.

---

## Serde Convention Notes (de.rs)

### `deserialize_any` for `Variant(tag, None)` returns `visit_string` (de.rs:74) -- POSTPONED

Loses variant semantics. `#[serde(untagged)]` with a `String` arm competing against an enum arm
will match String first. Serde's visitor protocol has no `visit_variant` -- this is a serde
limitation, not an SCN bug. Normal `deserialize_enum` works correctly. Workaround: put enum variant
before String variant in `#[serde(untagged)]`.

### Enum tagging strategy interaction

Only **externally tagged** enums call `deserialize_enum`. The other three strategies bypass it:

| Strategy | Calls `deserialize_enum`? | Uses instead |
|----------|--------------------------|-------------|
| External (default) | Yes | `EnumAccess` + `VariantAccess` |
| Internal (`#[serde(tag="t")]`) | No | `deserialize_any` -> Content buffering |
| Adjacent (`#[serde(tag, content)]`) | No | `deserialize_any` -> Content buffering |
| Untagged (`#[serde(untagged)]`) | No | `deserialize_any` -> Content buffering |

All work correctly because `deserialize_any` properly dispatches `Variant(tag, Some(payload))`
as a single-key map, matching serde's JSON-style externally tagged representation.

### Serde attribute compatibility

| Attribute | Works? | Notes |
|-----------|--------|-------|
| `#[serde(flatten)]` | Yes | Structs work; i128/u128 broken (serde bug) |
| `#[serde(untagged)]` | Mostly | Works through Content buffering; i128/u128 broken |
| `#[serde(tag = "type")]` | Yes | When serializer uses same attribute |
| `#[serde(tag, content)]` | Yes | When serializer uses same attribute |
| `#[serde(default)]` | Yes | Standard MapAccess behavior |
| `#[serde(rename)]` | Yes | Standard |
| `#[serde(skip)]` | Yes | Standard |

### Human-readable flag

Default `is_human_readable() = true` is correct and must not be changed. Types like `IpAddr`,
`Uuid`, `DateTime` use this to choose between string and binary representations. Changing it
would silently break serialization of these types.

---

## Implemented Features

### Public API

- `to_string`, `to_writer`, `from_str`, `from_slice`, `from_reader`: Standard serde format API.
- `to_value<T>` / `from_value<T>`: Convert between Rust types and `ScnValue` without text.
- `ScnValue` re-exported via `pub use value::ScnValue`.
- `Display` for `ScnValue`: Delegates to `emit_value`.

### Recursion depth limit

`MAX_DEPTH = 128`. Parser tracks `depth` counter, incremented by `enter_nested()` in `parse_array`,
`parse_map`, `parse_variant`; decremented by `leave_nested()` on return. 128 matches serde_json's
default. Go encoding/json uses 10,000; .NET System.Text.Json uses 64.

### NaN/Infinity literals

`nan`, `inf`, `-inf` are float keywords. `-nan` also accepted. `is_bare_key()` excludes `nan`/`inf`.
`parse_key()` handles bare nan/inf as keys in hand-written SCN.

### Hex/octal/binary integer literals

`0xFF`, `0o777`, `0b1010` with case-insensitive prefixes. Negative allowed: `-0x10`. Emitter
always outputs decimal. `make_integer_token()` handles Int vs Uint selection and overflow.

### Underscore digit separators

`1_000_000`, `0xFF_FF`, `1.23_45`, `1_0e1_0`. Validated by `scan_digits_with_underscores()`:
no leading/trailing/consecutive underscores. Stripped before `.parse()`.

### Full i128/u128 support

`ScnValue::Int(i128)` and `ScnValue::Uint(u128)`. Parser uses `i128`/`u128` for all integer paths.
Deserializer uses narrowest visit method: `visit_i64`/`visit_u64` when value fits, `visit_i128`/
`visit_u128` only for overflow. More capable than serde_json (which needed `arbitrary_precision`
feature) and RON (which gates i128 behind a feature flag).

### Block comments -- REJECTED

Not adding `/* */` comments. Line comments (`//`) are sufficient for a config format.

---

## Greedy Variant Parsing

SCN's greedy variant parsing is its most distinctive feature. After consuming an identifier, if the
next token can start a value, it is consumed as the variant's payload. This enables concise nested
variant syntax: `Const Int -7` → `Variant("Const", Variant("Int", -7))`.

**Commas are required** between items in arrays and entries in maps. The parser enforces this by
checking that each item/entry is followed by `,` or the closing bracket. This prevents the greedy
parsing foot-gun in maps (e.g., `{ mode: Fast count: 10 }` → error, because `Fast` consumes
`count` as payload and then `:` is not `,` or `}`).

**Note**: In arrays, `[Red Green Blue]` still silently parses as one nested variant
`Red(Green(Blue))` because greedy consumption happens within `parse_variant` before the comma check.
The comma check catches the case when there are leftover tokens (like in maps with `:`).

---

## Design Decisions (Correct, Keep As-Is)

### `Vec<(String, ScnValue)>` for Map
Preserves insertion order, O(n) lookup. For config-sized maps (3-15 fields), linear scan with
cache-friendly Vec is faster than hash lookup. serde_json uses `BTreeMap` or `IndexMap`.

### `Variant(String, Option<Box<ScnValue>>)` representation
Correctly distinguishes unit variants (no allocation) from newtype-wrapping-null. More precise than
`Box<ScnValue>` with Null-as-unit. RON's Value has no variant type at all -- SCN is strictly better.

### Two-phase serialization
Necessary for content-dependent formatting (inline small arrays, multiline complex arrays).
Standard for config formats (TOML also uses two-phase). serde_json uses streaming for primary API
but also provides `to_value()` for the two-phase path.

### String-only map keys at Value level
Matches the format grammar. Serializer coerces int/bool keys to strings (more permissive than
serde_json which rejects bool keys). Deserialization always delivers keys as `ScnValue::String`,
so `HashMap<i32, ...>` roundtrip would break (same limitation as JSON).

### thiserror for error derivation
Standard practice. Generates identical code to hand-written impls.

### Fast-path string scanning (parse.rs:146-174)
Scans for closing quote without escapes or non-ASCII, falls back to slow path. Simpler than
serde_json's SWAR/Mycroft approach (which processes 8 bytes at a time for ~10-20% speedup on
string-heavy workloads) but appropriate for config files where strings are short.

### Unicode escape format `\u{hex}` (Rust-style)
Cleaner than JSON's `\uXXXX` with surrogate pairs. `char::from_u32` rejects unpaired surrogates
(U+D800-U+DFFF) -- correct but undocumented in spec.

### Triple-quoted string indent stripping (parse.rs:261-279)
Uses "closing line determines indent" -- same algorithm as Swift SE-0168 and similar to Java
JEP 378 text blocks. Java uses a 7-step algorithm: split lines, compute minimum common whitespace
prefix across non-blank lines (including closing delimiter line), strip that prefix. SCN's approach
is simpler (uses only closing delimiter's indent) which is more lenient -- lines with less
indentation get `trim_start()` instead of erroring (Swift errors instead). Appropriate for a
config format.

### String emission (emit.rs:69-108)
Correctly escapes: control chars < 0x20, DEL 0x7F, backslash, quote. Does NOT escape U+2028/U+2029
(not needed -- SCN is not embedded in JavaScript). Non-ASCII passes through as raw UTF-8.
RFC 8259 does not require escaping 0x7F either, but SCN escapes it for robustness.

### Key emission (emit.rs:209-224)
`is_bare_key` correctly rejects keywords, empty strings, and non-identifier characters. No dashes
(unlike TOML's `A-Za-z0-9_-`), consistent with Rust identifier rules.

### Indent implementation (emit.rs:239-249)
64-byte space buffer handles 32 indent levels in single `write_all`. More efficient than
serde_json's per-level write loop. A pre-computed indent table would be slightly faster but the
buffer approach is simpler and adequate.

### Option handling (None -> null, Some -> transparent)
Identical to serde_json's approach. Industry standard for human-readable formats. RON uses explicit
`Some(value)` / `None` keywords instead.

### Byte array rejection
Serializer/deserializer reject `serialize_bytes`/`deserialize_bytes` with an error. Matches TOML's
approach. serde_json converts to an array of integers (verbose). If binary support is ever needed,
users should use `serde_bytes` crate with base64 encoding -- format-side concern, not SCN's job.

---

## SPEC.md Gaps (from research)

### Grammar incomplete
The EBNF grammar in SPEC.md is missing formal productions for:
- `escape` (referenced in STRING but not defined; spec has prose table instead)
- `raw_content` (triple-quoted string body)
- `comment` (`// ... \n`)
- `whitespace` (spaces, tabs, newlines, carriage returns)

### Edge cases undocumented in spec
The implementation handles these correctly but the spec should document:
- **Empty input**: Invalid (must contain one value). Tested, not in spec.
- **Trailing content**: Parse error. Implemented, not in spec prose.
- **Nesting depth**: 128-level limit. Implementation enforces, spec silent.
- **Unpaired surrogates**: Rejected by `char::from_u32` in `\u{...}`. Not in spec.
- **File encoding**: Must be UTF-8. Implicit from Rust `&str`, not stated.
- **BOM**: Not handled. Should state policy.

### Missing spec sections
- **Encoding**: "SCN documents MUST be valid UTF-8."
- **File metadata**: File extension `.scn`, provisional MIME type `text/x-scn`.
- **Implementation notes**: Emitter always outputs decimal, always includes commas, uses 2-space
  indentation. These are emitter conventions, not format requirements.

---

## Performance Characteristics

- **Parser**: Hand-written recursive descent, byte-level scanning, single lookahead. Fast-path
  string scanning avoids per-char allocation for simple strings. Slow-path multi-byte UTF-8 uses
  direct byte-pattern decoding (`decode_utf8_char()`) -- O(n) not O(n^2).
- **Number parsing**: Uses `str.parse::<i128>()` / `str.parse::<f64>()` after underscore stripping.
  serde_json uses manual digit-by-digit with POW10 table for floats, overflow macros for integers.
  SCN's approach is simpler and adequate for config files.
- **Float emission**: Uses `ryu::Buffer` for shortest roundtrip-safe output. Same as serde_json.
  Dragonbox is a newer/faster alternative but ryu is well-established.
- **Emitter**: Stack buffer for indentation (64 bytes, 32 levels). Direct `Write` trait.
  Content-dependent formatting (inline vs multiline arrays based on count + simplicity).
- **Overall**: Two-phase approach adds allocation proportional to data size vs streaming. Not a
  bottleneck for config-sized data. For gigabyte-scale data, a streaming serializer would be needed.

---

## Potential Improvements (Not Urgent)

### Compact mode (`to_string_compact`)
Only pretty mode exists. serde_json offers both `CompactFormatter` and `PrettyFormatter` via a
`Formatter` trait. Could add an `EmitConfig` struct with style options.

### More specific error variants
The `ScnError::Message(String)` catchall is used for all serde deserialization errors. RON has
~30 specific error variants (`ExpectedArray`, `NoSuchStructField`, etc.) for better diagnostics.
Overriding serde's `de::Error` methods (`invalid_type`, `unknown_field`, `missing_field`, etc.)
with specific variants would improve error messages.

### Error path tracking
`serde_path_to_error` crate can wrap the SCN deserializer to provide field paths in errors
(e.g., `"dependencies.serde.typo1"`). This is a separate concern from the format, not something
to build into SCN itself.

### Raw strings
RON supports `r"..."` / `r#"..."#` for strings without escape processing. Useful for regex
patterns and Windows paths. Triple-quoted strings partially cover this use case but require
multiline syntax. Low priority.

---

## RON Comparison (Closest Competitor)

| Feature | RON | SCN |
|---------|-----|-----|
| Unit variant | `None` | `None` |
| Newtype variant | `Some(42)` | `Some 42` |
| Tuple variant | `Pair(1, "hello")` | `Pair [1, "hello"]` |
| Struct variant | `Named(id: 7)` | `Named { id: 7 }` |
| Nested variant | `Inner(Const(5))` | `Inner Const 5` |
| Option Some | `Some(42)` | `42` (transparent) |
| Option None | `None` | `null` |
| Struct | `MyStruct(field: val)` | `{ field: val }` |
| Hex literals | `0xFF` | `0xFF` |
| `inf`/`nan` | `inf`, `NaN` | `inf`, `nan` |
| Raw strings | `r"..."`, `r#"..."#` | Not supported |
| Multiline strings | Not built-in | `"""..."""` |
| Block comments | `/* */` (nested) | Not supported |
| i128 support | Feature-gated | Native |
| Ambiguity | None (parens delimit) | Greedy parsing (commas needed) |

SCN is more concise for variants (no parentheses), more familiar for JSON users (braces/brackets),
and has multiline strings. RON has raw strings and no greedy parsing ambiguity.

---

## References

- [serde_json source](https://github.com/serde-rs/json) -- canonical serde format implementation
- [RON grammar](https://github.com/ron-rs/ron/blob/master/docs/grammar.md) -- closest Rust-centric format
- [ryu crate](https://docs.rs/ryu) -- fast float-to-string (Ryu algorithm, PLDI 2018)
- [serde_path_to_error](https://docs.rs/serde_path_to_error) -- external error path tracking
- [Serde enum representations](https://serde.rs/enum-representations.html) -- tagging strategies
- [RFC 8259](https://www.rfc-editor.org/rfc/rfc8259) -- JSON spec
- [TOML v1.0.0](https://toml.io/en/v1.0.0) -- TOML spec
- [JEP 378](https://openjdk.org/jeps/378) -- Java text blocks (multiline string algorithm)
- [Swift SE-0168](https://github.com/swiftlang/swift-evolution/blob/main/proposals/0168-multi-line-string-literals.md) -- Swift multiline strings
- [serde Content i128 bug](https://github.com/serde-rs/serde/issues/2576) -- i128 not supported in Content
- [serde_json SWAR strings](https://purplesyringa.moe/blog/i-sped-up-serde-json-strings-by-20-percent/) -- Mycroft string scanning
- [Wadler's Prettier Printer](https://homepages.inf.ed.ac.uk/wadler/papers/prettier/prettier.pdf) -- inline/multiline algorithm
