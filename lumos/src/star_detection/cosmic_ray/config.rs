/// Configuration for L.A.Cosmic cosmic ray detection.
#[derive(Debug, Clone)]
pub struct LACosmicConfig {
    /// Sigma threshold for cosmic ray detection.
    /// Pixels with Laplacian > threshold * noise are flagged.
    /// Typical value: 4.0-5.0
    pub sigma_clip: f32,
    /// Object detection limit (sigma above sky).
    /// Pixels below this are considered background.
    /// Typical value: 4.0-5.0
    pub obj_lim: f32,
    /// Fine structure growth radius in pixels.
    /// Controls how much to grow detected regions.
    /// Typical value: 1
    pub grow_radius: usize,
}

impl Default for LACosmicConfig {
    fn default() -> Self {
        Self {
            sigma_clip: 4.5,
            obj_lim: 5.0,
            grow_radius: 1,
        }
    }
}
