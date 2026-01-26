//! Synthetic data generators for visual tests.
//!
//! This module re-exports from [`crate::testing::synthetic`] for backwards compatibility.
//! New code should use `crate::testing::synthetic` directly.

#![allow(dead_code)]

// Re-export everything from the centralized synthetic module
pub use crate::testing::synthetic::*;
