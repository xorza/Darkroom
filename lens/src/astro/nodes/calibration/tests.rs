use std::path::{Path, PathBuf};
use std::sync::Mutex;

use common::{Buffer2, CancelToken, MutexExt, TempDir, fs};
use lumos::{
    CalibrationSet, CfaImage, CfaType, ImageMetadata, MasterBuildError, MasterCache, MasterKey,
    MasterRole, MasterSlot, SigmaThreshold,
};

use crate::astro::nodes::calibration::{
    CacheError, Caching, MastersRequest, RoleCachePaths, SourceDirCache,
};

#[test]
fn role_cache_requires_one_source_directory() {
    let frames = [
        Path::new("calibration/darks/a.raf"),
        Path::new("calibration/darks/b.raf"),
    ];
    let paths = RoleCachePaths::beside(&frames, "master_dark.fits").unwrap();
    assert_eq!(
        paths.master,
        PathBuf::from("calibration/darks/master_dark.fits")
    );
    assert_eq!(
        paths.marker,
        PathBuf::from("calibration/darks/master_dark.fits.source")
    );

    let mixed = [
        Path::new("calibration/darks/a.raf"),
        Path::new("archive/darks/b.raf"),
    ];
    let error = RoleCachePaths::beside(&mixed, "master_dark.fits").unwrap_err();
    assert!(matches!(
        error,
        CacheError::SourceDirs {
            first,
            other
        } if first == mixed[0] && other == mixed[1]
    ));
}

/// A master is read back under the key it was stored with and no other: the marker beside it
/// holds the key, and a slot of another key — other frames, another recipe — finds none. A store
/// that fails leaves no marker, so no master is read under a key it was not written for; the cache
/// reports it and the build keeps its master.
#[test]
fn a_master_is_read_back_under_its_key_alone() {
    let dir = TempDir::new("lens-master-cache");
    let frame = dir.join("a.raf");
    fs::write(&frame, b"a").unwrap();
    let frames = [frame.as_path()];
    let slot = |key| MasterSlot {
        role: MasterRole::Dark,
        frames: &frames,
        key,
    };
    let other_frame = dir.join("b.raf");
    fs::write(&other_frame, b"b").unwrap();
    let key = lumos_key(&frames);
    let other = lumos_key(&[other_frame.as_path()]);
    let cache = SourceDirCache {
        cancel: &CancelToken::never(),
    };
    let master = CfaImage::from_plane(
        Buffer2::new_filled((4, 4), 0.1),
        CfaType::Mono,
        ImageMetadata::default(),
    );

    assert!(cache.load(&slot(key)).is_none());
    cache.store(&slot(key), &master);
    assert_eq!(
        fs::read_to_string(dir.join("master_dark.fits.source")).unwrap(),
        key.to_string()
    );
    assert_eq!(
        cache.load(&slot(key)).map(|kept| kept.data().clone()),
        Some(master.data().clone())
    );
    assert!(cache.load(&slot(other)).is_none());

    // The master's path is a directory, which the write cannot replace.
    let master_path = dir.join("master_dark.fits");
    fs::remove_file(&master_path).unwrap();
    fs::create_dir(&master_path).unwrap();
    assert!(matches!(
        SourceDirCache::try_store(&slot(key), &master),
        Err(CacheError::WriteMaster { .. })
    ));
    cache.store(&slot(key), &master);
    assert!(!fs::exists(dir.join("master_dark.fits.source")));
    fs::remove_dir(&master_path).unwrap();
    assert!(cache.load(&slot(key)).is_none());
}

/// A key as lumos composes one for a dark of `frames`, by building its master through a cache that
/// records it.
fn lumos_key(frames: &[&Path]) -> MasterKey {
    #[derive(Debug, Default)]
    struct Recorder(Mutex<Option<MasterKey>>);
    impl MasterCache for Recorder {
        fn load(&self, slot: &MasterSlot<'_>) -> Option<CfaImage> {
            *self.0.lock_unpoisoned() = Some(slot.key);
            None
        }

        fn store(&self, _: &MasterSlot<'_>, _: &CfaImage) {}
    }
    let recorder = Recorder::default();
    // The frames are not images: the stack fails after the cache was read.
    lumos::CalibrationMasters::build(
        CalibrationSet {
            dark: frames,
            ..CalibrationSet::default()
        },
        &lumos::MastersConfig {
            hot_sigma: SigmaThreshold::DEFAULT,
            cache: Some(&recorder),
        },
        &lumos::IngestConfig::default(),
        lumos::ProgressMonitor::unattended(),
    )
    .unwrap_err();
    recorder.0.lock_unpoisoned().expect("the cache was read")
}

/// A missing frame is reported by its stack, cache on or off: lumos keys no master it cannot read
/// the frames of, and the cache is not asked.
#[test]
fn a_missing_frame_fails_the_stack_with_the_cache_on_or_off() {
    let dir = TempDir::new("lens-master-key-cache-off");
    let missing = dir.join("missing.raf");
    let build = |caching| {
        MastersRequest {
            frames: CalibrationSet {
                dark: vec![missing.clone()],
                ..CalibrationSet::default()
            },
            sigma: SigmaThreshold::new(3.0).unwrap(),
            caching,
        }
        .build(&CancelToken::never())
        .unwrap_err()
    };
    for caching in [Caching::BesideFrames, Caching::Off] {
        assert!(matches!(build(caching), MasterBuildError::Stack(_)));
    }
}
