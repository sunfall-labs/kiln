//! Weak allocation census for interpreter object graphs.
//!
//! `Rc` cycles have no external root to discover them from. These registries keep only weak
//! identities, so the collector can start a graph walk at every object and array allocated on
//! this interpreter thread without making the objects live. `Rc` values are not `Send`, which
//! keeps each registry confined to the thread that owns its realms.

use super::{ArrayData, ObjectData};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

struct WeakCensus<T> {
    entries: Vec<Weak<T>>,
    /// Counts only the weak references owned by `entries`, indexed by stable allocation
    /// identity. These are excluded from actor move-eligibility checks; user WeakRefs remain
    /// observable and still prevent moving a graph.
    internal_weak_counts: HashMap<usize, usize>,
    prune_at: usize,
}

impl<T> Default for WeakCensus<T> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            internal_weak_counts: HashMap::new(),
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
        *self
            .internal_weak_counts
            .entry(Rc::as_ptr(value) as usize)
            .or_default() += 1;
    }

    fn prune(&mut self) {
        let entries = &mut self.entries;
        let counts = &mut self.internal_weak_counts;
        entries.retain(|entry| {
            if entry.strong_count() > 0 {
                return true;
            }
            let identity = entry.as_ptr() as usize;
            if let Some(count) = counts.get_mut(&identity) {
                *count -= 1;
                if *count == 0 {
                    counts.remove(&identity);
                }
            }
            false
        });
        self.prune_at = self
            .entries
            .len()
            .saturating_add(self.entries.len().max(1024));
    }

    fn snapshot(&self) -> Vec<Weak<T>> {
        self.entries.clone()
    }

    fn internal_weak_counts(&self) -> HashMap<usize, usize> {
        self.internal_weak_counts.clone()
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

pub(crate) fn internal_object_weak_counts() -> HashMap<usize, usize> {
    CENSUS.with(|census| census.borrow().objects.internal_weak_counts())
}

pub(crate) fn internal_array_weak_counts() -> HashMap<usize, usize> {
    CENSUS.with(|census| census.borrow().arrays.internal_weak_counts())
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
        assert_eq!(
            census
                .internal_weak_counts()
                .get(&(Rc::as_ptr(&live) as usize)),
            Some(&1)
        );
        assert_eq!(Rc::weak_count(&live), 1);
        assert!(census.entries.len() <= 1025);
        census.prune();
        assert_eq!(census.entries.len(), 1);
        assert_eq!(
            census
                .internal_weak_counts()
                .get(&(Rc::as_ptr(&live) as usize)),
            Some(&1)
        );
        drop(live);
        census.prune();
        assert!(census.entries.is_empty());
        assert_eq!(census.internal_weak_counts.len(), 0);
    }

    #[test]
    fn pruning_removes_all_duplicate_weak_edges_for_an_expired_identity() {
        let mut census = WeakCensus::<usize>::default();
        let repeated = Rc::new(7);
        let identity = Rc::as_ptr(&repeated) as usize;
        census.register(&repeated);
        census.register(&repeated);

        let snapshot = census.internal_weak_counts();
        assert_eq!(snapshot.get(&identity), Some(&2));
        assert_eq!(Rc::weak_count(&repeated), 2);
        assert_eq!(Rc::strong_count(&repeated), 1);
        census.prune();
        assert_eq!(
            census.internal_weak_counts.get(&identity),
            Some(&2),
            "pruning must preserve duplicate entries while their Rc allocation is alive"
        );
        assert_eq!(Rc::weak_count(&repeated), 2);

        drop(repeated);
        census.prune();
        assert!(census.entries.is_empty());
        assert!(!census.internal_weak_counts.contains_key(&identity));
        // Numeric count snapshots own neither Weak nor Rc references and remain stable when the
        // census removes expired registration entries.
        assert_eq!(snapshot.get(&identity), Some(&2));
    }
}
