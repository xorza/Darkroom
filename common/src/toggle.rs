pub trait Toggle {
    fn toggle(&mut self) -> bool;
}

impl Toggle for bool {
    fn toggle(&mut self) -> bool {
        let was = *self;
        *self = !was;
        was
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toggle() {
        let mut toggle = false;
        assert!(!toggle.toggle());
        assert!(toggle);
        assert!(toggle.toggle());
        assert!(!toggle);
    }
}
