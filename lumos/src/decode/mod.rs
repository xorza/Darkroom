//! What identifies a decode: which decoder ran, at which version, under which options — what any
//! record of a decoded frame is keyed on, the decode cache's and a calibration master's alike.

pub(crate) mod decode_params;
pub(crate) mod decode_pins;
pub(crate) mod decoder_kind;
pub(crate) mod fnv1a;
