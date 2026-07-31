use super::*;

/// A `Both` value remains resident even when a later run neither executes
/// nor reads it.
#[tokio::test]
async fn both_value_stays_resident_outside_the_active_frontier() {
    let dir = TempDir::new("both-retained");
    let calls = Calls::default();

    // src(1) → sum(Both) = 2 → mult(Both) = 2 → print.
    let mut g = TestGraph::new();
    // `src` retains in RAM but never persists — the "non-reloadable value"
    // the last assertion is about.
    g.add("src", |n| source(1, &calls)(n).cache(CacheMode::Ram));
    g.add("sum", sum(CacheMode::Both));
    g.add("mult", mult(CacheMode::Both));
    g.add("print", |n| n.records());
    g.wire("src", 0, "sum", 0);
    g.wire("src", 0, "sum", 1);
    g.wire("sum", 0, "mult", 0);
    g.wire("src", 0, "mult", 1);
    g.wire("mult", 0, "print", 0);

    let mut e = disk_engine(&dir, g);
    e.run_sinks().await;
    assert!(
        e.holds_output("sum"),
        "sum is resident after the run that computed it"
    );

    let run = e.run_sinks().await;
    assert_eq!(run.ran_node_count, 1, "only print runs the second time");
    assert_eq!(
        e.output_i64("sum", 0),
        Some(2),
        "Both keeps the exact prior-run value resident outside the frontier"
    );
    assert!(
        e.holds_output("mult"),
        "the frontier value the run read is kept resident"
    );
    assert!(
        e.holds_output("src"),
        "a non-reloadable memory-only value is kept, never force-recomputed"
    );

    // An empty replacement store proves the later hit comes from retained
    // RAM, not from disk.
    let empty = TempDir::new("both-retained-empty");
    e.attach_disk_store(empty.path());
    e.edit(|g| g.constant("mult", 1, 3i64));
    let run = e.run_sinks().await;
    assert!(
        run.cached().contains(&"sum"),
        "sum is reused from retained RAM"
    );
    assert!(!run.ran().contains(&"sum"), "sum does not recompute");
    assert!(
        run.ran().contains(&"mult"),
        "the changed downstream recomputes"
    );
    assert!(
        e.holds_output("sum"),
        "the reused Both value remains resident"
    );
}

/// One row of the cache-mode matrix. Over a fresh store, build
/// `src → mult(mode) → print` (an impure sink, so `mult` is needed every
/// run), run twice on one engine, then reopen with empty RAM. Asserts the
/// four modes' *distinct* outcomes on the axes they differ on: cross-run
/// reuse, RAM retention after the run, and disk persistence.
async fn assert_mode_behavior(mode: CacheMode) {
    let dir = TempDir::new(&format!("mode-{mode:?}"));
    let calls = Calls::default();

    let mut e = disk_engine(&dir, source_mult_print(mode, 1, &calls));
    let run1 = e.run_sinks().await;
    assert!(
        run1.ran().contains(&"mult"),
        "{mode:?}: mult computes on the cold run"
    );

    let run2 = e.run_sinks().await;
    if mode == CacheMode::None {
        assert!(
            run2.ran().contains(&"mult"),
            "None recomputes every run its value is needed"
        );
        assert!(
            !run2.cached().contains(&"mult"),
            "None is never reported cached"
        );
    } else {
        assert!(
            run2.cached().contains(&"mult"),
            "{mode:?} reuses its cached output on run 2"
        );
        assert!(
            !run2.ran().contains(&"mult"),
            "{mode:?} does not recompute on run 2"
        );
    }

    // Slot retention after run 2: RAM-resident iff the mode keeps RAM.
    assert_eq!(
        e.holds_output("mult"),
        mode.caches_in_ram(),
        "{mode:?}: RAM retention must equal caches_in_ram()"
    );
    if mode.caches_in_ram() {
        assert_eq!(
            e.output_i64("mult", 0),
            Some(1),
            "{mode:?} keeps the resident value (1 * 1 = 1)"
        );
    }

    // A blob exists iff the mode persists to disk.
    assert_eq!(
        dir.entry_count() > 0,
        mode.persists_to_disk(),
        "{mode:?}: a blob exists iff persists_to_disk()"
    );

    // Reopen with empty RAM over the same store: only a disk-backed mode
    // survives.
    let mut e = disk_engine(&dir, source_mult_print(mode, 1, &calls));
    let reopen = e.run_sinks().await;
    if mode.persists_to_disk() {
        assert!(
            reopen.cached().contains(&"mult"),
            "{mode:?} reloads mult from disk on reopen"
        );
        assert!(
            !reopen.ran().contains(&"src"),
            "{mode:?}: the cut prunes src behind the disk hit"
        );
    } else {
        assert!(
            reopen.ran().contains(&"mult"),
            "{mode:?} has no disk blob, so mult recomputes on reopen"
        );
        assert!(
            reopen.ran().contains(&"src"),
            "{mode:?}: src recomputes to feed mult"
        );
    }
}

/// The four cache modes produce four distinct reuse / retention /
/// persistence behaviors — the parameterized proof that the mode actually
/// drives the engine.
#[tokio::test]
async fn cache_mode_matrix() {
    for mode in [
        CacheMode::None,
        CacheMode::Ram,
        CacheMode::Disk,
        CacheMode::Both,
    ] {
        assert_mode_behavior(mode).await;
    }
}

/// `None` is storage-only: it never taints downstream reproducibility.
/// `A(None) → B(Disk)` — B still has a content digest, so it persists and,
/// on reopen, is served from disk with A cut (not recomputed), exactly as if
/// A were an ordinary cached node. Contrast an `Impure` A, which *would*
/// strip B of its digest and force both to rerun.
#[tokio::test]
async fn none_upstream_does_not_disable_downstream_disk_cache() {
    let dir = TempDir::new("none-orthogonal");
    let calls = Calls::default();

    // src(1) → a = sum(None) = 2 → b = mult(Disk) = 4 → print.
    let build = |calls: &Calls| {
        let mut g = TestGraph::new();
        g.add("src", source(1, calls));
        g.add("a", sum(CacheMode::None));
        g.add("b", mult(CacheMode::Disk));
        g.add("print", |n| n.records());
        g.wire("src", 0, "a", 0);
        g.wire("src", 0, "a", 1);
        g.wire("a", 0, "b", 0);
        g.wire("a", 0, "b", 1);
        g.wire("b", 0, "print", 0);
        g
    };

    let mut e = disk_engine(&dir, build(&calls));
    let cold = e.run_sinks().await;
    assert!(
        cold.ran().contains(&"a") && cold.ran().contains(&"b"),
        "the cold run computes A and B"
    );
    assert!(
        dir.entry_count() > 0,
        "B(Disk) persists despite its None upstream"
    );

    // Reopen: B is a disk hit, so A(None) — read only by the reused B — is
    // cut, not recomputed. Setting A to None disabled neither B's cache nor
    // A's own reuse-cut.
    let mut e = disk_engine(&dir, build(&calls));
    let reopen = e.run_sinks().await;
    assert!(
        reopen.cached().contains(&"b"),
        "B reloads from disk on reopen"
    );
    assert!(
        !reopen.ran().contains(&"a"),
        "A(None) is cut behind the disk hit, not recomputed"
    );
    assert!(!reopen.ran().contains(&"src"), "src is cut behind A too");
}

/// Disabling RAM retention releases a surviving slot during install rather
/// than waiting for the end of a later run.
#[tokio::test]
async fn disabling_ram_retention_releases_resident_value_on_install() {
    for mode in [CacheMode::None, CacheMode::Disk] {
        let dir = TempDir::new(&format!("ram-downgrade-{mode:?}"));
        let mut g = TestGraph::new();
        g.add("mult", mult(CacheMode::Ram));
        g.add("print", |n| n.records());
        g.constant("mult", 0, 2i64);
        g.constant("mult", 1, 3i64);
        g.wire("mult", 0, "print", 0);

        let mut e = disk_engine(&dir, g);
        e.run_sinks().await;
        assert!(e.holds_output("mult"), "Ram retains the current pure value");

        e.edit(|g| g.cache("mult", mode));
        assert!(
            !e.holds_output("mult"),
            "{mode:?} releases the old RAM value during install"
        );
    }
}

/// A persisted node whose cone contains an impure node has digest `None`, so
/// it's never disk-cached even with `Disk` — on reopen it recomputes.
#[tokio::test]
async fn impure_cone_persist_node_is_not_disk_cached() {
    let dir = TempDir::new("impure-cone");
    let calls = Calls::default();
    let build = |calls: &Calls| {
        let mut g = source_mult_print(CacheMode::Disk, 11, calls);
        g.edit_func("src", |func| func.behavior = FuncBehavior::Impure);
        g
    };

    let mut e = disk_engine(&dir, build(&calls));
    e.run_sinks().await;

    // Reopen: mult must recompute — an impure cone has no digest, so it
    // never caches to disk.
    let mut e = disk_engine(&dir, build(&calls));
    let run = e.run_sinks().await;
    assert!(
        !run.cached().contains(&"mult"),
        "an impure-cone node must not be disk-cached"
    );
    assert!(run.ran().contains(&"mult"), "mult recomputes on reopen");
}

/// A RAM-only node (the default) is never written to disk even though its
/// cone is reproducible — only `Disk` opts in — so on reopen it recomputes.
#[tokio::test]
async fn memory_persistence_node_is_not_disk_cached() {
    let dir = TempDir::new("memory-persist");
    let calls = Calls::default();
    let build = |calls: &Calls| source_mult_print(CacheMode::Ram, 1, calls);

    let mut e = disk_engine(&dir, build(&calls));
    e.run_sinks().await;

    // Reopen: fresh RAM, nothing on disk for mult ⇒ it recomputes.
    let mut e = disk_engine(&dir, build(&calls));
    let run = e.run_sinks().await;
    assert!(
        !run.cached().contains(&"mult"),
        "a RAM-only node must not be disk-cached"
    );
    assert!(run.ran().contains(&"mult"), "mult recomputes on reopen");
}

/// A persisted node whose blob is on disk but whose custom output type has
/// *no registered codec* — a value written by a build that had the codec,
/// reopened by one that doesn't — is not reused from disk. It recomputes
/// rather than panicking during loading; with the codec it is served from
/// disk instead.
#[tokio::test]
async fn missing_codec_skips_disk_cache_instead_of_panicking() {
    use std::any::Any;
    use std::fmt;

    use async_trait::async_trait;
    use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

    use crate::data::codec::error::CodecError;
    use crate::library::TypeEntry;
    use crate::runtime::context::{ContextStore, ContextType};
    use crate::{CustomValue, CustomValueCodec, TypeId};

    /// Decode-side context resource: proves a codec can reach the runtime
    /// store while reconstructing a value read from disk.
    #[derive(Debug, Default)]
    struct DecodeProbe {
        decodes: usize,
    }
    const DECODE_PROBE: ContextType<DecodeProbe> = ContextType::new(DecodeProbe::default);

    const BLOB_TYPE: &str = "50be7976-6d55-4567-8389-13107b1698ba";

    #[derive(Debug)]
    struct Blob(Vec<u8>);
    impl fmt::Display for Blob {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "Blob({} bytes)", self.0.len())
        }
    }
    impl CustomValue for Blob {
        fn type_id(&self) -> TypeId {
            BLOB_TYPE.into()
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
            self
        }
    }

    #[derive(Debug)]
    struct BlobCodec;
    #[async_trait]
    impl CustomValueCodec for BlobCodec {
        fn version(&self) -> u32 {
            0
        }

        async fn encode(
            &self,
            value: &dyn CustomValue,
            writer: &mut (dyn AsyncWrite + Unpin + Send),
            _ctx: &mut ContextStore,
        ) -> std::result::Result<(), CodecError> {
            writer
                .write_all(&value.as_any().downcast_ref::<Blob>().unwrap().0)
                .await?;
            Ok(())
        }

        async fn decode(
            &self,
            reader: &mut (dyn AsyncRead + Unpin + Send),
            byte_len: u64,
            ctx: &mut ContextStore,
        ) -> std::result::Result<Arc<dyn CustomValue>, CodecError> {
            ctx.get(DECODE_PROBE).decodes += 1;
            let mut bytes = Vec::with_capacity(usize::try_from(byte_len)?);
            reader.read_to_end(&mut bytes).await?;
            Ok(Arc::new(Blob(bytes)))
        }
    }

    // A pure, disk-persisted sink emitting a custom `Blob`. The type's codec
    // is registered only when `with_codec` — and the store takes its codecs
    // from this same library, so that one flag decides both.
    let build = |with_codec: bool, recompute: &Calls| {
        let recompute = recompute.clone();
        let mut g = TestGraph::new();
        g.library.register_type(
            BLOB_TYPE,
            if with_codec {
                TypeEntry::custom_with_codec("Blob", Arc::new(BlobCodec))
            } else {
                TypeEntry::custom("Blob")
            },
        );
        // A `Custom` value has no `compute` shape — that writes a
        // `StaticValue` — so this body counts itself.
        let recompute = recompute.clone();
        g.add("make_blob", move |n: NodeSpec| {
            n.pure()
                .sink()
                .cache(CacheMode::Disk)
                .output(DataType::Custom(BLOB_TYPE.into()))
                .lambda(crate::async_lambda!(
                    move |Invocation { outputs, .. }| { counter = recompute.clone() } => {
                        counter.bump();
                        outputs[0] = DynamicValue::Custom(Arc::new(Blob(vec![9, 9, 9])));
                        Ok(())
                    }
                ))
        });
        g
    };

    let dir = TempDir::new("missing-codec");
    let recompute = Calls::default();

    // Run 1 (codec present): computes and writes the Blob to disk.
    let mut e = disk_engine(&dir, build(true, &recompute));
    e.run_sinks().await;
    assert_eq!(recompute.count(), 1, "the cold run computes");

    // Reopen with the codec: served from disk, and the hydration decode
    // reaches the engine's own runtime context store.
    let mut e = disk_engine(&dir, build(true, &recompute));
    let run = e.run_sinks().await;
    assert_eq!(recompute.count(), 1, "codec present ⇒ served from disk");
    assert!(run.cached().contains(&"make_blob"));
    assert_eq!(
        e.engine
            .executor
            .ctx_manager
            .contexts
            .get(DECODE_PROBE)
            .decodes,
        1,
        "the hydration decode reached the engine's runtime context store"
    );

    // Reopen WITHOUT the codec: the blob is present but undecodable, so it
    // is not flagged available — recompute, no panic.
    let mut e = disk_engine(&dir, build(false, &recompute));
    let run = e.run_sinks().await;
    assert_eq!(recompute.count(), 2, "a missing codec ⇒ recompute");
    assert!(
        !run.cached().contains(&"make_blob"),
        "an undecodable blob is not a cache hit"
    );
    assert!(
        run.ran().contains(&"make_blob"),
        "the node recomputes instead of tripping a failed frontier load"
    );
}
