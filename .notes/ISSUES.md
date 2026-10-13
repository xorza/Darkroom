# Open issues

- **`ops::transform::cpu` `read_tap` takes five parameters.** `pixels`, `w`, `h`, `x` and `y`, in both the aarch64 NEON path (`neon.rs`) and the x86 SSE path (`sse.rs`).
