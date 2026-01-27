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
    Star, StarDetectionConfig, StarDetectionConfigBuilder, StarDetectionDiagnostics,
    StarDetectionResult, find_stars,
};

// Star detection - background estimation
pub use crate::{
    BackgroundMap, IterativeBackgroundConfig, estimate_background, estimate_background_image,
    estimate_background_iterative, estimate_background_iterative_image,
};

// Registration - main API
pub use crate::{
    InterpolationMethod, RegistrationConfig, RegistrationConfigBuilder, RegistrationError,
    RegistrationResult, Registrator, TransformMatrix, TransformType, WarpConfig,
};

// Registration - convenience functions
pub use crate::{
    quick_register, register_star_positions, warp_to_reference, warp_to_reference_image,
};

// Stacking - main API
pub use crate::{
    CacheConfig, FrameType, ImageStack, MedianConfig, ProgressCallback, SigmaClipConfig,
    SigmaClippedConfig, StackingMethod, StackingProgress, StackingStage,
};

// Live stacking
pub use crate::{
    LiveFrameQuality, LiveStackAccumulator, LiveStackConfig, LiveStackConfigBuilder,
    LiveStackError, LiveStackMode,
};

// Multi-session stacking
pub use crate::{MultiSessionStack, Session, SessionConfig};

// Gradient removal
pub use crate::{
    GradientRemovalConfig, GradientRemovalError, remove_gradient, remove_gradient_image,
};
