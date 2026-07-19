# Common Architecture Review

Status: 2026-07-20

## Executive summary

`common` has a sound cross-crate core: serialization formats, cancellation,
typed IDs, introspection, spans, file discovery, and small numerical helpers
all have multiple legitimate consumers or define workspace-wide contracts.
The previous review's critical introspection work, fallible/sorted file
discovery, extension-policy split, and `KeyIndexVec` removal are reflected in
the current code and are no longer active findings. Ordered editor placement
state now uses `IndexMap` with its custom serialization beside the owning
document module (`darkroom/src/core/document/mod.rs:180`,
`darkroom/src/core/document/serde.rs:1-49`).

The remaining work is concentrated in three areas:

1. Make serialized output and authored-state publication safe at their
   persistence boundaries.
2. Return Scenarium- and Lumos-specific primitives to their sole production
   owners, reducing `common`'s dependency and API surface.
3. Remove avoidable copies and unconditional proc-macro build cost from the
   serialization/introspection layer.

The highest-priority correctness defect wholly inside `common` is the
asymmetric LZ4 size contract. The most consequential downstream risks are
direct overwrite of authored files and a persistent image cache keyed by a
64-bit non-cryptographic path hash plus whole-second modification time.

## Review scope

This review covers every production module under `common/src`, the
`common-derive` proc macro, both manifests, and production call sites needed to
establish ownership and persistence behavior. Tests, benches, examples, and
test-support code were excluded from findings.

| Disposition | Current modules |
| --- | --- |
| Keep in `common` | serialization and `SerdeFormat`, `CancelToken`, introspection traits/value types, typed-ID macro, `Span`, file discovery, `FloatExt`, constants/debug helpers |
| Move to Scenarium | `Shared`, `Slot`, `PauseGate`, `ReadyState` |
| Move to Lumos | `BitBuffer2`, `Vec2us`, `Rgb`, CPU detection, bounded parallel mapping, `FnvHasher`, `SharedFn` |

## Batch 1 — Critical: keep serialization pairs symmetric

- [x] **Reject LZ4 values during encode when the paired decoder would reject them.** Encoding and decoding share the same 1 GiB ceiling, the encoder validates the JSON byte length before allocating compression output, and the 32-bit header uses a checked conversion. Boundary tests cover the ceiling, ceiling plus one, and `u32::MAX + 1` without multi-gigabyte fixtures; the ordinary LZ4 round trip still verifies the wire format.

## Batch 2 — High: make persisted state durable and collision-resistant

- [ ] **Publish authored serialized state through one atomic replacement helper.** Document/graph export, library saves, preferences, and CFA masters still overwrite their final paths directly (`darkroom/src/core/io/persistence.rs:55-60`, `darkroom/src/core/io/library.rs:105-118`, `darkroom/src/core/io/preferences.rs:178-185`, `lumos/src/io/astro_image/cfa.rs:112-117`), so interruption or disk-full can destroy the last readable copy. Two private sibling-temp implementations already exist: the cache-oriented writer handles unique temp names and cleanup but deliberately skips durability (`scenarium/src/execution/disk_store/mod.rs:249-286`), while the token writer uses a fixed `.tmp` name (`darkroom/src/core/script/tcp/mod.rs:202-220`). Add a uniquely named, same-directory byte-publication helper to the shared file layer with explicit durable and cache modes; the durable path must flush the file and parent directory as required by the supported platforms before/after replacement. Route authored state through the durable mode and caches through the non-durable mode. Verify replacement, concurrent writers, cleanup after failure, and preservation of the prior file when writing or publication fails.

- [ ] **Replace `FnvHasher` as persistent frame-cache identity before relocating it.** `FnvHasher` is a fixed-seed 64-bit non-cryptographic hasher (`common/src/fnv.rs:1-34`), yet the frame cache uses only that path hash as its filename (`lumos/src/stacking/frame_store/mod.rs:325-329`). Reuse then checks plane length and a sidecar containing modification time truncated to whole seconds (`lumos/src/stacking/frame_store/mod.rs:336-341`, `lumos/src/stacking/combine/cache/loader/mod.rs:324-371`, `lumos/src/stacking/combine/cache/loader/mod.rs:390-398`). A hash collision between equal-shaped inputs can select another source's pixels, and rewriting one source twice in a second can reuse stale data. Use a collision-resistant full digest for persistent names and store/compare the source identity, byte length, and highest available modification-time resolution in the sidecar; keep FNV only for transient in-memory hashing if measurement still justifies it. Verify a forced key collision, a same-path rewrite within one second, and normal cache reuse.

## Batch 3 — Medium: put runtime synchronization in Scenarium

- [ ] **Move `Shared`, `Slot`, `PauseGate`, and `ReadyState` to their sole production owner and remove the resulting dependencies from `common`.** `Shared` only backs Scenarium's `SharedAnyState` (`common/src/shared.rs:5-28`, `scenarium/src/runtime/shared_any_state.rs:3-36`); `Slot` only serves the FPS event state (`scenarium/src/elements/worker_events_library.rs:44-55`, `scenarium/src/elements/worker_events_library.rs:76-105`); and `PauseGate`/`ReadyState` only coordinate Scenarium's worker event loop (`scenarium/src/worker/mod.rs:101-105`, `scenarium/src/worker/event_loop.rs:23-57`). Their presence makes every `common` consumer receive Tokio and arc-swap (`common/Cargo.toml:21`, `common/Cargo.toml:28`). Move the modules and tests into Scenarium, remove their root exports (`common/src/lib.rs:49-56`), and then remove unused dependencies from `common`. Validate Scenarium independently and use `cargo tree` to confirm unrelated `common` consumers no longer receive these runtime dependencies through this crate.

- [ ] **Make `PauseGate` close guards composable while moving it.** Each `close()` writes the same boolean (`common/src/pause_gate.rs:49-57`), and dropping any guard clears that boolean and wakes every waiter (`common/src/pause_gate.rs:61-70`). Two overlapping guards therefore reopen the gate when the first drops even though the second remains alive. Replace the flag with a close count and notify only on the `1 -> 0` transition, or enforce unique close ownership in the type. Verify two guards dropped in both orders and guards created from separate clones. The worker currently serializes its one guard (`scenarium/src/worker/mod.rs:179-195`), so this hardens the primitive's promised public behavior rather than fixing a currently observed worker race.

- [ ] **Reduce `Slot` to the send/observe contract Scenarium actually uses.** Production publishes with `send`, observes with `peek`, and waits with `peek_async` (`scenarium/src/elements/worker_events_library.rs:49-55`, `scenarium/src/elements/worker_events_library.rs:83-105`). `SlotError`, `take`, `take_async`, `try_take`, and `has_value` add a second ownership-transfer protocol in which a live peek makes taking fail (`common/src/slot.rs:6-8`, `common/src/slot.rs:47-88`, `common/src/slot.rs:109-115`), but no production caller uses it. Delete that protocol during relocation and keep a latest-value observable slot. Verify overwrite behavior, immediate observation, and wake-up of every pending observer.

## Batch 4 — Medium: return image-processing implementation to Lumos

- [ ] **Move `BitBuffer2`, `Vec2us`, and `Rgb` into coherent Lumos modules.** These public types (`common/src/lib.rs:37`, `common/src/lib.rs:51`, `common/src/lib.rs:58`) have no production consumer outside Lumos. `BitBuffer2` is used by star-detection/background masks (`lumos/src/stacking/star_detection/threshold_mask/mod.rs:25`, `lumos/src/background_mesh/workspace.rs:4`), `Vec2us` by image geometry and registration (`lumos/src/io/astro_image/mod.rs:18`, `lumos/src/stacking/registration/interpolation/mod.rs:15`), and `Rgb` by image operations (`lumos/src/image_ops/mod.rs:32`, `lumos/src/image_ops/stretching/mod.rs:27`). Relocate each type beside its domain owner, narrow visibility where possible, remove the root exports, and remove `aligned-vec` from `common/Cargo.toml:17`. Validate Lumos independently, including its SIMD and serialization paths.

- [ ] **Fix `BitBuffer2`'s empty iteration, dimension arithmetic, and hot assertions as part of the move.** Either zero dimension makes a buffer empty (`common/src/bit_buffer2.rs:126-136`), but `BitIter::next` checks only height before reading `(0, 0)` (`common/src/bit_buffer2.rs:324-345`), so iterating or converting a `0 x N` buffer panics. Construction also multiplies dimensions/stride without checked overflow (`common/src/bit_buffer2.rs:39-43`, `common/src/bit_buffer2.rs:65-79`), and per-bit access pays release assertions (`common/src/bit_buffer2.rs:138-178`) despite being used in pixel loops. Track one logical index or terminate on either zero dimension, validate allocation sizes with checked arithmetic at construction, and use `debug_assert!` for per-bit internal bounds while retaining cold input validation. Verify `0 x N`, `N x 0`, `0 x 0`, exact iterator lengths/conversions, overflow rejection, debug out-of-bounds behavior, and scalar/SIMD agreement.

- [ ] **Move or remove the remaining Lumos-only utilities.** CPU dispatch and bounded parallel mapping are production-used only by Lumos (`lumos/src/math/sum/mod.rs:29-70`, `lumos/src/stacking/pipeline/streaming.rs:6-86`), while `SharedFn` only wraps the `ProgressCallback` alias (`common/src/shared_fn.rs:3-39`, `lumos/src/stacking/progress.rs:25-35`). Move the CPU and fallible batching helpers into Lumos, reconciling their shape with Imaginarium's standalone detector where practical (`imaginarium/src/cpu_features.rs:1-55`); retain the infallible batching variant only if a production caller remains. Replace `SharedFn` with a domain-local optional `Arc<dyn Fn...>`, move any transient FNV use after the persistent-cache change, and remove obsolete modules/exports. This should remove Rayon from `common/Cargo.toml:20` and substantially narrow its utility surface. Validate bounded concurrency, input-order preservation, error propagation, SIMD dispatch on supported architectures, and progress callback behavior.

## Batch 5 — Low: remove avoidable copies and build dependencies

- [ ] **Give slice deserialization a direct slice dispatch instead of routing through `Read`.** `deserialize(&[u8], ..)` wraps the slice in `Cursor`, after which TOML and Bitcode copy the entire payload into scratch with `read_to_end` (`common/src/serde.rs:75-97`). Darkroom's undo stack already holds a contiguous range but deliberately calls the reader path, adding the same O(payload) copy on every decode (`darkroom/src/core/edit/action_stack/mod.rs:260-278`). Dispatch slice inputs directly to each backend's slice decoder and reserve `deserialize_from` for real readers; update undo to use the slice entry point. Verify identical decoding and trailing-data behavior for every format and confirm repeated undo/redo does not grow or fill an O(payload) decode scratch buffer.

- [ ] **Replace the exported normalization trait with a private consuming helper.** `NormalizeString` is publicly re-exported (`common/src/lib.rs:48`) but its only production use is internal TOML serialization (`common/src/serde.rs:6-7`, `common/src/serde.rs:52-54`). That caller already owns the `String`, yet an already normalized value is cloned (`common/src/normalize_string.rs:32-40`). Replace the trait with a private `fn normalize(String) -> String`: append a missing newline in place and allocate only when removing CR/CRLF. Verify empty, LF, CRLF, lone-CR, and missing-final-newline cases, plus capacity reuse on the no-CR path.

- [ ] **Make the introspection derive opt-in and narrow its parser features.** `common-derive` is an unconditional dependency (`common/Cargo.toml:15-16`) and is always re-exported (`common/src/introspect/mod.rs:299`), although Lens is the only production derive consumer (`lens/src/astro/configs.rs:11-50`, `lens/src/astro/configs.rs:196-209`). The workspace also enables Syn's `full` parser (`Cargo.toml:72`) while this macro only parses derive inputs, fields, types, and attributes. Put the proc-macro dependency/re-export behind an `introspect-derive` feature enabled by Lens, and give `common-derive` only the Syn features it uses rather than inheriting `full`. Verify the introspection traits/value types remain usable without the feature, all Lens derives still expand, and `cargo tree` for a non-Lens consumer no longer contains `common-derive` through `common`.

## Open questions

- [ ] **Is `CancelToken::reset` guaranteed to run only after every clone from the previous operation has quiesced?** Live clones share one flag, and `reset` clears it for all of them (`common/src/cancel_token.rs:12-22`, `common/src/cancel_token.rs:46-64`). The worker awaits its run before reset (`scenarium/src/worker/mod.rs:179-195`), which is safe only if every spawned/blocking task has joined. If quiescence is a hard invariant, narrow reset ownership and document it at the worker boundary; if operations can overlap, create a fresh token per operation so an old clone remains cancelled permanently.
