# SCN Text Format Specification

A human-readable data serialization format. Simple like JSON, readable like YAML, with first-class support for tagged variants (Rust enums).

## Design Goals

- Small spec (~JSON-scale)
- LL(1) grammar, trivial to parse with recursive descent
- No implicit typing — types determined by syntax alone
- First-class tagged variants for Rust enum serialization
- Comments, trailing commas, unquoted keys
- Brackets for structure (not indentation)

## Values

A document is a single **value**. A value is one of:

| Type | Syntax | Examples |
|------|--------|----------|
| Null | `null` | `null` |
| Bool | `true` or `false` | `true`, `false` |
| Integer | Digits with optional leading `-` | `42`, `-7`, `0` |
| Float | Number with `.` or `e`/`E` | `3.14`, `-1.0`, `2.5e10` |
| String | Double-quoted | `"hello"`, `""` |
| Array | `[` values `]` | `[1, 2, 3]` |
| Map | `{` pairs `}` | `{ name: "Alice" }` |
| Variant | `Tag`, `Tag value`, `Tag { fields }` | `None`, `Const 42`, `Bind { id: "..." }` |

## Null

```
null
```

## Booleans

```
true
false
```

Only these two keywords. No `yes`/`no`/`on`/`off`.

## Numbers

**Integers:** Optional `-` followed by digits. No leading zeros (except `0` itself).

```
0
42
-7
1_000_000
```

**Floats:** Must contain `.` or `e`/`E`. Optional leading `-`.

```
3.14
-1.0
0.5
2.5e10
1.0e-3
```

Integer vs float is distinguished by the presence of `.` or `e`/`E`.

**Integer range:** Signed integers up to 128-bit (`i128`): −170141183460469231731687303715884105728 to 170141183460469231731687303715884105727. Unsigned integers up to 128-bit (`u128`): 0 to 340282366920938463463374607431768211455. Values beyond these ranges are a parse error. Floats are IEEE 754 double-precision (`f64`).

**Special float values:** `nan`, `inf`, `-inf` are keywords for IEEE 754 special values. `-nan` is also accepted (NaN has no meaningful sign).

```
nan
inf
-inf
```

**Hex/octal/binary integers:** Prefix `0x`/`0X`, `0o`/`0O`, `0b`/`0B` followed by digits in the appropriate base. Case-insensitive for both prefix and hex digits. Can be negative with leading `-`.

```
0xFF
0o777
0b1010
-0x10
0XAB
```

**Underscore digit separators:** Underscores `_` can appear between digits for readability, in any numeric literal (decimal, hex, octal, binary, float). Rules (matching Rust):
- No leading underscore in a digit group
- No trailing underscore in a digit group
- No consecutive underscores

```
1_000_000
0xFF_FF
3.14_15
1_0e1_0
0b1111_0000
```

The emitter always outputs decimal integers and standard floats (no hex/octal/binary, no underscores). Roundtrip: `0xFF` → parse → emit → `255`.

## Strings

Double-quoted. Escape sequences:

| Escape | Character |
|--------|-----------|
| `\\` | Backslash |
| `\"` | Double quote |
| `\n` | Newline |
| `\r` | Carriage return |
| `\t` | Tab |
| `\0` | Null byte |
| `\u{XXXX}` | Unicode codepoint (1-6 hex digits) |

```
"hello world"
""
"line1\nline2"
"path\\to\\file"
"emoji \u{1f600}"
```

### Multiline Strings

Triple-quoted strings. No escape processing. Leading whitespace on each line is stripped up to the indentation of the closing `"""`.

```
"""
  This is a multiline string.
  No escaping needed.
  """
```

## Arrays

Comma-separated values between `[` and `]`. Trailing commas allowed. Newlines are whitespace and do not act as separators.

```
[1, 2, 3]

[
  "alice",
  "bob",
  "charlie",
]

["mixed", 42, true, null]

[]
```

**Note:** Commas between items are always required. The parser greedily consumes following tokens as variant payloads, so omitting commas would cause ambiguous parsing (e.g., `[Red Green Blue]` would parse as a single nested variant `Red(Green(Blue))` instead of three items).

## Maps

Key-value pairs between `{` and `}`. Keys followed by `:` then value. Pairs separated by commas. Trailing commas allowed. **Duplicate keys are an error.**

**Keys** are either:
- Bare identifiers: `[a-zA-Z_][a-zA-Z0-9_]*` (cannot be `true`, `false`, `null`)
- Quoted strings: `"any key"`

```
{ name: "Alice", age: 30 }

{
  name: "Alice",
  age: 30,
  active: true,
}

{}
```

## Variants (Tagged Unions)

Variants are bare identifiers that represent Rust enum values. Three forms:

### Unit Variant
A bare identifier with no payload. Terminated by a comma, closing bracket, or end of input.

```
None
AsFunction
Red
```

### Newtype Variant
Identifier followed by a single value.

```
Const 42
Const "hello"
Some [1, 2, 3]
```

### Struct Variant
Identifier followed by a map body `{ ... }`.

```
Bind {
  target_id: "579ae1d6-10a3-4906-8948-135cb7d7508b",
  port_idx: 0,
}
```

### Greedy Parsing

The parser is greedy: after reading an identifier, if the next token can start a value (`{`, `[`, `"`, number, `true`, `false`, `null`, or another identifier), it is consumed as the variant's payload. This means consecutive variants without commas are ambiguous:

```
// AMBIGUOUS — don't do this:
[None Const 10]     // parses as [Variant("None", Variant("Const", 10))]

// CORRECT — use commas:
[None, Const 10]    // parses as [Variant("None"), Variant("Const", 10)]
```

Nested variants work naturally through greedy parsing:

```
// Outer::Inner(Binding::None) serializes as:
Inner None          // Variant("Inner", Variant("None"))
```

### How Variants Map to Serde

Serde serializes Rust enums as:
- Unit variant → string `"None"`
- Newtype variant → `{ "Const": 42 }`
- Struct variant → `{ "Bind": { "target_id": "...", "port_idx": 0 } }`

SCN represents these more naturally:
- Unit variant → bare identifier `None`
- Newtype variant → `Const 42`
- Struct variant → `Bind { target_id: "...", port_idx: 0 }`

In the SCN value model, variants are stored as `Variant(tag, Option<Value>)` and map to serde's enum serialization protocol.

## Comments

Line comments start with `//` and extend to end of line.

```
{
  // This is a comment
  name: "Alice",  // inline comment
  age: 30,
}
```

## Separators

Items in arrays and key-value pairs in maps **must** be separated by commas. Trailing commas are always allowed. Whitespace (including newlines) separates tokens but does not serve as an item separator.

```
// Correct:
[1, 2, 3]
[Red, Green, Blue]
{ mode: Fast, count: 10 }

// Also correct (trailing commas):
[1, 2, 3,]
{ mode: Fast, count: 10, }

// INVALID — missing commas:
[1 2 3]
{ mode: Fast count: 10 }
```

## Whitespace

Spaces, tabs, newlines, and carriage returns are whitespace. Whitespace is not significant for structure (brackets define nesting). Whitespace separates tokens.

## Grammar

```
document  = value EOF

value     = 'null'
          | 'true' | 'false'
          | NUMBER
          | STRING
          | array
          | map
          | variant

array     = '[' (value ','?)* ']'
map       = '{' (pair ','?)* '}'
pair      = key ':' value
key       = IDENT | STRING

variant   = IDENT value?       // greedy: consumes next value if present

IDENT     = [a-zA-Z_][a-zA-Z0-9_]*   // excluding 'true', 'false', 'null', 'nan', 'inf'
NUMBER    = '-'? ( HEX_INT | OCT_INT | BIN_INT | DEC_NUMBER )
          | 'nan' | 'inf' | '-inf' | '-nan'
DEC_NUMBER = ('0' | [1-9] DIG*) ('.' [0-9] DIG*)? ([eE] [+-]? [0-9] DIG*)?
HEX_INT   = '0' [xX] HDIG+
OCT_INT   = '0' [oO] [0-7_]+        // same underscore rules
BIN_INT   = '0' [bB] [01_]+         // same underscore rules
DIG       = [0-9_]                   // no leading/trailing/consecutive underscores
HDIG      = [0-9a-fA-F_]            // no leading/trailing/consecutive underscores
STRING    = '"' (escape | [^"\\])* '"'
          | '"""' raw_content '"""'
```

### LL(1) Parsing Strategy

The first token determines the production:
- `{` → map
- `[` → array
- `"` or `"""` → string
- digit or `-` → number
- `true`/`false` → boolean
- `null` → null
- `nan`/`inf` → float (special values)
- identifier → variant (greedy: if next token starts a value, consume it as payload; otherwise unit variant)

## Full Example

```
// Scenarium graph
{
  nodes: [
    {
      id: "579ae1d6-10a3-4906-8948-135cb7d7508b",
      func_id: "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
      name: "mult",
      behavior: Once,
      inputs: [
        {
          name: "a",
          binding: Bind {
            target_id: "999c4d37-e0eb-4856-be3f-ad2090c84d8c",
            port_idx: 0,
          },
        },
        {
          name: "b",
          // Const wraps a StaticValue::Int — nested variants via greedy parsing
          binding: Const Int -7,
        },
        {
          name: "c",
          binding: None,
        },
      ],
      events: [
        {
          name: "on_complete",
          subscribers: [
            "b88ab7e2-17b7-46cb-bc8e-b428bb45141e",
          ],
        },
      ],
    },
  ],
}
```
