//! Rectangle cache manager for astrometry benchmark.
//!
//! Manages splitting a source image into random rectangles, caching them,
//! and storing astrometry.net results.

use super::local_solver::AstrometryStar;
use anyhow::{Context, Result, bail};
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Information about a cached rectangle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RectangleInfo {
    /// Unique identifier (hash of source + bounds).
    pub id: String,
    /// Original image filename (not full path).
    pub source_file: String,
    /// X offset in source image.
    pub x: usize,
    /// Y offset in source image.
    pub y: usize,
    /// Rectangle width.
    pub width: usize,
    /// Rectangle height.
    pub height: usize,
    /// Path to cached TIFF image (relative to cache dir).
    pub image_path: PathBuf,
    /// Path to cached astrometry result JSON (relative to cache dir).
    pub axy_path: Option<PathBuf>,
    /// Nova job ID if solved.
    pub job_id: Option<u64>,
}

/// Manifest storing all cached rectangles.
#[derive(Debug, Default, Serialize, Deserialize)]
struct CacheManifest {
    /// Version for future compatibility.
    version: u32,
    /// Source file checksums for validation.
    source_checksums: HashMap<String, String>,
    /// All cached rectangles.
    rectangles: Vec<RectangleInfo>,
}

/// Manager for rectangle caching.
#[derive(Debug)]
pub struct RectangleCache {
    cache_dir: PathBuf,
    manifest: CacheManifest,
}

impl RectangleCache {
    /// Create a new rectangle cache.
    ///
    /// Uses `LUMOS_TEST_CACHE_DIR` environment variable if set,
    /// otherwise falls back to system cache directory.
    pub fn new() -> Result<Self> {
        let cache_dir = if let Ok(dir) = std::env::var("LUMOS_TEST_CACHE_DIR") {
            PathBuf::from(dir).join("astrometry_benchmark")
        } else {
            dirs::cache_dir()
                .context("Could not determine cache directory")?
                .join("lumos")
                .join("astrometry_benchmark")
        };

        std::fs::create_dir_all(&cache_dir)
            .with_context(|| format!("Failed to create cache dir: {}", cache_dir.display()))?;

        std::fs::create_dir_all(cache_dir.join("rectangles"))
            .context("Failed to create rectangles dir")?;

        let mut cache = Self {
            cache_dir,
            manifest: CacheManifest::default(),
        };

        cache.load_manifest()?;
        Ok(cache)
    }

    /// Create a cache with a custom cache directory.
    pub fn with_cache_dir(cache_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&cache_dir)?;
        std::fs::create_dir_all(cache_dir.join("rectangles"))?;

        let mut cache = Self {
            cache_dir,
            manifest: CacheManifest::default(),
        };

        cache.load_manifest()?;
        Ok(cache)
    }

    /// Get the cache directory path.
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Load manifest from disk.
    fn load_manifest(&mut self) -> Result<()> {
        let manifest_path = self.cache_dir.join("manifest.json");
        if manifest_path.exists() {
            let contents =
                std::fs::read_to_string(&manifest_path).context("Failed to read manifest")?;
            self.manifest = serde_json::from_str(&contents).context("Failed to parse manifest")?;
            tracing::info!(
                "Loaded manifest with {} rectangles",
                self.manifest.rectangles.len()
            );
        }
        Ok(())
    }

    /// Save manifest to disk.
    pub fn save_manifest(&self) -> Result<()> {
        let manifest_path = self.cache_dir.join("manifest.json");
        let contents =
            serde_json::to_string_pretty(&self.manifest).context("Failed to serialize manifest")?;
        std::fs::write(&manifest_path, contents).context("Failed to write manifest")?;
        Ok(())
    }

    /// Get all cached rectangles.
    pub fn rectangles(&self) -> &[RectangleInfo] {
        &self.manifest.rectangles
    }

    /// Get rectangles that have been solved (have axy results).
    pub fn solved_rectangles(&self) -> Vec<&RectangleInfo> {
        self.manifest
            .rectangles
            .iter()
            .filter(|r| r.axy_path.is_some())
            .collect()
    }

    /// Get rectangles that need solving.
    #[allow(dead_code)]
    pub fn unsolved_rectangles(&self) -> Vec<&RectangleInfo> {
        self.manifest
            .rectangles
            .iter()
            .filter(|r| r.axy_path.is_none())
            .collect()
    }

    /// Generate random rectangles from a source image.
    ///
    /// Rectangles are cached as TIFF files. If rectangles already exist for
    /// this source, they are returned from cache.
    pub fn generate_rectangles(
        &mut self,
        source_image: &Path,
        num_rectangles: usize,
        min_size: (usize, usize),
        max_size: (usize, usize),
        seed: Option<u64>,
    ) -> Result<Vec<RectangleInfo>> {
        let source_name = source_image
            .file_name()
            .context("Invalid source path")?
            .to_string_lossy()
            .to_string();

        // Check if we already have rectangles for this source
        let existing: Vec<RectangleInfo> = self
            .manifest
            .rectangles
            .iter()
            .filter(|r| r.source_file == source_name)
            .cloned()
            .collect();

        if !existing.is_empty() && existing.len() >= num_rectangles {
            tracing::info!(
                "Using {} cached rectangles for {}",
                existing.len(),
                source_name
            );
            return Ok(existing);
        }

        // Load source image
        tracing::info!("Loading source image: {}", source_image.display());
        let image =
            crate::AstroImage::from_file(source_image).context("Failed to load source image")?;

        let img_width = image.width();
        let img_height = image.height();

        tracing::info!("Source image size: {}x{}", img_width, img_height);

        // Generate rectangles
        let mut rng = match seed {
            Some(s) => SmallRng::seed_from_u64(s),
            None => SmallRng::seed_from_u64(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos() as u64,
            ),
        };

        let mut rectangles = Vec::with_capacity(num_rectangles);

        for i in 0..num_rectangles {
            // Random size
            let width = rng.random_range(min_size.0..=max_size.0);
            let height = rng.random_range(min_size.1..=max_size.1);

            // Random position (ensure rectangle fits)
            let max_x = img_width.saturating_sub(width);
            let max_y = img_height.saturating_sub(height);

            if max_x == 0 || max_y == 0 {
                bail!(
                    "Source image too small for requested rectangle size. Image: {}x{}, min rect: {}x{}",
                    img_width,
                    img_height,
                    min_size.0,
                    min_size.1
                );
            }

            let x = rng.random_range(0..max_x);
            let y = rng.random_range(0..max_y);

            // Generate unique ID using simple hash
            let id = format!(
                "{:08x}",
                simple_hash(&format!("{}_{}_{}_{}_{}", source_name, x, y, width, height))
            );

            let image_path = PathBuf::from("rectangles").join(format!("rect_{}.fits", id));

            let rect = RectangleInfo {
                id,
                source_file: source_name.clone(),
                x,
                y,
                width,
                height,
                image_path,
                axy_path: None,
                job_id: None,
            };

            // Extract and save rectangle
            self.save_rectangle(&image, &rect)?;
            rectangles.push(rect.clone());
            self.manifest.rectangles.push(rect);

            tracing::info!(
                "Generated rectangle {}/{}: {}x{} at ({}, {})",
                i + 1,
                num_rectangles,
                width,
                height,
                x,
                y
            );
        }

        self.save_manifest()?;
        Ok(rectangles)
    }

    /// Extract and save a rectangle from the source image.
    fn save_rectangle(&self, source: &crate::AstroImage, rect: &RectangleInfo) -> Result<()> {
        let full_path = self.cache_dir.join(&rect.image_path);

        // Convert to grayscale if needed
        let grayscale = source.to_grayscale();

        // Extract rectangle pixels
        let src_width = grayscale.width();
        let src_pixels = grayscale.pixels();
        let mut pixels = Vec::with_capacity(rect.width * rect.height);

        for y in rect.y..(rect.y + rect.height) {
            for x in rect.x..(rect.x + rect.width) {
                let idx = y * src_width + x;
                pixels.push(src_pixels[idx]);
            }
        }

        // Save as FITS (required for image2xy)
        save_grayscale_fits(&pixels, rect.width, rect.height, &full_path)?;

        tracing::debug!("Saved rectangle to {}", full_path.display());
        Ok(())
    }

    /// Get the full path to a rectangle's image file.
    pub fn rectangle_image_path(&self, rect: &RectangleInfo) -> PathBuf {
        self.cache_dir.join(&rect.image_path)
    }

    /// Get the full path to a rectangle's axy result file.
    #[allow(dead_code)]
    pub fn rectangle_axy_path(&self, rect: &RectangleInfo) -> Option<PathBuf> {
        rect.axy_path.as_ref().map(|p| self.cache_dir.join(p))
    }

    /// Save astrometry.net result for a rectangle.
    pub fn save_axy_result(
        &mut self,
        rect_id: &str,
        job_id: u64,
        stars: &[AstrometryStar],
    ) -> Result<()> {
        let rect = self
            .manifest
            .rectangles
            .iter_mut()
            .find(|r| r.id == rect_id)
            .context("Rectangle not found")?;

        let axy_path = PathBuf::from("rectangles").join(format!("rect_{}.axy.json", rect_id));
        let full_path = self.cache_dir.join(&axy_path);

        let contents = serde_json::to_string_pretty(stars).context("Failed to serialize stars")?;
        std::fs::write(&full_path, contents).context("Failed to write axy result")?;

        rect.axy_path = Some(axy_path);
        rect.job_id = Some(job_id);

        self.save_manifest()?;
        tracing::info!(
            "Saved {} stars for rectangle {} (job {})",
            stars.len(),
            rect_id,
            job_id
        );

        Ok(())
    }

    /// Load cached astrometry.net stars for a rectangle.
    pub fn load_axy_result(&self, rect: &RectangleInfo) -> Result<Vec<AstrometryStar>> {
        let axy_path = rect
            .axy_path
            .as_ref()
            .context("No axy result cached for this rectangle")?;

        let full_path = self.cache_dir.join(axy_path);
        let contents = std::fs::read_to_string(&full_path)
            .with_context(|| format!("Failed to read axy file: {}", full_path.display()))?;

        let stars: Vec<AstrometryStar> =
            serde_json::from_str(&contents).context("Failed to parse axy result")?;

        Ok(stars)
    }

    /// Load a rectangle's image pixels.
    pub fn load_rectangle_image(&self, rect: &RectangleInfo) -> Result<(Vec<f32>, usize, usize)> {
        let path = self.rectangle_image_path(rect);
        let image = crate::AstroImage::from_file(&path)
            .with_context(|| format!("Failed to load rectangle image: {}", path.display()))?;

        Ok((image.pixels().to_vec(), image.width(), image.height()))
    }

    /// Clear all cached data.
    #[allow(dead_code)]
    pub fn clear(&mut self) -> Result<()> {
        // Remove rectangles directory
        let rect_dir = self.cache_dir.join("rectangles");
        if rect_dir.exists() {
            std::fs::remove_dir_all(&rect_dir)?;
            std::fs::create_dir_all(&rect_dir)?;
        }

        // Clear manifest
        self.manifest = CacheManifest::default();
        self.save_manifest()?;

        tracing::info!("Cleared astrometry benchmark cache");
        Ok(())
    }
}

/// Simple FNV-1a hash for generating unique IDs.
fn simple_hash(s: &str) -> u32 {
    let mut hash: u32 = 2166136261;
    for byte in s.bytes() {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(16777619);
    }
    hash
}

/// Save grayscale image as FITS (required for image2xy).
fn save_grayscale_fits(pixels: &[f32], width: usize, height: usize, path: &Path) -> Result<()> {
    use fitsio::FitsFile;
    use fitsio::images::{ImageDescription, ImageType};

    // Remove existing file if present (fitsio won't overwrite)
    if path.exists() {
        std::fs::remove_file(path)?;
    }

    let description = ImageDescription {
        data_type: ImageType::Float,
        dimensions: &[height, width], // FITS uses [NAXIS2, NAXIS1] = [height, width]
    };

    let mut fptr = FitsFile::create(path)
        .with_custom_primary(&description)
        .open()
        .with_context(|| format!("Failed to create FITS file: {}", path.display()))?;

    let hdu = fptr.primary_hdu().context("Failed to get primary HDU")?;

    // Write the pixel data
    hdu.write_image(&mut fptr, pixels)
        .context("Failed to write FITS image data")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_creation() {
        let temp_dir = std::env::temp_dir().join(format!("lumos_test_{}", std::process::id()));
        let cache = RectangleCache::with_cache_dir(temp_dir.clone()).unwrap();
        assert!(cache.cache_dir().exists());
        assert!(cache.rectangles().is_empty());

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_manifest_persistence() {
        let temp_dir =
            std::env::temp_dir().join(format!("lumos_test_manifest_{}", std::process::id()));

        {
            let mut cache = RectangleCache::with_cache_dir(temp_dir.clone()).unwrap();
            cache.manifest.rectangles.push(RectangleInfo {
                id: "test123".to_string(),
                source_file: "test.tiff".to_string(),
                x: 0,
                y: 0,
                width: 100,
                height: 100,
                image_path: PathBuf::from("rectangles/test.tiff"),
                axy_path: None,
                job_id: None,
            });
            cache.save_manifest().unwrap();
        }

        {
            let cache = RectangleCache::with_cache_dir(temp_dir.clone()).unwrap();
            assert_eq!(cache.rectangles().len(), 1);
            assert_eq!(cache.rectangles()[0].id, "test123");
        }

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
