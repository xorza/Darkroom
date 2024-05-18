pub trait Toggle {
    fn toggle(&mut self) -> bool;
    fn on(&mut self) -> bool;
}

impl Toggle for bool {
    fn toggle(&mut self) -> bool {
        let was = *self;
        *self = !was;
        was
    }

    fn on(&mut self) -> bool {
        let was = *self;
        *self = true;
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

    #[test]
    fn test_on() {
        let mut toggle = false;
        assert!(!toggle.on());
        assert!(toggle);
        assert!(toggle.on());
        assert!(toggle);
    }
}
