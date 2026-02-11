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

**Integers:** Optional `-` followed by digits. No leading zeros except `0` itself.

```
0
42
-7
1000000
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

Comma-separated or newline-separated values between `[` and `]`. Trailing commas allowed.

```
[1, 2, 3]

[
  "alice"
  "bob"
  "charlie"
]

["mixed", 42, true, null]

[]
```

## Maps

Key-value pairs between `{` and `}`. Keys followed by `:` then value. Pairs separated by commas or newlines. Trailing commas allowed.

**Keys** are either:
- Bare identifiers: `[a-zA-Z_][a-zA-Z0-9_]*` (cannot be `true`, `false`, `null`)
- Quoted strings: `"any key"`

```
{ name: "Alice", age: 30 }

{
  name: "Alice"
  age: 30
  active: true
}

{}
```

## Variants (Tagged Unions)

Three forms matching Rust enum serialization:

### Unit Variant
A bare identifier (uppercase start distinguishes from keys in context).

```
None
AsFunction
Red
```

Serialized by serde as a string. During deserialization, serde's enum visitor receives the identifier as the variant name.

### Newtype Variant
Tag followed by a single value.

```
Const 42
Const "hello"
Some [1, 2, 3]
```

### Struct Variant
Tag followed by a map body `{ ... }`.

```
Bind {
  target_id: "579ae1d6-10a3-4906-8948-135cb7d7508b"
  port_idx: 0
}

Enum {
  type_id: "abc-123"
  variant_name: "Red"
}
```

### How Variants Map to Serde

Serde serializes Rust enums as:
- Unit variant → string `"None"`
- Newtype variant → `{ "Const": 42 }`
- Struct variant → `{ "Bind": { "target_id": "...", "port_idx": 0 } }`

SCN represents these more naturally:
- Unit variant → bare identifier `None`
- Newtype variant → `Const 42`
- Struct variant → `Bind { target_id: "..." port_idx: 0 }`

In the SCN value model, variants are stored as `Variant(tag, Option<Value>)` and map to serde's enum serialization protocol.

## Comments

Line comments start with `//` and extend to end of line.

```
{
  // This is a comment
  name: "Alice"  // inline comment
  age: 30
}
```

## Separators

Items in arrays and maps can be separated by:
- Commas: `[1, 2, 3]`
- Newlines: items on separate lines
- Both: `[1,\n2,\n3,\n]`

Trailing commas are always allowed.

## Whitespace

Spaces, tabs, newlines, and carriage returns are whitespace. Whitespace is not significant for structure (brackets define nesting). Whitespace separates tokens.

## Grammar (LL(1))

```
document  = value EOF

value     = 'null'
          | 'true' | 'false'
          | NUMBER
          | STRING
          | array
          | map
          | variant

array     = '[' (value (sep value)*)? ']'
map       = '{' (pair (sep pair)*)? '}'
pair      = key ':' value
key       = IDENT | STRING

variant   = IDENT value?       // only when IDENT is not a keyword

sep       = ',' | NEWLINE      // at least one required between items
                               // (trailing comma OK)

IDENT     = [a-zA-Z_][a-zA-Z0-9_]*   // excluding 'true', 'false', 'null'
NUMBER    = '-'? [0-9]+ ('.' [0-9]+)? ([eE] [+-]? [0-9]+)?
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
- identifier → variant (look ahead: if next token starts a value, it's newtype/struct variant; otherwise unit variant)

## Full Example

```
// Scenarium graph
{
  nodes: [
    {
      id: "579ae1d6-10a3-4906-8948-135cb7d7508b"
      func_id: "a1b2c3d4-e5f6-7890-abcd-ef1234567890"
      name: "mult"
      behavior: Once
      inputs: [
        {
          name: "a"
          binding: Bind {
            target_id: "999c4d37-e0eb-4856-be3f-ad2090c84d8c"
            port_idx: 0
          }
        }
        {
          name: "b"
          binding: Const Int -7
        }
        {
          name: "c"
          binding: None
        }
      ]
      events: [
        {
          name: "on_complete"
          subscribers: [
            "b88ab7e2-17b7-46cb-bc8e-b428bb45141e"
          ]
        }
      ]
    }
  ]
}
```
