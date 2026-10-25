//! [`Capture`]: what a RAW file states of its exposure and its camera at capture.

/// What a RAW file states of its exposure and its camera at capture, as LibRaw reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Capture {
    /// The ISO speed, `0` where the file states none.
    pub iso_speed: f32,
    /// The shutter time in seconds, `0` where the file states none.
    pub shutter: f32,
    /// The lens's focal length in mm, `0` where the file states none.
    pub focal_len: f32,
    /// The sensor's temperature in °C its maker notes state; `-1000` where they state none.
    pub sensor_temperature: f32,
    /// The camera body's temperature in °C its maker notes state; `-1000` where they state none.
    pub camera_temperature: f32,
}
