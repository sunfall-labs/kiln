//! Quiescent-state reclamation of the process-shared cache state (ADR 0071, realms issue 04
//! step 4).
//!
//! Inline caches, method paths and the shared key table are written by whichever thread's realm
//! learns something first and read by every other realm's thread without a lock. A reader never
//! sees half a write -- the prelude publishes each answer with one release store of its key word
//! -- but a writer that *replaces* an answer cannot reuse its storage while another thread may
//! still be in the middle of reading it. The turn boundary says when that is over: a thread
//! between two turns holds no generated frame (ADR 0041), so it holds no pointer into shared
//! cache state either. Storage retired at some moment may be reused or freed once every thread
//! that was running a realm then has since passed a turn boundary.
//!
//! The bookkeeping is one process-wide epoch and one word per thread:
//!
//! * A retirement takes a *tag*, the epoch at that moment, and advances the epoch.
//! * A thread running a realm is *online*. At every turn boundary it records the epoch it has
//!   seen; everything retired before that is no longer in its hands.
//! * A thread that is not running a realm -- parked in its drive between turns, or not hosting
//!   one at all -- is offline and holds nothing.
//! * A tag is safe once every online thread has seen a later epoch.
//!
//! A single-realm process pays a load and a store per turn and nothing else: with no other
//! thread online, a retirement is safe at once (tag 0), which is exactly the in-place overwrite
//! the caches did before realms could run concurrently. With several online, asking whether a
//! tag is safe scans the threads only when one of them has advanced since the last scan.

use core::ffi::c_void;
use kiln_shared_graph::Handle;
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering, fence};
use std::sync::{Arc, Mutex, PoisonError};

/// A thread's seen epoch while it is offline: later than every tag.
const OFFLINE: u64 = u64::MAX;

enum Retired {
    Malloc { tag: u64, pointer: usize },
    Graph { tag: u64, _handle: Handle },
}

impl Retired {
    fn tag(&self) -> u64 {
        match self {
            Self::Malloc { tag, .. } | Self::Graph { tag, .. } => *tag,
        }
    }
}

/// The epochs, the threads and the records waiting to be freed: one per process ([`PROCESS`]),
/// and one per unit test.
struct Domain {
    /// Starts above zero, because tag 0 means "safe now".
    epoch: AtomicU64,
    /// Threads online now.
    online: AtomicUsize,
    /// A lower bound on the seen epoch of every online thread, as last computed: a tag below it
    /// is safe without looking at the threads again. Only grows.
    safe_below: AtomicU64,
    /// Counts the events that can raise the lowest seen epoch: a thread passing a turn boundary,
    /// going offline or unregistering. (Coming online can only lower it.)
    advances: AtomicU64,
    /// `advances` as it was when `safe_below` was last computed. While the two are equal no
    /// thread has moved since, so a tag at or above `safe_below` is still unsafe and a cache
    /// miss asking about one answers without taking the `threads` lock. A busy worker sitting in
    /// one long turn would otherwise make every miss beside it scan every thread under a mutex.
    computed_at: AtomicU64,
    /// Every registered thread, by the word it publishes its seen epoch in.
    threads: Mutex<Vec<Arc<AtomicU64>>>,
    /// Records handed to [`kiln_realm_retire`] and not yet freed, with their tags.
    retired: Mutex<Vec<Retired>>,
    /// `retired`'s length, readable without the lock by every turn boundary.
    pending: AtomicUsize,
}

/// One thread's registration: its seen word, and how many online brackets it is inside.
struct Participant {
    seen: Arc<AtomicU64>,
    depth: Cell<u32>,
}

impl Domain {
    const fn new() -> Self {
        Self {
            epoch: AtomicU64::new(1),
            online: AtomicUsize::new(0),
            safe_below: AtomicU64::new(1),
            advances: AtomicU64::new(0),
            computed_at: AtomicU64::new(u64::MAX),
            threads: Mutex::new(Vec::new()),
            retired: Mutex::new(Vec::new()),
            pending: AtomicUsize::new(0),
        }
    }

    fn register(&self) -> Participant {
        let seen = Arc::new(AtomicU64::new(OFFLINE));
        self.threads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Arc::clone(&seen));
        Participant {
            seen,
            depth: Cell::new(0),
        }
    }

    fn unregister(&self, participant: &Participant) {
        if participant.depth.replace(0) != 0 {
            self.leave(participant);
        }
        self.threads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|seen| !Arc::ptr_eq(seen, &participant.seen));
        self.advances.fetch_add(1, Ordering::SeqCst);
    }

    /// Offline to online: announce, then fence, so the thread's reads of shared cache state that
    /// follow cannot be ordered before the announcement. Paired with the retirer's sequentially
    /// consistent unpublish-then-count in [`Self::retire_tag`]: either the retirer counts this
    /// thread, or this thread reads the unpublished word.
    fn arrive(&self, participant: &Participant) {
        self.online.fetch_add(1, Ordering::SeqCst);
        participant
            .seen
            .store(self.epoch.load(Ordering::SeqCst), Ordering::SeqCst);
        fence(Ordering::SeqCst);
    }

    /// Online to offline: everything the thread read is behind it.
    fn leave(&self, participant: &Participant) {
        participant.seen.store(OFFLINE, Ordering::Release);
        self.online.fetch_sub(1, Ordering::SeqCst);
        self.advances.fetch_add(1, Ordering::SeqCst);
    }

    /// A turn boundary: nothing retired until now is in the thread's hands.
    fn quiesce(&self, participant: &Participant) {
        if participant.depth.get() != 0 {
            participant
                .seen
                .store(self.epoch.load(Ordering::SeqCst), Ordering::Release);
            self.advances.fetch_add(1, Ordering::SeqCst);
        }
        if self.pending.load(Ordering::Relaxed) != 0 {
            self.free_safe_records();
        }
    }

    /// The tag for storage just unpublished by a caller that is (`caller_online`) or is not
    /// online itself: 0 when no other thread is online.
    fn retire_tag(&self, caller_online: bool) -> u64 {
        let tag = self.epoch.fetch_add(1, Ordering::SeqCst);
        if self.online.load(Ordering::SeqCst) <= usize::from(caller_online) {
            0
        } else {
            tag
        }
    }

    /// The lowest epoch any online thread has seen, recorded as the new safe bound.
    fn lowest_seen(&self) -> u64 {
        // Read before the scan: an advance during it leaves `computed_at` behind, so the next
        // question scans again rather than trusting a bound that may already be stale.
        let advances = self.advances.load(Ordering::SeqCst);
        let lowest = self
            .threads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|seen| seen.load(Ordering::SeqCst))
            .min()
            .unwrap_or(OFFLINE)
            .min(self.epoch.load(Ordering::SeqCst));
        self.safe_below.fetch_max(lowest, Ordering::AcqRel);
        self.computed_at.store(advances, Ordering::Release);
        lowest
    }

    fn is_safe(&self, tag: u64) -> bool {
        if tag == 0 || self.safe_below.load(Ordering::Acquire) > tag {
            return true;
        }
        // Nobody has advanced since the bound was computed, and it did not clear this tag.
        if self.computed_at.load(Ordering::Acquire) == self.advances.load(Ordering::SeqCst) {
            return false;
        }
        self.lowest_seen() > tag
    }

    fn retire(&self, tag: u64, record: *mut c_void) {
        let mut retired = self.retired.lock().unwrap_or_else(PoisonError::into_inner);
        retired.push(Retired::Malloc {
            tag,
            pointer: record as usize,
        });
        self.pending.store(retired.len(), Ordering::Relaxed);
    }

    fn retire_graph(&self, tag: u64, handle: Handle) {
        let mut retired = self.retired.lock().unwrap_or_else(PoisonError::into_inner);
        retired.push(Retired::Graph {
            tag,
            _handle: handle,
        });
        self.pending.store(retired.len(), Ordering::Relaxed);
    }

    fn free_safe_records(&self) {
        let Ok(mut retired) = self.retired.try_lock() else {
            return;
        };
        let bound = self.lowest_seen();
        retired.retain(|record| {
            if record.tag() < bound {
                if let Retired::Malloc { pointer, .. } = record {
                    // SAFETY: the prelude allocated the record with `malloc` and gave it up at
                    // retirement; every thread that could have read it has passed a boundary.
                    unsafe { free(*pointer as *mut c_void) };
                }
                false
            } else {
                true
            }
        });
        self.pending.store(retired.len(), Ordering::Relaxed);
    }
}

/// The native collector dropped its last facade lease. Hold the graph until every online lane
/// has passed a turn boundary, even if another lane still has a raw read pointer into its arena.
pub(super) fn retire_shared(handle: Handle) {
    handle.invalidate_if_last();
    match PROCESS.retire_tag(this_thread_online()) {
        0 => drop(handle),
        tag => PROCESS.retire_graph(tag, handle),
    }
}

static PROCESS: Domain = Domain::new();

unsafe extern "C" {
    fn free(pointer: *mut c_void);
}

/// This thread's registration with [`PROCESS`], made on first use and withdrawn at thread exit.
struct Local(Participant);

impl Drop for Local {
    fn drop(&mut self) {
        PROCESS.unregister(&self.0);
    }
}

std::thread_local! {
    static LOCAL: Local = Local(PROCESS.register());
}

/// A bracket in which this thread may read shared cache state: a realm entered, a realm torn
/// down, an HTTP event delivered into the language. Brackets nest; the outermost one makes the
/// thread online and its end makes it offline.
pub(super) struct Online(());

pub(super) fn online() -> Online {
    let _ = LOCAL.try_with(|local| {
        let depth = local.0.depth.get();
        if depth == 0 {
            PROCESS.arrive(&local.0);
        }
        local.0.depth.set(depth.saturating_add(1));
    });
    Online(())
}

impl Drop for Online {
    fn drop(&mut self) {
        let _ = LOCAL.try_with(|local| {
            let depth = local.0.depth.get().saturating_sub(1);
            local.0.depth.set(depth);
            if depth == 0 {
                PROCESS.leave(&local.0);
            }
        });
    }
}

/// A turn boundary on this thread; frees what every thread has now passed.
pub(super) fn quiescent() {
    let _ = LOCAL.try_with(|local| PROCESS.quiesce(&local.0));
}

/// A drive parked between turns is offline until it wakes, so an idle realm holds nobody's
/// reclamation back. What the language is called for while it is parked -- an HTTP event --
/// brings the thread online for that call ([`online`]).
pub(super) struct Parked(u32);

pub(super) fn park() -> Parked {
    let depth = LOCAL
        .try_with(|local| {
            let depth = local.0.depth.replace(0);
            if depth != 0 {
                PROCESS.leave(&local.0);
            }
            depth
        })
        .unwrap_or(0);
    Parked(depth)
}

impl Drop for Parked {
    fn drop(&mut self) {
        let depth = self.0;
        let _ = LOCAL.try_with(|local| {
            if depth != 0 {
                PROCESS.arrive(&local.0);
            }
            local.0.depth.set(depth);
        });
    }
}

fn this_thread_online() -> bool {
    LOCAL
        .try_with(|local| local.0.depth.get() != 0)
        .unwrap_or(false)
}

/// Called by a writer that has just unpublished a slot of shared cache state (its key word no
/// longer names any layout). Returns the tag to keep beside the slot, or 0 when no other thread
/// is online and the storage may be rewritten at once.
#[unsafe(no_mangle)]
pub extern "C" fn kiln_realm_retire_tag() -> u64 {
    PROCESS.retire_tag(this_thread_online())
}

/// Whether storage retired under `tag` may be reused: every thread online since then has
/// passed a turn boundary.
#[unsafe(no_mangle)]
pub extern "C" fn kiln_realm_retired_safe(tag: u64) -> i32 {
    i32::from(PROCESS.is_safe(tag))
}

/// Gives up a `malloc`ed record that has just been unpublished (a replaced shared key table):
/// freed at once when no other thread is online, otherwise at the first turn boundary after
/// every online thread has passed one.
///
/// # Safety
///
/// `record` must come from `malloc`, be unreachable from any published word, and not be
/// retired twice.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_retire(record: *mut c_void) -> i32 {
    if record.is_null() {
        return 0;
    }
    match PROCESS.retire_tag(this_thread_online()) {
        // SAFETY: the caller's contract; no other thread can hold it.
        0 => unsafe { free(record) },
        tag => PROCESS.retire(tag, record),
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_is_safe_once_every_online_thread_has_passed_a_boundary() {
        let domain = Domain::new();
        let (me, other) = (domain.register(), domain.register());
        for participant in [&me, &other] {
            participant.depth.set(1);
            domain.arrive(participant);
        }
        let tag = domain.retire_tag(true);
        assert_ne!(tag, 0, "another thread is online");
        assert!(!domain.is_safe(tag));
        domain.quiesce(&other);
        assert!(
            !domain.is_safe(tag),
            "the retiring thread has not passed a boundary"
        );
        domain.quiesce(&me);
        assert!(domain.is_safe(tag));
    }

    #[test]
    fn an_unsafe_answer_is_reused_until_a_thread_advances() {
        let domain = Domain::new();
        let (me, other) = (domain.register(), domain.register());
        for participant in [&me, &other] {
            participant.depth.set(1);
            domain.arrive(participant);
        }
        let tag = domain.retire_tag(true);
        domain.quiesce(&me);
        assert!(
            !domain.is_safe(tag),
            "the other thread has not passed a boundary"
        );
        let scanned = domain.computed_at.load(Ordering::SeqCst);
        assert!(!domain.is_safe(tag));
        assert_eq!(
            domain.computed_at.load(Ordering::SeqCst),
            scanned,
            "no thread moved, so the answer came without a scan"
        );
        domain.quiesce(&other);
        assert!(
            domain.is_safe(tag),
            "an advance makes the next question scan"
        );
    }

    #[test]
    fn a_thread_alone_or_parked_holds_nothing_back() {
        let domain = Domain::new();
        let (me, other) = (domain.register(), domain.register());
        me.depth.set(1);
        domain.arrive(&me);
        assert_eq!(domain.retire_tag(true), 0, "no other thread is online");
        other.depth.set(1);
        domain.arrive(&other);
        let tag = domain.retire_tag(true);
        domain.quiesce(&me);
        assert!(!domain.is_safe(tag));
        // Parking, as `park` does it: offline, and out of every bracket until it wakes.
        other.depth.set(0);
        domain.leave(&other);
        assert!(domain.is_safe(tag), "a parked thread is offline");
        domain.unregister(&me);
        domain.unregister(&other);
        assert_eq!(domain.online.load(Ordering::SeqCst), 0);
    }
}
