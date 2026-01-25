//! Catalog query client for astronomical surveys.
//!
//! Supports querying star positions from SDSS, Pan-STARRS, and Gaia DR3.

use super::wcs::SkyBounds;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::time::Duration;

/// Catalog source for star queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogSource {
    /// Sloan Digital Sky Survey
    Sdss,
    /// Pan-STARRS (via MAST)
    PanStarrs,
    /// Gaia Data Release 3
    GaiaDr3,
}

/// A star from a catalog query.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CatalogStar {
    /// Right Ascension (degrees)
    pub ra: f64,
    /// Declination (degrees)
    pub dec: f64,
    /// Magnitude in primary band
    pub mag: f32,
    /// Magnitude error
    pub mag_err: f32,
}

/// Client for querying astronomical catalogs.
#[derive(Debug)]
pub struct CatalogClient {
    client: reqwest::blocking::Client,
}

impl Default for CatalogClient {
    fn default() -> Self {
        Self::new()
    }
}

impl CatalogClient {
    /// Create a new catalog client.
    pub fn new() -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .expect("Failed to create HTTP client");

        Self { client }
    }

    /// Query stars in a circular region.
    ///
    /// # Arguments
    /// * `source` - Catalog to query
    /// * `ra` - Center RA (degrees)
    /// * `dec` - Center Dec (degrees)
    /// * `radius_deg` - Search radius (degrees)
    /// * `mag_limit` - Faint magnitude limit
    pub fn query_region(
        &self,
        source: CatalogSource,
        ra: f64,
        dec: f64,
        radius_deg: f64,
        mag_limit: f32,
    ) -> Result<Vec<CatalogStar>> {
        match source {
            CatalogSource::Sdss => self.query_sdss_region(ra, dec, radius_deg, mag_limit),
            CatalogSource::PanStarrs => self.query_panstarrs_region(ra, dec, radius_deg, mag_limit),
            CatalogSource::GaiaDr3 => self.query_gaia_region(ra, dec, radius_deg, mag_limit),
        }
    }

    /// Query stars within sky bounds.
    pub fn query_box(
        &self,
        source: CatalogSource,
        bounds: &SkyBounds,
        mag_limit: f32,
    ) -> Result<Vec<CatalogStar>> {
        // Use cone search with radius encompassing the box
        let ra = bounds.ra_center();
        let dec = bounds.dec_center();
        let radius = bounds.radius() * 1.1; // 10% margin

        let stars = self.query_region(source, ra, dec, radius, mag_limit)?;

        // Filter to actual bounds
        Ok(stars
            .into_iter()
            .filter(|s| bounds.contains(s.ra, s.dec))
            .collect())
    }

    /// Query SDSS catalog via SkyServer.
    fn query_sdss_region(
        &self,
        ra: f64,
        dec: f64,
        radius_deg: f64,
        mag_limit: f32,
    ) -> Result<Vec<CatalogStar>> {
        // SQL query for point sources (type=6 means star)
        let sql = format!(
            r#"
            SELECT ra, dec, psfMag_r, psfMagErr_r
            FROM PhotoObj
            WHERE ra BETWEEN {} AND {}
              AND dec BETWEEN {} AND {}
              AND type = 6
              AND psfMag_r < {}
              AND psfMag_r > 0
            ORDER BY psfMag_r
            "#,
            ra - radius_deg / dec.to_radians().cos().max(0.1),
            ra + radius_deg / dec.to_radians().cos().max(0.1),
            dec - radius_deg,
            dec + radius_deg,
            mag_limit
        );

        let url = "https://skyserver.sdss.org/dr18/SkyServerWS/SearchTools/SqlSearch";

        let response = self
            .client
            .get(url)
            .query(&[("cmd", sql.as_str()), ("format", "json")])
            .send()
            .context("Failed to query SDSS")?;

        if !response.status().is_success() {
            anyhow::bail!("SDSS query failed: {}", response.status());
        }

        let data: SdssResponse = response.json().context("Failed to parse SDSS response")?;

        let center_ra = ra;
        let center_dec = dec;

        let stars = data
            .0
            .into_iter()
            .filter_map(|row| {
                let star_ra = row.ra?;
                let star_dec = row.dec?;
                let mag = row.psfMag_r? as f32;
                let mag_err = row.psfMagErr_r.unwrap_or(0.0) as f32;

                // Filter by actual distance from query center
                let dist = angular_distance(star_ra, star_dec, center_ra, center_dec);
                if dist <= radius_deg {
                    Some(CatalogStar {
                        ra: star_ra,
                        dec: star_dec,
                        mag,
                        mag_err,
                    })
                } else {
                    None
                }
            })
            .collect();

        Ok(stars)
    }

    /// Query Pan-STARRS catalog via MAST.
    fn query_panstarrs_region(
        &self,
        ra: f64,
        dec: f64,
        radius_deg: f64,
        mag_limit: f32,
    ) -> Result<Vec<CatalogStar>> {
        let url = format!(
            "https://catalogs.mast.stsci.edu/api/v0.1/panstarrs/dr2/mean.json\
             ?ra={}&dec={}&radius={}&nDetections.gte=1&rMeanPSFMag.lt={}&columns=[raMean,decMean,rMeanPSFMag,rMeanPSFMagErr]&pagesize=50000",
            ra, dec, radius_deg, mag_limit
        );

        let response = self
            .client
            .get(&url)
            .send()
            .context("Failed to query Pan-STARRS")?;

        if !response.status().is_success() {
            anyhow::bail!("Pan-STARRS query failed: {}", response.status());
        }

        let data: PanStarrsResponse = response
            .json()
            .context("Failed to parse Pan-STARRS response")?;

        let stars = data
            .data
            .into_iter()
            .filter_map(|row| {
                let ra = row.raMean?;
                let dec = row.decMean?;
                let mag = row.rMeanPSFMag? as f32;
                let mag_err = row.rMeanPSFMagErr.unwrap_or(0.0) as f32;

                Some(CatalogStar {
                    ra,
                    dec,
                    mag,
                    mag_err,
                })
            })
            .collect();

        Ok(stars)
    }

    /// Query Gaia DR3 catalog via TAP.
    fn query_gaia_region(
        &self,
        ra: f64,
        dec: f64,
        radius_deg: f64,
        mag_limit: f32,
    ) -> Result<Vec<CatalogStar>> {
        // ADQL query using CIRCLE for cone search
        let adql = format!(
            r#"
            SELECT ra, dec, phot_g_mean_mag
            FROM gaiadr3.gaia_source
            WHERE 1 = CONTAINS(
                POINT('ICRS', ra, dec),
                CIRCLE('ICRS', {}, {}, {})
            )
            AND phot_g_mean_mag < {}
            "#,
            ra, dec, radius_deg, mag_limit
        );

        let url = "https://gea.esac.esa.int/tap-server/tap/sync";

        let response = self
            .client
            .post(url)
            .form(&[
                ("REQUEST", "doQuery"),
                ("LANG", "ADQL"),
                ("FORMAT", "json"),
                ("QUERY", &adql),
            ])
            .send()
            .context("Failed to query Gaia")?;

        if !response.status().is_success() {
            anyhow::bail!("Gaia query failed: {}", response.status());
        }

        let data: GaiaResponse = response.json().context("Failed to parse Gaia response")?;

        let stars = data
            .data
            .into_iter()
            .map(|row| CatalogStar {
                ra: row[0],
                dec: row[1],
                mag: row[2] as f32,
                mag_err: 0.01, // Gaia doesn't return error in this simple query
            })
            .collect();

        Ok(stars)
    }
}

/// Calculate angular distance between two points (degrees).
fn angular_distance(ra1: f64, dec1: f64, ra2: f64, dec2: f64) -> f64 {
    let ra1 = ra1.to_radians();
    let dec1 = dec1.to_radians();
    let ra2 = ra2.to_radians();
    let dec2 = dec2.to_radians();

    let sin_dec1 = dec1.sin();
    let sin_dec2 = dec2.sin();
    let cos_dec1 = dec1.cos();
    let cos_dec2 = dec2.cos();
    let cos_dra = (ra2 - ra1).cos();

    let cos_d = sin_dec1 * sin_dec2 + cos_dec1 * cos_dec2 * cos_dra;
    cos_d.clamp(-1.0, 1.0).acos().to_degrees()
}

// Response types for JSON parsing

#[derive(Debug, Deserialize)]
struct SdssResponse(Vec<SdssRow>);

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct SdssRow {
    ra: Option<f64>,
    dec: Option<f64>,
    psfMag_r: Option<f64>,
    psfMagErr_r: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct PanStarrsResponse {
    data: Vec<PanStarrsRow>,
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct PanStarrsRow {
    raMean: Option<f64>,
    decMean: Option<f64>,
    rMeanPSFMag: Option<f64>,
    rMeanPSFMagErr: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct GaiaResponse {
    data: Vec<Vec<f64>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_angular_distance() {
        // Same point
        assert!((angular_distance(180.0, 45.0, 180.0, 45.0)).abs() < 1e-10);

        // 1 degree apart in Dec
        let dist = angular_distance(180.0, 45.0, 180.0, 46.0);
        assert!((dist - 1.0).abs() < 0.01);
    }

    #[test]
    #[ignore] // Requires network
    fn test_sdss_query() {
        let client = CatalogClient::new();
        let stars = client
            .query_region(CatalogSource::Sdss, 180.0, 0.0, 0.05, 18.0)
            .unwrap();

        println!("Found {} SDSS stars", stars.len());
        for star in stars.iter().take(5) {
            println!(
                "  RA={:.4}, Dec={:.4}, mag={:.2}",
                star.ra, star.dec, star.mag
            );
        }
    }

    #[test]
    #[ignore] // Requires network
    fn test_panstarrs_query() {
        let client = CatalogClient::new();
        let stars = client
            .query_region(CatalogSource::PanStarrs, 180.0, 45.0, 0.05, 18.0)
            .unwrap();

        println!("Found {} Pan-STARRS stars", stars.len());
        for star in stars.iter().take(5) {
            println!(
                "  RA={:.4}, Dec={:.4}, mag={:.2}",
                star.ra, star.dec, star.mag
            );
        }
    }

    #[test]
    #[ignore] // Requires network
    fn test_gaia_query() {
        let client = CatalogClient::new();
        let stars = client
            .query_region(CatalogSource::GaiaDr3, 180.0, 45.0, 0.05, 18.0)
            .unwrap();

        println!("Found {} Gaia stars", stars.len());
        for star in stars.iter().take(5) {
            println!(
                "  RA={:.4}, Dec={:.4}, mag={:.2}",
                star.ra, star.dec, star.mag
            );
        }
    }
}
