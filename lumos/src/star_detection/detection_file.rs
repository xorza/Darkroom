//! Save and load `DetectionResult` as sidecar files.
//!
//! Sidecar path: `{image_path}.detection` using SCN text format.

use std::path::{Path, PathBuf};

use super::detector::DetectionResult;

/// Save detection result as a `.detection` sidecar file next to the image.
pub fn save_detection_result(image_path: &Path, result: &DetectionResult) -> std::io::Result<()> {
    let text =
        common::serde_scn::to_string(result).map_err(|e| std::io::Error::other(e.to_string()))?;
    std::fs::write(sidecar_path(image_path), text)
}

/// Load detection result from a `.detection` sidecar file.
///
/// Returns `None` if the sidecar is missing or cannot be parsed.
pub fn load_detection_result(image_path: &Path) -> Option<DetectionResult> {
    let text = std::fs::read_to_string(sidecar_path(image_path)).ok()?;
    common::serde_scn::from_str(&text).ok()
}

fn sidecar_path(image_path: &Path) -> PathBuf {
    let mut p = image_path.as_os_str().to_owned();
    p.push(".detection");
    PathBuf::from(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::star_detection::detector::{ChannelStats, Diagnostics};
    use crate::star_detection::star::Star;
    use arrayvec::ArrayVec;
    use glam::DVec2;
    use std::sync::atomic::{AtomicU32, Ordering};

    static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_dir() -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "lumos_detection_file_test_{id}_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn make_result() -> DetectionResult {
        let mut channel_stats = ArrayVec::new();
        channel_stats.push(ChannelStats {
            median: 100.5,
            mad: 3.2,
        });
        channel_stats.push(ChannelStats {
            median: 98.0,
            mad: 2.8,
        });
        channel_stats.push(ChannelStats {
            median: 102.1,
            mad: 3.5,
        });

        DetectionResult {
            stars: vec![Star {
                pos: DVec2::new(10.5, 20.3),
                flux: 500.0,
                fwhm: 2.8,
                eccentricity: 0.1,
                snr: 45.0,
                peak: 0.7,
                sharpness: 0.3,
                roundness1: 0.05,
                roundness2: -0.02,
            }],
            diagnostics: Diagnostics {
                final_star_count: 1,
                median_fwhm: 2.8,
                median_snr: 45.0,
                ..Default::default()
            },
            channel_stats,
        }
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let dir = temp_dir();
        let image_path = dir.join("frame.tiff");
        // Create a dummy file so the path exists
        std::fs::write(&image_path, b"dummy").unwrap();

        let result = make_result();
        save_detection_result(&image_path, &result).unwrap();

        // Sidecar should exist
        let sidecar = sidecar_path(&image_path);
        assert!(sidecar.exists());

        // Load should return matching result
        let loaded = load_detection_result(&image_path).unwrap();
        assert_eq!(loaded.stars.len(), 1);
        assert!((loaded.stars[0].pos.x - 10.5).abs() < 1e-10);
        assert!((loaded.stars[0].pos.y - 20.3).abs() < 1e-10);
        assert!((loaded.stars[0].flux - 500.0).abs() < f32::EPSILON);
        assert!((loaded.stars[0].fwhm - 2.8).abs() < f32::EPSILON);
        assert_eq!(loaded.diagnostics.final_star_count, 1);
        assert!((loaded.diagnostics.median_fwhm - 2.8).abs() < f32::EPSILON);

        // Channel stats roundtrip
        assert_eq!(loaded.channel_stats.len(), 3);
        assert!((loaded.channel_stats[0].median - 100.5).abs() < f32::EPSILON);
        assert!((loaded.channel_stats[0].mad - 3.2).abs() < f32::EPSILON);
        assert!((loaded.channel_stats[1].median - 98.0).abs() < f32::EPSILON);
        assert!((loaded.channel_stats[2].mad - 3.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_load_returns_none_when_missing() {
        let path = PathBuf::from("/tmp/nonexistent_image_99999.tiff");
        assert!(load_detection_result(&path).is_none());
    }

    #[test]
    fn test_sidecar_path_format() {
        let p = sidecar_path(Path::new("/data/frame_0001.tiff"));
        assert_eq!(p, PathBuf::from("/data/frame_0001.tiff.detection"));
    }
}
