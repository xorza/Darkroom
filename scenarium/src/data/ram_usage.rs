use std::ops::Add;
use std::ops::AddAssign;

/// Bytes a value holds, split between CPU and GPU memory.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RamUsage {
    pub cpu: usize,
    pub gpu: usize,
}

impl RamUsage {
    pub const fn total(&self) -> usize {
        self.cpu + self.gpu
    }
}

impl Add for RamUsage {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self {
            cpu: self.cpu + rhs.cpu,
            gpu: self.gpu + rhs.gpu,
        }
    }
}

impl AddAssign for RamUsage {
    fn add_assign(&mut self, rhs: Self) {
        self.cpu += rhs.cpu;
        self.gpu += rhs.gpu;
    }
}
