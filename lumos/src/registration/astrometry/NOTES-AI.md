# Astrometry Module - AI Implementation Notes

## Overview

This module provides astrometric plate solving functionality, computing WCS (World Coordinate System) coordinates from detected stars by matching them against star catalogs.

## Research Summary (2026-01-27)

### Star Catalogs

#### Gaia DR3 (Primary Recommended)
- **Coverage**: All-sky, 1.8+ billion sources
- **Accuracy**: Sub-milliarcsecond astrometry for bright stars
- **Magnitude range**: ~3 to 21 mag
- **Released**: June 13, 2022
- **Future**: Gaia DR4 expected December 2026

**Access Methods**:
1. **VizieR Cone Search**: `http://vizier.unistra.fr/viz-bin/conesearch/I/355/gaiadr3?RA=<ra>&DEC=<dec>&SR=<radius>`
2. **ESA Gaia Archive TAP**: `https://gea.esac.esa.int/tap-server/tap`
3. **STScI MAST TAP**: `https://mast.stsci.edu/vo-tap/api/v0.1/gaiadr3/`
4. **NOIRLab Data Lab**: `https://datalab.noirlab.edu/tap`

**Response Formats**: VOTable, FITS, CSV, JSON

#### UCAC4 (Alternative/Offline)
- **Coverage**: All-sky, 113 million sources
- **Accuracy**: 15-20 mas for 10-14 mag stars
- **Magnitude range**: 8-16 mag
- **Size**: ~9GB (binary zone files)

**Data Format**:
- 900 binary zone files (0.2° declination zones)
- 78-byte fixed-length records per star
- Intel byte order
- Directory structure: `u4b/z001-z900`, `u4i/` (index files)
- Star ID format: `UCAC4-zzz-nnnnnn`

### Plate Solving Algorithms

#### Quad/Tetrahedron Geometric Hashing (Astrometry.net / ASTAP)
Most successful approach for blind plate solving:

1. **Quad Formation**: Select 4 closest stars, form tetrahedron
2. **Hash Code Computation**:
   - Define local coordinate system with stars A,B as origin and (1,1)
   - Compute positions of C,D in this coordinate system
   - Hash = (xC, yC, xD, yD) - 4D continuous code
   - Alternative: 6 normalized distance ratios
3. **Properties**:
   - Invariant to translation, rotation, scale
   - ~0.01 uniqueness threshold for matching
4. **Matching**:
   - Build k-d tree index of catalog quads
   - Query with image quad hash codes
   - Verify with Bayesian decision test

#### Triangle Matching (Existing in lumos)
- Already implemented in `registration/triangle/`
- Uses geometric hashing with triangles
- 3 distance ratios per triangle
- Works well with initial position hint

### WCS (World Coordinate System) Format

Standard FITS header keywords for astrometric calibration:

#### Core Keywords
```
CTYPE1 = 'RA---TAN'     / Projection type (gnomonic/tangent plane)
CTYPE2 = 'DEC--TAN'
CRVAL1 = <ra_deg>       / RA at reference pixel (degrees)
CRVAL2 = <dec_deg>      / Dec at reference pixel (degrees)
CRPIX1 = <x_pix>        / Reference pixel X
CRPIX2 = <y_pix>        / Reference pixel Y
```

#### CD Matrix (Linear Transformation)
```
CD1_1 = <deg/pix>       / Partial derivative dRA/dx
CD1_2 = <deg/pix>       / Partial derivative dRA/dy
CD2_1 = <deg/pix>       / Partial derivative dDec/dx
CD2_2 = <deg/pix>       / Partial derivative dDec/dy
```

**Derived Quantities**:
- Pixel scale: `1/sqrt(CD1_1² + CD1_2²)` deg/pixel
- Rotation: `atan2(CD1_2, CD1_1)` radians
- If CD1_1 < 0: RA increases to the left

#### SIP Distortion (Optional)
```
A_ORDER = n             / Polynomial order for x distortion
B_ORDER = n             / Polynomial order for y distortion
A_p_q, B_p_q           / Distortion coefficients
AP_ORDER, BP_ORDER     / Inverse distortion
AP_p_q, BP_p_q         / Inverse coefficients
```

### Coordinate Transformations

#### Pixel to Sky (Standard Coordinates)
1. Apply CD matrix: `(xi, eta) = CD × (x - CRPIX1, y - CRPIX2)`
2. De-project from tangent plane to sphere
3. Convert to RA/Dec

#### Tangent Plane Projection (Gnomonic)
```
xi  = cos(dec) * sin(ra - ra0) / D
eta = (sin(dec) * cos(dec0) - cos(dec) * sin(dec0) * cos(ra - ra0)) / D
D   = sin(dec) * sin(dec0) + cos(dec) * cos(dec0) * cos(ra - ra0)
```

Inverse:
```
ra  = ra0 + atan2(xi, cos(dec0) - eta * sin(dec0))
dec = atan2((eta * cos(dec0) + sin(dec0)) * cos(ra - ra0), cos(dec0) - eta * sin(dec0))
```

## Proposed Implementation

### Module Structure
```
astrometry/
├── mod.rs              # Main API: PlateSolver, WCS, CatalogSource
├── wcs.rs              # WCS struct with pixel<->sky transforms
├── catalog.rs          # CatalogSource trait, GaiaCatalog, UCAC4Catalog
├── quad_hash.rs        # Quad formation and geometric hashing
├── solver.rs           # PlateSolver with solve() method
├── tests.rs            # Unit and integration tests
└── NOTES-AI.md         # This file
```

### Key Types

```rust
/// World Coordinate System solution
#[derive(Debug, Clone)]
pub struct Wcs {
    pub crpix: (f64, f64),      // Reference pixel
    pub crval: (f64, f64),      // Reference sky coords (deg)
    pub cd: [[f64; 2]; 2],      // CD matrix
    pub ctype: (String, String), // Projection type
    // Optional SIP distortion
    pub sip_a: Option<Vec<Vec<f64>>>,
    pub sip_b: Option<Vec<Vec<f64>>>,
}

/// Catalog star entry
#[derive(Debug, Clone)]
pub struct CatalogStar {
    pub ra: f64,        // Right ascension (degrees)
    pub dec: f64,       // Declination (degrees)
    pub mag: f64,       // Magnitude
    pub id: Option<String>,
}

/// Plate solving configuration
#[derive(Debug, Clone)]
pub struct PlateSolverConfig {
    pub catalog: CatalogSource,
    pub search_radius: f64,     // Field search radius (degrees)
    pub mag_limit: f64,         // Faintest magnitude to use
    pub quad_size_range: (f64, f64), // Min/max quad scale
    pub match_tolerance: f64,   // Hash matching tolerance
    pub min_matches: usize,     // Minimum quad matches for solution
}

/// Catalog source
pub enum CatalogSource {
    Gaia { endpoint: String },
    Ucac4 { data_dir: PathBuf },
    Custom(Box<dyn CatalogProvider>),
}

pub trait CatalogProvider: Send + Sync {
    fn query_region(&self, ra: f64, dec: f64, radius: f64, mag_limit: f64)
        -> Result<Vec<CatalogStar>, CatalogError>;
}
```

### Solving Algorithm

1. **Detect image stars** (existing `find_stars()`)
2. **Query catalog** for approximate field center
3. **Build quad hashes** for both image and catalog stars
4. **Match quads** using k-d tree nearest neighbor
5. **Vote for transformation** - accumulate matching quad pairs
6. **RANSAC refinement** on best hypothesis
7. **Compute WCS** from final transformation
8. **Verify** - residuals should be << 1 arcsec

### Integration Points

- Reuse `spatial::KdTree` for hash code indexing
- Reuse `ransac::RansacEstimator` for robust fitting
- Compatible with existing `TransformMatrix`
- Output WCS can update `AstroImageMetadata`

## References

- [Astrometry.net Paper](https://www.researchgate.net/publication/45877878_Astrometrynet_Blind_astrometric_calibration_of_arbitrary_astronomical_images)
- [ASTAP Algorithm](https://www.hnsky.org/astap_astrometric_solving.htm)
- [WCS Standard (FITS)](https://www.aanda.org/articles/aa/full/2002/45/aah3859/aah3859.right.html)
- [Gaia at VizieR](https://vizier.cds.unistra.fr/viz-bin/VizieR-2?-source=gaia-dr3)
- [UCAC4 Documentation](https://irsa.ipac.caltech.edu/data/USNO/UCAC4/readme_u4.txt)
- [Siril Plate Solving](https://siril.readthedocs.io/en/stable/astrometry/platesolving.html)
