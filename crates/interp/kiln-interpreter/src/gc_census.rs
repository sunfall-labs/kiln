//! Weak allocation census for interpreter object graphs.
//!
//! `Rc` cycles have no external root to discover them from. These registries keep only weak
//! identities, so the collector can start a graph walk at every object and array allocated on
//! this interpreter thread without making the objects live. `Rc` values are not `Send`, which
//! keeps each registry confined to the thread that owns its realms.

use super::{ArrayData, ObjectData};
use std::cell::RefCell;
use std::rc::{Rc, Weak};

struct WeakCensus<T> {
    entries: Vec<Weak<T>>,
    prune_at: usize,
}

impl<T> Default for WeakCensus<T> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            prune_at: 0,
        }
    }
}

impl<T> WeakCensus<T> {
    fn register(&mut self, value: &Rc<T>) {
        if self.entries.len() >= self.prune_at {
            self.prune();
        }
        self.entries.push(Rc::downgrade(value));
    }

    fn prune(&mut self) {
        self.entries.retain(|entry| entry.strong_count() > 0);
        self.prune_at = self
            .entries
            .len()
            .saturating_add(self.entries.len().max(1024));
    }

    fn snapshot(&self) -> Vec<Weak<T>> {
        self.entries.clone()
    }
}

#[derive(Default)]
struct ThreadCensus {
    objects: WeakCensus<RefCell<ObjectData>>,
    arrays: WeakCensus<ArrayData>,
}

thread_local! {
    static CENSUS: RefCell<ThreadCensus> = RefCell::new(ThreadCensus::default());
}

pub(crate) fn register_object(value: &Rc<RefCell<ObjectData>>) {
    CENSUS.with(|census| census.borrow_mut().objects.register(value));
}

pub(crate) fn tracked_object(data: ObjectData) -> Rc<RefCell<ObjectData>> {
    let object = Rc::new(RefCell::new(data));
    register_object(&object);
    object
}

pub(crate) fn register_array(value: &Rc<ArrayData>) {
    CENSUS.with(|census| census.borrow_mut().arrays.register(value));
}

pub(crate) fn object_candidates() -> Vec<Weak<RefCell<ObjectData>>> {
    CENSUS.with(|census| census.borrow().objects.snapshot())
}

pub(crate) fn array_candidates() -> Vec<Weak<ArrayData>> {
    CENSUS.with(|census| census.borrow().arrays.snapshot())
}

pub(crate) fn prune_dead_candidates() {
    CENSUS.with(|census| {
        let mut census = census.borrow_mut();
        census.objects.prune();
        census.arrays.prune();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn census_is_weak_and_prunes_expired_allocations() {
        let mut census = WeakCensus::<usize>::default();
        let live = Rc::new(42);
        census.register(&live);
        for value in 0..4096 {
            census.register(&Rc::new(value));
        }
        assert_eq!(Rc::strong_count(&live), 1);
        assert!(census.entries.len() <= 1025);
        census.prune();
        assert_eq!(census.entries.len(), 1);
        drop(live);
        census.prune();
        assert!(census.entries.is_empty());
    }
}
