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