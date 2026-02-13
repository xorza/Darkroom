Perform a code quality review of each submodule in the given module. For each submodule:

1. Read all source files thoroughly
2. Identify opportunities for simplification, generalization, and consistency improvements
3. Assess each finding on three dimensions (1-5 scale):
   - **Impact**: How much does this improve correctness, performance, readability, or maintainability?
   - **Meaningfulness**: Is this a real improvement or just cosmetic/stylistic?
   - **Invasiveness**: How many files/lines change? How risky is the refactor?

Categories to look for:
- **Simplifications**: Overly complex logic that can be reduced. Unnecessary abstractions, indirection, or generics. Code that does more than needed.
- **Generalizations**: Duplicated patterns across submodules that could share a common implementation. Similar structs/enums/traits that could be unified. Copy-pasted logic with minor variations.
- **Consistency**: Naming inconsistencies (similar things named differently). Different error handling patterns for the same kind of errors. Mixed conventions (e.g., some modules use iterators, others use index loops for the same pattern). Inconsistent API shapes across sibling modules.
- **Dead code**: Unused functions, unreachable branches, redundant checks, stale comments.
- **API cleanliness**: Public API that exposes internal details. Functions that take too many parameters (should be a struct). Return types that leak implementation.

Process all submodules in parallel. After all submodules are reviewed, produce a single consolidated report.

## Output Format

Write the report to `REVIEW.md` in the module's directory. Use this format:

```markdown
# Code Review: <module name>

## Summary
Brief overview of findings and overall code quality assessment.

## Findings

### Priority 1 — High Impact, Low Invasiveness
(Easy wins that meaningfully improve the code)

#### [F1] <Title>
- **Location**: `file.rs:123-145`
- **Category**: Simplification / Generalization / Consistency / Dead code / API
- **Impact**: 4/5 — <why>
- **Meaningfulness**: 5/5 — <why>
- **Invasiveness**: 1/5 — <why>
- **Description**: What the issue is and how to fix it.

### Priority 2 — High Impact, Moderate Invasiveness
(Significant improvements that require careful refactoring)

### Priority 3 — Moderate Impact
(Nice-to-have improvements)

### Priority 4 — Low Priority
(Minor issues, cosmetic, or speculative improvements)

## Cross-Cutting Patterns
Patterns that appear across multiple submodules.
```

Sort findings within each priority group by impact (highest first). Do NOT include findings that are purely stylistic with no functional benefit. Focus on changes that make the code genuinely better.

Do NOT make any code changes. This is a read-only analysis task.
