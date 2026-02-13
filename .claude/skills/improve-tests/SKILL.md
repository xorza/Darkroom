Improve tests for each submodule in the given module. For each submodule:

1. Read all source files (production and test code) thoroughly
2. Understand what the production code does, its public API, code paths, and edge cases
3. Evaluate existing tests, then make changes

## What to do

### Add missing tests
- Write tests for public functions/methods that have no tests
- Add edge case tests: empty input, single element, boundary values, zero, negative
- For SIMD implementations, add scalar reference comparison tests if missing
- All new tests must have hand-computed expected values with math shown in comments
- Assert exact outputs, not vague ranges. Test that different parameters produce different results

### Remove redundant tests
- Delete tests that only check "it doesn't panic" without verifying outputs
- Delete tests with vague assertions like `result > 0` or `result < 100`
- Delete tautological tests that re-implement production logic to compute "expected" values
- Delete duplicate tests that verify the same thing as another test

### Improve weak tests
- Replace vague assertions with exact expected values (show the math)
- Add parameter sensitivity checks: test with param A → expect X, param B → expect Y, assert X ≠ Y
- Split tests that are too large and test too many things at once

### Simplify and generalize
- Extract duplicated test setup into shared helper functions
- Extract common assertion patterns into reusable helpers
- Remove test helpers or test data that leaked into production modules — move to `#[cfg(test)]` or test files
- Unify similar test patterns across submodules

## Process

Process all submodules in parallel. After all changes, run:
```
cargo nextest run && cargo fmt && cargo check && cargo clippy --all-targets -- -D warnings
```
Fix any issues until everything passes.
