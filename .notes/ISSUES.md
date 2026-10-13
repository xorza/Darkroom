# Open issues

- **lumos `memory_budget` still fails on macOS arm64 in about one gate run of twenty.** One of 21 runs of `./x.py gate -p lumos` failed a `memory_budget` test, which was not captured; 20 runs of the binary alone passed. A session of two workers counts the heap around handing a run to its pool, so an idle worker's first sleep — on which std boxes a `Condvar`'s `pthread_cond_t`, where it has no futex — can land inside a measurement.
