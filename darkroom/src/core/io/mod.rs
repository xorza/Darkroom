//! On-disk I/O and session state: zipped [`document`] archives, the
//! preferences and the per-document
//! disk-[`cache`] location.

pub(crate) mod cache;
pub(crate) mod document;
pub(crate) mod preferences;
