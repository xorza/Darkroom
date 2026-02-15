# Code Review: serde_scn

## Summary

Overall code quality is high. The two-phase architecture is clean, the parser is correct and well-structured, the emitter produces readable output, and the serde integration follows standard patterns. The module is ~1,600 lines across 6 files, with ~2,300 lines of thorough tests.

The most significant finding is a silent data corruption bug in variant tag emission. The remaining findings are formatting improvements and minor API polish.

## Findings

### Priority 1 — High Impact, Low Invasiveness

#### [F1] Variant tags equal to keywords produce unparseable output
- **Location**: `emit.rs:195`, `value.rs:109-136`
- **Category**: Correctness
- **Impact**: 4/5 — Silent roundtrip failure (data corruption)
- **Meaningfulness**: 5/5 — Actual bug, not cosmetic
- **Invasiveness**: 2/5 — Add validation in 3 serializer methods
- **Description**: If a variant tag equals a keyword (`true`, `false`, `null`, `nan`, `inf`) — possible via `#[serde(rename = "true")]` — `emit_variant` writes it as a bare keyword. The parser re-reads it as the keyword type, not a variant. Example: `ScnValue::Variant("true", None)` emits as `true`, parses back as `ScnValue::Bool(true)`. Roundtrip silently corrupts data.

  **Fix**: In `serialize_unit_variant`, `serialize_newtype_variant`, and `serialize_struct_variant`, validate the variant name with `emit::is_bare_key(variant)` (make it `pub(super)`). If validation fails, return `Err(ScnError::Message(...))`. This catches the issue at serialization time with a clear error, rather than producing unparseable output.

#### [F2] Emitter omits trailing commas in multiline arrays and maps — REJECTED
- **Location**: `emit.rs:144-146`, `emit.rs:175-177`
- **Status**: Rejected by user preference. Keeping emitter output without trailing commas.

#### [F3] `unexpected()` produces generic messages for integer types
- **Location**: `de.rs:15-20`
- **Category**: API cleanliness
- **Impact**: 3/5 — Better error messages for type mismatches
- **Meaningfulness**: 4/5 — Users see "positive integer" instead of the actual value
- **Invasiveness**: 1/5 — Small change in one function
- **Description**: `ScnValue::Int(i)` maps to `Unexpected::Other("positive integer")` or `"negative integer"`, losing the actual value. `ScnValue::Uint` maps to `"unsigned integer"`. Compare with `Float(f)` which passes the value via `Unexpected::Float(*f)`. Error messages say "invalid type: positive integer, expected a string" instead of showing the value.

  **Fix**: Use `Unexpected::Signed(i as i64)` / `Unexpected::Unsigned(u as u64)` when the value fits, keeping `Other(...)` with the formatted value for i128/u128 overflow cases:
  ```rust
  ScnValue::Int(i) => {
      if let Ok(v) = i64::try_from(i) {
          de::Unexpected::Signed(v)
      } else {
          // Static leak-free: just describe the type
          de::Unexpected::Other("128-bit integer")
      }
  }
  ```

#### [F4] Missing `#[derive(Debug)]` on serializer builder structs
- **Location**: `value.rs:187`, `value.rs:229`, `value.rs:253`, `value.rs:312`
- **Category**: Consistency
- **Impact**: 2/5 — Debug printing if builders appear in error contexts
- **Meaningfulness**: 4/5 — Violates project rule ("Always add `#[derive(Debug)]` to structs")
- **Invasiveness**: 1/5 — Add 4 derive attributes
- **Description**: `SeqBuilder`, `TupleVariantBuilder`, `MapBuilder`, and `StructVariantBuilder` lack `#[derive(Debug)]`. All other structs in the module have it. Add `#[derive(Debug)]` to each.

### Priority 2 — High Impact, Moderate Invasiveness

#### [F5] Maps always emit multiline, even for trivial single-entry cases
- **Location**: `emit.rs:154-183`
- **Category**: API cleanliness
- **Impact**: 3/5 — Better readability for small maps
- **Meaningfulness**: 3/5 — Consistent with array inline behavior
- **Invasiveness**: 2/5 — Add heuristic similar to `emit_array`
- **Description**: Arrays with up to 8 simple values emit inline (`[1, 2, 3]`), but maps always emit multiline, even for `{ x: 1 }`. The spec examples show inline maps: `{ name: "Alice", age: 30 }`. A similar heuristic could inline maps with 1-3 simple values:
  ```rust
  if entries.len() <= 3 && entries.iter().all(|(_, v)| is_simple(v)) {
      // inline: { key: value, key: value }
  }
  ```

#### [F6] Inline array heuristic ignores total character width
- **Location**: `emit.rs:128`
- **Category**: API cleanliness
- **Impact**: 2/5 — Prevents overly wide lines
- **Meaningfulness**: 3/5 — Real readability issue for long string arrays
- **Invasiveness**: 2/5 — Add width estimation to `is_simple` or inline check
- **Description**: The inline heuristic checks `items.len() <= 8 && items.iter().all(is_simple)`, but doesn't consider total width. An array of 8 strings like `["very long string one", "very long string two", ...]` would be inlined into a single extremely long line. Add a width estimate: sum of approximate element widths (e.g., string length + 2 for quotes + 2 for ", " separator), and only inline if total < ~80-100 characters.

### Priority 3 — Moderate Impact

#### [F7] Missing `size_hint` on SeqDeserializer
- **Location**: `de.rs:327-338`
- **Category**: API cleanliness
- **Impact**: 2/5 — Minor performance for sequence deserialization
- **Meaningfulness**: 3/5 — Standard practice in serde deserializers (serde_json, RON both provide it)
- **Invasiveness**: 1/5 — Add one 3-line method
- **Description**: `SeqDeserializer` wraps `IntoIter<Vec<ScnValue>>` which knows its exact remaining length, but doesn't expose it via `size_hint()`. Serde visitors use this to pre-allocate `Vec::with_capacity`. Add:
  ```rust
  fn size_hint(&self) -> Option<usize> {
      Some(self.iter.len())
  }
  ```

#### [F8] Duplicated underscore validation in parser
- **Location**: `parse.rs:477-498` and `parse.rs:512-531`
- **Category**: Simplification
- **Impact**: 2/5 — Code deduplication, single point for underscore rules
- **Meaningfulness**: 2/5 — Both copies are correct and small
- **Invasiveness**: 2/5 — Refactor `scan_digits_with_underscores` to accept a digit predicate
- **Description**: `scan_digits_with_underscores` is hardcoded to `is_ascii_digit()`. `read_prefixed_integer` has its own copy of the same underscore validation loop with a custom `is_valid_digit` predicate. The logic (no leading, trailing, or consecutive underscores) is duplicated. Could unify by making `scan_digits_with_underscores` accept `is_valid_digit: fn(u8) -> bool`.

## Cross-Cutting Patterns

### Error specificity
The `ScnError::Message(String)` variant serves as a catch-all for all serde errors (type mismatches, missing fields, unknown fields, etc.). This is adequate for a config format but means error messages don't have structured types for programmatic handling. RON has ~30 specific variants. Adding even a few (e.g., `MissingField`, `UnknownField`, `InvalidType`) would improve diagnostics. Low priority — the current string messages are already informative enough for human readers.

### Emitter consistency
The emitter is conservative — it always emits valid SCN that round-trips correctly (modulo F1). The parser is more permissive (accepts hex, underscores, triple-quoted strings, comments). This parser-permissive / emitter-canonical asymmetry is a deliberate design choice matching JSON's approach and is correct.

### Architecture quality
The two-phase `T -> ScnValue -> text` architecture is clean and well-suited for content-dependent formatting decisions. The `ScnValue` IR is minimal (9 variants) and precisely matches the format's grammar. The module boundaries (parse/emit/de/value/error) are logical and each file has a single responsibility. No dead code, no unnecessary abstractions, no backward-compatibility shims.
