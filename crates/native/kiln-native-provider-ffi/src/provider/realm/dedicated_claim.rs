//! Turn-boundary control for realms that keep a dedicated OS thread.
//!
//! A coordinator holds only numeric realm IDs and `Send` synchronization state. The realm's
//! `Rc` and generated C stack never leave its thread. The continuous drive checks this gate
//! before selecting each language turn, including after an external wake from a parked drive.

use super::{PausedTurnCommand, PausedTurnResult};
use kiln_runtime::DriveSignal;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

struct Command {
    value: PausedTurnCommand,
    reply: mpsc::SyncSender<PausedTurnResult>,
}

#[derive(Default)]
struct State {
    claim: u64,
    paused: bool,
    running: bool,
    closed: bool,
    command: Option<Command>,
}

pub(crate) struct DedicatedGate {
    wake: DriveSignal,
    state: Mutex<State>,
    changed: Condvar,
}

impl DedicatedGate {
    fn new(wake: DriveSignal) -> Self {
        Self {
            wake,
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        }
    }

    fn claim(&self, id: u64) -> bool {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if id == 0 || state.claim != 0 || state.closed {
            return false;
        }
        state.claim = id;
        drop(state);
        // `Notify` is shared with the host completion waiter. A notification delivered before
        // that waiter starts is retained as a permit; the next drive boundary sees the claim.
        self.wake.notify_one();
        true
    }

    fn wait_paused(&self, id: u64, deadline: Instant) -> bool {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        while state.claim == id && !state.paused && !state.closed {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            let waited = self.changed.wait_timeout(state, remaining);
            state = waited.unwrap_or_else(PoisonError::into_inner).0;
        }
        state.claim == id && state.paused && !state.closed
    }

    fn control(&self, id: u64, command: PausedTurnCommand) -> PausedTurnResult {
        let (reply, answer) = mpsc::sync_channel(1);
        {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if state.claim != id
                || !state.paused
                || state.closed
                || state.running
                || state.command.is_some()
            {
                return PausedTurnResult::Unavailable;
            }
            state.command = Some(Command {
                value: command,
                reply,
            });
            self.changed.notify_all();
        }
        // Once a control starts, releasing the pair before it finishes would let a mutator
        // resume beside the proof. A callback that never returns is a fatal transaction bug;
        // there is deliberately no timeout that abandons a running callback.
        answer.recv().unwrap_or(PausedTurnResult::Unavailable)
    }

    fn release(&self, id: u64) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.claim != id {
            return;
        }
        while state.running {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        state.command = None;
        state.claim = 0;
        self.changed.notify_all();
    }

    fn close(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.closed = true;
        state.command = None;
        self.changed.notify_all();
    }

    /// Called only by the owning realm at a boundary with no generated JS frame live.
    pub(crate) fn boundary(&self, mut control: impl FnMut(PausedTurnCommand) -> PausedTurnResult) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.claim == 0 || state.closed {
            return;
        }
        let id = state.claim;
        state.paused = true;
        self.changed.notify_all();
        loop {
            if state.claim != id || state.closed {
                state.paused = false;
                self.changed.notify_all();
                return;
            }
            if let Some(command) = state.command.take() {
                state.running = true;
                drop(state);
                let result = control(command.value);
                state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                state.running = false;
                let _ = command.reply.send(result);
                self.changed.notify_all();
            } else {
                state = self
                    .changed
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        }
    }
}

static NEXT_CLAIM: AtomicU64 = AtomicU64::new(1);
static TARGETS: OnceLock<Mutex<HashMap<u64, Arc<DedicatedGate>>>> = OnceLock::new();

fn targets() -> &'static Mutex<HashMap<u64, Arc<DedicatedGate>>> {
    TARGETS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) struct DedicatedRegistration {
    realm: u64,
    gate: Arc<DedicatedGate>,
}

impl DedicatedRegistration {
    pub(crate) fn new(realm: u64, wake: DriveSignal) -> Option<Self> {
        if realm == 0 {
            return None;
        }
        let gate = Arc::new(DedicatedGate::new(wake));
        let mut targets = targets().lock().ok()?;
        if targets.contains_key(&realm) {
            return None;
        }
        targets.insert(realm, Arc::clone(&gate));
        Some(Self { realm, gate })
    }

    pub(crate) fn gate(&self) -> Arc<DedicatedGate> {
        Arc::clone(&self.gate)
    }
}

impl Drop for DedicatedRegistration {
    fn drop(&mut self) {
        self.gate.close();
        if let Ok(mut targets) = targets().lock()
            && targets
                .get(&self.realm)
                .is_some_and(|entry| Arc::ptr_eq(entry, &self.gate))
        {
            targets.remove(&self.realm);
        }
    }
}

pub(crate) struct DedicatedPairClaim {
    id: u64,
    first: (u64, Arc<DedicatedGate>),
    second: (u64, Arc<DedicatedGate>),
}

impl DedicatedPairClaim {
    pub(crate) fn control(&self, realm: u64, command: PausedTurnCommand) -> PausedTurnResult {
        if realm == self.first.0 {
            self.first.1.control(self.id, command)
        } else if realm == self.second.0 {
            self.second.1.control(self.id, command)
        } else {
            PausedTurnResult::Unavailable
        }
    }
}

impl Drop for DedicatedPairClaim {
    fn drop(&mut self) {
        self.second.1.release(self.id);
        self.first.1.release(self.id);
    }
}

/// Claim two dedicated realms in numeric order and acknowledge both boundaries before any
/// control callback. A failed claim releases every partial reservation.
pub(crate) fn claim_pair(
    sender: u64,
    receiver: u64,
    timeout: Duration,
) -> Option<DedicatedPairClaim> {
    if sender == 0 || receiver == 0 || sender == receiver {
        return None;
    }
    let (first, second) = {
        let targets = targets().lock().ok()?;
        let a = (sender, Arc::clone(targets.get(&sender)?));
        let b = (receiver, Arc::clone(targets.get(&receiver)?));
        if a.0 < b.0 { (a, b) } else { (b, a) }
    };
    let id = NEXT_CLAIM.fetch_add(1, Ordering::Relaxed);
    if id == 0 || !first.1.claim(id) {
        return None;
    }
    if !second.1.claim(id) {
        first.1.release(id);
        return None;
    }
    let claim = DedicatedPairClaim { id, first, second };
    let deadline = Instant::now() + timeout;
    if claim.first.1.wait_paused(id, deadline) && claim.second.1.wait_paused(id, deadline) {
        Some(claim)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        kiln_realm_create, kiln_realm_destroy, kiln_realm_drive, kiln_realm_enter,
        kiln_realm_set_timeout, kiln_realm_use_wall_clock,
    };
    use super::*;
    use core::ffi::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn claimed_dedicated_boundaries_run_control_on_each_owner_thread() {
        let owner_a = Arc::new(Mutex::new(None));
        let owner_b = Arc::new(Mutex::new(None));
        let visits = Arc::new(AtomicUsize::new(0));
        let run =
            |realm, owner: Arc<Mutex<Option<std::thread::ThreadId>>>, visits: Arc<AtomicUsize>| {
                let wake = Arc::new(tokio::sync::Notify::new());
                let registration = DedicatedRegistration::new(realm, wake).expect("register realm");
                std::thread::spawn(move || {
                    *owner.lock().expect("owner lock") = Some(std::thread::current().id());
                    let gate = registration.gate();
                    let deadline = Instant::now() + Duration::from_secs(1);
                    while gate.state.lock().expect("gate lock").claim == 0 {
                        assert!(Instant::now() < deadline, "coordinator claims realm");
                        std::thread::yield_now();
                    }
                    gate.boundary(|command| {
                        assert_eq!(command.kind, 7);
                        visits.fetch_add(1, Ordering::AcqRel);
                        PausedTurnResult::Ready
                    });
                })
            };
        let a = run(111, Arc::clone(&owner_a), Arc::clone(&visits));
        let b = run(222, Arc::clone(&owner_b), Arc::clone(&visits));
        let claim = claim_pair(222, 111, Duration::from_secs(1)).expect("both boundaries pause");
        assert_eq!(
            claim.control(
                111,
                PausedTurnCommand {
                    kind: 7,
                    ids: [1, 2, 3]
                }
            ),
            PausedTurnResult::Ready
        );
        assert_eq!(
            claim.control(
                222,
                PausedTurnCommand {
                    kind: 7,
                    ids: [4, 5, 6]
                }
            ),
            PausedTurnResult::Ready
        );
        assert_eq!(visits.load(Ordering::Acquire), 2);
        assert_ne!(
            *owner_a.lock().expect("owner lock"),
            *owner_b.lock().expect("owner lock")
        );
        drop(claim);
        a.join().expect("first realm resumes");
        b.join().expect("second realm resumes");
    }

    #[test]
    fn timed_out_claim_releases_an_unarrived_realm() {
        let first = DedicatedRegistration::new(333, Arc::new(tokio::sync::Notify::new()))
            .expect("first registration");
        let second = DedicatedRegistration::new(444, Arc::new(tokio::sync::Notify::new()))
            .expect("second registration");
        assert!(claim_pair(333, 444, Duration::from_millis(1)).is_none());
        let first = first.gate();
        let second = second.gate();
        assert_eq!(first.state.lock().expect("first state").claim, 0);
        assert_eq!(second.state.lock().expect("second state").claim, 0);
    }

    #[test]
    fn continuous_provider_drives_acknowledge_at_turn_boundaries() {
        unsafe extern "C" fn mark(context: *mut c_void, _: *mut c_void, _: *mut c_void) -> i32 {
            // SAFETY: the worker owns this writable slot through its drive.
            unsafe { *context.cast::<u8>() = 1 };
            0
        }
        unsafe extern "C" fn no_drop(_: *mut c_void) {}
        unsafe extern "C" fn arm(context: *mut c_void) -> i32 {
            let mut id = 0;
            // SAFETY: the timer slot and callbacks live through the continuous drive.
            unsafe {
                kiln_realm_set_timeout(750.0, Some(mark), Some(no_drop), context, &raw mut id)
            }
        }
        let (started, handles) = mpsc::channel();
        let run = |started: mpsc::Sender<u64>| {
            std::thread::spawn(move || {
                let mut realm = 0;
                let mut fired = 0u8;
                // SAFETY: all callback slots stay on this worker thread through drive/teardown.
                assert_eq!(unsafe { kiln_realm_create(&raw mut realm) }, 0);
                // The gate is visible before the first generated module entry.
                started.send(realm).expect("realm ID reaches coordinator");
                assert_eq!(kiln_realm_use_wall_clock(realm), 0);
                assert_eq!(
                    unsafe { kiln_realm_enter(realm, Some(arm), (&raw mut fired).cast()) },
                    0
                );
                assert_eq!(
                    unsafe { kiln_realm_drive(realm, std::ptr::null_mut(), std::ptr::null_mut()) },
                    0
                );
                assert_eq!(fired, 1);
                assert_eq!(kiln_realm_destroy(realm), 0);
            })
        };
        let first = run(started.clone());
        let second = run(started);
        let a = handles
            .recv_timeout(Duration::from_secs(2))
            .expect("first realm starts");
        let b = handles
            .recv_timeout(Duration::from_secs(2))
            .expect("second realm starts");
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let ready = targets().lock().expect("registry lock");
            if ready.contains_key(&a) && ready.contains_key(&b) {
                break;
            }
            drop(ready);
            assert!(Instant::now() < deadline, "both newly created realms register");
            std::thread::yield_now();
        }
        let claim = claim_pair(a, b, Duration::from_secs(1)).expect("both drives pause");
        assert!(claim.first.1.state.lock().expect("first gate").paused);
        assert!(claim.second.1.state.lock().expect("second gate").paused);
        drop(claim);
        first.join().expect("first drive completes");
        second.join().expect("second drive completes");
    }
}
