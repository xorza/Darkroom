use std::any::Any;

#[derive(Debug, Default)]
pub struct AnyState {
    boxed: Option<Box<dyn Any + Send>>,
}

impl AnyState {
    pub fn is_none(&self) -> bool {
        self.boxed.is_none()
    }

    pub fn is_some<T>(&self) -> bool
    where
        T: Any + Send,
    {
        match &self.boxed {
            None => false,
            Some(v) => v.is::<T>(),
        }
    }

    pub fn get<T>(&self) -> Option<&T>
    where
        T: Any + Send,
    {
        self.boxed
            .as_ref()
            .and_then(|boxed| boxed.downcast_ref::<T>())
    }

    pub fn get_mut<T>(&mut self) -> Option<&mut T>
    where
        T: Any + Send,
    {
        self.boxed
            .as_mut()
            .and_then(|boxed| boxed.downcast_mut::<T>())
    }

    pub fn set<T>(&mut self, value: T)
    where
        T: Any + Send,
    {
        self.boxed = Some(Box::new(value));
    }

    /// Drop the held value (whatever its type), releasing anything it owns —
    /// e.g. a watcher whose RAII guard keeps an OS resource open.
    pub fn clear(&mut self) {
        self.boxed = None;
    }

    /// The held `T`, seeding an **empty** slot with `T::default()`.
    ///
    /// Panics if the slot already holds a different type — see
    /// [`Self::get_or_insert_with`].
    pub fn get_or_default<T>(&mut self) -> &mut T
    where
        T: Any + Send + Default,
    {
        self.get_or_insert_with(T::default)
    }

    /// The held `T`, seeding an **empty** slot from `make`.
    ///
    /// # Panics
    ///
    /// If the slot holds some other type. A populated slot of the wrong
    /// type is not an empty slot: state is keyed by its owning function,
    /// so a mismatch means two owners are reading one slot
    /// — a wiring bug in the caller, not a runtime condition. Treating
    /// it as empty would drop whatever the other owner put there (an
    /// RAII watcher guard, an open handle, accumulated progress) and
    /// hand back a freshly defaulted value, so the symptom surfaces
    /// later as state that silently forgets itself.
    ///
    /// The downcast this checks is one the call already performs, so
    /// the guard costs nothing beyond the branch that was there.
    pub fn get_or_insert_with<T>(&mut self, make: impl FnOnce() -> T) -> &mut T
    where
        T: Any + Send,
    {
        match self.boxed.as_ref() {
            Some(boxed) => assert!(
                boxed.is::<T>(),
                "AnyState holds another type; requested {}",
                std::any::type_name::<T>(),
            ),
            None => self.boxed = Some(Box::new(make())),
        }
        self.boxed
            .as_mut()
            .expect("slot filled above")
            .downcast_mut::<T>()
            .expect("type checked above")
    }
}

#[cfg(test)]
mod tests {
    use crate::runtime::any_state::AnyState;

    /// An empty slot seeds; a populated slot of the right type is handed
    /// back **as it stands**, which is the whole point of the helper —
    /// a re-seed here would reset accumulated state every invocation.
    #[test]
    fn seeding_fills_only_an_empty_slot() {
        let mut state = AnyState::default();
        assert!(state.is_none());

        *state.get_or_default::<u32>() += 7;
        assert_eq!(
            *state.get_or_default::<u32>(),
            7,
            "second call must not reseed"
        );
        assert!(state.is_some::<u32>());

        // `clear` releases whatever was held, so the slot seeds again.
        state.clear();
        assert_eq!(*state.get_or_insert_with(|| 41u32), 41);
        assert_eq!(*state.get_or_insert_with(|| 0u32), 41);
    }

    /// A populated slot of the wrong type is a caller wiring bug, not an
    /// empty slot: state is keyed by its owning function, so a mismatch
    /// means two owners share a slot. Seeding over it would drop the
    /// other owner's value and return a silently reset default.
    #[test]
    #[should_panic(expected = "AnyState holds another type")]
    fn seeding_over_a_populated_mismatch_panics() {
        let mut state = AnyState::default();
        state.set(String::from("owned by someone else"));
        state.get_or_default::<u32>();
    }

    /// The non-defaulting twin refuses identically — the guard lives in
    /// the shared body, so neither entry point can drift past it.
    #[test]
    #[should_panic(expected = "AnyState holds another type")]
    fn seeding_with_a_closure_over_a_mismatch_panics() {
        let mut state = AnyState::default();
        state.set(7u32);
        state.get_or_insert_with(String::new);
    }

    /// The non-seeding readers stay fallible: a mismatch there is a
    /// question with an answer ("not this type"), not a wiring bug.
    #[test]
    fn plain_reads_report_a_mismatch_rather_than_panicking() {
        let mut state = AnyState::default();
        state.set(9u32);
        assert_eq!(state.get::<u32>(), Some(&9));
        assert_eq!(state.get::<String>(), None);
        assert_eq!(state.get_mut::<String>(), None);
        assert!(!state.is_some::<String>());
    }
}
