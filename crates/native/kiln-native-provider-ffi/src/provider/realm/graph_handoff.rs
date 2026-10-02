//! Two owner-lane callbacks enter before either realm stages an actor graph.
//!
//! The coordinator and this registry hold numeric IDs and synchronization state only. Each
//! callback runs its C controls under its own active realm. No queued scheduler command remains
//! after sender unlink: the already running receiver adopts the chunk, then the already running
//! sender consumes its escrow. A same-lane pair must refuse before creating this handoff.

use super::{PausedTurnCommand, PausedTurnResult, active_realm_id, invoke_active_graph_control};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::Duration;
#[cfg(test)]
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Waiting,
    Started,
    Staged,
    Reserved,
    Detached,
    Adopted,
    Done,
    Failed,
}

struct State {
    ready: [bool; 2],
    cancelled: bool,
    phase: Phase,
}

pub(crate) struct GraphHandoff {
    sender: u64,
    receiver: u64,
    request: u64,
    reservation: u64,
    state: Mutex<State>,
    changed: Condvar,
}

impl GraphHandoff {
    fn new(sender: u64, receiver: u64, request: u64, reservation: u64) -> Self {
        Self {
            sender,
            receiver,
            request,
            reservation,
            state: Mutex::new(State {
                ready: [false; 2],
                cancelled: false,
                phase: Phase::Waiting,
            }),
            changed: Condvar::new(),
        }
    }

    fn command(&self, kind: u32, ids: [u64; 3]) -> PausedTurnResult {
        invoke_active_graph_control(PausedTurnCommand { kind, ids })
    }

    fn transition(&self, phase: Phase) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.phase = phase;
        self.changed.notify_all();
    }

    fn await_phase(&self, wanted: Phase) -> Phase {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        while state.phase != wanted && state.phase != Phase::Failed {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        state.phase
    }

    fn enter(&self, side: usize) -> bool {
        if side > 1
            || active_realm_id()
                != if side == 0 {
                    self.sender
                } else {
                    self.receiver
                }
        {
            return false;
        }
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.cancelled || state.ready[side] {
            return false;
        }
        state.ready[side] = true;
        self.changed.notify_all();
        while state.phase == Phase::Waiting && !state.cancelled {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        // The peer may advance from Started to Staged before this waiter reacquires the
        // mutex. Both callbacks were already marked ready, so any later phase still enters.
        !state.cancelled
    }

    fn run_sender(&self) -> PausedTurnResult {
        if !self.enter(0) {
            return PausedTurnResult::Unavailable;
        }
        let staged = self.command(3, [self.request, self.reservation, self.receiver]);
        if staged != PausedTurnResult::Ready {
            self.transition(Phase::Failed);
            return staged;
        }
        self.transition(Phase::Staged);
        if self.await_phase(Phase::Reserved) == Phase::Failed {
            if self.command(9, [self.request, self.reservation, 0]) != PausedTurnResult::Ready {
                std::process::abort();
            }
            return PausedTurnResult::Incomplete;
        }
        if self.command(5, [self.reservation, self.sender, self.request]) != PausedTurnResult::Ready
        {
            if self.command(9, [self.request, self.reservation, 0]) != PausedTurnResult::Ready {
                std::process::abort();
            }
            self.transition(Phase::Failed);
            return PausedTurnResult::Incomplete;
        }
        self.transition(Phase::Detached);
        if self.await_phase(Phase::Adopted) != Phase::Adopted {
            std::process::abort();
        }
        if self.command(7, [self.request, self.reservation, 0]) != PausedTurnResult::Ready {
            std::process::abort();
        }
        self.transition(Phase::Done);
        PausedTurnResult::Ready
    }

    fn run_receiver(&self) -> PausedTurnResult {
        if !self.enter(1) {
            return PausedTurnResult::Unavailable;
        }
        if self.await_phase(Phase::Staged) == Phase::Failed {
            return PausedTurnResult::Incomplete;
        }
        let reserved = self.command(4, [self.reservation, self.sender, self.request]);
        if reserved != PausedTurnResult::Ready {
            self.transition(Phase::Failed);
            return reserved;
        }
        self.transition(Phase::Reserved);
        if self.await_phase(Phase::Detached) == Phase::Failed {
            if self.command(8, [self.reservation, 0, 0]) != PausedTurnResult::Ready {
                std::process::abort();
            }
            return PausedTurnResult::Incomplete;
        }
        if self.command(6, [self.reservation, self.sender, self.request]) != PausedTurnResult::Ready
        {
            std::process::abort();
        }
        self.transition(Phase::Adopted);
        PausedTurnResult::Ready
    }

    #[cfg(test)]
    pub(crate) fn wait_ready(&self, timeout: Duration) -> bool {
        let Some(deadline) = Instant::now().checked_add(timeout) else {
            return false;
        };
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        while !state.ready.iter().all(|ready| *ready) && !state.cancelled {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            state = self
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        state.ready.iter().all(|ready| *ready) && !state.cancelled
    }

    /// Both callbacks may be queued behind unrelated turns. Only cancellation, teardown or a
    /// callback result may stop readiness; elapsed time is not a failed ownership proof.
    pub(crate) fn wait_ready_while(&self, mut pending: impl FnMut() -> bool) -> bool {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        while !state.ready.iter().all(|ready| *ready) && !state.cancelled && pending() {
            state = self
                .changed
                .wait_timeout(state, Duration::from_millis(10))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        state.ready.iter().all(|ready| *ready) && !state.cancelled && pending()
    }

    pub(crate) fn start(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        assert!(state.ready.iter().all(|ready| *ready) && !state.cancelled);
        state.phase = Phase::Started;
        self.changed.notify_all();
    }

    pub(crate) fn cancel(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(
            state.phase,
            Phase::Waiting,
            "cancel only before any C mutation"
        );
        state.cancelled = true;
        self.changed.notify_all();
    }

    pub(crate) fn done(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .phase
            == Phase::Done
    }

    pub(crate) fn refused_safely(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.cancelled || state.phase == Phase::Failed
    }
}

static NEXT_HANDOFF: AtomicU64 = AtomicU64::new(1);
static HANDOFFS: OnceLock<Mutex<HashMap<u64, Arc<GraphHandoff>>>> = OnceLock::new();

fn handoffs() -> &'static Mutex<HashMap<u64, Arc<GraphHandoff>>> {
    HANDOFFS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) struct Registration {
    id: u64,
    pub(crate) handoff: Arc<GraphHandoff>,
}

impl Registration {
    pub(crate) fn new(sender: u64, receiver: u64, request: u64, reservation: u64) -> Option<Self> {
        if sender == 0 || receiver == 0 || sender == receiver || request == 0 || reservation == 0 {
            return None;
        }
        let id = NEXT_HANDOFF.fetch_add(1, Ordering::Relaxed);
        if id == 0 {
            return None;
        }
        let handoff = Arc::new(GraphHandoff::new(sender, receiver, request, reservation));
        let mut registry = handoffs().lock().ok()?;
        registry.try_reserve(1).ok()?;
        registry.insert(id, Arc::clone(&handoff));
        Some(Self { id, handoff })
    }

    pub(crate) fn id(&self) -> u64 {
        self.id
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        if let Ok(mut registry) = handoffs().lock() {
            if registry
                .get(&self.id)
                .is_some_and(|entry| Arc::ptr_eq(entry, &self.handoff))
            {
                registry.remove(&self.id);
            }
        }
    }
}

pub(crate) fn run_active(id: u64, side: u64) -> PausedTurnResult {
    let Some(handoff) = handoffs()
        .lock()
        .ok()
        .and_then(|registry| registry.get(&id).cloned())
    else {
        return PausedTurnResult::Unavailable;
    };
    match side {
        0 => handoff.run_sender(),
        1 => handoff.run_receiver(),
        _ => PausedTurnResult::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        kiln_realm_create, kiln_realm_destroy, kiln_realm_enter, kiln_realm_set_graph_control,
    };
    use super::*;
    use core::ffi::c_void;
    use std::sync::mpsc;

    static SEQUENCE: OnceLock<Mutex<Vec<u32>>> = OnceLock::new();
    static TEST_LOCK: Mutex<()> = Mutex::new(());
    /// The reservation these tests' handoffs carry. Every graph command names its handoff's
    /// reservation, and the control hook is process-wide and never removed, so another test's
    /// graph traffic reaches `fake_control` too; it records only commands about this one.
    const RESERVATION: u64 = 0x6b69_6c6e_5f74_6573;

    unsafe extern "C" fn fake_control(kind: u32, a: u64, b: u64, c: u64) -> i32 {
        if [a, b, c].contains(&RESERVATION) {
            SEQUENCE
                .get_or_init(|| Mutex::new(Vec::new()))
                .lock()
                .unwrap()
                .push(kind);
        }
        0
    }

    #[repr(C)]
    struct Entry {
        handoff: u64,
        side: u64,
        expected: PausedTurnResult,
    }

    unsafe extern "C" fn enter_handoff(context: *mut c_void) -> i32 {
        let entry = unsafe { &*context.cast::<Entry>() };
        i32::from(run_active(entry.handoff, entry.side) != entry.expected)
    }

    struct Owners {
        realms: [u64; 2],
        callbacks: [(mpsc::Sender<u64>, std::thread::JoinHandle<()>); 2],
    }

    impl Owners {
        fn new(expected: PausedTurnResult) -> Self {
            let (created, ids) = mpsc::channel();
            let callbacks = std::array::from_fn(|side| {
                let (go, start) = mpsc::channel();
                let created = created.clone();
                let thread = std::thread::spawn(move || {
                    let mut realm = 0;
                    assert_eq!(unsafe { kiln_realm_create(&raw mut realm) }, 0);
                    created.send((side, realm)).expect("publish realm ID");
                    let handoff = start.recv().expect("handoff ID");
                    let entry = Entry {
                        handoff,
                        side: side as u64,
                        expected,
                    };
                    assert_eq!(
                        unsafe {
                            kiln_realm_enter(
                                realm,
                                Some(enter_handoff),
                                (&raw const entry).cast_mut().cast(),
                            )
                        },
                        0
                    );
                    assert_eq!(kiln_realm_destroy(realm), 0);
                });
                (go, thread)
            });
            let mut realms = [0; 2];
            for _ in 0..2 {
                let (side, realm) = ids
                    .recv_timeout(Duration::from_secs(10))
                    .expect("owner realm");
                realms[side] = realm;
            }
            Self { realms, callbacks }
        }

        fn start(&self, side: usize, handoff: u64) {
            self.callbacks[side]
                .0
                .send(handoff)
                .expect("start callback");
        }

        fn finish(self) {
            for (_, thread) in self.callbacks {
                thread.join().expect("owner callback");
            }
        }
    }

    #[test]
    fn both_owner_callbacks_enter_before_the_detach_sequence() {
        let _serial = TEST_LOCK.lock().unwrap();
        assert_eq!(kiln_realm_set_graph_control(Some(fake_control)), 0);
        SEQUENCE
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .unwrap()
            .clear();
        let owners = Owners::new(PausedTurnResult::Ready);
        let handoff = Registration::new(owners.realms[0], owners.realms[1], 7, RESERVATION)
            .expect("register handoff");
        owners.start(0, handoff.id());
        owners.start(1, handoff.id());
        let ready = handoff.handoff.wait_ready(Duration::from_secs(10));
        let untouched = SEQUENCE.get().unwrap().lock().unwrap().is_empty();
        if ready {
            handoff.handoff.start();
        } else {
            handoff.handoff.cancel();
        }
        owners.finish();
        assert!(ready, "both owner callbacks entered");
        assert!(untouched, "no C mutation before both enter");
        assert!(handoff.handoff.done());
        assert_eq!(*SEQUENCE.get().unwrap().lock().unwrap(), [3, 4, 5, 6, 7]);
    }

    #[test]
    fn cancellation_before_start_releases_ready_and_late_owner_callbacks() {
        let _serial = TEST_LOCK.lock().unwrap();
        assert_eq!(kiln_realm_set_graph_control(Some(fake_control)), 0);
        for entered in 0..=2 {
            SEQUENCE
                .get_or_init(|| Mutex::new(Vec::new()))
                .lock()
                .unwrap()
                .clear();
            let owners = Owners::new(PausedTurnResult::Unavailable);
            let handoff = Registration::new(owners.realms[0], owners.realms[1], 7, RESERVATION)
                .expect("register handoff");
            for side in 0..entered {
                owners.start(side, handoff.id());
            }
            // Observe actual callback entry before cancelling, rather than hoping a sleep
            // lands between scheduling and mutation. Late callbacks are deliberately withheld.
            let state = handoff.handoff.state.lock().unwrap();
            let (state, _) = handoff
                .handoff
                .changed
                .wait_timeout_while(state, Duration::from_secs(10), |state| {
                    state.ready.iter().filter(|ready| **ready).count() != entered
                })
                .unwrap();
            let observed = state.ready.iter().filter(|ready| **ready).count() == entered;
            drop(state);
            assert!(!handoff.handoff.wait_ready_while(|| false));
            handoff.handoff.cancel();
            for side in entered..2 {
                owners.start(side, handoff.id());
            }
            owners.finish();
            assert!(
                observed,
                "expected {entered} callbacks to enter before cancellation"
            );
            assert!(handoff.handoff.refused_safely());
            assert!(!handoff.handoff.done());
            assert!(SEQUENCE.get().unwrap().lock().unwrap().is_empty());
        }
    }
}
