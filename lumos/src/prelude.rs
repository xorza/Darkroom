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
pub use crate::{
    BackgroundEstimate, Star, StarDetectionConfig, StarDetectionDiagnostics, StarDetectionResult,
};

// Registration - main API
pub use crate::{
    InterpolationMethod, RegistrationConfig, RegistrationError, RegistrationResult, Transform,
    TransformType,
};

// Registration - convenience functions
pub use crate::{register, register_positions, warp};

// Stacking - main API
pub use crate::{
    CacheConfig, FrameType, ImageStack, MedianConfig, ProgressCallback, SigmaClipConfig,
    SigmaClippedConfig, StackingMethod, StackingProgress, StackingStage,
};

// Gradient removal
pub use crate::{
    GradientRemovalConfig, GradientRemovalError, remove_gradient, remove_gradient_image,
};
