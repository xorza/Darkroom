//! [`DrizzleKernel`]: how a drop of flux spreads over the output grid.

/// Drizzle kernel type for distributing flux.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DrizzleKernel {
    /// Square kernel: the exact overlap area of each drop's transformed quadrilateral with every
    /// output pixel, by STScI's `boxer` / `sgarea` edge integration. Correct for any transform,
    /// rotation and shear included. Default, as in DrizzlePac, PixInsight and Siril.
    /// Reference: `STScI` cdrizzlebox.c `do_kernel_square` / `boxer` / `sgarea`.
    #[default]
    Square,
    /// Turbo kernel: axis-aligned rectangular drop centered on the transformed pixel center.
    /// Approximation of true square kernel — always aligned with output X/Y axes regardless
    /// of rotation, so it is exact only at rotations of 0°, 90° and 180°.
    /// (Named "turbo" in `STScI` `DrizzlePac`; "square" there integrates the transformed polygon's
    /// edges, as [`Self::Square`] does.)
    Turbo,
    /// Point kernel - single pixel contribution.
    /// Fastest but requires very good dithering.
    Point,
    /// Gaussian droplet whose FWHM is the drop size, `pixfrac·scale` output pixels, as in
    /// `STScI` drizzle. Smoother output, slight flux redistribution.
    Gaussian,
    /// A Lanczos interpolation of the frame at each output pixel. Only valid at pixfrac = 1 and
    /// scale = 1, where it resamples rather than drizzles, and the slowest kernel. Its weights go
    /// negative, so it rings at sharp edges as any Lanczos resampling does.
    Lanczos,
}

impl DrizzleKernel {
    /// Every kernel.
    pub const ALL: [Self; 5] = [
        Self::Square,
        Self::Turbo,
        Self::Point,
        Self::Gaussian,
        Self::Lanczos,
    ];
}
