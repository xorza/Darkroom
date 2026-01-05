pub trait BoolExt {
    fn then_else<T>(self, when_true: T, when_false: T) -> T;
    fn then_else_with<T, F, G>(self, when_true: F, when_false: G) -> T
    where
        F: FnOnce() -> T,
        G: FnOnce() -> T;
}

impl BoolExt for bool {
    fn then_else<T>(self, when_true: T, when_false: T) -> T {
        if self {
            when_true
        } else {
            when_false
        }
    }

    fn then_else_with<T, F, G>(self, when_true: F, when_false: G) -> T
    where
        F: FnOnce() -> T,
        G: FnOnce() -> T,
    {
        if self {
            when_true()
        } else {
            when_false()
        }
    }
}
