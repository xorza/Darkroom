# Serialization Format Design Research

Research for designing a new human-readable serialization format: simple like JSON, readable like YAML, easy to parse.

---

## 1. Landscape of Existing Formats

### JSON
- **Spec size:** ~1,969 words, 6 railroad diagrams. Frozen spec.
- **Strengths:** Universal support. Unambiguous parsing. Simple data model (objects, arrays, strings, numbers, booleans, null).
- **Weaknesses:** No comments. No trailing commas. No multiline strings. Mandatory quoting of all keys and strings. No date/time type. No integer vs float distinction. No binary data. Duplicate keys = undefined behavior.

### YAML
- **Spec size:** 23,449 words, 80+ pages.
- **Strengths:** Human-readable. Comments. Multiline strings. Anchors/aliases for reuse. Massive adoption (k8s, Ansible, GitHub Actions, Docker Compose).
- **Weaknesses:** The Norway problem (`NO` -> `false`). Implicit type coercion (`3.10` -> `3.1`, `22:22` -> `1342`). 9 different multiline string syntaxes. Security vulnerabilities from deserialization (Ruby RCE 2013, SnakeYaml CVE-2022-1471). Indentation sensitivity makes large files hard to edit and templating fragile. YAML 1.1 vs 1.2 incompatibilities.

### TOML
- **Spec size:** ~3,339 words.
- **Strengths:** Strict typing, no implicit coercion. First-class date/time types (RFC 3339). Comments. Maps to hash table.
- **Weaknesses:** Deeply nested structures become extremely verbose. At least 3 syntaxes for objects, 2 for arrays. Array-of-tables (`[[array]]`) is confusing. Lines added at end of file can silently go into wrong section. PyTOML maintainer called it "a bad file format."

### KDL (KDL Document Language)
- **Strengths:** Node-based (XML-like but lighter). Nodes have positional values, named properties, and children. No implicit typing. KDL 2.0 uses `#true`/`#false`/`#null`.
- **Weaknesses:** Unfamiliar data model. Limited ecosystem. KDL 2.0 breaking changes.

### SDLang
- **Strengths:** XML-like structure, less verbose. Type-aware (strings, ints, floats, dates, booleans, null, binary). Newline-terminated tags.
- **Weaknesses:** Limited adoption.

### HCL (HashiCorp)
- **Strengths:** Variables, functions, expressions, modules. Both native and JSON syntax. Used via Terraform.
- **Weaknesses:** Ad-hoc abstractions from infrastructure (not language design) backgrounds. Nix "stood the test of time far better."

### Hjson
- **Strengths:** JSON superset. Optional quotes, comments, trailing commas, multiline strings. Adopted by broot (switching from TOML).
- **Weaknesses:** Multiple comment styles. Inconsistent trailing comma handling.

### JSON5
- **Strengths:** JSON superset. Comments. Trailing commas. Unquoted keys. Multiline strings. Hex numbers. Based on ES5.
- **Weaknesses:** "Too many unnecessary features." Doesn't consolidate with JSONC/Hjson.

### RON (Rusty Object Notation)
- **Strengths:** Maps to Rust type system (structs, enums, tuples, Option). Trailing commas. Comments. Serde integration.
- **Weaknesses:** Very Rust-centric. Not self-describing.

### NestedText
- **Strengths:** Only one type: strings. No quoting or escaping. Indentation for hierarchy. Comments. Eliminates all type coercion.
- **Weaknesses:** Indentation-sensitive. No native booleans/numbers -- all deferred to application.

### ASON
- **Strengths:** Explicit number types (u8, i32, f32, f64). DateTime, Tuple, Char types. Variant/algebraic types. Eliminates null.
- **Weaknesses:** Very new, minimal ecosystem.

### Pkl (Apple)
- **Strengths:** Code generation for JSON/YAML/Go/Swift/Kotlin. Immutability. Inheritance/composition. Open-source.
- **Weaknesses:** Requires JVM runtime.

### Jsonnet (Google)
- **Strengths:** Pure functional. Hermetic/deterministic. JSON superset. Lazy evaluation. Prototype-based OOP. Used at Google scale.
- **Weaknesses:** Debugging difficult. Dynamic typing. Templates become their own complexity.

### CUE
- **Strengths:** Based on lattice theory. Order-independent composition. Automatic constraint reduction. JSON superset. Solid theory.
- **Weaknesses:** Restrictions on inheritance limit boilerplate removal. No general recursion or functions. High learning curve.

### Newer Formats (2024-2026)

| Format | Key Idea | Status |
|--------|----------|--------|
| **KYAML** (k8s 1.34) | Strict YAML subset, flow-style `{}[]`, always double-quoted, comments + trailing commas | Production |
| **RCL** | Gradually typed JSON superset, functional, generates JSON/YAML/TOML | Active |
| **MAML** | JSON-compatible + optional commas + multiline raw strings + int/float distinction | New |
| **CCL** | Category-theory-based, everything key-value, everything strings, monoid homomorphism | Proof-of-concept |
| **Confetti** | "Markdown for config files," minimalistic, untyped | Experimental |
| **Ziggy** | Zig-inspired, tagged unions, trailing comma controls formatting | Active |
| **HUML** | YAML-like but stricter/consistent, `::` for vectors | Experimental |

---

## 2. Known Problems Worth Solving

### The Norway Problem (YAML)
Country code `NO` parses as boolean `false`. Also: `yes`/`no`, `on`/`off`, `y`/`n` become booleans. Version `3.10` becomes float `3.1`. Port `22:22` becomes sexagesimal integer 1342.

**Lesson:** Implicit typing is a design mistake. Types must be determined by syntax, not by guessing from content.

### JSON Verbosity for Humans
Mandatory quoting of all keys and strings. No comments. No trailing commas. No multiline strings. "Not fast enough for serialization; not human-readable enough for configuration."

**Lesson:** The minimum viable improvement over JSON is: comments + trailing commas + optional key quotes.

### TOML Deep Nesting
```toml
[servers.alpha.config.network.ipv4]
address = "10.0.0.1"
```
Deeply nested structures require repeating long paths. Array-of-tables `[[]]` is confusing. Lines at end of file silently enter wrong section.

**Lesson:** Nesting must be visually clear and locally scoped. Avoid section headers that create "distant action."

### YAML Spec Complexity
23,449 words. 9 multiline string syntaxes. Inconsistent parser implementations. Different behavior between YAML 1.1 and 1.2.

**Lesson:** Spec should be small (JSON-sized). One way to do things. No ambiguity between implementations.

### Security
YAML tags enable arbitrary object deserialization -> RCE. Every Ruby on Rails app was vulnerable in 2013.

**Lesson:** Data formats should not have features that enable code execution. No tags, no custom constructors.

---

## 3. Design Principles

### From the Research

1. **No implicit typing.** Types determined by syntax alone, not content guessing.
2. **Small spec.** Target JSON-scale (~2000 words). One way to express each concept.
3. **Comments are mandatory.** Every config format needs them.
4. **Trailing commas.** Reduces diff noise, prevents editing errors.
5. **Context-free grammar, ideally LL(1).** One-token lookahead, no backtracking, trivial to implement.
6. **No features that enable code execution.** Pure data, no tags/constructors.
7. **Familiar syntax.** Build on JSON/C-family knowledge, not novel concepts.
8. **Good error messages.** Format design should make error location trivial.
9. **Explicit is better than implicit.** Quote strings when ambiguous. Distinct syntax for each type.
10. **Autoformattable.** Single canonical representation to prevent style debates.

### The Configuration Complexity Clock

A recurring cycle:
1. Hardcoded values
2. Config files (simple key-value)
3. Rules engine (config becomes executable)
4. DSL (team builds custom language)
5. Return to code (abandon DSL, use real language)
6. Back to step 1

**Key insight:** Don't try to be a programming language. A data format should be purely declarative. If users need abstraction, they should use a real language to generate the data.

### Trade-off: Simplicity vs Expressiveness

The research strongly suggests staying on the "simple data" side:
- Programmable formats (Dhall, CUE, Jsonnet, Nickel, Pkl) each reinvent a programming language
- "Abstraction matters more than syntax" -- if you need abstraction, use a real language
- The "JSON + improvements" space (JSON5, Hjson, KYAML) has proven demand
- "All strings + external schema" (NestedText, CCL, StrictYAML) is the simplest possible approach but loses self-description

**Sweet spot:** A format that is self-describing (unlike NestedText) with explicit types (unlike YAML) but minimal syntax (unlike TOML's 3 object syntaxes).

---

## 4. Parsing Considerations

### Parser Approach Ranking
1. **Hand-written recursive descent** -- 9/10 top languages use this. Best control, best error messages.
2. **Parser combinators** -- 5-10x slower but easier to maintain. Good for prototyping.
3. **Parser generators** -- Less common in modern implementations.

### Grammar Design
- **LL(1):** One-token lookahead. No backtracking. Extremely fast, simple implementation.
- **Context-free:** Required for efficient parsing. Avoid context-sensitivity.
- **Whitespace:** If using significant whitespace, handle via lexer preprocessing (emit INDENT/DEDENT tokens). But significant whitespace makes templating/generation harder and is error-prone in large files.

### Whitespace Significance Trade-offs
| For | Against |
|-----|---------|
| ~10% shorter files | Tab/space mixing creates invisible bugs |
| Forces readable formatting | Hard to determine intent when indentation wrong |
| Single way to read structure | Makes templating/generation much harder |
| | Doesn't scale to very large files |
| | Copy-paste between indent levels breaks |

**Recommendation from research:** Use brackets for structure (like JSON), not indentation (like YAML/Python). This makes the format unambiguous, easy to generate, and avoids invisible bugs.

---

## 5. Type System Design

### Approaches Spectrum

| Approach | Example | Pros | Cons |
|----------|---------|------|------|
| All-strings | NestedText, CCL | Zero ambiguity, simplest | Not self-describing |
| Implicit | YAML 1.1 | Minimal syntax | Norway problem, data corruption |
| Syntax-explicit | JSON, TOML | Unambiguous | Quoting overhead |
| Type-tagged | YAML `!!`, ASON | Maximum precision | Verbose, potential security risk |
| Schema-based | Protobuf | Most efficient | Requires schema distribution |

### Recommended: Syntax-Explicit (JSON-style, improved)

Each type has a distinct, unambiguous syntax:
- **Strings:** Quoted (single or double). No unquoted strings to avoid Norway problem.
- **Numbers:** Digits with optional decimal point. Distinguish integer vs float by presence of `.`
- **Booleans:** Only `true`/`false`. No `yes`/`no`/`on`/`off`.
- **Null:** Only `null`. No `~` or empty value.
- **Arrays:** `[...]`
- **Objects/Maps:** `{...}`

### Types Worth Considering
- **Multiline strings:** Essential. One syntax only (not YAML's 9).
- **Date/Time:** TOML's RFC 3339 approach is good. But adds parser complexity.
- **Binary data:** Base64-encoded strings with a prefix? Or omit entirely (use strings).
- **Integer vs Float:** Worth distinguishing (JSON's lack of this is a real problem).

---

## 6. Key Design Decisions to Make

### Must-Have Features (consensus from research)
- [ ] Comments (line comments at minimum, `//` or `#`)
- [ ] Trailing commas in arrays and objects
- [ ] Multiline strings (one clear syntax)
- [ ] Integer vs float distinction
- [ ] No implicit typing
- [ ] Explicit `true`/`false`/`null` only
- [ ] Simple, small spec

### Debatable Features
- [ ] Unquoted keys (convenient but adds parsing complexity)
- [ ] Single-quoted vs double-quoted strings (or just one quote type)
- [ ] Date/time literals (useful but adds complexity)
- [ ] Hex/octal/binary integer literals
- [ ] String escaping rules (JSON-style `\n` or raw strings?)
- [ ] Significant whitespace vs brackets
- [ ] Optional commas vs required commas
- [ ] Root element: must be object? Can be any value?

### Anti-Features (consensus: avoid)
- [ ] ~~Anchors/aliases/references~~
- [ ] ~~Custom tags/type constructors~~
- [ ] ~~Multiple syntaxes for the same concept~~
- [ ] ~~Implicit boolean conversion (yes/no/on/off)~~
- [ ] ~~Sexagesimal numbers~~
- [ ] ~~String interpolation~~
- [ ] ~~Include/import directives~~
- [ ] ~~Variables or expressions~~

---

## 7. Interesting Ideas from Specific Formats

### From Ziggy
Trailing comma controls formatting:
```
# Trailing comma = vertical layout
items: [
  "one",
  "two",
  "three",
]

# No trailing comma = horizontal layout
items: ["one", "two", "three"]
```

### From KDL
Node-based model: values can be positional AND named on the same line:
```kdl
node "positional" key="named" {
    child "value"
}
```

### From KYAML
Strict subset approach: every document is also valid in the parent format (YAML), enabling gradual migration.

### From CCL
Mathematical foundation: semigroup (combining configs is associative), monoid (empty config is identity), homomorphism (parsing preserves structure). Enables parallel parsing.

### From ASON
Algebraic types in data: `variant::Color("red")`, `Option::Some(42)`, `Option::None`. Useful for Rust interop.

### From MAML
Raw multiline strings with explicit delimiters:
```
description: ```
  This is a multiline
  string with no escaping needed.
```
```

### From Confetti
"Markdown for configuration" -- optimized for the 80% case of flat key-value pairs, with minimal syntax for the remaining 20%.

---

## 8. Sources

### Critical Analysis of Existing Formats
- [The YAML document from hell](https://ruudvanasseldonk.com/2023/01/11/the-yaml-document-from-hell)
- [YAML: probably not so great after all](https://www.arp242.net/yaml-config.html)
- [noyaml.com](https://noyaml.com/)
- [The Norway Problem](https://hitchdev.com/strictyaml/why/implicit-typing-removed/)
- [What is wrong with TOML?](https://hitchdev.com/strictyaml/why-not/toml/)
- [Things I don't like in configuration languages](https://medv.io/blog/things-i-dont-like-in-configuration-languages)
- [Why broot switched from TOML to Hjson](https://dystroy.org/blog/hjson-in-broot/)

### Design Philosophy
- [A reasonable configuration language](https://ruudvanasseldonk.com/2024/a-reasonable-configuration-language) (RCL)
- [Abstraction, not syntax](https://ruudvanasseldonk.com/2025/abstraction-not-syntax)
- [The most elegant configuration language](https://chshersh.com/blog/2025-01-06-the-most-elegant-configuration-language.html) (CCL)
- [Jsonnet Language Design](https://jsonnet.org/articles/design.html)
- [Nickel RATIONALE.md](https://github.com/tweag/nickel/blob/master/RATIONALE.md)
- [Beyond YAML](https://tm.kehrenberg.net/a/better-config-format/)
- [Google SRE Configuration Design](https://sre.google/workbook/configuration-design/)

### New Formats
- [KDL](https://kdl.dev/)
- [KYAML Reference](https://kubernetes.io/docs/reference/encodings/kyaml/)
- [MAML](https://maml.dev/)
- [Ziggy](https://ziggy-lang.io/documentation/about/)
- [HUML](https://huml.io/)
- [Confetti](https://confetti.hgs3.me/)
- [NestedText](https://nestedtext.org/en/latest/)
- [ASON](https://github.com/hemashushu/ason)

### Parsing
- [Parser generators vs handwritten parsers (2021 survey)](https://notes.eatonphil.com/parser-generators-vs-handwritten-parsers-survey-2021.html)
- [Building whitespace-sensitive languages](https://arxiv.org/html/2510.08200v1)
- [Principled parsing for indentation-sensitive languages (paper)](https://michaeldadams.org/papers/layout_parsing/LayoutParsing.pdf)

### Practical Experience
- [Making a Very Bad Data Serialization Language](https://blog.khutchins.com/posts/making-a-very-bad-data-serialization-language/)
- [Your configs suck? Try a real programming language](https://beepb00p.xyz/configs-suck.html)
- [Configuration Complexity Clock](https://www.baytechconsulting.com/blog/configuration-complexity-clock-explained)
