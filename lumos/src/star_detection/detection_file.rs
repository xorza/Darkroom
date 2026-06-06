//! Persist a [`DetectionResult`] as a JSON sidecar next to its image.
//!
//! The sidecar path is `{image_path}.detection`. Each file stores a [`Sidecar`]
//! envelope — a format version, the image's mtime, a fingerprint of the
//! [`Config`] that produced the result, and the result itself — so a cached
//! result is reused only when the image is unchanged (same mtime) and the
//! detection config still matches.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use common::FnvHasher;
use serde::{Deserialize, Serialize};

use crate::astro_image::error::ImageError;

use super::config::Config;
use super::detector::DetectionResult;

/// Bumped when the on-disk envelope or [`DetectionResult`] layout changes
/// incompatibly. A version mismatch is treated as a cache miss, never an error
/// in the cache path.
const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct Sidecar {
    format_version: u32,
    image_mtime: u64,
    config_fingerprint: u64,
    result: DetectionResult,
}

/// Sidecar path for an image: `{image_path}.detection`.
pub fn sidecar_path(image_path: &Path) -> PathBuf {
    let mut p = image_path.as_os_str().to_owned();
    p.push(".detection");
    PathBuf::from(p)
}

/// A file's mtime as whole seconds since the Unix epoch, or `None` if it can't
/// be read. Whole seconds match the freshness convention in the stacking cache.
fn file_mtime_secs(path: &Path) -> Option<u64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(
        modified
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
}

/// Fingerprint of a detection config, used to invalidate sidecars written under
/// a different config. Hashes the `Debug` form with the fixed-seed [`FnvHasher`]
/// (stable across runs and toolchains, unlike `DefaultHasher`); a `Debug`-format
/// change can only ever cause a cache miss (safe), never a false hit.
fn config_fingerprint(config: &Config) -> u64 {
    let mut hasher = FnvHasher::new();
    format!("{config:?}").hash(&mut hasher);
    hasher.finish()
}

/// Write `result` as a sidecar next to `image_path`, recording the image's mtime
/// and `config` so the cache can later verify the result still applies.
pub fn save_detection_result(
    image_path: &Path,
    result: &DetectionResult,
    config: &Config,
) -> Result<(), ImageError> {
    let path = sidecar_path(image_path);
    let sidecar = Sidecar {
        format_version: FORMAT_VERSION,
        image_mtime: file_mtime_secs(image_path).unwrap_or(0),
        config_fingerprint: config_fingerprint(config),
        result: result.clone(),
    };
    let text = serde_json::to_string_pretty(&sidecar).map_err(|e| ImageError::Sidecar {
        path: path.clone(),
        reason: e.to_string(),
    })?;
    std::fs::write(&path, text).map_err(|source| ImageError::Io { path, source })
}

/// Read and validate the sidecar envelope for `image_path`: existence, parse,
/// and format version. Freshness and config checks live in [`load_if_fresh`].
fn read_sidecar(image_path: &Path) -> Result<Sidecar, ImageError> {
    let path = sidecar_path(image_path);
    let text = std::fs::read_to_string(&path).map_err(|source| ImageError::Io {
        path: path.clone(),
        source,
    })?;
    let sidecar: Sidecar = serde_json::from_str(&text).map_err(|e| ImageError::Sidecar {
        path: path.clone(),
        reason: e.to_string(),
    })?;
    if sidecar.format_version != FORMAT_VERSION {
        return Err(ImageError::Sidecar {
            path,
            reason: format!(
                "unsupported format version {} (expected {FORMAT_VERSION})",
                sidecar.format_version
            ),
        });
    }
    Ok(sidecar)
}

/// Read the sidecar for `image_path`, without any freshness or config check.
pub fn load_detection_result(image_path: &Path) -> Result<DetectionResult, ImageError> {
    read_sidecar(image_path).map(|sidecar| sidecar.result)
}

/// Load the cached result only if it is safe to reuse: the sidecar exists and
/// parses, its stored image mtime equals the image's current mtime, and it was
/// written under a config matching `config`. Any error or mismatch yields `None`
/// (a cache miss).
pub(crate) fn load_if_fresh(image_path: &Path, config: &Config) -> Option<DetectionResult> {
    let current_mtime = file_mtime_secs(image_path)?;
    let sidecar = read_sidecar(image_path).ok()?;
    if sidecar.image_mtime != current_mtime {
        return None;
    }
    if sidecar.config_fingerprint != config_fingerprint(config) {
        return None;
    }
    Some(sidecar.result)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use glam::DVec2;

    use super::*;
    use crate::AstroImage;
    use crate::astro_image::ImageDimensions;
    use crate::star_detection::detector::{Diagnostics, StarDetector};
    use crate::star_detection::star::Star;
    use crate::testing::synthetic::star_field::{StarFieldConfig, generate_star_field};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lumos_sidecar_{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn star_at(x: f64, y: f64, flux: f32) -> Star {
        Star {
            pos: DVec2::new(x, y),
            flux,
            fwhm: 3.0,
            eccentricity: 0.1,
            snr: 25.0,
            peak: 0.5,
            sharpness: 0.4,
            roundness1: 0.0,
            roundness2: 0.0,
        }
    }

    fn result_with(stars: Vec<Star>) -> DetectionResult {
        DetectionResult {
            stars,
            diagnostics: Default::default(),
        }
    }

    /// Set a file's mtime to `base + secs` for deterministic freshness tests.
    fn bump_mtime(path: &Path, base: std::time::SystemTime, secs: u64) {
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(base + Duration::from_secs(secs))
            .unwrap();
    }

    #[test]
    fn test_sidecar_path_format() {
        let p = sidecar_path(Path::new("/data/frame_0001.tiff"));
        assert_eq!(p, PathBuf::from("/data/frame_0001.tiff.detection"));
    }

    #[test]
    fn round_trip_preserves_result() {
        let dir = temp_dir("round_trip");
        let img = dir.join("frame.fits");
        let diagnostics = Diagnostics {
            final_star_count: 2,
            median_fwhm: 3.25,
            ..Default::default()
        };
        let original = DetectionResult {
            stars: vec![star_at(10.5, 20.25, 100.0), star_at(30.0, 40.0, 250.5)],
            diagnostics,
        };
        let config = Config::default();

        save_detection_result(&img, &original, &config).unwrap();
        assert!(sidecar_path(&img).exists());
        let loaded = load_detection_result(&img).unwrap();

        // serde_json round-trips f32/f64 losslessly, so re-serializing both must match.
        assert_eq!(
            serde_json::to_string(&original.stars).unwrap(),
            serde_json::to_string(&loaded.stars).unwrap()
        );
        assert_eq!(loaded.stars.len(), 2);
        assert_eq!(loaded.stars[0].pos, DVec2::new(10.5, 20.25));
        assert_eq!(loaded.diagnostics.final_star_count, 2);
        assert_eq!(loaded.diagnostics.median_fwhm, 3.25);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_rejects_corrupt_and_bad_version() {
        let dir = temp_dir("corrupt");
        let img = dir.join("frame.fits");
        // Image exists throughout, so every miss below is due to the sidecar itself.
        std::fs::write(&img, b"image-bytes").unwrap();

        // Corrupt JSON → explicit error and a cache miss.
        std::fs::write(sidecar_path(&img), b"not json {").unwrap();
        assert!(matches!(
            load_detection_result(&img),
            Err(ImageError::Sidecar { .. })
        ));
        assert!(load_if_fresh(&img, &Config::default()).is_none());

        // Valid JSON, wrong format version → error and a cache miss.
        let bad = format!(
            "{{\"format_version\":999,\"image_mtime\":0,\"config_fingerprint\":0,\"result\":{}}}",
            serde_json::to_string(&result_with(vec![])).unwrap()
        );
        std::fs::write(sidecar_path(&img), bad).unwrap();
        assert!(matches!(
            load_detection_result(&img),
            Err(ImageError::Sidecar { .. })
        ));
        assert!(load_if_fresh(&img, &Config::default()).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_if_fresh_respects_mtime() {
        let dir = temp_dir("mtime");
        let img = dir.join("frame.fits");
        std::fs::write(&img, b"image-bytes").unwrap();
        save_detection_result(
            &img,
            &result_with(vec![star_at(1.0, 1.0, 9.0)]),
            &Config::default(),
        )
        .unwrap();

        // Sidecar records the image's current mtime → fresh.
        assert!(load_if_fresh(&img, &Config::default()).is_some());

        // Changing the image's mtime makes the stored mtime stale → miss.
        let base = std::fs::metadata(&img).unwrap().modified().unwrap();
        bump_mtime(&img, base, 60);
        assert!(load_if_fresh(&img, &Config::default()).is_none());

        // Re-saving records the new mtime → fresh again.
        save_detection_result(
            &img,
            &result_with(vec![star_at(1.0, 1.0, 9.0)]),
            &Config::default(),
        )
        .unwrap();
        assert!(load_if_fresh(&img, &Config::default()).is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_if_fresh_respects_config() {
        let dir = temp_dir("config");
        let img = dir.join("frame.fits");
        std::fs::write(&img, b"image-bytes").unwrap();

        let config_a = Config::default();
        let mut config_b = Config::default();
        config_b.min_snr += 7.0; // different config → different fingerprint
        assert_ne!(config_fingerprint(&config_a), config_fingerprint(&config_b));

        save_detection_result(&img, &result_with(vec![star_at(2.0, 2.0, 5.0)]), &config_a).unwrap();

        // Same config → hit; changed config → miss.
        assert!(load_if_fresh(&img, &config_a).is_some());
        assert!(load_if_fresh(&img, &config_b).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_file_cached_hits_cache_then_force_refreshes() {
        let dir = temp_dir("cached_e2e");
        let cfg = StarFieldConfig::default();
        let (buf, _truth) = generate_star_field(&cfg);
        let image = AstroImage::from_pixels(
            ImageDimensions::new(cfg.width, cfg.height, 1),
            buf.into_vec(),
        );
        let img_path = dir.join("field.tiff");
        image.save(&img_path).unwrap();

        let mut detector = StarDetector::new();

        // First call is a cache miss: detect + write sidecar.
        let first = detector.detect_file_cached(&img_path).unwrap();
        assert!(!first.stars.is_empty(), "should detect stars on cache miss");
        assert!(sidecar_path(&img_path).exists());

        // Overwrite the sidecar with a 1-star sentinel under the same config so it
        // is a valid hit; the next cached call must return it, not re-detect.
        let sentinel = result_with(vec![star_at(1.0, 2.0, 3.0)]);
        save_detection_result(&img_path, &sentinel, detector.config()).unwrap();
        let hit = detector.detect_file_cached(&img_path).unwrap();
        assert_eq!(
            hit.stars.len(),
            1,
            "cache hit must return the sentinel, not re-detect"
        );

        // Force path ignores the cache and re-detects the full field.
        let forced = detector.detect_file(&img_path).unwrap();
        assert!(
            forced.stars.len() > 1,
            "force should re-detect the full field, got {}",
            forced.stars.len()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
