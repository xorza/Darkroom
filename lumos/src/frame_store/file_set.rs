//! [`FileSet`]: a frame set's files as a run takes them, each read once.

use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::frame::admitted::Admitted;
use crate::frame_store::decode_cache::DecodeCache;
use crate::frame_store::decode_cache::cached_source::CachedSource;
use crate::frame_store::decode_cache::frame_spill::Committed;
use crate::frame_store::decode_cache::kept_frame::KeptFrame;
use crate::frame_store::error::{FrameStoreError, LoadError};
use crate::frame_store::run_storage::RunStorage;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::stackable_image::StackableImage;
use crate::io::decoder::{Decoder, Peek};
use crate::io::frame_info::{DecoderBytes, FrameInfo};
use crate::io::load_ctx::LoadCtx;

/// A frame set's files as a run takes them through its storage: each one's state in the decode
/// cache, read once before any frame is decoded, and the frame the plan is read off, which its
/// decode takes rather than reads again. So a run reads each file and each commit once.
#[derive(Debug)]
pub(crate) struct FileSet<'a, P, D: Decoder> {
    paths: &'a [P],
    storage: RunStorage,
    /// Each path's source and the frame the cache committed for it, until its decode takes them;
    /// for a run that keeps a decode cache. A path whose source does not read holds none, and its
    /// decode reports why.
    probes: Option<Vec<Mutex<Option<Probe>>>>,
    /// The frames the decode cache holds none of, by index, as the probes found them: the frames
    /// the run writes to it. Empty without a cache.
    uncached: Vec<usize>,
    /// The frame the plan was read off, and what reading it left, until its decode takes it.
    first: Mutex<Option<Box<First<D>>>>,
}

/// A path's source as the decode cache keys it, and the frame committed for it then.
#[derive(Debug)]
struct Probe {
    source: CachedSource,
    committed: Option<Committed>,
}

/// The frame a plan was read off: its index, and what reading it left for its decode.
#[derive(Debug)]
struct First<D: Decoder> {
    idx: usize,
    early: Early<D>,
}

/// What reading a frame for the plan left for its decode.
#[derive(Debug)]
enum Early<D: Decoder> {
    /// What its header read read: a camera RAW's file.
    Read(D::Read),
    /// The frame decoded, for a format with no header that answers.
    Decoded(D::Image),
}

impl<D: Decoder> Early<D> {
    /// The frame of `path`, decoded under `ctx` from what was read of it.
    fn into_image(self, path: &Path, ctx: &LoadCtx) -> Result<D::Image, LoadError> {
        Ok(match self {
            Self::Read(read) => D::load_read(read, path, ctx)?,
            Self::Decoded(image) => image,
        })
    }

    /// The bytes held until the decode takes it.
    fn held_bytes(&self) -> usize {
        match self {
            Self::Read(read) => D::read_len(read),
            Self::Decoded(image) => image.dimensions().decoded_bytes(),
        }
    }
}

/// What a plan reads off a frame set: the frame that stands for every frame, at its index, and
/// the bytes the set holds for that frame's decode until the decode takes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SetPeek {
    pub(crate) idx: usize,
    pub(crate) info: FrameInfo,
    pub(crate) held: usize,
}

/// A frame of a set taken through a decode cache: kept by a run before, or decoded, with its
/// source as it was read before the decode, for the cache to keep.
#[derive(Debug)]
pub(crate) enum Taken<I> {
    Kept(Box<KeptFrame>),
    Decoded { image: I, source: CachedSource },
}

/// A path's source and what the cache committed for it, as a decode takes them.
#[derive(Debug)]
struct Resolved {
    source: CachedSource,
    committed: Option<Committed>,
}

impl<'a, P: AsRef<Path>, D: Decoder> FileSet<'a, P, D> {
    /// The frames of `paths`, decoded by `D` through `storage`: each path's state in its decode
    /// cache read under `ctx`, when it keeps one.
    pub(crate) fn new(paths: &'a [P], storage: RunStorage, ctx: &LoadCtx) -> Self {
        let probes: Option<Vec<_>> = storage.kept().map(|cache| {
            paths
                .iter()
                .map(|path| probe::<D>(cache, path.as_ref(), ctx))
                .collect()
        });
        let uncached = probes
            .iter()
            .flatten()
            .enumerate()
            .filter_map(|(idx, probe)| {
                probe
                    .as_ref()
                    .is_none_or(|probe| probe.committed.is_none())
                    .then_some(idx)
            });
        Self {
            paths,
            uncached: uncached.collect(),
            probes: probes.map(|probes| probes.into_iter().map(Mutex::new).collect()),
            storage,
            first: Mutex::new(None),
        }
    }

    pub(crate) const fn len(&self) -> usize {
        self.paths.len()
    }

    pub(crate) fn path(&self, idx: usize) -> &Path {
        self.paths[idx].as_ref()
    }

    /// Where the frames are read from and written to.
    pub(crate) const fn storage(&self) -> &RunStorage {
        &self.storage
    }

    /// How many frames the run writes to the decode cache: those it holds no frame of. None
    /// without a cache.
    pub(crate) const fn uncached(&self) -> usize {
        self.uncached.len()
    }

    /// What the plan reads off the set under `ctx`: the first frame the decode cache does not
    /// hold, by its header or by its decode, which that frame's decode then takes; or frame 0 as
    /// its commit records it when the cache holds every frame, so no decoder runs.
    ///
    /// # Errors
    ///
    /// The frame's decode, for a format with no header that answers.
    pub(crate) fn peek(&self, ctx: &LoadCtx) -> Result<SetPeek, LoadError> {
        let idx = match (&self.probes, self.uncached.first()) {
            (Some(probes), None) => return Ok(committed_peek(&probes[0])),
            (Some(_), Some(&idx)) => idx,
            (None, _) => 0,
        };
        let path = self.path(idx);
        let (info, early) = if let Some(Peek { info, read }) = D::peek(path, ctx) {
            (info, read.map(Early::Read))
        } else {
            let image = D::load(path, ctx)?;
            (FrameInfo::of_decoded(&image), Some(Early::Decoded(image)))
        };
        let held = early.as_ref().map_or(0, Early::held_bytes);
        let mut first = lock(&self.first);
        debug_assert!(first.is_none(), "a plan peeks its set once");
        *first = early.map(|early| Box::new(First { idx, early }));
        Ok(SetPeek { idx, info, held })
    }

    /// Frame `idx` in RAM, under `ctx`: read back from the decode cache when it holds the frame
    /// for the source as it is now, decoded and kept there otherwise; decoded without a cache.
    ///
    /// # Errors
    ///
    /// The decode, or the cache.
    pub(crate) fn decode(&self, idx: usize, ctx: &LoadCtx) -> Result<D::Image, LoadError> {
        let path = self.path(idx);
        let early = self.take_early(idx);
        let Some(cache) = self.storage.kept() else {
            return load(early, path, ctx);
        };
        let Resolved { source, committed } = self.resolve(cache, idx, early.is_some(), ctx)?;
        if let Some(image) =
            committed.and_then(|committed| KeptFrame::reuse_image(&cache.spill(&source), committed))
        {
            return Ok(image);
        }
        let image = load(early, path, ctx)?;
        cache.keep(&source, &image, Admitted::measure(&image))?;
        Ok(image)
    }

    /// Frame `idx` through the decode cache, under `ctx`: its planes as a run before kept them for
    /// the source as it is now, mapped, or its decode, for the caller to admit and keep.
    ///
    /// # Errors
    ///
    /// The decode, or the cache.
    pub(crate) fn take(&self, idx: usize, ctx: &LoadCtx) -> Result<Taken<D::Image>, LoadError> {
        let cache = self
            .storage
            .kept()
            .expect("a frame is taken through a decode cache");
        let early = self.take_early(idx);
        let Resolved { source, committed } = self.resolve(cache, idx, early.is_some(), ctx)?;
        if let Some(kept) =
            committed.and_then(|committed| KeptFrame::reuse(&cache.spill(&source), committed))
        {
            return Ok(Taken::Kept(Box::new(kept)));
        }
        Ok(Taken::Decoded {
            image: load(early, self.path(idx), ctx)?,
            source,
        })
    }

    /// What the plan's read left of frame `idx`, taken.
    fn take_early(&self, idx: usize) -> Option<Early<D>> {
        let mut first = lock(&self.first);
        if first.as_ref().is_some_and(|first| first.idx == idx) {
            first.take().map(|first| first.early)
        } else {
            None
        }
    }

    /// Frame `idx`'s source under `ctx` and its commit, from its probe while the source reads as
    /// it did. A frame `read` for the plan is decoded from what was read after the probe, so its
    /// probe stands whatever the source did since: a keep refuses it if it changed. One whose
    /// probe found no source changed before it was read, which is refused here. Otherwise a source
    /// that changed since its probe is read again, and its commit with it.
    fn resolve(
        &self,
        cache: &DecodeCache,
        idx: usize,
        read: bool,
        ctx: &LoadCtx,
    ) -> Result<Resolved, LoadError> {
        let probes = self
            .probes
            .as_ref()
            .expect("a run with a decode cache probes each path");
        match lock(&probes[idx]).take() {
            Some(Probe { source, committed }) if read || source.is_current() => {
                return Ok(Resolved { source, committed });
            }
            None if read => {
                return Err(FrameStoreError::SourceChanged {
                    path: self.path(idx).to_path_buf(),
                }
                .into());
            }
            _ => {}
        }
        let source = CachedSource::of(self.path(idx), D::KIND, ctx)?;
        let committed = cache.committed(&source);
        Ok(Resolved { source, committed })
    }
}

/// `path`'s source decoded by `D` under `ctx`, and what `cache` committed for it; `None` when the
/// source does not read.
fn probe<D: Decoder>(cache: &DecodeCache, path: &Path, ctx: &LoadCtx) -> Option<Probe> {
    let source = CachedSource::of(path, D::KIND, ctx).ok()?;
    let committed = cache.committed(&source);
    Some(Probe { source, committed })
}

/// What a plan reads off a frame the cache holds: its shape and pattern as committed, and no
/// decoder, as none runs.
fn committed_peek(probe: &Mutex<Option<Probe>>) -> SetPeek {
    let probe = lock(probe);
    let committed = probe
        .as_ref()
        .and_then(|probe| probe.committed.as_ref())
        .expect("a frame the cache holds was probed");
    SetPeek {
        idx: 0,
        info: FrameInfo {
            dimensions: committed.decoded.dimensions,
            cfa_type: committed.admitted.facts.cfa_type.unwrap_or(CfaType::Mono),
            decoder: DecoderBytes::default(),
        },
        held: 0,
    }
}

/// The frame of `path` under `ctx`, from what the plan's read left of it when it read it.
fn load<D: Decoder>(
    early: Option<Early<D>>,
    path: &Path,
    ctx: &LoadCtx,
) -> Result<D::Image, LoadError> {
    match early {
        Some(early) => early.into_image(path, ctx),
        None => Ok(D::load(path, ctx)?),
    }
}

/// `mutex` locked, poisoned or not: every critical section on it is a take or a store of plain
/// fields, so no panic can leave it half-updated.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use common::{TempDir, fs};

    use crate::frame_store::file_set::{FileSet, SetPeek};
    use crate::frame_store::run_storage::RunStorage;
    use crate::image::cfa::cfa_type::CfaType;
    use crate::image::channel_layout::ChannelLayout;
    use crate::image::image_dimensions::ImageDimensions;
    use crate::image::linear_image::LinearImage;
    use crate::ingest::ingest_config::IngestConfig;
    use crate::ingest::ingest_run::IngestRun;
    use crate::io::decoder::linear_decoder::LinearDecoder;
    use crate::io::frame_info::{DecoderBytes, FrameInfo};
    use crate::io::load_ctx::LoadCtx;
    use crate::io::writer;
    use crate::memory::run_memory::RunMemory;

    /// A float TIFF of `width` × 2 RGB at `level` in `dir`, which no header read answers for.
    fn tiff(dir: &Path, name: &str, width: usize, level: f32) -> PathBuf {
        let image = LinearImage::from_planar_channels(
            ImageDimensions::new((width, 2), ChannelLayout::Rgb),
            [level; 3].map(|level| vec![level; width * 2]),
        );
        let path = dir.join(name);
        writer::raster(&image, &path).unwrap();
        path
    }

    /// The storage of a run over `cache_dir`, which keeps a decode cache when `keep_cache`.
    fn storage(cache_dir: &Path, keep_cache: bool) -> RunStorage {
        let ingest = IngestConfig {
            keep_cache,
            ..IngestConfig::with_cache_dir(cache_dir.to_path_buf())
        };
        RunStorage::decoding(&IngestRun::planned(RunMemory::new(1 << 30, None), &ingest)).unwrap()
    }

    /// The level of channel 0's first sample.
    fn level(image: &LinearImage) -> f32 {
        image.channel(0).pixels()[0]
    }

    /// A format with no header to read is decoded for the plan, and that decode is the one its
    /// frame takes: the frame decodes after its file is gone. The set holds it until then, 3 × 2
    /// pixels of three 4-byte samples and a flag byte, 78 B.
    #[test]
    fn the_decode_for_the_plan_is_the_one_its_frame_takes() {
        let dir = TempDir::new("file_set_early");
        let paths = [
            tiff(dir.path(), "a.tiff", 3, 0.25),
            tiff(dir.path(), "b.tiff", 3, 0.5),
        ];
        let ctx = LoadCtx::default();
        let files = FileSet::<_, LinearDecoder>::new(&paths, storage(dir.path(), false), &ctx);
        assert_eq!(files.uncached(), 0);
        let peek = files.peek(&ctx).unwrap();
        assert_eq!(
            peek,
            SetPeek {
                idx: 0,
                info: FrameInfo {
                    dimensions: ImageDimensions::new((3, 2), ChannelLayout::Rgb),
                    cfa_type: CfaType::Mono,
                    decoder: DecoderBytes::default(),
                },
                held: 78,
            }
        );
        fs::remove_file(&paths[0]).unwrap();
        assert_eq!(level(&files.decode(0, &ctx).unwrap()), 0.25);
        assert_eq!(level(&files.decode(1, &ctx).unwrap()), 0.5);
        assert!(
            files.decode(0, &ctx).is_err(),
            "taken once, then read again"
        );
    }

    /// The decode cache is read once for each frame, before any decode. A set none of whose
    /// frames it holds peeks frame 0, and keeps each frame as it decodes it. The next set over the
    /// same files finds all three held: it reads its shape off frame 0's commit, with no decoder,
    /// and holds nothing. Once frame 1 is rewritten, as 4 × 2, it alone is uncached, and the peek
    /// decodes it. A frame rewritten after the set probed it, frame 2, is probed again at its
    /// decode, and decodes as it now is.
    #[test]
    fn the_cache_is_probed_once_and_the_peek_reads_the_first_frame_it_lacks() {
        let dir = TempDir::new("file_set_cache");
        let paths = [0.25, 0.5, 0.75]
            .iter()
            .enumerate()
            .map(|(idx, &level)| tiff(dir.path(), &format!("{idx}.tiff"), 3, level))
            .collect::<Vec<_>>();
        let ctx = LoadCtx::default();
        let set = || FileSet::<_, LinearDecoder>::new(&paths, storage(dir.path(), true), &ctx);

        let first = set();
        assert_eq!(first.uncached(), 3);
        assert_eq!(first.peek(&ctx).unwrap().idx, 0);
        for idx in 0..3 {
            first.decode(idx, &ctx).unwrap();
        }

        let held = set();
        assert_eq!(held.uncached(), 0);
        assert_eq!(
            held.peek(&ctx).unwrap(),
            SetPeek {
                idx: 0,
                info: FrameInfo {
                    dimensions: ImageDimensions::new((3, 2), ChannelLayout::Rgb),
                    cfa_type: CfaType::Mono,
                    decoder: DecoderBytes::default(),
                },
                held: 0,
            }
        );

        tiff(dir.path(), "1.tiff", 4, 0.125);
        let changed = set();
        assert_eq!(changed.uncached(), 1);
        let peek = changed.peek(&ctx).unwrap();
        assert_eq!((peek.idx, peek.held), (1, 4 * 2 * 13));
        assert_eq!(
            peek.info.dimensions,
            ImageDimensions::new((4, 2), ChannelLayout::Rgb)
        );
        tiff(dir.path(), "2.tiff", 3, 0.0625);
        for (idx, expected) in [(0, 0.25), (1, 0.125), (2, 0.0625)] {
            assert_eq!(
                level(&changed.decode(idx, &ctx).unwrap()),
                expected,
                "{idx}"
            );
        }
        assert_eq!(set().uncached(), 0, "each decode kept its frame");
    }
}
