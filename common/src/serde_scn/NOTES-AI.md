# serde_scn — Research & Implementation Notes

Custom human-readable serialization format (SCN) with serde integration. JSON-like simplicity,
first-class Rust enum/variant support, comments, trailing commas, unquoted keys.

## Architecture

Two-phase approach: `T → ScnValue → text` (serialize) and `text → ScnValue → T` (deserialize).

| File | Role |
|------|------|
| `mod.rs` | Public API: `to_string`, `to_writer`, `from_str`, `from_slice`, `from_reader` |
| `value.rs` | `ScnValue` enum (IR) + `ValueSerializer` (T → ScnValue) |
| `parse.rs` | Lexer + recursive-descent parser (text → ScnValue) |
| `emit.rs` | Pretty-printer (ScnValue → text) |
| `de.rs` | Serde `Deserializer` impl (ScnValue → T) |
| `error.rs` | `ScnError` with thiserror |
| `tests.rs` | Roundtrip + spec coverage tests |
| `SPEC.md` | Format specification |

---

## Known Issues

### LOW: O(n²) UTF-8 re-validation in string slow path (parse.rs:215)

For each multi-byte character, `std::str::from_utf8(&self.input[self.pos..])` validates the ENTIRE
remaining input. A string of N multi-byte characters (e.g., CJK text) hits O(n²) validation.

**Fix**: Decode UTF-8 character directly from leading byte pattern (`0xC0..=0xDF` → 2-byte,
`0xE0..=0xEF` → 3-byte, `0xF0..=0xF7` → 4-byte) instead of calling `from_utf8` on the tail.
Not urgent — only affects strings with many non-ASCII characters.

---

## Serde Convention Notes (de.rs)

### `deserialize_any` for `Variant(tag, None)` returns `visit_string` (de.rs:58)

Loses variant semantics. Can cause issues with `#[serde(untagged)]` where a String variant may
match before an enum variant. The Variant-with-payload case correctly uses single-entry map.

**Low priority** — only affects advanced serde attributes with variants inside `deserialize_any`.

---

## Missing Features

### Public API gaps

- **`to_value<T>` / `from_value<T>`**: One-liners wrapping existing infrastructure. serde_json has
  these. Useful for programmatic ScnValue manipulation.
- **`ScnValue` not public from module**: `value` module has no `pub use`. Should re-export
  `ScnValue` from `mod.rs`.
- **`Display` for `ScnValue`**: Would enable `println!("{value}")`. Delegate to `emit_value`.

### No recursion depth limit (parse.rs)

Deeply nested input can cause stack overflow. RON uses a `guard_recursion!` macro that decrements a
counter before entering nested structures and returns `ExceededRecursionLimit` at zero. serde_json
uses a similar `disable_recursion_limit` mechanism.

**Fix**: Add optional recursion limit to Parser (default ~128). Decrement on `parse_value` for
arrays, maps, variants; increment on return.

### No NaN/Infinity literals

NaN and Infinity serialize as `null` (lossy). This is documented in the spec. Compare:
- RON: `inf`, `-inf`, `nan`
- TOML: `inf`, `+inf`, `-inf`, `nan`
- JSON5: `Infinity`, `-Infinity`, `NaN`
- JSON: no support (same as SCN)

**Optional enhancement**: Add `inf`/`-inf`/`nan` as float literals. Natural fit for a Rust-centric
format. Would require parser changes (recognize as keywords), emitter changes (emit literals), and
spec update.

### No hex/octal/binary integer literals

RON supports `0xFF`, `0o777`, `0b1010`, plus underscore separators (`1_000_000`). SCN does not.
These are useful for colors, bit flags, and large numbers.

**Optional enhancement**: Low priority. Most SCN use cases (graph serialization) don't need these.

### No block comments

Only `//` line comments. RON supports `/* */` with nesting. Block comments are useful for
temporarily disabling large sections of config.

**Optional enhancement**: Add nested `/* */` comments. Simple lexer addition. Must be nested
(unlike C) so commenting out content containing `*/` doesn't silently break.

### No hex/octal/binary literals or underscore separators

RON supports `0xFF`, `0o77`, `0b1010`, `1_000_000`. TOML supports all of these too.
Hex is useful for colors and bit flags.

**Optional enhancement**: Add `0x`/`0o`/`0b` prefixes in `read_number()` (check after leading `0`).
Parser should accept, emitter should always emit decimal for roundtrip consistency. Underscore
separators are trivial to add (skip `_` between digits).

---

## Greedy Variant Parsing — Foot-Guns

SCN's greedy variant parsing is its most unique and most dangerous feature.

**Array of unit variants without commas**: `[Red Green Blue]` parses as `[Red(Green(Blue))]` — one
nested item, not three. Always use commas: `[Red, Green, Blue]`.

**Map value followed by next key without comma**:
```
{ mode: Fast
  count: 10 }
```
Parses `Fast` greedily consuming `count` as payload, then `count` consumes `10`. Parser then sees
`:` which is unexpected → parse error. The emitter always emits commas, so roundtripped output is
safe. But hand-written SCN without commas in maps with variant values will fail.

**Spec should be stronger**: "Commas are always required between array items and map entries.
Omitting commas is accepted for non-variant simple values but not recommended."

---

## Design Decisions (Correct, Keep As-Is)

### `Vec<(String, ScnValue)>` for Map

Preserves insertion order, O(n) lookup. For config-sized maps (3-15 fields), linear scan with
cache-friendly Vec is faster than hash lookup. serde_json uses `BTreeMap` or `IndexMap`.

### `Variant(String, Option<Box<ScnValue>>)` representation

Correctly distinguishes unit variants (no allocation) from newtype-wrapping-null. More precise than
`Box<ScnValue>` with Null-as-unit. RON's Value has no variant type at all — SCN is strictly better.

### Two-phase serialization

Necessary for SCN's content-dependent formatting (inline small arrays, multiline complex arrays).
A streaming serializer can't peek ahead to decide formatting. Correct trade-off for a config format.

### String-only map keys at Value level

Matches the format grammar. Parser only accepts identifiers/strings as keys. Serializer coerces
non-string keys to strings. No need for Value-typed keys like RON.

### thiserror for error derivation

Standard practice. Generates identical code to hand-written impls.

### No i128/u128 support

serde_json also lacks this. Extremely rare in config data. serde fallback to i64/u64 is sufficient.

### Fast-path string scanning (parse.rs:146-174)

Scans for closing quote without escapes or non-ASCII, falls back to slow path. Simpler than
serde_json's SWAR/Mycroft approach but appropriate for SCN's use case (config files, not megabyte
JSON blobs).

### Unicode escape format `\u{hex}` (Rust-style)

Cleaner than JSON's `\uXXXX` with surrogate pairs for supplementary characters. Documented in spec.

### Triple-quoted string indent stripping (parse.rs:261-279)

Uses "closing line determines indent" — same algorithm as Swift SE-0168 multiline strings. Lines
with less indentation than the closing `"""` get `trim_start()` (Swift errors instead). The lenient
fallback is appropriate for a config format.

### String emission (emit.rs:69-108)

Correctly escapes all required characters (control chars < 0x20, DEL 0x7F, backslash, quote). Does
NOT escape U+2028/U+2029 (not needed — SCN is not embedded in JavaScript). Non-ASCII passes through
as raw UTF-8 (correct — only ASCII control characters need escaping).

### Key emission (emit.rs:209-224)

`is_bare_key` correctly rejects keywords (`true`/`false`/`null`), empty strings, and non-identifier
characters. Matches SPEC.md grammar: `IDENT = [a-zA-Z_][a-zA-Z0-9_]*`. No dashes (unlike TOML) —
consistent with Rust identifier rules.

### Indent implementation (emit.rs:239-249)

64-byte space buffer handles 32 indent levels in single `write_all`. Loops for deeper nesting. More
efficient than serde_json's per-level write approach. Adequate for config files.

---

## Minor Improvements

### Io error display (error.rs:8)

`#[error("IO error")]` hides the underlying error. Change to `#[error("IO error: {0}")]`.

### JSON-style map fallback in `deserialize_enum` (de.rs:225-231)

Accepts `ScnValue::Map` with 1 entry as an enum. Useful for JSON interop. Safe because only
triggered when `deserialize_enum` is called (serde knows the target type). Keep if interop needed.

### Serde attribute compatibility

| Attribute | Works? | Notes |
|-----------|--------|-------|
| `#[serde(flatten)]` | Yes | Structs work; enum-in-flatten edge cases possible |
| `#[serde(untagged)]` | Mostly | Works through Content buffering |
| `#[serde(tag = "type")]` | Yes | When serializer uses same attribute |
| `#[serde(tag, content)]` | Yes | When serializer uses same attribute |
| `#[serde(default)]` | Yes | Standard MapAccess behavior |

---

## Spec Completeness Gaps

- **Duplicate map keys**: Spec does not define behavior. Current parser silently accepts last value.
  Should spec say "implementation-defined" or "error"?
- **Maximum nesting depth**: Spec does not define. Should mention stack overflow risk for untrusted
  input.
- **Number limits**: Spec does not define max integer or float precision. i64 and u64 ranges are
  implicit from implementation.
- **Grammar**: The EBNF in SPEC.md is complete and correct for the implemented features.

---

## Performance Characteristics

- **Parser**: Hand-written recursive descent, byte-level scanning, single lookahead. Appropriate for
  config files. Fast-path string scanning avoids per-char allocation for simple strings.
- **Emitter**: Stack buffer for indentation (64 bytes, handles 32 levels). Direct `Write` trait.
  Content-dependent formatting (inline vs multiline arrays).
- **Overall**: Two-phase approach adds allocation overhead vs single-pass, but enables
  human-readable formatting decisions. Not a bottleneck for config-sized data.

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
| Hex literals | `0xFF` | Not supported |
| `inf`/`nan` | `inf`, `NaN` | Not supported (→ `null`) |
| Raw strings | `r"..."`, `r#"..."#` | Not supported |
| Multiline strings | Not built-in | `"""..."""` |
| Block comments | `/* */` (nested) | Not supported |
| Ambiguity | None (parens delimit) | Greedy parsing (commas needed) |

SCN is more concise for variants (no parentheses), more familiar for JSON users (braces/brackets),
and has multiline strings. RON has richer numeric support and no greedy parsing ambiguity.

---

## References

- [serde_json source](https://github.com/serde-rs/json) — canonical serde format implementation
- [RON grammar](https://github.com/ron-rs/ron/blob/master/docs/grammar.md) — closest comparable
  Rust-centric format
- [ryu crate](https://docs.rs/ryu) — fast float-to-string used by serde_json
- [serde_path_to_error](https://docs.rs/serde_path_to_error) — external error path tracking
- [Serde enum representations](https://serde.rs/enum-representations.html) — tagging strategies
- [RFC 8259](https://www.rfc-editor.org/rfc/rfc8259) — JSON spec for escape requirements
