use std::ops::Mul;

use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub(crate) struct Transform2D {
    r0: [f32; 3],
    pad0: f32,
    r1: [f32; 3],
    pad1: f32,
    r2: [f32; 3],
    pad2: f32,
}

impl Default for Transform2D {
    fn default() -> Self {
        Self {
            r0: [1.0, 0.0, 0.0],
            r1: [0.0, 1.0, 0.0],
            r2: [0.0, 0.0, 1.0],

            pad0: 0.0,
            pad1: 0.0,
            pad2: 0.0,
        }
    }
}

impl Transform2D {
    pub fn aspect(&mut self, width: u32, height: u32) -> &mut Self {
        self.r0[0] *= width as f32 / height as f32;

        self
    }
    pub fn scale(&mut self, x: f32, y: f32) -> &mut Self {
        self.r0[0] *= x;
        self.r1[1] *= y;

        self
    }
    pub fn rotate(&mut self, angle: f32) -> &mut Self {
        let c = angle.cos();
        let s = angle.sin();

        let r0 = self.r0;
        let r1 = self.r1;

        self.r0[0] = r0[0] * c + r1[0] * s;
        self.r0[1] = r0[1] * c + r1[1] * s;
        self.r0[2] = r0[2] * c + r1[2] * s;

        self.r1[0] = r1[0] * c - r0[0] * s;
        self.r1[1] = r1[1] * c - r0[1] * s;
        self.r1[2] = r1[2] * c - r0[2] * s;

        self
    }
    pub fn translate(&mut self, x: f32, y: f32) -> &mut Self {
        self.r2[0] += self.r0[0] * x + self.r1[0] * y;
        self.r2[1] += self.r0[1] * x + self.r1[1] * y;
        self.r2[2] += self.r0[2] * x + self.r1[2] * y;

        self
    }
    pub fn center(&mut self) -> &mut Self {
        self.translate(0.5, 0.5)
    }
    pub fn uncenter(&mut self) -> &mut Self {
        self.translate(-0.5, -0.5)
    }

    pub fn size_in_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}

impl<'a, 'b> Mul<&'b Transform2D> for &'a Transform2D {
    type Output = Transform2D;

    fn mul(self, rhs: &'b Transform2D) -> Self::Output {
        Transform2D {
            r0: [
                self.r0[0] * rhs.r0[0] + self.r0[1] * rhs.r1[0] + self.r0[2] * rhs.r2[0],
                self.r0[0] * rhs.r0[1] + self.r0[1] * rhs.r1[1] + self.r0[2] * rhs.r2[1],
                self.r0[0] * rhs.r0[2] + self.r0[1] * rhs.r1[2] + self.r0[2] * rhs.r2[2],
            ],
            r1: [
                self.r1[0] * rhs.r0[0] + self.r1[1] * rhs.r1[0] + self.r1[2] * rhs.r2[0],
                self.r1[0] * rhs.r0[1] + self.r1[1] * rhs.r1[1] + self.r1[2] * rhs.r2[1],
                self.r1[0] * rhs.r0[2] + self.r1[1] * rhs.r1[2] + self.r1[2] * rhs.r2[2],
            ],
            r2: [
                self.r2[0] * rhs.r0[0] + self.r2[1] * rhs.r1[0] + self.r2[2] * rhs.r2[0],
                self.r2[0] * rhs.r0[1] + self.r2[1] * rhs.r1[1] + self.r2[2] * rhs.r2[1],
                self.r2[0] * rhs.r0[2] + self.r2[1] * rhs.r1[2] + self.r2[2] * rhs.r2[2],
            ],
            pad0: 0.0,
            pad1: 0.0,
            pad2: 0.0,
        }
    }
}

impl PartialEq for Transform2D {
    fn eq(&self, other: &Self) -> bool {
        self.r0 == other.r0
            && self.r1 == other.r1
            && self.r2 == other.r2
        // Pads are not compared, as they hold no meaningful information
    }
}

impl Eq for Transform2D {}


#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub(crate) struct Vert2D(pub [f32; 2], pub [f32; 2]);

impl Vert2D {
    pub fn rect_one() -> [Vert2D; 4] {
        [
            // @formatter:off
            Vert2D([-1.0, -1.0], [0.0, 0.0]),
            Vert2D([ 1.0, -1.0], [1.0, 0.0]),
            Vert2D([-1.0,  1.0], [0.0, 1.0]),
            Vert2D([ 1.0,  1.0], [1.0, 1.0]),
            // @formatter:on
        ]
    }
}
