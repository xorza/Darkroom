# Open issues

- **lumos `memory_budget` can still fail under heavy load.** `tests/memory_budget/live_heap.rs` counts every thread's allocations, and the test harness's own threads allocate between tests — a finished test's thread exiting, the main thread recording its result — while the next test, holding `HEAP.exclusive()`, measures. Seen once in 29 runs at a load average of ~12, as `holds_the_same_heap_for_any_frame_count` reading a larger `peak(3)` than `peak(4)`.
