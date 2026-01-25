//! FITS image download from astronomical surveys.
//!
//! Supports downloading images from SDSS and Pan-STARRS with local caching.

use anyhow::{Context, Result};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Image fetcher with local caching.
#[derive(Debug)]
pub struct ImageFetcher {
    cache_dir: PathBuf,
    client: reqwest::blocking::Client,
}

impl ImageFetcher {
    /// Create a new image fetcher with default cache directory.
    pub fn new() -> Result<Self> {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from(".cache"))
            .join("lumos")
            .join("survey_images");

        Self::with_cache_dir(cache_dir)
    }

    /// Create a new image fetcher with specified cache directory.
    pub fn with_cache_dir(cache_dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&cache_dir).context("Failed to create cache directory")?;

        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(300)) // 5 min timeout for large downloads
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self { cache_dir, client })
    }

    /// Get the cache directory path.
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Check if an image is already cached.
    pub fn is_cached(&self, key: &str) -> Option<PathBuf> {
        let path = self.cache_dir.join(key);
        if path.exists() { Some(path) } else { None }
    }

    /// Fetch an SDSS image.
    ///
    /// # Arguments
    /// * `run` - SDSS run number
    /// * `camcol` - Camera column (1-6)
    /// * `field` - Field number
    /// * `band` - Filter band (u, g, r, i, z)
    /// * `rerun` - Rerun number (typically 301)
    ///
    /// # Returns
    /// Path to the downloaded/cached FITS file.
    pub fn fetch_sdss(
        &self,
        run: u32,
        camcol: u8,
        field: u16,
        band: char,
        rerun: u32,
    ) -> Result<PathBuf> {
        let cache_key = format!("sdss_{}_{}_{}_{}.fits", run, camcol, field, band);

        // Check cache first
        if let Some(path) = self.is_cached(&cache_key) {
            tracing::debug!("Using cached SDSS image: {}", path.display());
            return Ok(path);
        }

        // SDSS DR17 image URL pattern
        // Example: https://data.sdss.org/sas/dr17/eboss/photoObj/frames/301/2505/1/frame-r-002505-1-0032.fits.bz2
        let url = format!(
            "https://data.sdss.org/sas/dr17/eboss/photoObj/frames/{}/{}/{}/frame-{}-{:06}-{}-{:04}.fits.bz2",
            rerun, run, camcol, band, run, camcol, field
        );

        tracing::info!("Downloading SDSS image from {}", url);

        let response = self
            .client
            .get(&url)
            .send()
            .context("Failed to download SDSS image")?;

        if !response.status().is_success() {
            anyhow::bail!(
                "SDSS download failed: {} for URL {}",
                response.status(),
                url
            );
        }

        let compressed_data = response.bytes().context("Failed to read response")?;

        // Decompress bzip2
        let mut decompressor = bzip2::read::BzDecoder::new(&compressed_data[..]);
        let mut fits_data = Vec::new();
        decompressor
            .read_to_end(&mut fits_data)
            .context("Failed to decompress SDSS image")?;

        // Save to cache
        let cache_path = self.cache_dir.join(&cache_key);
        let mut file = File::create(&cache_path).context("Failed to create cache file")?;
        file.write_all(&fits_data)
            .context("Failed to write cache file")?;

        tracing::info!("Cached SDSS image: {}", cache_path.display());

        Ok(cache_path)
    }

    /// Fetch a Pan-STARRS cutout image.
    ///
    /// # Arguments
    /// * `ra` - Center RA (degrees)
    /// * `dec` - Center Dec (degrees)
    /// * `size_arcmin` - Cutout size (arcminutes, max ~30)
    /// * `band` - Filter band (g, r, i, z, y)
    ///
    /// # Returns
    /// Path to the downloaded/cached FITS file.
    pub fn fetch_panstarrs(
        &self,
        ra: f64,
        dec: f64,
        size_arcmin: f32,
        band: char,
    ) -> Result<PathBuf> {
        let cache_key = format!("ps1_{:.4}_{:.4}_{:.1}_{}.fits", ra, dec, size_arcmin, band);

        // Check cache first
        if let Some(path) = self.is_cached(&cache_key) {
            tracing::debug!("Using cached Pan-STARRS image: {}", path.display());
            return Ok(path);
        }

        // PS1 Image Cutout Service
        // First, get the list of available images
        let _size_deg = size_arcmin / 60.0;
        let size_pix = (size_arcmin * 60.0 / 0.25) as u32; // 0.25 arcsec/pixel

        let url = format!(
            "https://ps1images.stsci.edu/cgi-bin/ps1cutouts\
             ?ra={}&dec={}&size={}&filter={}&output_size={}&format=fits",
            ra, dec, size_pix, band, size_pix
        );

        tracing::info!("Requesting Pan-STARRS cutout from {}", url);

        // PS1 cutout service returns HTML with links; we need to use the fitscut service directly
        let fitscut_url = format!(
            "https://ps1images.stsci.edu/cgi-bin/fitscut.cgi\
             ?ra={}&dec={}&size={}&format=fits&red={}",
            ra, dec, size_pix, band
        );

        let response = self
            .client
            .get(&fitscut_url)
            .send()
            .context("Failed to download Pan-STARRS image")?;

        if !response.status().is_success() {
            anyhow::bail!(
                "Pan-STARRS download failed: {} for URL {}",
                response.status(),
                fitscut_url
            );
        }

        let fits_data = response.bytes().context("Failed to read response")?;

        // Save to cache
        let cache_path = self.cache_dir.join(&cache_key);
        let mut file = File::create(&cache_path).context("Failed to create cache file")?;
        file.write_all(&fits_data)
            .context("Failed to write cache file")?;

        tracing::info!("Cached Pan-STARRS image: {}", cache_path.display());

        Ok(cache_path)
    }

    /// Clear the image cache.
    #[allow(dead_code)]
    pub fn clear_cache(&self) -> Result<()> {
        if self.cache_dir.exists() {
            fs::remove_dir_all(&self.cache_dir).context("Failed to remove cache directory")?;
            fs::create_dir_all(&self.cache_dir).context("Failed to recreate cache directory")?;
        }
        Ok(())
    }

    /// Get total size of cached images in bytes.
    #[allow(dead_code)]
    pub fn cache_size(&self) -> u64 {
        let mut total = 0;
        if let Ok(entries) = fs::read_dir(&self.cache_dir) {
            for entry in entries.flatten() {
                if let Ok(metadata) = entry.metadata() {
                    total += metadata.len();
                }
            }
        }
        total
    }
}

impl Default for ImageFetcher {
    fn default() -> Self {
        Self::new().expect("Failed to create ImageFetcher")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn test_fetcher() -> ImageFetcher {
        let cache_dir = env::temp_dir().join("lumos_test_cache");
        ImageFetcher::with_cache_dir(cache_dir).unwrap()
    }

    #[test]
    fn test_cache_dir_creation() {
        let fetcher = test_fetcher();
        assert!(fetcher.cache_dir().exists());
    }

    #[test]
    #[ignore] // Requires network
    fn test_fetch_sdss() {
        let fetcher = test_fetcher();

        // Fetch a known SDSS field
        // Run 2505, camcol 1, field 32 is a well-known test field
        let path = fetcher.fetch_sdss(2505, 1, 32, 'r', 301).unwrap();

        assert!(path.exists());
        println!("Downloaded SDSS image to: {}", path.display());

        // Verify it's a valid FITS file by checking magic bytes
        let mut file = File::open(&path).unwrap();
        let mut header = [0u8; 30];
        file.read_exact(&mut header).unwrap();
        assert!(header.starts_with(b"SIMPLE"));
    }

    #[test]
    #[ignore] // Requires network
    fn test_fetch_panstarrs() {
        let fetcher = test_fetcher();

        // Fetch a Pan-STARRS cutout
        // M31 center region
        let path = fetcher.fetch_panstarrs(10.6847, 41.2687, 5.0, 'r').unwrap();

        assert!(path.exists());
        println!("Downloaded Pan-STARRS image to: {}", path.display());
    }

    #[test]
    fn test_cache_lookup() {
        let fetcher = test_fetcher();

        // Non-existent file should return None
        assert!(fetcher.is_cached("nonexistent.fits").is_none());
    }
}
