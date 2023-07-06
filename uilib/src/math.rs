use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable, Default)]
pub struct UVec2 {
    pub x: u32,
    pub y: u32,
}

impl UVec2 {
    pub fn new(x: u32, y: u32) -> Self {
        Self { x, y }
    }
    pub fn all(v: u32) -> Self {
        Self { x: v, y: v }
    }
}


#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable, Default)]
pub struct UVec4 {
    pub x: u32,
    pub y: u32,
    pub z: u32,
    pub w: u32,
}

impl UVec4 {
    pub fn new(x: u32, y: u32, z: u32, w: u32) -> Self {
        Self { x, y, z, w }
    }
    pub fn all(v: u32) -> Self {
        Self { x: v, y: v, z: v, w: v }
    }
}


#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable, Default)]
pub struct FVec4 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl FVec4 {
    pub fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }
    pub fn all(v: f32) -> Self {
        Self { x: v, y: v, z: v, w: v }
    }
}


impl From<glam::UVec2> for UVec2 {
    fn from(v: glam::UVec2) -> Self {
        Self {
            x: v.x,
            y: v.y,
        }
    }
}

impl From<UVec2> for glam::UVec2 {
    fn from(v: UVec2) -> Self {
        Self {
            x: v.x,
            y: v.y,
        }
    }
}