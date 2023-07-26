pub trait Apply<T, F>
where
    F: FnOnce(&T),
{
    fn apply(&self, f: F);
}
impl<T, F> Apply<T, F> for Option<&T>
where
    F: FnOnce(&T),
{
    fn apply(&self, f: F) {
        if let Some(v) = self {
            f(*v);
        }
    }
}

pub trait ApplyMut<T, F>
where
    F: FnOnce(&mut T),
{
    fn apply_mut(&mut self, f: F);
}
impl<T, F> ApplyMut<T, F> for Option<&mut T>
where
    F: FnOnce(&mut T),
{
    fn apply_mut(&mut self, f: F) {
        if let Some(v) = self {
            f(*v);
        }
    }
}

pub trait TakeWith<T, F>
where
    F: FnOnce(T),
{
    fn take_with(&mut self, f: F);
}
impl<T, F> TakeWith<T, F> for Option<T>
where
    F: FnOnce(T),
{
    fn take_with(&mut self, f: F) {
        if let Some(v) = self.take() {
            f(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply() {
        let x = Some(3);
        let mut y = 12;
        x
            .as_ref()
            .apply(|v| y += *v);
        assert_eq!(y, 15);
    }

    #[test]
    fn test_apply_mut() {
        let mut x = Some(1);
        x
            .as_mut()
            .apply_mut(|v| *v += 1);
        assert_eq!(x, Some(2));
    }

    #[test]
    fn test_take_with() {
        let mut x = Some(1);
        x.take_with(|v| assert_eq!(v, 1));
        assert_eq!(x, None);
    }
}