//! The ingest stage: how each entry point reads its frames and where it parks them.

pub(crate) mod error;
pub(crate) mod frame_admission;
pub(crate) mod frame_check;
pub(crate) mod frame_intake;
pub(crate) mod ingest_config;
pub(crate) mod ingest_run;
pub(crate) mod set_facts;
