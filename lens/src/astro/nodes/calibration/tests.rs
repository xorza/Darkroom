use std::fs;
use std::io;
use std::path::PathBuf;
use std::slice;

use common::{CancelToken, TempDir};
use imaginarium::Buffer2;
use lumos::{
    CachedMaster, CalibrationSet, CfaImage, CfaType, FitsChecksumPolicy, ImageMetadata,
    IngestConfig, MasterBuildError, MasterCache, MasterInputs, MasterRecipe, MasterRole,
};

use crate::astro::nodes::calibration::{
    CacheError, Caching, FrameSetKeyError, MastersRequest, RoleCachePaths, SourceDirectoryCache,
    frame_set_key,
};

#[test]
fn role_cache_requires_one_source_directory() {
    let frames = [
        PathBuf::from("calibration/darks/a.raf"),
        PathBuf::from("calibration/darks/b.raf"),
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
        PathBuf::from("calibration/darks/a.raf"),
        PathBuf::from("archive/darks/b.raf"),
    ];
    let error = RoleCachePaths::beside(&mixed, "master_dark.fits").unwrap_err();
    assert!(matches!(
        error,
        CacheError::SourceDirectories {
            first,
            other
        } if first == mixed[0] && other == mixed[1]
    ));
}

#[test]
fn master_source_key_changes_with_the_frame_set() {
    let dir = TempDir::new("lens-master-source-key");
    let first = dir.join("a.raf");
    let second = dir.join("b.raf");
    fs::write(&first, b"a").unwrap();
    let one_frame = frame_set_key(slice::from_ref(&first)).unwrap();
    assert_eq!(frame_set_key(slice::from_ref(&first)).unwrap(), one_frame);

    fs::write(&second, b"bb").unwrap();
    let two_frames = frame_set_key(&[first.clone(), second.clone()]).unwrap();
    assert_ne!(two_frames, one_frame);
    // The key follows the selection order: a reordered set sums in another
    // order, so its master can differ in the last bits.
    let reordered = frame_set_key(&[second.clone(), first.clone()]).unwrap();
    assert_ne!(reordered, two_frames);
    fs::write(&first, b"aaa").unwrap();
    let edited = frame_set_key(&[first.clone(), second]).unwrap();
    assert_ne!(edited, two_frames);
    assert_ne!(frame_set_key(&[]).unwrap(), edited);

    fs::remove_file(&first).unwrap();
    let error = frame_set_key(slice::from_ref(&first)).unwrap_err();
    assert!(matches!(
        error,
        FrameSetKeyError::Metadata { path, source }
            if path == first && source.kind() == io::ErrorKind::NotFound
    ));
}

/// A master is kept under its frames as the cache read them before the stack: a frame
/// rewritten while its role is stacked leaves the marker on the old content, so the next build
/// stacks the role again rather than reuse a master of frames that are gone. One stored under
/// frames that stayed is read back, under its own recipe and no other.
#[test]
fn a_master_is_kept_under_its_frames_as_read_before_the_stack() {
    let dir = TempDir::new("lens-master-key-before-stack");
    let frame = dir.join("a.raf");
    fs::write(&frame, b"a").unwrap();
    let frames = [frame.clone()];
    let inputs = MasterInputs {
        role: MasterRole::Dark,
        frames: &frames,
        subtracted: None,
        recipe: MasterRecipe::of(MasterRole::Dark, &IngestConfig::default()),
    };
    let cache = SourceDirectoryCache {
        caching: Caching::BesideFrames,
        cancel: &CancelToken::never(),
    };
    let master = CfaImage::from_plane(
        Buffer2::new_filled(4, 4, 0.1),
        CfaType::Mono,
        ImageMetadata::default(),
    );
    let missing = || match cache.load(&inputs).unwrap() {
        CachedMaster::Missing(Some(place)) => place,
        CachedMaster::Missing(None) => panic!("the cache is on"),
        CachedMaster::Kept(_) => panic!("no master is kept for the frames as they are"),
    };

    let place = missing();
    let read = place.key.clone();
    fs::write(&frame, b"bb").unwrap();
    cache.store(Some(place), &master).unwrap();
    assert_eq!(
        fs::read_to_string(dir.join("master_dark.fits.source")).unwrap(),
        read
    );
    let place = missing();
    assert_ne!(place.key, read);

    cache.store(Some(place), &master).unwrap();
    assert!(matches!(
        cache.load(&inputs).unwrap(),
        CachedMaster::Kept(kept) if kept.data == master.data
    ));

    // Another recipe over the same frames, as another lumos or other decode options give, finds
    // none.
    let mut strict = IngestConfig::default();
    strict.fits.checksum = FitsChecksumPolicy::RequireValid;
    let restacked = MasterInputs {
        recipe: MasterRecipe::of(MasterRole::Dark, &strict),
        ..inputs
    };
    assert!(matches!(
        cache.load(&restacked).unwrap(),
        CachedMaster::Missing(Some(_))
    ));
}

/// A missing frame fails the key's `stat` with the cache on, and with it off the key is never
/// computed: the stack is what reports the missing file.
#[test]
fn frame_set_key_runs_only_with_the_cache_on() {
    let dir = TempDir::new("lens-master-key-cache-off");
    let missing = dir.join("missing.raf");
    let build = |caching| {
        MastersRequest {
            frames: CalibrationSet {
                dark: vec![missing.clone()],
                ..CalibrationSet::default()
            },
            sigma: 3.0,
            caching,
        }
        .build(&CancelToken::never())
        .unwrap_err()
    };
    assert!(matches!(
        build(Caching::BesideFrames),
        MasterBuildError::Cache(CacheError::FrameSet(_))
    ));
    assert!(matches!(build(Caching::Off), MasterBuildError::Stack(_)));
}
