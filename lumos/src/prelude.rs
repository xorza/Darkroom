//! Prelude: the main pipeline API in one glob import.
//!
//! `use lumos::prelude::*` brings in the entry point for each stage — load, calibrate,
//! detect, register, stack, drizzle, and the end-to-end orchestrator — plus the core types
//! needed to call them. For the complete published surface (every config knob, error, and
//! diagnostic type) import from the crate root instead (`use lumos::...`).

// Core image + data types
pub use crate::{AstroImage, CfaImage, ImageDimensions, Star};

// Load / decode — plus `AstroImage::from_file` (method on the type above)
pub use crate::raw::load_raw_cfa;

// Calibration — `CalibrationMasters::{from_files, from_images, calibrate}`
pub use crate::CalibrationMasters;

// Detection
pub use crate::{StarDetectionConfig, StarDetector};

// Registration
pub use crate::{RegistrationConfig, Transform, WarpResult, WarpTransform, register, warp};

// Stacking
pub use crate::{ProgressCallback, StackConfig, stack, stack_images};

// Drizzle
pub use crate::{DrizzleConfig, drizzle_images, drizzle_stack};

// End-to-end: detect → register → warp → stack
pub use crate::{AlignStackConfig, AlignStackResult, Reference, align_and_stack};
