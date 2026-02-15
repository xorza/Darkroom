//! Save `DetectionResult` as sidecar files.
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

fn sidecar_path(image_path: &Path) -> PathBuf {
    let mut p = image_path.as_os_str().to_owned();
    p.push(".detection");
    PathBuf::from(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sidecar_path_format() {
        let p = sidecar_path(Path::new("/data/frame_0001.tiff"));
        assert_eq!(p, PathBuf::from("/data/frame_0001.tiff.detection"));
    }
}
