//! [`Toggle`]: the state a toggle widget shows.

/// Whether a toggle widget shows its "on" state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Toggle {
    Off,
    On,
}

impl From<bool> for Toggle {
    fn from(on: bool) -> Self {
        if on { Self::On } else { Self::Off }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flag_maps_onto_its_state() {
        assert_eq!(
            (Toggle::from(false), Toggle::from(true)),
            (Toggle::Off, Toggle::On)
        );
    }
}
