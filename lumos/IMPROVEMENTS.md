
#### B. Suboptimal SIMD u16→f32 Conversion (`astro_image/libraw.rs:normalize_chunk_sse2`)

**Issue**: Manual scalar conversion bypasses SIMD:
```rust
let v0 = input[idx] as i32;
let v1 = input[idx + 1] as i32;
// ... manual element-by-element construction
```

**Suggestion**: Use `_mm_cvtepu16_epi32` for proper SSE2 widening instructions.

---
