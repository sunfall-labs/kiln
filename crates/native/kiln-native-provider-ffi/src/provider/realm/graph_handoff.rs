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
use std::time::{Duration, Instant};

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
        !state.cancelled && state.phase == Phase::Started
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

    unsafe extern "C" fn fake_control(kind: u32, _: u64, _: u64, _: u64) -> i32 {
        SEQUENCE
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .unwrap()
            .push(kind);
        0
    }

    #[repr(C)]
    struct Entry {
        handoff: u64,
        side: u64,
    }

    unsafe extern "C" fn enter_handoff(context: *mut c_void) -> i32 {
        let entry = unsafe { &*context.cast::<Entry>() };
        i32::from(run_active(entry.handoff, entry.side) != PausedTurnResult::Ready)
    }

    #[test]
    fn both_owner_callbacks_enter_before_the_detach_sequence() {
        assert_eq!(kiln_realm_set_graph_control(Some(fake_control)), 0);
        SEQUENCE
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .unwrap()
            .clear();
        let (created, ids) = mpsc::channel();
        let (sender_go, sender_start) = mpsc::channel();
        let (receiver_go, receiver_start) = mpsc::channel();
        let run = |side: u64, start: mpsc::Receiver<u64>, created: mpsc::Sender<(u64, u64)>| {
            std::thread::spawn(move || {
                let mut realm = 0;
                assert_eq!(unsafe { kiln_realm_create(&raw mut realm) }, 0);
                created.send((side, realm)).expect("publish realm ID");
                let handoff = start.recv().expect("handoff ID");
                let entry = Entry { handoff, side };
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
            })
        };
        let sender_thread = run(0, sender_start, created.clone());
        let receiver_thread = run(1, receiver_start, created);
        let first = ids
            .recv_timeout(Duration::from_secs(1))
            .expect("first realm");
        let second = ids
            .recv_timeout(Duration::from_secs(1))
            .expect("second realm");
        let sender = if first.0 == 0 { first.1 } else { second.1 };
        let receiver = if first.0 == 1 { first.1 } else { second.1 };
        let handoff = Registration::new(sender, receiver, 7, 11).expect("register handoff");
        sender_go.send(handoff.id()).expect("start sender callback");
        receiver_go
            .send(handoff.id())
            .expect("start receiver callback");
        assert!(handoff.handoff.wait_ready(Duration::from_secs(1)));
        assert!(
            SEQUENCE.get().unwrap().lock().unwrap().is_empty(),
            "no C mutation before both enter"
        );
        handoff.handoff.start();
        sender_thread.join().expect("sender callback");
        receiver_thread.join().expect("receiver callback");
        assert!(handoff.handoff.done());
        assert_eq!(*SEQUENCE.get().unwrap().lock().unwrap(), [3, 4, 5, 6, 7]);
    }
}
