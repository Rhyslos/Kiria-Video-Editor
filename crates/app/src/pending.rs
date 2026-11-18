//! A value being dragged (or typed) and not yet an undo step: held while
//! the hand is on it, shown at once, and finished when the hand lets go or
//! the panel turns to something else.
//!
//! Finishing always compares the value with what the project holds for its
//! key and hands it back only when they differ, so a drag that ends where it
//! began, or one left hanging on a clip no longer shown, never makes an undo
//! step that changes nothing.

/// A value being set on `K` (a clip, say), not yet an undo step.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending<K, V> {
    waiting: Option<(K, V)>,
}

impl<K, V> Default for Pending<K, V> {
    fn default() -> Self {
        Pending { waiting: None }
    }
}

impl<K: PartialEq, V: PartialEq> Pending<K, V> {
    /// Holds `value` for `key`, in place of whatever was held.
    pub fn set(&mut self, key: K, value: V) {
        self.waiting = Some((key, value));
    }

    /// Lets go of what is held without finishing it.
    pub fn clear(&mut self) {
        self.waiting = None;
    }

    /// What is held, if anything.
    pub fn get(&self) -> Option<(&K, &V)> {
        self.waiting.as_ref().map(|(key, value)| (key, value))
    }

    /// The value held for `key`; `None` when nothing is, or it is for
    /// another key.
    pub fn value_for(&self, key: &K) -> Option<&V> {
        self.get()
            .filter(|(held, _)| *held == key)
            .map(|(_, value)| value)
    }

    /// Ends what is held: it comes back when it differs from `stored`, what
    /// the project holds for its key (`None` when that is gone, which
    /// changes nothing).
    pub fn finish(&mut self, stored: impl FnOnce(&K) -> Option<V>) -> Option<(K, V)> {
        let (key, value) = self.waiting.take()?;
        let differs = stored(&key).is_some_and(|stored| stored != value);
        differs.then_some((key, value))
    }

    /// Ends what is held once the hand is off it (`busy` false); see
    /// [`Pending::finish`].
    pub fn finish_if_released(&mut self, busy: bool, stored: impl FnOnce(&K) -> Option<V>) -> Option<(K, V)> {
        if busy {
            return None;
        }
        self.finish(stored)
    }

    /// Ends what is held when its key is no longer the one shown (`shown`
    /// false for it): another clip was chosen mid-drag. See
    /// [`Pending::finish`].
    pub fn finish_on_change(
        &mut self,
        shown: impl FnOnce(&K) -> bool,
        stored: impl FnOnce(&K) -> Option<V>,
    ) -> Option<(K, V)> {
        match &self.waiting {
            Some((key, _)) if !shown(key) => self.finish(stored),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the "project" holds: key 1 holds 10, key 2 holds 20.
    fn stored(key: &u32) -> Option<i32> {
        match key {
            1 => Some(10),
            2 => Some(20),
            _ => None,
        }
    }

    #[test]
    fn a_value_back_where_it_began_finishes_with_nothing() {
        let mut pending = Pending::default();
        pending.set(1, 10);
        assert_eq!(pending.finish(stored), None);
        assert_eq!(pending.get(), None, "let go all the same");
        // A key the project no longer has changes nothing either.
        pending.set(3, 5);
        assert_eq!(pending.finish(stored), None);
    }

    #[test]
    fn a_drag_is_one_value_once_let_go() {
        let mut pending = Pending::default();
        for value in [11, 12, 13] {
            pending.set(1, value);
            assert_eq!(pending.finish_if_released(true, stored), None, "still held");
            assert_eq!(pending.value_for(&1), Some(&value));
            assert_eq!(pending.value_for(&2), None);
        }
        assert_eq!(pending.finish_if_released(false, stored), Some((1, 13)));
        assert_eq!(pending.finish_if_released(false, stored), None, "only once");
    }

    #[test]
    fn choosing_another_key_mid_drag_finishes_the_old_one() {
        let mut pending = Pending::default();
        pending.set(1, 15);
        assert_eq!(pending.finish_on_change(|key| *key == 1, stored), None);
        assert_eq!(pending.value_for(&1), Some(&15), "still the one shown: kept");
        assert_eq!(pending.finish_on_change(|key| *key == 2, stored), Some((1, 15)));
        assert_eq!(pending.get(), None);
        // Left where it began: finished with nothing.
        pending.set(1, 10);
        assert_eq!(pending.finish_on_change(|key| *key == 2, stored), None);
        assert_eq!(pending.get(), None);
    }
}
