//! Star catalog access for astrometric solving.
//!
//! Provides interfaces to query star catalogs like Gaia DR3 for reference stars.

use std::fmt;

/// A star from an astronomical catalog.
#[derive(Debug, Clone)]
pub struct CatalogStar {
    /// Right ascension in degrees (J2000)
    pub ra: f64,

    /// Declination in degrees (J2000)
    pub dec: f64,

    /// Magnitude (typically G-band for Gaia)
    pub mag: f64,

    /// Optional catalog identifier (e.g., Gaia source_id)
    pub id: Option<String>,
}

impl CatalogStar {
    /// Create a new catalog star.
    pub fn new(ra: f64, dec: f64, mag: f64) -> Self {
        Self {
            ra,
            dec,
            mag,
            id: None,
        }
    }

    /// Create a new catalog star with an identifier.
    pub fn with_id(ra: f64, dec: f64, mag: f64, id: impl Into<String>) -> Self {
        Self {
            ra,
            dec,
            mag,
            id: Some(id.into()),
        }
    }
}

/// Error type for catalog operations.
#[derive(Debug, Clone)]
pub enum CatalogError {
    /// Network error during catalog query
    NetworkError(String),

    /// Invalid response from catalog service
    InvalidResponse(String),

    /// Catalog file not found or invalid
    FileError(String),

    /// No stars found in the queried region
    NoStarsFound,

    /// Query parameters are invalid
    InvalidQuery(String),

    /// Catalog service timeout
    Timeout,
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CatalogError::NetworkError(msg) => write!(f, "Network error: {}", msg),
            CatalogError::InvalidResponse(msg) => write!(f, "Invalid response: {}", msg),
            CatalogError::FileError(msg) => write!(f, "File error: {}", msg),
            CatalogError::NoStarsFound => write!(f, "No stars found in queried region"),
            CatalogError::InvalidQuery(msg) => write!(f, "Invalid query: {}", msg),
            CatalogError::Timeout => write!(f, "Catalog query timed out"),
        }
    }
}

impl std::error::Error for CatalogError {}

/// Source of catalog data for plate solving.
#[derive(Debug, Clone)]
pub enum CatalogSource {
    /// Query Gaia DR3 via VizieR cone search
    GaiaVizier {
        /// VizieR endpoint URL (default: CDS Strasbourg)
        endpoint: String,
    },

    /// Use pre-loaded catalog stars (for testing or offline use)
    Preloaded {
        /// Pre-loaded star list
        stars: Vec<CatalogStar>,
    },
}

impl Default for CatalogSource {
    fn default() -> Self {
        Self::gaia_vizier()
    }
}

impl CatalogSource {
    /// Create a Gaia VizieR source with the default CDS endpoint.
    pub fn gaia_vizier() -> Self {
        CatalogSource::GaiaVizier {
            endpoint: "http://vizier.cds.unistra.fr/viz-bin/conesearch/I/355/gaiadr3".to_string(),
        }
    }

    /// Create a Gaia VizieR source with a custom endpoint.
    pub fn gaia_vizier_custom(endpoint: impl Into<String>) -> Self {
        CatalogSource::GaiaVizier {
            endpoint: endpoint.into(),
        }
    }

    /// Create a source from pre-loaded stars (for testing or offline use).
    pub fn preloaded(stars: Vec<CatalogStar>) -> Self {
        CatalogSource::Preloaded { stars }
    }

    /// Query the catalog for stars in a region.
    ///
    /// # Arguments
    /// * `ra` - Right ascension of center in degrees
    /// * `dec` - Declination of center in degrees
    /// * `radius` - Search radius in degrees
    /// * `mag_limit` - Faintest magnitude to include
    ///
    /// # Returns
    /// Vector of catalog stars sorted by magnitude (brightest first)
    pub fn query_region(
        &self,
        ra: f64,
        dec: f64,
        radius: f64,
        mag_limit: f64,
    ) -> Result<Vec<CatalogStar>, CatalogError> {
        match self {
            CatalogSource::GaiaVizier { endpoint } => {
                query_vizier_cone(endpoint, ra, dec, radius, mag_limit)
            }
            CatalogSource::Preloaded { stars } => {
                // Filter by position and magnitude
                let mut result: Vec<_> = stars
                    .iter()
                    .filter(|s| {
                        s.mag <= mag_limit && angular_separation(ra, dec, s.ra, s.dec) <= radius
                    })
                    .cloned()
                    .collect();

                // Sort by magnitude
                result.sort_by(|a, b| {
                    a.mag
                        .partial_cmp(&b.mag)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });

                if result.is_empty() {
                    Err(CatalogError::NoStarsFound)
                } else {
                    Ok(result)
                }
            }
        }
    }
}

/// Query VizieR cone search for Gaia stars.
fn query_vizier_cone(
    endpoint: &str,
    ra: f64,
    dec: f64,
    radius: f64,
    mag_limit: f64,
) -> Result<Vec<CatalogStar>, CatalogError> {
    // Build URL with cone search parameters
    let url = format!("{}?RA={:.6}&DEC={:.6}&SR={:.6}", endpoint, ra, dec, radius);

    // For now, we use a blocking HTTP client
    // In production, this could be async or use a connection pool
    let response = ureq::get(&url)
        .call()
        .map_err(|e: ureq::Error| CatalogError::NetworkError(e.to_string()))?;

    if response.status() != 200 {
        return Err(CatalogError::InvalidResponse(format!(
            "HTTP status {}",
            response.status()
        )));
    }

    let body = response
        .into_body()
        .read_to_string()
        .map_err(|e| CatalogError::InvalidResponse(e.to_string()))?;

    // Parse VOTable response
    parse_votable_stars(&body, mag_limit)
}

/// Parse VOTable XML response to extract stars.
fn parse_votable_stars(votable: &str, mag_limit: f64) -> Result<Vec<CatalogStar>, CatalogError> {
    // Simple XML parsing for VOTable
    // Look for TABLEDATA section and extract RA, Dec, mag columns

    let mut stars = Vec::new();

    // Parse FIELD elements to find column indices
    // VizieR Gaia columns are typically: RA_ICRS, DE_ICRS, Gmag
    let fields: Vec<&str> = votable
        .split("<FIELD")
        .skip(1)
        .filter_map(|s| s.split('>').next())
        .collect();

    let mut ra_idx = None;
    let mut dec_idx = None;
    let mut mag_idx = None;

    for (i, field) in fields.iter().enumerate() {
        let field_lower = field.to_lowercase();
        if field_lower.contains("name=\"ra") || field_lower.contains("name=\"_ra") {
            ra_idx = Some(i);
        } else if field_lower.contains("name=\"de") || field_lower.contains("name=\"_de") {
            dec_idx = Some(i);
        } else if field_lower.contains("name=\"gmag")
            || field_lower.contains("name=\"phot_g_mean_mag")
            || field_lower.contains("name=\"mag")
        {
            mag_idx = Some(i);
        }
    }

    // Default column positions if not found explicitly (VizieR Gaia standard order)
    let ra_col_idx = ra_idx.unwrap_or(0);
    let dec_col_idx = dec_idx.unwrap_or(1);
    let mag_col_idx = mag_idx.unwrap_or(5); // Gmag is typically column 5 in VizieR Gaia

    // Find TABLEDATA section
    let tabledata_start = votable
        .find("<TABLEDATA>")
        .ok_or_else(|| CatalogError::InvalidResponse("No TABLEDATA found".to_string()))?;
    let tabledata_end = votable
        .find("</TABLEDATA>")
        .ok_or_else(|| CatalogError::InvalidResponse("No TABLEDATA end found".to_string()))?;

    let tabledata = &votable[tabledata_start..tabledata_end];

    // Parse each TR (row)
    for row in tabledata.split("<TR>").skip(1) {
        let row_end = row.find("</TR>").unwrap_or(row.len());
        let row_data = &row[..row_end];

        // Extract TD values
        let values: Vec<&str> = row_data
            .split("<TD>")
            .skip(1)
            .filter_map(|td| td.split("</TD>").next())
            .map(|v| v.trim())
            .collect();

        if values.len() <= ra_col_idx.max(dec_col_idx).max(mag_col_idx) {
            continue;
        }

        // Parse values
        let ra = values[ra_col_idx].parse::<f64>().ok();
        let dec = values[dec_col_idx].parse::<f64>().ok();
        let mag = values[mag_col_idx].parse::<f64>().ok();

        if let (Some(ra), Some(dec), Some(mag)) = (ra, dec, mag)
            && mag <= mag_limit
        {
            stars.push(CatalogStar::new(ra, dec, mag));
        }
    }

    if stars.is_empty() {
        return Err(CatalogError::NoStarsFound);
    }

    // Sort by magnitude (brightest first)
    stars.sort_by(|a, b| {
        a.mag
            .partial_cmp(&b.mag)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(stars)
}

/// Compute angular separation between two sky positions in degrees.
pub fn angular_separation(ra1: f64, dec1: f64, ra2: f64, dec2: f64) -> f64 {
    let ra1_rad = ra1.to_radians();
    let dec1_rad = dec1.to_radians();
    let ra2_rad = ra2.to_radians();
    let dec2_rad = dec2.to_radians();

    let delta_ra = ra2_rad - ra1_rad;
    let (sin_dec1, cos_dec1) = dec1_rad.sin_cos();
    let (sin_dec2, cos_dec2) = dec2_rad.sin_cos();

    // Vincenty formula for better numerical stability
    let term1 = (cos_dec2 * delta_ra.sin()).powi(2);
    let term2 = (cos_dec1 * sin_dec2 - sin_dec1 * cos_dec2 * delta_ra.cos()).powi(2);
    let numerator = (term1 + term2).sqrt();
    let denominator = sin_dec1 * sin_dec2 + cos_dec1 * cos_dec2 * delta_ra.cos();

    numerator.atan2(denominator).to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_catalog_star_new() {
        let star = CatalogStar::new(180.0, 45.0, 10.5);
        assert!((star.ra - 180.0).abs() < 1e-10);
        assert!((star.dec - 45.0).abs() < 1e-10);
        assert!((star.mag - 10.5).abs() < 1e-10);
        assert!(star.id.is_none());
    }

    #[test]
    fn test_catalog_star_with_id() {
        let star = CatalogStar::with_id(180.0, 45.0, 10.5, "Gaia DR3 12345");
        assert_eq!(star.id, Some("Gaia DR3 12345".to_string()));
    }

    #[test]
    fn test_angular_separation_same_point() {
        let sep = angular_separation(180.0, 45.0, 180.0, 45.0);
        assert!(sep.abs() < 1e-10);
    }

    #[test]
    fn test_angular_separation_poles() {
        // North to south pole should be 180 degrees
        let sep = angular_separation(0.0, 90.0, 0.0, -90.0);
        assert!((sep - 180.0).abs() < 1e-10);
    }

    #[test]
    fn test_angular_separation_one_degree() {
        // Points 1 degree apart along equator
        let sep = angular_separation(0.0, 0.0, 1.0, 0.0);
        assert!((sep - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_preloaded_catalog() {
        let stars = vec![
            CatalogStar::new(180.0, 45.0, 8.0),
            CatalogStar::new(180.1, 45.1, 10.0),
            CatalogStar::new(180.2, 45.2, 12.0),
            CatalogStar::new(190.0, 45.0, 9.0), // Far away
        ];

        let source = CatalogSource::preloaded(stars);

        // Query within 0.5 degree radius, mag limit 11
        let result = source.query_region(180.0, 45.0, 0.5, 11.0).unwrap();

        assert_eq!(result.len(), 2);
        assert!((result[0].mag - 8.0).abs() < 1e-10); // Brightest first
        assert!((result[1].mag - 10.0).abs() < 1e-10);
    }

    #[test]
    fn test_preloaded_catalog_no_stars() {
        let stars = vec![CatalogStar::new(0.0, 0.0, 8.0)];

        let source = CatalogSource::preloaded(stars);

        // Query far from the star
        let result = source.query_region(180.0, 45.0, 1.0, 15.0);
        assert!(matches!(result, Err(CatalogError::NoStarsFound)));
    }

    #[test]
    fn test_catalog_error_display() {
        let err = CatalogError::NetworkError("connection refused".to_string());
        assert_eq!(err.to_string(), "Network error: connection refused");

        let err = CatalogError::NoStarsFound;
        assert_eq!(err.to_string(), "No stars found in queried region");
    }
}
