//! The ingest stage: how each entry point reads its frames and where it parks them.

pub(crate) mod frame_admission;
pub(crate) mod frame_step;
pub(crate) mod ingest_config;
pub(crate) mod ingest_run;
