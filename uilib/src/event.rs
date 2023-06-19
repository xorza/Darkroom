use glam::UVec2;

pub enum Event {
    Resize {
        size: UVec2,
    },
    Unknown,
}

#[derive(PartialEq, Debug)]
pub enum EventResult {
    Continue,
    Exit,
}

