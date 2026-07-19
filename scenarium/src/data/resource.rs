use crate::{CancelToken, DynamicValue};

pub trait ResourceStamper: Send + Sync + std::fmt::Debug {
    /// Implementations doing non-trivial work should poll `cancel` and return promptly.
    /// A stamp returned after cancellation is discarded.
    fn stamp(&self, value: &DynamicValue, cancel: &CancelToken) -> ResourceStamp;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResourceStamp {
    bytes: [u8; Self::CAPACITY],
    len: u8,
}

impl ResourceStamp {
    pub const CAPACITY: usize = 32;

    pub fn from_bytes(bytes: &[u8]) -> Self {
        assert!(
            bytes.len() <= Self::CAPACITY,
            "resource stamp exceeds {} bytes — hash wide identities down",
            Self::CAPACITY
        );
        let mut stamp = ResourceStamp::default();
        stamp.bytes[..bytes.len()].copy_from_slice(bytes);
        stamp.len = bytes.len() as u8;
        stamp
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_stamp_packs_exact_bytes_inline() {
        assert_eq!(ResourceStamp::from_bytes(&[1, 2, 3]).as_bytes(), [1, 2, 3]);
        assert_eq!(ResourceStamp::from_bytes(&[]).as_bytes(), [0u8; 0]);
        let full = (0..32).collect::<Vec<_>>();
        assert_eq!(ResourceStamp::from_bytes(&full).as_bytes(), &full);
        assert_ne!(
            ResourceStamp::from_bytes(&[1]),
            ResourceStamp::from_bytes(&[1, 0])
        );
        assert!(std::panic::catch_unwind(|| ResourceStamp::from_bytes(&[0; 33])).is_err());
    }
}
