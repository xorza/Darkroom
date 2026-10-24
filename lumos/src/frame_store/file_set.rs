//! [`FileSet`]: a frame set's files as a run takes them, each read once.

use std::mem;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::frame::admitted::Admitted;
use crate::frame_store::decode_cache::DecodeCache;
use crate::frame_store::decode_cache::cached_source::CachedSource;
use crate::frame_store::decode_cache::frame_spill::{Committed, KeptGain};
use crate::frame_store::decode_cache::kept_frame::{KeptFrame, KeptImage};
use crate::frame_store::error::{FrameStoreError, LoadError};
use crate::frame_store::run_storage::RunStorage;
use crate::frame_store::stored_image::StoredImage;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::flat_gain::shared_gains::SharedGains;
use crate::image::stackable_image::StackableImage;
use crate::ingest::ingest_run::IngestRun;
use crate::io::decoder::{Decoder, Peek};
use crate::io::frame_info::{DecoderBytes, FrameInfo};
use crate::io::load_ctx::LoadCtx;

/// A frame set's files as a run takes them through its storage: each one's state in the decode
/// cache, read once before any frame is decoded, and the frame the plan is read off, which its
/// decode takes rather than reads again. So a run reads each file and each commit once.
#[derive(Debug)]
pub(crate) struct FileSet<'a, P, D: Decoder> {
    paths: &'a [P],
    /// The directory the run writes under: its decode cache and its scratch.
    cache_dir: PathBuf,
    /// The decode cache and what it holds of the set, for a run that keeps one.
    kept: Option<KeptSet>,
    /// The frame the plan was read off, and what reading it left, until its decode takes it.
    first: Mutex<Option<Box<First<D>>>>,
    /// The source gains the set's frames share, each held once.
    gains: SharedGains,
}

/// A path's source as the decode cache keys it, and the frame committed for it then.
#[derive(Debug)]
struct Probe {
    source: CachedSource,
    committed: Option<Committed>,
}

/// A run's decode cache, and what it holds of a frame set's paths.
#[derive(Debug)]
struct KeptSet {
    cache: DecodeCache,
    /// What each path's probe found, until its decode takes it.
    probes: Vec<Mutex<Probed>>,
    /// The frames the cache holds none of, by index, as the probes found them: the frames the run
    /// writes to it.
    uncached: Vec<usize>,
}

/// What a path's probe found, until its decode takes it.
#[derive(Debug)]
enum Probed {
    Found(Box<Probe>),
    /// The source did not read: its decode reads it again, and reports why.
    Unread,
    /// The path's decode took what its probe found.
    Taken,
}

impl Probed {
    /// The frame the cache committed for the path when it was probed.
    const fn committed(&self) -> Option<&Committed> {
        match self {
            Self::Found(probe) => probe.committed.as_ref(),
            Self::Unread | Self::Taken => None,
        }
    }
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
    /// What its header read read: a camera RAW's file, or a FITS file's reader.
    Read(D::Read),
    /// The frame decoded, for a format with no header that answers.
    Decoded(D::Image),
}

impl<D: Decoder> Early<D> {
    /// The frame of `path`, decoded under `ctx` from what was read of it, its flat gain shared
    /// through `gains`.
    fn into_image(
        self,
        path: &Path,
        ctx: &LoadCtx,
        gains: &SharedGains,
    ) -> Result<D::Image, LoadError> {
        Ok(match self {
            Self::Read(read) => D::load_read(read, path, ctx, gains)?,
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

/// What a plan reads off a frame set: the frame that stands for every frame, at its index, the
/// bytes the set holds for that frame's decode until the decode takes them, and the bytes of the
/// source gains its frames share, which it holds from their decode to the end of the run: each
/// distinct gain the decode cache holds, and one for each frame it does not, as many as there can
/// be before those frames decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SetPeek {
    pub(crate) idx: usize,
    pub(crate) info: FrameInfo,
    pub(crate) held: usize,
    pub(crate) gains: usize,
}

/// A frame of a set in RAM, and its planes where the decode cache keeps them, for a run that keeps
/// one.
#[derive(Debug)]
pub(crate) struct Loaded<I> {
    pub(crate) image: I,
    pub(crate) kept: Option<StoredImage>,
}

/// A frame of a set taken through a decode cache: kept by a run before, or decoded, with its
/// source as it was read before the decode, for the cache to keep.
#[derive(Debug)]
pub(crate) enum Taken<I> {
    Kept(Box<KeptFrame>),
    Decoded { image: I, source: CachedSource },
}

impl<'a, P: AsRef<Path>, D: Decoder> FileSet<'a, P, D> {
    /// The frames of `paths`, decoded by `D` as `run` decodes: through its decode cache, opened
    /// and each path's state in it read, when it keeps one.
    ///
    /// # Errors
    ///
    /// The decode cache cannot be opened.
    pub(crate) fn new(paths: &'a [P], run: &IngestRun) -> Result<Self, FrameStoreError> {
        let kept = run
            .keep_cache
            .then(|| -> Result<KeptSet, FrameStoreError> {
                let cache = DecodeCache::open(&run.cache_dir)?;
                let probes: Vec<Probed> = paths
                    .iter()
                    .map(|path| probe::<D>(&cache, path.as_ref(), &run.ctx))
                    .collect();
                let uncached = probes
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, probed)| probed.committed().is_none().then_some(idx))
                    .collect();
                Ok(KeptSet {
                    cache,
                    probes: probes.into_iter().map(Mutex::new).collect(),
                    uncached,
                })
            })
            .transpose()?;
        Ok(Self {
            paths,
            cache_dir: run.cache_dir.clone(),
            kept,
            first: Mutex::new(None),
            gains: SharedGains::default(),
        })
    }

    pub(crate) const fn len(&self) -> usize {
        self.paths.len()
    }

    pub(crate) fn path(&self, idx: usize) -> &Path {
        self.paths[idx].as_ref()
    }

    /// Where the frames are read from and written to.
    pub(crate) fn storage(&self) -> RunStorage<'_> {
        RunStorage::new(&self.cache_dir, self.kept.as_ref().map(|kept| &kept.cache))
    }

    /// How many frames the run writes to the decode cache: those it holds no frame of. None
    /// without a cache.
    pub(crate) fn uncached(&self) -> usize {
        self.kept.as_ref().map_or(0, |kept| kept.uncached.len())
    }

    /// What the plan reads off the set under `ctx`: the first frame the decode cache does not
    /// hold, by its header or by its decode, which that frame's decode then takes; or frame 0 as
    /// its commit records it when the cache holds every frame, so no decoder runs.
    ///
    /// # Errors
    ///
    /// The frame's decode, for a format with no header that answers.
    pub(crate) fn peek(&self, ctx: &LoadCtx) -> Result<SetPeek, LoadError> {
        let idx = match &self.kept {
            Some(kept) => {
                if let Some(&idx) = kept.uncached.first() {
                    idx
                } else {
                    let info = committed_info(&kept.probes[0]);
                    return Ok(SetPeek {
                        idx: 0,
                        info,
                        held: 0,
                        gains: self.gain_bytes(info.gain_bytes),
                    });
                }
            }
            None => 0,
        };
        let path = self.path(idx);
        let (info, early) = if let Some(Peek { info, read }) = D::peek(path, ctx) {
            (info, Early::Read(read))
        } else {
            let image = D::load(path, ctx, &self.gains)?;
            (FrameInfo::of_decoded(&image), Early::Decoded(image))
        };
        let held = early.held_bytes();
        let mut first = lock(&self.first);
        debug_assert!(first.is_none(), "a plan peeks its set once");
        *first = Some(Box::new(First { idx, early }));
        Ok(SetPeek {
            idx,
            info,
            held,
            gains: self.gain_bytes(info.gain_bytes),
        })
    }

    /// The bytes of the source gains the frames share, at the most: each distinct gain of the
    /// frames the decode cache holds, as their commits state it, and `uncached` for each of the
    /// others, whose gains are known only once they decode.
    fn gain_bytes(&self, uncached: usize) -> usize {
        let Some(kept) = &self.kept else {
            return self.len().saturating_mul(uncached);
        };
        let mut gains: Vec<KeptGain> = Vec::new();
        let mut bytes = 0;
        for probed in &kept.probes {
            let probed = lock(probed);
            let Some(Committed { decoded, .. }) = probed.committed() else {
                continue;
            };
            if let Some(gain) = decoded.gain
                && gains.iter().all(|held| held.key != gain.key)
            {
                gains.push(gain);
                bytes += gain.bytes(decoded.dimensions.size());
            }
        }
        kept.uncached
            .len()
            .saturating_mul(uncached)
            .saturating_add(bytes)
    }

    /// Frame `idx` in RAM, under `ctx`, with its planes where the decode cache keeps them: read
    /// back from the cache when it holds the frame for the source as it is now, decoded and kept
    /// there otherwise; decoded, and kept nowhere, without a cache.
    ///
    /// # Errors
    ///
    /// The decode, or the cache.
    pub(crate) fn decode(&self, idx: usize, ctx: &LoadCtx) -> Result<Loaded<D::Image>, LoadError> {
        let early = self.take_early(idx);
        let Some(kept) = &self.kept else {
            return Ok(Loaded {
                image: self.load(early, idx, ctx)?,
                kept: None,
            });
        };
        let cache = &kept.cache;
        let Probe { source, committed } = self.resolve(kept, idx, early.as_ref(), ctx)?;
        if let Some(KeptImage { image, stored }) = committed.and_then(|committed| {
            KeptFrame::reuse_image(&cache.spill(&source), committed, &self.gains)
        }) {
            return Ok(Loaded {
                image,
                kept: Some(stored),
            });
        }
        let image = self.load(early, idx, ctx)?;
        let kept = cache.keep(&source, &image, Admitted::measure(&image))?;
        Ok(Loaded {
            image,
            kept: Some(kept.image),
        })
    }

    /// Frame `idx` through the decode cache, under `ctx`: its planes as a run before kept them for
    /// the source as it is now, mapped, or its decode, for the caller to admit and keep.
    ///
    /// # Errors
    ///
    /// The decode, or the cache.
    pub(crate) fn take(&self, idx: usize, ctx: &LoadCtx) -> Result<Taken<D::Image>, LoadError> {
        let kept = self
            .kept
            .as_ref()
            .expect("a frame is taken through a decode cache");
        let cache = &kept.cache;
        let early = self.take_early(idx);
        let Probe { source, committed } = self.resolve(kept, idx, early.as_ref(), ctx)?;
        if let Some(kept) = committed
            .and_then(|committed| KeptFrame::reuse(&cache.spill(&source), committed, &self.gains))
        {
            return Ok(Taken::Kept(Box::new(kept)));
        }
        Ok(Taken::Decoded {
            image: self.load(early, idx, ctx)?,
            source,
        })
    }

    /// Frame `idx` under `ctx`, from what the plan's read left of it when it read it, its flat gain
    /// shared with the set's.
    fn load(
        &self,
        early: Option<Early<D>>,
        idx: usize,
        ctx: &LoadCtx,
    ) -> Result<D::Image, LoadError> {
        let path = self.path(idx);
        match early {
            Some(early) => early.into_image(path, ctx, &self.gains),
            None => Ok(D::load(path, ctx, &self.gains)?),
        }
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
    /// it did. A frame the plan read, `early`, is decoded from what was read after the probe, so
    /// its probe stands whatever the source did since: a keep refuses it if it changed. One whose
    /// probe found no source changed before it was read, which is refused here. Otherwise a source
    /// that changed since its probe is read again, and its commit with it.
    fn resolve(
        &self,
        kept: &KeptSet,
        idx: usize,
        early: Option<&Early<D>>,
        ctx: &LoadCtx,
    ) -> Result<Probe, LoadError> {
        let read = early.is_some();
        match mem::replace(&mut *lock(&kept.probes[idx]), Probed::Taken) {
            Probed::Found(probe) if read || probe.source.is_current() => return Ok(*probe),
            Probed::Unread if read => {
                return Err(FrameStoreError::SourceChanged {
                    path: self.path(idx).to_path_buf(),
                }
                .into());
            }
            Probed::Found(_) | Probed::Unread | Probed::Taken => {}
        }
        let source = CachedSource::of(self.path(idx), D::KIND, ctx)?;
        let committed = kept.cache.committed(&source);
        Ok(Probe { source, committed })
    }
}

/// `path`'s source decoded by `D` under `ctx`, and what `cache` committed for it.
fn probe<D: Decoder>(cache: &DecodeCache, path: &Path, ctx: &LoadCtx) -> Probed {
    match CachedSource::of(path, D::KIND, ctx) {
        Ok(source) => Probed::Found(Box::new(Probe {
            committed: cache.committed(&source),
            source,
        })),
        Err(_) => Probed::Unread,
    }
}

/// What a plan reads off a frame the cache holds: its shape, pattern and gain as committed, and no
/// decoder, as none runs.
fn committed_info(probed: &Mutex<Probed>) -> FrameInfo {
    let probed = lock(probed);
    let committed = probed
        .committed()
        .expect("a frame the cache holds was probed");
    let dimensions = committed.decoded.dimensions;
    FrameInfo {
        dimensions,
        cfa_type: committed.admitted.facts.cfa_type.unwrap_or(CfaType::Mono),
        decoder: DecoderBytes::default(),
        gain_bytes: committed
            .decoded
            .gain
            .map_or(0, |gain| gain.bytes(dimensions.size())),
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
    use std::sync::Arc;

    use common::{TempDir, fs};
    use imaginarium::Buffer2;

    use crate::frame_store::file_set::{FileSet, Loaded, SetPeek};
    use crate::image::cfa::cfa_pattern::CfaPattern;
    use crate::image::cfa::cfa_type::CfaType;
    use crate::image::cfa::mosaic::Mosaic;
    use crate::image::channel_layout::ChannelLayout;
    use crate::image::flat_gain::FlatGain;
    use crate::image::flat_gain::frame_gain::FrameGain;
    use crate::image::image_dimensions::ImageDimensions;
    use crate::image::linear_image::LinearImage;
    use crate::ingest::ingest_config::IngestConfig;
    use crate::ingest::ingest_run::IngestRun;
    use crate::io::decoder::linear_decoder::LinearDecoder;
    use crate::io::frame_info::{DecoderBytes, FrameInfo};
    use crate::io::writer;

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

    /// A run over `cache_dir`, which keeps a decode cache when `keep_cache`.
    fn run(cache_dir: &Path, keep_cache: bool) -> IngestRun {
        let ingest = IngestConfig {
            keep_cache,
            ..IngestConfig::with_cache_dir(cache_dir.to_path_buf())
        };
        IngestRun::planned(1 << 30, &ingest)
    }

    /// The level of channel 0's first sample.
    fn level(loaded: &Loaded<LinearImage>) -> f32 {
        loaded.image.channel(0).pixels()[0]
    }

    /// The level of channel 0's first sample in the planes the decode cache keeps, which hold
    /// what the image holds; `None` when it keeps none.
    fn kept_level(loaded: &Loaded<LinearImage>) -> Option<f32> {
        let kept = loaded.kept.as_ref()?;
        assert!(
            kept.planes()
                .enumerate()
                .all(|(channel, kept)| kept == loaded.image.channel(channel).pixels())
        );
        kept.planes().next().map(|plane| plane[0])
    }

    /// A format with no header to read is decoded for the plan, and that decode is the one its
    /// frame takes: the frame decodes after its file is gone. The set holds it until then, 3 × 2
    /// pixels of three 4-byte samples and a flag byte, 78 B. Without a decode cache, no planes
    /// are kept.
    #[test]
    fn the_decode_for_the_plan_is_the_one_its_frame_takes() {
        let dir = TempDir::new("file_set_early");
        let paths = [
            tiff(dir.path(), "a.tiff", 3, 0.25),
            tiff(dir.path(), "b.tiff", 3, 0.5),
        ];
        let ctx = run(dir.path(), false).ctx;
        let files = FileSet::<_, LinearDecoder>::new(&paths, &run(dir.path(), false)).unwrap();
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
                    gain_bytes: 0,
                },
                held: 78,
                gains: 0,
            }
        );
        fs::remove_file(&paths[0]).unwrap();
        assert_eq!(level(&files.decode(0, &ctx).unwrap()), 0.25);
        let second = files.decode(1, &ctx).unwrap();
        assert_eq!((level(&second), kept_level(&second)), (0.5, None));
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
    /// decode, and decodes as it now is. Each decode comes with the planes the cache keeps of it,
    /// whether it kept them then or a set before did.
    #[test]
    fn the_cache_is_probed_once_and_the_peek_reads_the_first_frame_it_lacks() {
        let dir = TempDir::new("file_set_cache");
        let paths = [0.25, 0.5, 0.75]
            .iter()
            .enumerate()
            .map(|(idx, &level)| tiff(dir.path(), &format!("{idx}.tiff"), 3, level))
            .collect::<Vec<_>>();
        let ctx = run(dir.path(), false).ctx;
        let set = || FileSet::<_, LinearDecoder>::new(&paths, &run(dir.path(), true)).unwrap();

        let first = set();
        assert_eq!(first.uncached(), 3);
        assert_eq!(first.peek(&ctx).unwrap().idx, 0);
        for (idx, expected) in [0.25, 0.5, 0.75].into_iter().enumerate() {
            let loaded = first.decode(idx, &ctx).unwrap();
            assert_eq!(kept_level(&loaded), Some(expected), "{idx}");
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
                    gain_bytes: 0,
                },
                held: 0,
                gains: 0,
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
            let loaded = changed.decode(idx, &ctx).unwrap();
            assert_eq!(
                (level(&loaded), kept_level(&loaded)),
                (expected, Some(expected)),
                "{idx}"
            );
        }
        assert_eq!(set().uncached(), 0, "each decode kept its frame");
    }

    /// Lights of one flat share its gain, which the set holds once. Three 3 × 2 RGB FITS lights
    /// of one gain, 2 × 2 nodes for each of three channels, 48 B: before any decodes, the set
    /// charges a gain to each frame it does not hold in a cache, 144 B, and its frames decode to one
    /// gain. Once the cache holds all three, their commits name one gain, 48 B, which their reuse
    /// shares as well.
    #[test]
    fn the_frames_of_one_gain_share_it_and_the_set_charges_each_once() {
        let dir = TempDir::new("file_set_gain");
        let gain = Arc::new(FlatGain::of_divisor(
            &Buffer2::from_fn(3, 2, |x, y| 0.5 + 0.1 * (x + y) as f32),
            &CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb)),
            |_| false,
        ));
        let paths = [0.25f32, 0.5, 0.75].map(|level| {
            let mut image = LinearImage::from_planar_channels(
                ImageDimensions::new((3, 2), ChannelLayout::Rgb),
                [level; 3].map(|level| vec![level; 6]),
            );
            image.set_flat_gain(Some(FrameGain::Shared(Arc::clone(&gain))));
            let path = dir.join(format!("{level}.fits"));
            writer::linear_fits(&image, &path).unwrap();
            path
        });
        let ctx = run(dir.path(), false).ctx;
        let shared = |files: &FileSet<'_, PathBuf, LinearDecoder>| {
            let gains: Vec<Arc<FlatGain>> = (0..3)
                .map(
                    |idx| match files.decode(idx, &ctx).unwrap().image.flat_gain() {
                        Some(FrameGain::Shared(gain)) => Arc::clone(gain),
                        other => panic!("a decoded gain is shared, not {other:?}"),
                    },
                )
                .collect();
            assert!(gains.iter().all(|held| Arc::ptr_eq(held, &gains[0])));
            assert!(gains[0].planes().eq(gain.planes()));
        };
        for keep_cache in [false, true] {
            let files =
                FileSet::<_, LinearDecoder>::new(&paths, &run(dir.path(), keep_cache)).unwrap();
            let peek = files.peek(&ctx).unwrap();
            assert_eq!(
                (peek.info.gain_bytes, peek.gains),
                (48, 144),
                "{keep_cache}"
            );
            shared(&files);
        }
        let kept = FileSet::<_, LinearDecoder>::new(&paths, &run(dir.path(), true)).unwrap();
        assert_eq!(kept.uncached(), 0);
        let peek = kept.peek(&ctx).unwrap();
        assert_eq!((peek.info.gain_bytes, peek.gains), (48, 48));
        shared(&kept);
    }
}
