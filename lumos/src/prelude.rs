//! Prelude module for convenient imports.
//!
//! This module re-exports the most commonly used types and traits from the library.
//!
//! # Usage
//!
//! ```rust,ignore
//! use lumos::prelude::*;
//! ```

// Core image types
pub use crate::{AstroImage, AstroImageMetadata, BitPix, HotPixelMap, ImageDimensions};

// Calibration
pub use crate::CalibrationMasters;

// Star detection - main API
pub use crate::{Star, StarDetectionConfig, StarDetectionDiagnostics, StarDetectionResult};

// Star detection - background estimation
pub use crate::{BackgroundConfig, BackgroundMap};

// Registration - main API
pub use crate::{
    InterpolationMethod, RegistrationConfig, RegistrationError, RegistrationResult, Registrator,
    Transform, TransformType, WarpConfig,
};

// Registration - convenience functions
pub use crate::warp_to_reference_image;

// Stacking - main API
pub use crate::{
    CacheConfig, FrameType, ImageStack, MedianConfig, ProgressCallback, SigmaClipConfig,
    SigmaClippedConfig, StackingMethod, StackingProgress, StackingStage,
};

// Gradient removal
pub use crate::{
    GradientRemovalConfig, GradientRemovalError, remove_gradient, remove_gradient_image,
};
