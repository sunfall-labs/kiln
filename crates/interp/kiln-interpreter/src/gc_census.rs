//! Weak allocation census for interpreter object graphs.
//!
//! `Rc` cycles have no external root to discover them from. These registries keep only weak
//! identities, so the collector can start a graph walk at every object and array allocated on
//! this interpreter thread without making the objects live. `Rc` values are not `Send`, which
//! keeps each registry confined to the thread that owns its realms.

use super::{ArrayData, ObjectData, ProxyData};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
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

    /// Count only the weak handles physically owned by this census. Actor transfer calls this
    /// infrequently; deriving the snapshot here avoids a HashMap lookup/update for every object
    /// registration. `as_ptr` is non-owning, and iterating under the caller's census borrow makes
    /// this a consistent snapshot without upgrading or cloning any weak handle.
    fn internal_weak_counts(&self) -> HashMap<usize, usize> {
        let mut counts = HashMap::new();
        for entry in &self.entries {
            *counts.entry(entry.as_ptr() as usize).or_default() += 1;
        }
        counts
    }
}

#[derive(Default)]
struct ThreadCensus {
    objects: WeakCensus<RefCell<ObjectData>>,
    arrays: WeakCensus<ArrayData>,
    proxies: WeakCensus<ProxyData>,
    allocations_since_collection: usize,
    next_collection_at: usize,
}

thread_local! {
    static CENSUS: RefCell<ThreadCensus> = RefCell::new(ThreadCensus::default());
    static ACTIVE_VM_RUNS: Cell<usize> = const { Cell::new(0) };
}

pub(crate) struct VmRunGuard {
    outermost: bool,
}

impl VmRunGuard {
    pub(crate) fn outermost(&self) -> bool {
        self.outermost
    }
}

impl Drop for VmRunGuard {
    fn drop(&mut self) {
        ACTIVE_VM_RUNS.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

pub(crate) fn enter_vm_run() -> VmRunGuard {
    let outermost = ACTIVE_VM_RUNS.with(|depth| {
        let previous = depth.get();
        depth.set(previous.saturating_add(1));
        previous == 0
    });
    VmRunGuard { outermost }
}

pub(crate) fn register_object(value: &Rc<RefCell<ObjectData>>) {
    CENSUS.with(|census| {
        let mut census = census.borrow_mut();
        census.objects.register(value);
        census.note_allocation();
    });
}

pub(crate) fn tracked_object(data: ObjectData) -> Rc<RefCell<ObjectData>> {
    let object = Rc::new(RefCell::new(data));
    register_object(&object);
    object
}

pub(crate) fn register_array(value: &Rc<ArrayData>) {
    CENSUS.with(|census| {
        let mut census = census.borrow_mut();
        census.arrays.register(value);
        census.note_allocation();
    });
}

pub(crate) fn register_proxy(value: &Rc<ProxyData>) {
    CENSUS.with(|census| {
        let mut census = census.borrow_mut();
        census.proxies.register(value);
        census.note_allocation();
    });
}

impl ThreadCensus {
    fn note_allocation(&mut self) {
        self.allocations_since_collection = self.allocations_since_collection.saturating_add(1);
    }
}

/// The VM checks this only at an opcode boundary and at a coarse instruction interval. It does
/// not inspect or upgrade any candidate and therefore is safe while allocation constructors
/// are otherwise active.
pub(crate) fn automatic_collection_due(module_allocations: usize, module_threshold: usize) -> bool {
    CENSUS.with(|census| {
        let census = census.borrow();
        let threshold = if census.next_collection_at == 0 {
            4096
        } else {
            census.next_collection_at
        };
        census.allocations_since_collection >= threshold
            || module_allocations >= module_threshold.max(4096)
    })
}

/// Scale the next collection interval to the live graph. One graph-sized allocation interval
/// between passes gives amortized linear work as a long-lived heap grows, while empty/churning
/// heaps still get collected periodically.
pub(crate) fn collection_completed(live_candidates: usize) {
    CENSUS.with(|census| {
        let mut census = census.borrow_mut();
        census.allocations_since_collection = 0;
        census.next_collection_at = live_candidates.max(4096);
    });
}

pub(crate) fn object_candidates() -> Vec<Weak<RefCell<ObjectData>>> {
    CENSUS.with(|census| census.borrow().objects.snapshot())
}

pub(crate) fn array_candidates() -> Vec<Weak<ArrayData>> {
    CENSUS.with(|census| census.borrow().arrays.snapshot())
}

pub(crate) fn proxy_candidates() -> Vec<Weak<ProxyData>> {
    CENSUS.with(|census| census.borrow().proxies.snapshot())
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
        census.proxies.prune();
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
        assert!(census.internal_weak_counts().is_empty());
    }

    #[test]
    fn derived_counts_preserve_duplicate_weak_edges_through_pruning() {
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
            census.internal_weak_counts().get(&identity),
            Some(&2),
            "pruning must preserve duplicate entries while their Rc allocation is alive"
        );
        assert_eq!(Rc::weak_count(&repeated), 2);

        drop(repeated);
        census.prune();
        assert!(census.entries.is_empty());
        assert!(!census.internal_weak_counts().contains_key(&identity));
        // Numeric count snapshots own neither Weak nor Rc references and remain stable when the
        // census removes expired registration entries.
        assert_eq!(snapshot.get(&identity), Some(&2));
    }

    #[test]
    fn automatic_collection_is_limited_to_the_outermost_vm_run() {
        let outer = enter_vm_run();
        assert!(outer.outermost());

        // A callback can enter another VM while the caller retains Rust borrows. That nested
        // execution must leave collection pressure pending for the next outer opcode boundary.
        let nested = enter_vm_run();
        assert!(!nested.outermost());
        drop(nested);
        drop(outer);

        assert!(enter_vm_run().outermost());
    }
}
