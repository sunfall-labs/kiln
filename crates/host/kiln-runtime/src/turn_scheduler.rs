//! Shared scheduler lanes for pinned and movable realm turns (ADR 0070).
//!
//! A lane constructs and owns its `!Send` bodies. Movable bodies cross lanes only between
//! turns under an exclusive ticket lease. Other threads hold tickets and commands, never a
//! borrowed realm pointer.

use std::cell::Cell;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::task::Waker;
use std::time::{Duration, Instant};

use crate::Reactor;
use crate::turn_queue::{ReadyQueue, TurnLease, TurnOutcome, TurnTicket};

/// Realm work executed only by its assigned scheduler lane.
pub trait PinnedTurn {
    /// Run one language turn, polling any local tasks with `waker` before parking.
    /// The implementation must return with no generated frame on its C stack.
    fn run_turn(&mut self, waker: &Waker) -> TurnOutcome;

    /// The lane identity for a turn body that can become pinned while it runs.
    fn run_turn_on(&mut self, waker: &Waker, _lane: usize) -> TurnOutcome {
        self.run_turn(waker)
    }

    /// Run a bounded, non-JavaScript operation while this body is paused by a pair claim.
    /// Implementations must not enter a language turn or retain a pointer from `command`.
    fn paused_control(&mut self, _command: PausedTurnCommand, _lane: usize) -> PausedTurnResult {
        PausedTurnResult::Unavailable
    }
}

/// Opaque numeric IDs for a lane-local operation. No realm heap pointer crosses the queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PausedTurnCommand {
    pub kind: u32,
    pub ids: [u64; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PausedTurnResult {
    Ready,
    Alias,
    Incomplete,
    Unavailable,
}

const PAUSED_CONTROL_PENDING: u8 = 0;
const PAUSED_CONTROL_RUNNING: u8 = 1;
const PAUSED_CONTROL_CANCELLED: u8 = 2;

impl<F> PinnedTurn for F
where
    F: FnMut(&Waker) -> TurnOutcome + 'static,
{
    fn run_turn(&mut self, waker: &Waker) -> TurnOutcome {
        self(waker)
    }
}

/// A turn body whose owner has proved that no thread-affine state survives a turn boundary.
/// The scheduler transfers this body only after `run_turn` has returned.
pub trait MovableTurn: PinnedTurn + Send {}

impl<T: PinnedTurn + Send> MovableTurn for T {}

type Factory = Box<dyn FnOnce(Arc<Reactor>, Arc<TurnTicket>) -> Box<dyn PinnedTurn> + Send>;
type MovableFactory =
    Box<dyn FnOnce(Arc<Reactor>, Arc<TurnTicket>, usize) -> Box<dyn MovableTurn> + Send>;

enum Command {
    Install {
        realm_id: u64,
        ticket: Arc<TurnTicket>,
        status: Arc<AtomicU8>,
        factory: Factory,
    },
    InstallMovable {
        entry: Arc<MovableEntry>,
        factory: MovableFactory,
    },
    Remove(u64),
    PausedControl {
        realm_id: u64,
        claim: u64,
        command: PausedTurnCommand,
        permit: Arc<AtomicU8>,
        result: mpsc::SyncSender<PausedTurnResult>,
    },
    Drain,
    Deadlines(BinaryHeap<Reverse<(Instant, u64, u64)>>),
}

struct LaneControl {
    index: usize,
    queue: Arc<ReadyQueue>,
    control: Arc<TurnTicket>,
    commands: Mutex<VecDeque<Command>>,
}

impl LaneControl {
    fn command(&self, command: Command) {
        self.commands
            .lock()
            .expect("scheduler command lock poisoned")
            .push_back(command);
        self.control.wake();
    }
}

struct RealmEntry {
    body: Box<dyn PinnedTurn>,
    status: Arc<AtomicU8>,
    ticket: Arc<TurnTicket>,
    deadline_epoch: u64,
}

struct MovableEntry {
    body: Mutex<Option<Box<dyn MovableTurn>>>,
    status: Arc<AtomicU8>,
    ticket: Arc<TurnTicket>,
    deadline_epoch: AtomicU64,
}

type MovableRealms = Arc<Mutex<HashMap<u64, Arc<MovableEntry>>>>;

std::thread_local! {
    // A replacement hands its deadlines to the lane whose blocking call created it.
    static ACTIVE_LANE: Cell<Option<(usize, usize)>> = const { Cell::new(None) };
}

struct ActiveLaneGuard(Option<(usize, usize)>);

impl ActiveLaneGuard {
    fn enter(queue: &Arc<ReadyQueue>, lane: usize) -> Self {
        let identity = Arc::as_ptr(queue) as usize;
        Self(ACTIVE_LANE.with(|slot| slot.replace(Some((identity, lane)))))
    }
}

impl Drop for ActiveLaneGuard {
    fn drop(&mut self) {
        ACTIVE_LANE.with(|slot| slot.set(self.0.take()));
    }
}

/// Observable state of a scheduler turn body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TurnStatus {
    Active = 0,
    Complete = 1,
    Cancelled = 2,
    Panicked = 3,
}

impl TurnStatus {
    fn read(value: u8) -> Self {
        match value {
            0 => Self::Active,
            1 => Self::Complete,
            2 => Self::Cancelled,
            3 => Self::Panicked,
            _ => unreachable!("invalid scheduler status"),
        }
    }
}

/// A cross-thread ticket for a realm whose body is owned by this scheduler. It contains no
/// realm pointer and can be used to request a pair pause from outside either turn body.
#[derive(Clone)]
pub struct TurnPauseTarget {
    ticket: Arc<TurnTicket>,
    status: Arc<AtomicU8>,
    coordinator: Arc<TurnPauseCoordinator>,
}

impl TurnPauseTarget {
    pub fn id(&self) -> u64 {
        self.ticket.realm_id()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnClaimError {
    SameRealm,
    ForeignScheduler,
    Unavailable,
    Busy,
}

struct TurnPauseEntry {
    claim: u64,
    acknowledged: bool,
    deferred_cancel: Option<Box<dyn FnOnce() + Send>>,
}

struct TurnPauseState {
    next_claim: u64,
    entries: HashMap<u64, TurnPauseEntry>,
}

struct TurnPauseCoordinator {
    queue_identity: usize,
    pending: AtomicUsize,
    state: Mutex<TurnPauseState>,
    changed: Condvar,
}

impl TurnPauseCoordinator {
    fn new(queue: &Arc<ReadyQueue>) -> Self {
        Self {
            queue_identity: Arc::as_ptr(queue) as usize,
            pending: AtomicUsize::new(0),
            state: Mutex::new(TurnPauseState {
                next_claim: 1,
                entries: HashMap::new(),
            }),
            changed: Condvar::new(),
        }
    }

    /// This is called only after a ticket is leased and before its body runs. A running body
    /// must return first; the queued wake then brings it here with every generated frame gone.
    fn acknowledge_boundary(&self, id: u64) -> bool {
        if self.pending.load(Ordering::Acquire) == 0 {
            return false;
        }
        let mut state = self.state.lock().expect("turn pause lock poisoned");
        let Some(entry) = state.entries.get_mut(&id) else {
            return false;
        };
        entry.acknowledged = true;
        self.changed.notify_all();
        true
    }

    fn acknowledged_for(&self, id: u64, claim: u64) -> bool {
        self.state
            .lock()
            .expect("turn pause lock poisoned")
            .entries
            .get(&id)
            .is_some_and(|entry| entry.claim == claim && entry.acknowledged)
    }

    /// A cancellation may retire the ticket immediately, but dropping a claimed body would
    /// invalidate its paused heap. Defer only the body removal until the claim releases it.
    fn defer_cancel(
        &self,
        id: u64,
        action: Box<dyn FnOnce() + Send>,
    ) -> Result<(), Box<dyn FnOnce() + Send>> {
        let mut state = self.state.lock().expect("turn pause lock poisoned");
        let Some(entry) = state.entries.get_mut(&id) else {
            return Err(action);
        };
        entry.deferred_cancel = Some(action);
        self.changed.notify_all();
        Ok(())
    }
}

/// A pair of scheduler bodies paused at their turn boundaries. The owner must release this
/// before waiting for either realm to run again. It is deliberately independent of graph
/// tickets: a successful wait is a scheduling fact, not an ownership certificate.
pub struct TurnPairClaim {
    _scheduler: Arc<PinnedTurnScheduler>,
    coordinator: Arc<TurnPauseCoordinator>,
    claim: u64,
    targets: Vec<TurnPauseTarget>,
}

/// A queued paused-lane callback. Before it starts the caller may cancel it; once running,
/// dropping the handle cannot release the pair claim safely until the callback has returned.
pub struct PendingTurnControl {
    permit: Arc<AtomicU8>,
    result: mpsc::Receiver<PausedTurnResult>,
}

impl PendingTurnControl {
    /// Before a two-sided handoff starts, either control completing means it could not enter
    /// the rendezvous. Consuming that result is safe: a successful control cannot finish until
    /// the coordinator starts the handoff.
    pub fn finished_before_start(&self) -> bool {
        !matches!(self.result.try_recv(), Err(mpsc::TryRecvError::Empty))
    }

    pub fn cancel_pending(&self) -> bool {
        self.permit
            .compare_exchange(
                PAUSED_CONTROL_PENDING,
                PAUSED_CONTROL_CANCELLED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    pub fn wait(self, timeout: Duration) -> PausedTurnResult {
        match self.result.recv_timeout(timeout) {
            Ok(answer) => answer,
            Err(mpsc::RecvTimeoutError::Disconnected) => PausedTurnResult::Unavailable,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if self.cancel_pending() {
                    PausedTurnResult::Unavailable
                } else {
                    // A started callback owns a realm under the claim. Wait for it rather than
                    // letting its mutator resume beside a detached graph.
                    self.result.recv().unwrap_or(PausedTurnResult::Unavailable)
                }
            }
        }
    }

    /// A graph coordinator may wait behind an unrelated language turn for arbitrarily long.
    /// Cancellation ends a still-queued control; a control that already started must finish
    /// before its realm claim can be released.
    pub fn wait_while(self, mut pending: impl FnMut() -> bool) -> PausedTurnResult {
        loop {
            match self.result.recv_timeout(Duration::from_millis(10)) {
                Ok(answer) => return answer,
                Err(mpsc::RecvTimeoutError::Disconnected) => return PausedTurnResult::Unavailable,
                Err(mpsc::RecvTimeoutError::Timeout) if !pending() => {
                    if self.cancel_pending() {
                        return PausedTurnResult::Unavailable;
                    }
                    return self.result.recv().unwrap_or(PausedTurnResult::Unavailable);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
}

impl TurnPairClaim {
    fn member(&self, target: &TurnPauseTarget) -> Option<&TurnPauseTarget> {
        self.targets
            .iter()
            .find(|claimed| Arc::ptr_eq(&claimed.ticket, &target.ticket))
    }

    /// Wait only from a coordinator thread, never from a scheduler lane: a lane could own
    /// either claimed realm and cannot acknowledge while its OS thread is blocked here.
    pub fn wait(&self, timeout: Duration) -> bool {
        let on_lane = ACTIVE_LANE.with(|slot| {
            slot.get()
                .is_some_and(|(queue, _)| queue == self.coordinator.queue_identity)
        });
        if on_lane {
            return false;
        }
        let Some(deadline) = Instant::now().checked_add(timeout) else {
            return false;
        };
        let mut state = self
            .coordinator
            .state
            .lock()
            .expect("turn pause lock poisoned");
        loop {
            if self.targets.iter().any(|target| {
                TurnStatus::read(target.status.load(Ordering::Acquire)) != TurnStatus::Active
            }) {
                return false;
            }
            if self.targets.iter().all(|target| {
                state
                    .entries
                    .get(&target.id())
                    .is_some_and(|entry| entry.claim == self.claim && entry.acknowledged)
            }) {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let (next, _) = self
                .coordinator
                .changed
                .wait_timeout(state, (deadline - now).min(Duration::from_millis(10)))
                .expect("turn pause wait poisoned");
            state = next;
        }
    }

    /// Wait for both realm turns to reach a boundary. The caller supplies the request's live
    /// state; scheduler contention does not turn a valid ownership move into a refusal.
    pub fn wait_while(&self, mut pending: impl FnMut() -> bool) -> bool {
        let on_lane = ACTIVE_LANE.with(|slot| {
            slot.get()
                .is_some_and(|(queue, _)| queue == self.coordinator.queue_identity)
        });
        if on_lane {
            return false;
        }
        let mut state = self
            .coordinator
            .state
            .lock()
            .expect("turn pause lock poisoned");
        loop {
            if !pending()
                || self.targets.iter().any(|target| {
                    TurnStatus::read(target.status.load(Ordering::Acquire)) != TurnStatus::Active
                })
            {
                return false;
            }
            if self.targets.iter().all(|target| {
                state
                    .entries
                    .get(&target.id())
                    .is_some_and(|entry| entry.claim == self.claim && entry.acknowledged)
            }) {
                return true;
            }
            state = self
                .coordinator
                .changed
                .wait_timeout(state, Duration::from_millis(10))
                .expect("turn pause wait poisoned")
                .0;
        }
    }

    /// Ask one paused body to perform a bounded control operation on a scheduler lane. A pinned
    /// body runs on its owner lane; a movable body can run on any lane between turns. Both
    /// bodies must already have acknowledged this claim. The operation is synchronous and must
    /// neither run JavaScript nor block on either realm; the claim stays held until it returns.
    pub fn control(
        &self,
        target: &TurnPauseTarget,
        command: PausedTurnCommand,
    ) -> PausedTurnResult {
        let Some(pending) = self.enqueue_control(target, command, None) else {
            return PausedTurnResult::Unavailable;
        };
        pending.wait(Duration::from_secs(1))
    }

    pub fn control_while(
        &self,
        target: &TurnPauseTarget,
        command: PausedTurnCommand,
        pending: impl FnMut() -> bool,
    ) -> PausedTurnResult {
        let Some(control) = self.enqueue_control(target, command, None) else {
            return PausedTurnResult::Unavailable;
        };
        control.wait_while(pending)
    }

    /// Choose distinct lanes for two callbacks that must both enter before either unlinks a
    /// graph. A pinned pair on one lane, or a one-lane pool with two movable bodies, refuses.
    pub fn concurrent_control_lanes(
        &self,
        first: &TurnPauseTarget,
        second: &TurnPauseTarget,
    ) -> Option<[usize; 2]> {
        if Arc::ptr_eq(&first.ticket, &second.ticket) {
            return None;
        }
        let a = self.member(first)?;
        let b = self.member(second)?;
        let a_pin = a.ticket.pinned_lane();
        let b_pin = b.ticket.pinned_lane();
        let choose = |avoid: usize| (0..self._scheduler.base_lanes).find(|lane| *lane != avoid);
        match (a_pin, b_pin) {
            (Some(a), Some(b)) if a != b => Some([a, b]),
            (Some(_), Some(_)) => None,
            (Some(a), None) => Some([a, choose(a)?]),
            (None, Some(b)) => Some([choose(b)?, b]),
            (None, None) if self._scheduler.base_lanes >= 2 => Some([0, 1]),
            (None, None) => None,
        }
    }

    /// Queue without waiting so two graph handoff callbacks can enter on distinct lanes.
    /// `lane` is a reviewed concurrent-control choice for a movable body; a pinned body must
    /// use its owner lane.
    pub fn enqueue_control(
        &self,
        target: &TurnPauseTarget,
        command: PausedTurnCommand,
        lane: Option<usize>,
    ) -> Option<PendingTurnControl> {
        let on_lane = ACTIVE_LANE.with(|slot| {
            slot.get()
                .is_some_and(|(queue, _)| queue == self.coordinator.queue_identity)
        });
        if on_lane
            || self.member(target).is_none()
            || !self
                .targets
                .iter()
                .all(|target| self.coordinator.acknowledged_for(target.id(), self.claim))
        {
            return None;
        }
        let target = self.member(target)?;
        if TurnStatus::read(target.status.load(Ordering::Acquire)) != TurnStatus::Active {
            return None;
        }
        let lane = match (target.ticket.pinned_lane(), lane) {
            (Some(owner), Some(requested)) if owner != requested => return None,
            (Some(owner), _) => owner,
            (None, Some(requested)) if requested < self._scheduler.base_lanes => requested,
            (None, Some(_)) => return None,
            (None, None) => 0,
        };
        let (result, received) = mpsc::sync_channel(1);
        let permit = Arc::new(AtomicU8::new(PAUSED_CONTROL_PENDING));
        self._scheduler.lanes[lane].command(Command::PausedControl {
            realm_id: target.id(),
            claim: self.claim,
            command,
            permit: Arc::clone(&permit),
            result,
        });
        Some(PendingTurnControl {
            permit,
            result: received,
        })
    }
}

impl Drop for TurnPairClaim {
    fn drop(&mut self) {
        let actions = {
            let mut state = self
                .coordinator
                .state
                .lock()
                .expect("turn pause lock poisoned");
            let mut actions = Vec::new();
            for target in &self.targets {
                if state
                    .entries
                    .get(&target.id())
                    .is_some_and(|entry| entry.claim == self.claim)
                {
                    let entry = state
                        .entries
                        .remove(&target.id())
                        .expect("claimed entry exists");
                    if let Some(action) = entry.deferred_cancel {
                        actions.push(action);
                    }
                }
            }
            self.coordinator
                .pending
                .store(state.entries.len(), Ordering::Release);
            self.coordinator.changed.notify_all();
            actions
        };
        for target in &self.targets {
            target.ticket.wake();
        }
        for action in actions {
            action();
        }
    }
}

/// The cross-thread name of a realm hosted on one scheduler lane.
pub struct PinnedTurnHandle {
    ticket: Arc<TurnTicket>,
    lane: Arc<LaneControl>,
    status: Arc<AtomicU8>,
    pause: Arc<TurnPauseCoordinator>,
}

impl PinnedTurnHandle {
    pub fn pause_target(&self) -> TurnPauseTarget {
        TurnPauseTarget {
            ticket: Arc::clone(&self.ticket),
            status: Arc::clone(&self.status),
            coordinator: Arc::clone(&self.pause),
        }
    }
    pub fn id(&self) -> u64 {
        self.ticket.realm_id()
    }

    pub fn status(&self) -> TurnStatus {
        TurnStatus::read(self.status.load(Ordering::Acquire))
    }

    /// Queue one turn, coalescing with any wake already outstanding.
    pub fn wake(&self) -> bool {
        self.ticket.wake()
    }

    /// Retire the realm and drop its body on its owning lane.
    pub fn cancel(&self) {
        if self
            .status
            .compare_exchange(
                TurnStatus::Active as u8,
                TurnStatus::Cancelled as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            self.ticket.cancel();
            let id = self.id();
            let lane = Arc::clone(&self.lane);
            let action: Box<dyn FnOnce() + Send> =
                Box::new(move || lane.command(Command::Remove(id)));
            if let Err(action) = self.pause.defer_cancel(id, action) {
                action();
            }
        }
    }
}

impl Drop for PinnedTurnHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// The cross-thread name of a movable realm. Its body is exclusively leased for each turn.
pub struct MovableTurnHandle {
    entry: Arc<MovableEntry>,
    realms: MovableRealms,
    lanes: Arc<Vec<Arc<LaneControl>>>,
    pause: Arc<TurnPauseCoordinator>,
}

impl MovableTurnHandle {
    pub fn pause_target(&self) -> TurnPauseTarget {
        TurnPauseTarget {
            ticket: Arc::clone(&self.entry.ticket),
            status: Arc::clone(&self.entry.status),
            coordinator: Arc::clone(&self.pause),
        }
    }
    pub fn id(&self) -> u64 {
        self.entry.ticket.realm_id()
    }

    pub fn status(&self) -> TurnStatus {
        TurnStatus::read(self.entry.status.load(Ordering::Acquire))
    }

    pub fn wake(&self) -> bool {
        self.entry.ticket.wake()
    }

    pub fn cancel(&self) {
        if self
            .entry
            .status
            .compare_exchange(
                TurnStatus::Active as u8,
                TurnStatus::Cancelled as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            self.entry.ticket.cancel();
            let entry = Arc::clone(&self.entry);
            let realms = Arc::clone(&self.realms);
            let lanes = Arc::clone(&self.lanes);
            let id = self.id();
            let action: Box<dyn FnOnce() + Send> = Box::new(move || {
                if let Some(lane) = entry.ticket.pinned_lane() {
                    lanes[lane].command(Command::Remove(id));
                } else {
                    // A running turn can return its body just before cancellation. Both paths
                    // use the body lock, so either path takes the sole remaining owner.
                    let body = entry
                        .body
                        .lock()
                        .expect("movable body lock poisoned")
                        .take();
                    drop(body);
                    realms
                        .lock()
                        .expect("movable realm lock poisoned")
                        .remove(&id);
                }
            });
            if let Err(action) = self.pause.defer_cancel(id, action) {
                action();
            }
        }
    }
}

impl Drop for MovableTurnHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Fixed scheduler lanes sharing one Tokio reactor and ready queue.
/// Pinned realms always requeue onto their owning lane. The same queue can also hand an
/// unconstrained ticket to the first available lane after its body's migration audit.
pub struct PinnedTurnScheduler {
    reactor: Arc<Reactor>,
    movable: MovableRealms,
    lanes: Arc<Vec<Arc<LaneControl>>>,
    pause: Arc<TurnPauseCoordinator>,
    base_lanes: usize,
    deterministic: bool,
    threads: Vec<std::thread::JoinHandle<()>>,
    replacements: Mutex<Vec<Option<std::thread::JoinHandle<()>>>>,
    next_realm_id: AtomicU64,
}

/// A synchronous operation holds this until it returns. Dropping it drains the replacement
/// lane; a realm that pinned there keeps that lane alive until its body is destroyed.
pub struct BlockingTurnGuard {
    _scheduler: Arc<PinnedTurnScheduler>,
    lane: Arc<LaneControl>,
}

impl Drop for BlockingTurnGuard {
    fn drop(&mut self) {
        self.lane.command(Command::Drain);
    }
}

impl PinnedTurnScheduler {
    pub fn new(lane_count: usize) -> std::io::Result<Self> {
        let reactor = Reactor::shared_wall().map_err(|error| error.0)?;
        let base_lanes = lane_count.max(1);
        let max_replacements = std::env::var("KILN_SCHEDULER_REPLACEMENTS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(base_lanes)
            .min(64);
        Self::with_reactor(base_lanes, max_replacements, reactor)
    }

    /// One FIFO lane with its own pausable reactor for reproducible realm turns.
    /// The factory may pause this reactor before it schedules deterministic tasks.
    pub fn deterministic() -> std::io::Result<Self> {
        let reactor = Reactor::current_thread().map_err(|error| error.0)?;
        let mut scheduler = Self::with_reactor(1, 0, reactor)?;
        scheduler.deterministic = true;
        Ok(scheduler)
    }

    fn with_reactor(
        lane_count: usize,
        max_replacements: usize,
        reactor: Arc<Reactor>,
    ) -> std::io::Result<Self> {
        let total_lanes = lane_count.saturating_add(max_replacements);
        let queue = ReadyQueue::with_lanes(total_lanes);
        let lanes = Arc::new(
            (0..total_lanes)
                .map(|index| {
                    Arc::new(LaneControl {
                        index,
                        control: queue.ticket_on_lane(0, index),
                        queue: Arc::clone(&queue),
                        commands: Mutex::new(VecDeque::new()),
                    })
                })
                .collect::<Vec<_>>(),
        );
        let mut scheduler = Self {
            reactor,
            movable: Arc::new(Mutex::new(HashMap::new())),
            pause: Arc::new(TurnPauseCoordinator::new(&queue)),
            lanes,
            base_lanes: lane_count,
            deterministic: false,
            threads: Vec::new(),
            replacements: Mutex::new((0..max_replacements).map(|_| None).collect()),
            next_realm_id: AtomicU64::new(1),
        };
        for index in 0..lane_count {
            let lane = Arc::clone(&scheduler.lanes[index]);
            let lane_for_thread = Arc::clone(&lane);
            let reactor_for_thread = Arc::clone(&scheduler.reactor);
            let movable_for_thread = Arc::clone(&scheduler.movable);
            let pause_for_thread = Arc::clone(&scheduler.pause);
            let thread = std::thread::Builder::new()
                .name(format!("kiln-turn-{index}"))
                .stack_size(crate::workers::WORKER_STACK)
                .spawn(move || {
                    run_lane(
                        lane_for_thread,
                        reactor_for_thread,
                        movable_for_thread,
                        pause_for_thread,
                        None,
                    )
                })?;
            scheduler.threads.push(thread);
        }
        Ok(scheduler)
    }

    pub fn lane_count(&self) -> usize {
        self.base_lanes
    }

    pub fn is_deterministic(&self) -> bool {
        self.deterministic
    }

    /// Claim two live scheduler bodies in ascending realm-ID order. `wait` must run outside a
    /// lane after the requesting language turn has returned. The claim cannot authorize a graph
    /// move by itself: caller must still prove roots and reserve the receiver mailbox.
    pub fn claim_pair(
        self: &Arc<Self>,
        a: &TurnPauseTarget,
        b: &TurnPauseTarget,
    ) -> Result<TurnPairClaim, TurnClaimError> {
        if a.id() == b.id() {
            return Err(TurnClaimError::SameRealm);
        }
        self.claim_targets(vec![a.clone(), b.clone()], None)
    }

    pub fn claim_pair_while(
        self: &Arc<Self>,
        a: &TurnPauseTarget,
        b: &TurnPauseTarget,
        mut pending: impl FnMut() -> bool,
    ) -> Result<TurnPairClaim, TurnClaimError> {
        if a.id() == b.id() {
            return Err(TurnClaimError::SameRealm);
        }
        self.claim_targets(vec![a.clone(), b.clone()], Some(&mut pending))
    }

    /// Claim one scheduled realm when its peer has a dedicated owner thread. The caller
    /// separately claims that peer's gate and keeps both leases through publication.
    pub fn claim_one(
        self: &Arc<Self>,
        target: &TurnPauseTarget,
    ) -> Result<TurnPairClaim, TurnClaimError> {
        self.claim_targets(vec![target.clone()], None)
    }

    pub fn claim_one_while(
        self: &Arc<Self>,
        target: &TurnPauseTarget,
        mut pending: impl FnMut() -> bool,
    ) -> Result<TurnPairClaim, TurnClaimError> {
        self.claim_targets(vec![target.clone()], Some(&mut pending))
    }

    fn claim_targets(
        self: &Arc<Self>,
        mut targets: Vec<TurnPauseTarget>,
        mut pending: Option<&mut dyn FnMut() -> bool>,
    ) -> Result<TurnPairClaim, TurnClaimError> {
        if targets.is_empty()
            || targets
                .iter()
                .any(|target| !Arc::ptr_eq(&self.pause, &target.coordinator))
        {
            return Err(TurnClaimError::ForeignScheduler);
        }
        targets.sort_by_key(TurnPauseTarget::id);
        let claim = {
            let mut state = self.pause.state.lock().expect("turn pause lock poisoned");
            loop {
                if targets.iter().any(|target| {
                    TurnStatus::read(target.status.load(Ordering::Acquire)) != TurnStatus::Active
                }) || pending.as_mut().is_some_and(|live| !live())
                {
                    return Err(TurnClaimError::Unavailable);
                }
                if !targets
                    .iter()
                    .any(|target| state.entries.contains_key(&target.id()))
                {
                    break;
                }
                if pending.is_none() {
                    return Err(TurnClaimError::Busy);
                }
                state = self
                    .pause
                    .changed
                    .wait_timeout(state, Duration::from_millis(10))
                    .expect("turn pause wait poisoned")
                    .0;
            }
            let claim = state.next_claim;
            state.next_claim = state
                .next_claim
                .checked_add(1)
                .ok_or(TurnClaimError::Unavailable)?;
            for target in &targets {
                state.entries.insert(
                    target.id(),
                    TurnPauseEntry {
                        claim,
                        acknowledged: false,
                        deferred_cancel: None,
                    },
                );
            }
            self.pause
                .pending
                .store(state.entries.len(), Ordering::Release);
            claim
        };
        for target in &targets {
            target.ticket.wake();
        }
        Ok(TurnPairClaim {
            _scheduler: Arc::clone(self),
            coordinator: Arc::clone(&self.pause),
            claim,
            targets,
        })
    }

    /// Supply one temporary lane for movable work while a scheduled turn blocks its OS thread.
    /// The configured replacement limit is a hard bound; exhaustion is returned to the caller.
    pub fn enter_blocking(self: &Arc<Self>) -> io::Result<BlockingTurnGuard> {
        let mut replacements = self
            .replacements
            .lock()
            .expect("scheduler replacement lock poisoned");
        for slot in replacements.iter_mut() {
            if slot.as_ref().is_some_and(|thread| thread.is_finished()) {
                let _ = slot.take().expect("finished replacement exists").join();
            }
        }
        let Some((slot, place)) = replacements
            .iter_mut()
            .enumerate()
            .find(|(_, place)| place.is_none())
        else {
            return Err(io::Error::other(
                "scheduler replacement lane limit exhausted",
            ));
        };
        let lane = Arc::clone(&self.lanes[self.base_lanes + slot]);
        let lane_for_thread = Arc::clone(&lane);
        let reactor = Arc::clone(&self.reactor);
        let movable = Arc::clone(&self.movable);
        let pause = Arc::clone(&self.pause);
        let queue_identity = Arc::as_ptr(&self.lanes[0].queue) as usize;
        let owner = ACTIVE_LANE.with(|slot| {
            slot.get()
                .and_then(|(identity, lane)| (identity == queue_identity).then_some(lane))
                .unwrap_or(0)
        });
        let handoff = Arc::clone(&self.lanes[owner]);
        let thread = std::thread::Builder::new()
            .name(format!("kiln-turn-replacement-{slot}"))
            .stack_size(crate::workers::WORKER_STACK)
            .spawn(move || run_lane(lane_for_thread, reactor, movable, pause, Some(handoff)))?;
        *place = Some(thread);
        Ok(BlockingTurnGuard {
            _scheduler: Arc::clone(self),
            lane,
        })
    }

    /// Construct a realm on one lane and queue its first turn. The factory is sent across
    /// threads; the returned body need not be `Send` and never leaves its lane.
    pub fn spawn_pinned<F, T>(&self, factory: F) -> PinnedTurnHandle
    where
        F: FnOnce(Arc<Reactor>, Arc<TurnTicket>) -> T + Send + 'static,
        T: PinnedTurn + 'static,
    {
        self.spawn_pinned_with_target(move |reactor, ticket, _| factory(reactor, ticket))
    }

    /// Construct a pinned body with its cross-thread pause identity. The target has no
    /// reference to the lane-local body and is safe to register under a provider realm ID.
    pub fn spawn_pinned_with_target<F, T>(&self, factory: F) -> PinnedTurnHandle
    where
        F: FnOnce(Arc<Reactor>, Arc<TurnTicket>, TurnPauseTarget) -> T + Send + 'static,
        T: PinnedTurn + 'static,
    {
        let id = self.next_realm_id.fetch_add(1, Ordering::Relaxed);
        let lane = Arc::clone(&self.lanes[(id as usize - 1) % self.base_lanes]);
        let ticket = lane.queue.ticket_on_lane(id, lane.index);
        let status = Arc::new(AtomicU8::new(TurnStatus::Active as u8));
        let target = TurnPauseTarget {
            ticket: Arc::clone(&ticket),
            status: Arc::clone(&status),
            coordinator: Arc::clone(&self.pause),
        };
        lane.command(Command::Install {
            realm_id: id,
            ticket: Arc::clone(&ticket),
            status: Arc::clone(&status),
            factory: Box::new(move |reactor, ticket| Box::new(factory(reactor, ticket, target))),
        });
        PinnedTurnHandle {
            ticket,
            lane,
            status,
            pause: Arc::clone(&self.pause),
        }
    }

    /// Queue a body that is safe to move between scheduler lanes after each completed turn.
    /// Its constructor runs on the caller's thread, so it must not capture lane-local state.
    pub fn spawn_movable<T>(&self, body: T) -> MovableTurnHandle
    where
        T: MovableTurn + 'static,
    {
        let id = self.next_realm_id.fetch_add(1, Ordering::Relaxed);
        let ticket = self.lanes[0].queue.ticket(id);
        let entry = Arc::new(MovableEntry {
            body: Mutex::new(Some(Box::new(body))),
            status: Arc::new(AtomicU8::new(TurnStatus::Active as u8)),
            ticket: Arc::clone(&ticket),
            deadline_epoch: AtomicU64::new(0),
        });
        self.movable
            .lock()
            .expect("movable realm lock poisoned")
            .insert(id, Arc::clone(&entry));
        ticket.wake();
        MovableTurnHandle {
            entry,
            realms: Arc::clone(&self.movable),
            lanes: Arc::clone(&self.lanes),
            pause: Arc::clone(&self.pause),
        }
    }

    /// Construct a movable body on one scheduler lane. The factory may permanently pin its
    /// ticket there before the first wake if initialization publishes thread-affine state.
    pub fn spawn_movable_with<F, T>(&self, factory: F) -> MovableTurnHandle
    where
        F: FnOnce(Arc<Reactor>, Arc<TurnTicket>, usize) -> T + Send + 'static,
        T: MovableTurn + 'static,
    {
        self.spawn_movable_with_target(move |reactor, ticket, lane, _| {
            factory(reactor, ticket, lane)
        })
    }

    /// Construct a movable body with its cross-thread pause identity. The target contains only
    /// scheduler state and may be registered under a host realm ID during construction.
    pub fn spawn_movable_with_target<F, T>(&self, factory: F) -> MovableTurnHandle
    where
        F: FnOnce(Arc<Reactor>, Arc<TurnTicket>, usize, TurnPauseTarget) -> T + Send + 'static,
        T: MovableTurn + 'static,
    {
        let id = self.next_realm_id.fetch_add(1, Ordering::Relaxed);
        let lane = Arc::clone(&self.lanes[(id as usize - 1) % self.base_lanes]);
        let entry = Arc::new(MovableEntry {
            body: Mutex::new(None),
            status: Arc::new(AtomicU8::new(TurnStatus::Active as u8)),
            ticket: lane.queue.ticket(id),
            deadline_epoch: AtomicU64::new(0),
        });
        let target = TurnPauseTarget {
            ticket: Arc::clone(&entry.ticket),
            status: Arc::clone(&entry.status),
            coordinator: Arc::clone(&self.pause),
        };
        lane.command(Command::InstallMovable {
            entry: Arc::clone(&entry),
            factory: Box::new(move |reactor, ticket, lane| {
                Box::new(factory(reactor, ticket, lane, target))
            }),
        });
        MovableTurnHandle {
            entry,
            realms: Arc::clone(&self.movable),
            lanes: Arc::clone(&self.lanes),
            pause: Arc::clone(&self.pause),
        }
    }
}

impl Drop for PinnedTurnScheduler {
    fn drop(&mut self) {
        if let Some(lane) = self.lanes.first() {
            lane.queue.close();
        }
        for thread in self.threads.drain(..) {
            if thread.thread().id() != std::thread::current().id() {
                let _ = thread.join();
            }
        }
        for thread in self
            .replacements
            .get_mut()
            .expect("scheduler replacement lock poisoned")
            .iter_mut()
            .filter_map(Option::take)
        {
            if thread.thread().id() != std::thread::current().id() {
                let _ = thread.join();
            }
        }
    }
}

fn run_lane(
    lane: Arc<LaneControl>,
    reactor: Arc<Reactor>,
    movable: MovableRealms,
    pause: Arc<TurnPauseCoordinator>,
    handoff: Option<Arc<LaneControl>>,
) {
    let _active_lane = ActiveLaneGuard::enter(&lane.queue, lane.index);
    let mut realms = HashMap::<u64, RealmEntry>::new();
    // One heap per lane schedules all pure realm timers without a Tokio task or OS thread per
    // realm. The epoch invalidates an old deadline when an external wake runs that realm first.
    let mut deadlines = BinaryHeap::<Reverse<(Instant, u64, u64)>>::new();
    let mut draining = false;
    loop {
        let now = Instant::now();
        while let Some(&Reverse((at, realm_id, epoch))) = deadlines.peek() {
            if at > now {
                break;
            }
            deadlines.pop();
            if let Some(realm) = realms.get(&realm_id) {
                if realm.deadline_epoch == epoch {
                    realm.ticket.wake();
                }
            } else {
                let entry = movable
                    .lock()
                    .expect("movable realm lock poisoned")
                    .get(&realm_id)
                    .cloned();
                if let Some(realm) = entry
                    && realm.deadline_epoch.load(Ordering::Acquire) == epoch
                {
                    realm.ticket.wake();
                }
            }
        }
        if draining && realms.is_empty() {
            if let Some(handoff) = &handoff {
                handoff.command(Command::Deadlines(std::mem::take(&mut deadlines)));
            }
            break;
        }
        let lease = match deadlines.peek() {
            Some(&Reverse((at, _, _))) => lane
                .queue
                .take_timeout_for_lane(lane.index, at.saturating_duration_since(now)),
            None => lane.queue.take_for_lane(lane.index),
        };
        let Some(lease) = lease else {
            // A timed wait returns None both on timeout and on shutdown. A closed queue rejects
            // the control ticket, so use the next deadline to distinguish the ordinary timeout.
            if deadlines
                .peek()
                .is_some_and(|Reverse((at, _, _))| *at <= Instant::now())
            {
                continue;
            }
            break;
        };
        let realm_id = lease.realm_id();
        if realm_id == 0 {
            let commands = {
                let mut shared = lane
                    .commands
                    .lock()
                    .expect("scheduler command lock poisoned");
                std::mem::take(&mut *shared)
            };
            for command in commands {
                match command {
                    Command::Install {
                        realm_id,
                        ticket,
                        status,
                        factory,
                    } => {
                        if status.load(Ordering::Acquire) != TurnStatus::Active as u8 {
                            continue;
                        }
                        match catch_unwind(AssertUnwindSafe(|| {
                            factory(Arc::clone(&reactor), Arc::clone(&ticket))
                        })) {
                            Ok(body) => {
                                realms.insert(
                                    realm_id,
                                    RealmEntry {
                                        body,
                                        status,
                                        ticket: Arc::clone(&ticket),
                                        deadline_epoch: 0,
                                    },
                                );
                                ticket.wake();
                            }
                            Err(_) => {
                                status.store(TurnStatus::Panicked as u8, Ordering::Release);
                                ticket.cancel();
                            }
                        }
                    }
                    Command::InstallMovable { entry, factory } => {
                        if entry.status.load(Ordering::Acquire) != TurnStatus::Active as u8 {
                            continue;
                        }
                        let ticket = Arc::clone(&entry.ticket);
                        match catch_unwind(AssertUnwindSafe(|| {
                            factory(Arc::clone(&reactor), Arc::clone(&ticket), lane.index)
                        })) {
                            Ok(body) => {
                                let mut registry =
                                    movable.lock().expect("movable realm lock poisoned");
                                if entry.status.load(Ordering::Acquire) == TurnStatus::Active as u8
                                {
                                    if ticket.pinned_lane() == Some(lane.index) {
                                        realms.insert(
                                            ticket.realm_id(),
                                            RealmEntry {
                                                body,
                                                status: Arc::clone(&entry.status),
                                                ticket: Arc::clone(&ticket),
                                                deadline_epoch: 0,
                                            },
                                        );
                                    } else {
                                        *entry.body.lock().expect("movable body lock poisoned") =
                                            Some(body);
                                        registry.insert(ticket.realm_id(), entry);
                                    }
                                    drop(registry);
                                    ticket.wake();
                                }
                            }
                            Err(_) => {
                                entry
                                    .status
                                    .store(TurnStatus::Panicked as u8, Ordering::Release);
                                ticket.cancel();
                            }
                        }
                    }
                    Command::Remove(realm_id) => {
                        realms.remove(&realm_id);
                    }
                    Command::PausedControl {
                        realm_id,
                        claim,
                        command,
                        permit,
                        result,
                    } => {
                        if permit
                            .compare_exchange(
                                PAUSED_CONTROL_PENDING,
                                PAUSED_CONTROL_RUNNING,
                                Ordering::AcqRel,
                                Ordering::Acquire,
                            )
                            .is_err()
                        {
                            continue;
                        }
                        let answer = if !pause.acknowledged_for(realm_id, claim) {
                            PausedTurnResult::Unavailable
                        } else if let Some(realm) = realms.get_mut(&realm_id) {
                            if TurnStatus::read(realm.status.load(Ordering::Acquire))
                                != TurnStatus::Active
                            {
                                PausedTurnResult::Unavailable
                            } else {
                                match catch_unwind(AssertUnwindSafe(|| {
                                    realm.body.paused_control(command, lane.index)
                                })) {
                                    Ok(answer) => answer,
                                    Err(_) => {
                                        realm
                                            .status
                                            .store(TurnStatus::Panicked as u8, Ordering::Release);
                                        realm.ticket.cancel();
                                        let owner = Arc::clone(&lane);
                                        let action: Box<dyn FnOnce() + Send> =
                                            Box::new(move || {
                                                owner.command(Command::Remove(realm_id));
                                            });
                                        if let Err(action) = pause.defer_cancel(realm_id, action) {
                                            action();
                                        }
                                        PausedTurnResult::Unavailable
                                    }
                                }
                            }
                        } else {
                            let entry = movable
                                .lock()
                                .expect("movable realm lock poisoned")
                                .get(&realm_id)
                                .cloned();
                            if let Some(entry) = entry
                                && TurnStatus::read(entry.status.load(Ordering::Acquire))
                                    == TurnStatus::Active
                            {
                                let answer = {
                                    let mut body =
                                        entry.body.lock().expect("movable body lock poisoned");
                                    body.as_mut().map(|body| {
                                        catch_unwind(AssertUnwindSafe(|| {
                                            body.paused_control(command, lane.index)
                                        }))
                                    })
                                };
                                match answer {
                                    Some(Ok(answer)) => answer,
                                    Some(Err(_)) => {
                                        entry
                                            .status
                                            .store(TurnStatus::Panicked as u8, Ordering::Release);
                                        entry.ticket.cancel();
                                        let registry = Arc::clone(&movable);
                                        let action: Box<dyn FnOnce() + Send> =
                                            Box::new(move || {
                                                registry
                                                    .lock()
                                                    .expect("movable realm lock poisoned")
                                                    .remove(&realm_id);
                                            });
                                        if let Err(action) = pause.defer_cancel(realm_id, action) {
                                            action();
                                        }
                                        PausedTurnResult::Unavailable
                                    }
                                    None => PausedTurnResult::Unavailable,
                                }
                            } else {
                                PausedTurnResult::Unavailable
                            }
                        };
                        let _ = result.send(answer);
                    }
                    Command::Drain => draining = true,
                    Command::Deadlines(other) => deadlines.extend(other),
                }
            }
            lease.finish(TurnOutcome::Parked);
            continue;
        }
        let Some(realm) = realms.get_mut(&realm_id) else {
            run_movable(
                lease,
                lane.index,
                &movable,
                &pause,
                &mut realms,
                &mut deadlines,
            );
            continue;
        };
        if pause.acknowledge_boundary(realm_id) {
            lease.finish(TurnOutcome::Parked);
            continue;
        }
        let waker = Waker::from(lease.ticket());
        let result = catch_unwind(AssertUnwindSafe(|| {
            realm.body.run_turn_on(&waker, lane.index)
        }));
        match result {
            Ok(outcome) => {
                let done = matches!(outcome, TurnOutcome::Done);
                realm.deadline_epoch = realm.deadline_epoch.wrapping_add(1);
                if let TurnOutcome::ParkedUntil(at) = &outcome {
                    deadlines.push(Reverse((*at, realm_id, realm.deadline_epoch)));
                }
                if done {
                    let _ = realm.status.compare_exchange(
                        TurnStatus::Active as u8,
                        TurnStatus::Complete as u8,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    );
                }
                lease.finish(outcome);
                if done {
                    realms.remove(&realm_id);
                }
            }
            Err(_) => {
                realm
                    .status
                    .store(TurnStatus::Panicked as u8, Ordering::Release);
                lease.finish(TurnOutcome::Done);
                realms.remove(&realm_id);
            }
        }
    }
    // The thread that constructed each `!Send` body also drops it.
}

fn run_movable(
    lease: TurnLease,
    lane: usize,
    movable: &MovableRealms,
    pause: &TurnPauseCoordinator,
    pinned: &mut HashMap<u64, RealmEntry>,
    deadlines: &mut BinaryHeap<Reverse<(Instant, u64, u64)>>,
) {
    let id = lease.realm_id();
    let Some(entry) = movable
        .lock()
        .expect("movable realm lock poisoned")
        .get(&id)
        .cloned()
    else {
        lease.finish(TurnOutcome::Done);
        return;
    };
    if entry.status.load(Ordering::Acquire) != TurnStatus::Active as u8 {
        lease.finish(TurnOutcome::Done);
        return;
    }
    if pause.acknowledge_boundary(id) {
        lease.finish(TurnOutcome::Parked);
        return;
    }
    let Some(mut body) = entry
        .body
        .lock()
        .expect("movable body lock poisoned")
        .take()
    else {
        // An exclusive ticket means another lane cannot still own this body.
        entry
            .status
            .store(TurnStatus::Panicked as u8, Ordering::Release);
        lease.finish(TurnOutcome::Done);
        movable
            .lock()
            .expect("movable realm lock poisoned")
            .remove(&id);
        return;
    };
    let waker = Waker::from(lease.ticket());
    let result = catch_unwind(AssertUnwindSafe(|| body.run_turn_on(&waker, lane)));
    match result {
        Ok(outcome) => {
            let done = matches!(outcome, TurnOutcome::Done);
            let epoch = entry
                .deadline_epoch
                .fetch_add(1, Ordering::AcqRel)
                .wrapping_add(1);
            if let TurnOutcome::ParkedUntil(at) = &outcome {
                deadlines.push(Reverse((*at, id, epoch)));
            }
            if done {
                let _ = entry.status.compare_exchange(
                    TurnStatus::Active as u8,
                    TurnStatus::Complete as u8,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
            } else if entry.status.load(Ordering::Acquire) == TurnStatus::Active as u8 {
                if entry.ticket.pinned_lane() == Some(lane) {
                    pinned.insert(
                        id,
                        RealmEntry {
                            body,
                            status: Arc::clone(&entry.status),
                            ticket: Arc::clone(&entry.ticket),
                            deadline_epoch: epoch,
                        },
                    );
                    movable
                        .lock()
                        .expect("movable realm lock poisoned")
                        .remove(&id);
                } else {
                    let mut slot = entry.body.lock().expect("movable body lock poisoned");
                    if entry.status.load(Ordering::Acquire) == TurnStatus::Active as u8 {
                        *slot = Some(body);
                    }
                }
            }
            lease.finish(outcome);
            if done || entry.status.load(Ordering::Acquire) != TurnStatus::Active as u8 {
                movable
                    .lock()
                    .expect("movable realm lock poisoned")
                    .remove(&id);
            }
        }
        Err(_) => {
            entry
                .status
                .store(TurnStatus::Panicked as u8, Ordering::Release);
            lease.finish(TurnOutcome::Done);
            movable
                .lock()
                .expect("movable realm lock poisoned")
                .remove(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};

    #[test]
    fn one_scheduled_realm_can_pause_beside_a_dedicated_peer() {
        struct Body(Arc<AtomicUsize>);
        impl PinnedTurn for Body {
            fn run_turn(&mut self, _: &Waker) -> TurnOutcome {
                TurnOutcome::Parked
            }
            fn paused_control(&mut self, command: PausedTurnCommand, _: usize) -> PausedTurnResult {
                if command.kind != 7 {
                    return PausedTurnResult::Unavailable;
                }
                self.0.fetch_add(1, Ordering::SeqCst);
                PausedTurnResult::Ready
            }
        }
        let scheduler = Arc::new(PinnedTurnScheduler::new(1).expect("lane starts"));
        let visits = Arc::new(AtomicUsize::new(0));
        let realm = scheduler.spawn_pinned({
            let visits = Arc::clone(&visits);
            move |_, _| Body(visits)
        });
        let claim = scheduler
            .claim_one(&realm.pause_target())
            .expect("one claim");
        assert!(claim.wait(Duration::from_secs(1)));
        assert_eq!(
            claim.control(
                &realm.pause_target(),
                PausedTurnCommand {
                    kind: 7,
                    ids: [0; 3]
                }
            ),
            PausedTurnResult::Ready
        );
        assert_eq!(visits.load(Ordering::SeqCst), 1);
        drop(claim);
    }

    #[test]
    fn pair_control_runs_on_scheduler_lanes_without_resuming_turns() {
        struct ControlledBody {
            runs: Arc<AtomicUsize>,
            observed: std::sync::mpsc::Sender<std::thread::ThreadId>,
            owner: Option<std::thread::ThreadId>,
            pinned: bool,
            last_turn_lane: Option<usize>,
        }
        impl PinnedTurn for ControlledBody {
            fn run_turn(&mut self, _waker: &Waker) -> TurnOutcome {
                self.owner = Some(std::thread::current().id());
                self.runs.fetch_add(1, Ordering::SeqCst);
                self.observed.send(self.owner.unwrap()).unwrap();
                TurnOutcome::Parked
            }

            fn run_turn_on(&mut self, waker: &Waker, lane: usize) -> TurnOutcome {
                self.last_turn_lane = Some(lane);
                self.run_turn(waker)
            }

            fn paused_control(
                &mut self,
                command: PausedTurnCommand,
                lane: usize,
            ) -> PausedTurnResult {
                assert_eq!(command.kind, 7);
                assert_eq!(command.ids, [11, 12, 13]);
                assert_eq!(
                    ACTIVE_LANE.with(|slot| slot.get().map(|(_, active)| active)),
                    Some(lane)
                );
                if self.pinned {
                    assert_eq!(self.owner, Some(std::thread::current().id()));
                    assert_eq!(self.last_turn_lane, Some(lane));
                }
                PausedTurnResult::Ready
            }
        }

        let scheduler = Arc::new(PinnedTurnScheduler::new(2).expect("lanes start"));
        let (pinned_tx, pinned_rx) = std::sync::mpsc::channel();
        let (movable_tx, movable_rx) = std::sync::mpsc::channel();
        let pinned_runs = Arc::new(AtomicUsize::new(0));
        let movable_runs = Arc::new(AtomicUsize::new(0));
        let pinned = scheduler.spawn_pinned({
            let runs = Arc::clone(&pinned_runs);
            move |_, _| ControlledBody {
                runs,
                observed: pinned_tx,
                owner: None,
                pinned: true,
                last_turn_lane: None,
            }
        });
        let movable = scheduler.spawn_movable(ControlledBody {
            runs: Arc::clone(&movable_runs),
            observed: movable_tx,
            owner: None,
            pinned: false,
            last_turn_lane: None,
        });
        pinned_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        movable_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let claim = scheduler
            .claim_pair(&pinned.pause_target(), &movable.pause_target())
            .expect("pair claimed");
        let command = PausedTurnCommand {
            kind: 7,
            ids: [11, 12, 13],
        };
        assert_eq!(
            claim.control(&pinned.pause_target(), command),
            PausedTurnResult::Unavailable
        );
        assert!(claim.wait(Duration::from_secs(1)));
        assert_eq!(
            claim.control(&pinned.pause_target(), command),
            PausedTurnResult::Ready
        );
        assert_eq!(
            claim.control(&movable.pause_target(), command),
            PausedTurnResult::Ready
        );
        let lanes = claim
            .concurrent_control_lanes(&pinned.pause_target(), &movable.pause_target())
            .expect("distinct control lanes");
        assert_ne!(lanes[0], lanes[1]);
        let first = claim
            .enqueue_control(&pinned.pause_target(), command, Some(lanes[0]))
            .expect("pinned control queued");
        let second = claim
            .enqueue_control(&movable.pause_target(), command, Some(lanes[1]))
            .expect("movable control queued");
        assert_eq!(first.wait(Duration::from_secs(1)), PausedTurnResult::Ready);
        assert_eq!(second.wait(Duration::from_secs(1)), PausedTurnResult::Ready);
        assert_eq!(pinned_runs.load(Ordering::SeqCst), 1);
        assert_eq!(movable_runs.load(Ordering::SeqCst), 1);
        drop(claim);
    }

    #[test]
    fn panicking_pair_control_defers_body_drop_until_claim_release() {
        struct PanicBody(Arc<AtomicUsize>);
        impl PinnedTurn for PanicBody {
            fn run_turn(&mut self, _waker: &Waker) -> TurnOutcome {
                TurnOutcome::Parked
            }

            fn paused_control(
                &mut self,
                _command: PausedTurnCommand,
                _lane: usize,
            ) -> PausedTurnResult {
                panic!("control failed");
            }
        }
        impl Drop for PanicBody {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        let scheduler = Arc::new(PinnedTurnScheduler::new(2).expect("lanes start"));
        let drops = Arc::new(AtomicUsize::new(0));
        let first = scheduler.spawn_pinned({
            let drops = Arc::clone(&drops);
            move |_, _| PanicBody(drops)
        });
        let second = scheduler.spawn_movable(|_: &Waker| TurnOutcome::Parked);
        let claim = scheduler
            .claim_pair(&first.pause_target(), &second.pause_target())
            .expect("pair claimed");
        assert!(claim.wait(Duration::from_secs(1)));
        assert_eq!(
            claim.control(
                &first.pause_target(),
                PausedTurnCommand {
                    kind: 1,
                    ids: [0; 3]
                }
            ),
            PausedTurnResult::Unavailable
        );
        assert_eq!(first.status(), TurnStatus::Panicked);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(claim);
        let deadline = Instant::now() + Duration::from_secs(1);
        while drops.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < deadline, "panicked body was not removed");
            std::thread::yield_now();
        }
    }

    #[test]
    fn pair_control_cancels_a_command_behind_an_unrelated_blocked_turn() {
        struct ControlledBody(Arc<AtomicUsize>);
        impl PinnedTurn for ControlledBody {
            fn run_turn(&mut self, _waker: &Waker) -> TurnOutcome {
                TurnOutcome::Parked
            }

            fn paused_control(
                &mut self,
                _command: PausedTurnCommand,
                _lane: usize,
            ) -> PausedTurnResult {
                self.0.fetch_add(1, Ordering::SeqCst);
                PausedTurnResult::Ready
            }
        }

        let scheduler = Arc::new(PinnedTurnScheduler::new(1).expect("lane starts"));
        let controls = Arc::new(AtomicUsize::new(0));
        let first = scheduler.spawn_pinned({
            let controls = Arc::clone(&controls);
            move |_, _| ControlledBody(controls)
        });
        let second = scheduler.spawn_movable(|_: &Waker| TurnOutcome::Parked);
        let claim = scheduler
            .claim_pair(&first.pause_target(), &second.pause_target())
            .expect("pair claimed");
        assert!(claim.wait(Duration::from_secs(1)));

        let (entered, entered_rx) = std::sync::mpsc::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        let blocker = scheduler.spawn_pinned(move |_, _| {
            move |_: &Waker| {
                entered.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
                TurnOutcome::Done
            }
        });
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(
            claim.control(
                &first.pause_target(),
                PausedTurnCommand {
                    kind: 1,
                    ids: [0; 3]
                }
            ),
            PausedTurnResult::Unavailable
        );
        drop(claim);
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while blocker.status() == TurnStatus::Active {
            assert!(Instant::now() < deadline, "blocking turn did not finish");
            std::thread::yield_now();
        }
        assert_eq!(controls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn pair_claim_waits_for_turn_boundaries_and_defers_cancelled_body_drop() {
        struct ParkedBody {
            runs: Arc<AtomicUsize>,
            drops: Arc<AtomicUsize>,
        }
        impl PinnedTurn for ParkedBody {
            fn run_turn(&mut self, _waker: &Waker) -> TurnOutcome {
                self.runs.fetch_add(1, Ordering::SeqCst);
                TurnOutcome::Parked
            }
        }
        impl Drop for ParkedBody {
            fn drop(&mut self) {
                self.drops.fetch_add(1, Ordering::SeqCst);
            }
        }

        let scheduler =
            Arc::new(PinnedTurnScheduler::deterministic().expect("one deterministic lane"));
        let (entered, entered_rx) = std::sync::mpsc::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        let first_runs = Arc::new(AtomicUsize::new(0));
        let first = scheduler.spawn_pinned({
            let runs = Arc::clone(&first_runs);
            move |_, _| {
                let mut initial = true;
                move |_: &Waker| {
                    runs.fetch_add(1, Ordering::SeqCst);
                    if initial {
                        initial = false;
                        entered.send(()).unwrap();
                        release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                    }
                    TurnOutcome::Parked
                }
            }
        });
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let second_runs = Arc::new(AtomicUsize::new(0));
        let second_drops = Arc::new(AtomicUsize::new(0));
        let second = scheduler.spawn_movable(ParkedBody {
            runs: Arc::clone(&second_runs),
            drops: Arc::clone(&second_drops),
        });
        let first_target = first.pause_target();
        let second_target = second.pause_target();
        assert_eq!(
            scheduler.claim_pair(&first_target, &first_target).err(),
            Some(TurnClaimError::SameRealm)
        );
        let claim = scheduler
            .claim_pair(&second_target, &first_target)
            .expect("pair claimed");
        assert!(claim.targets[0].id() < claim.targets[1].id());
        assert!(
            !claim.wait(Duration::from_millis(10)),
            "running turn cannot acknowledge before returning"
        );
        assert_eq!(
            scheduler.claim_pair(&first_target, &second_target).err(),
            Some(TurnClaimError::Busy)
        );
        release.send(()).unwrap();
        assert!(
            claim.wait(Duration::from_secs(1)),
            "both realms reached a boundary"
        );
        let before = (
            first_runs.load(Ordering::SeqCst),
            second_runs.load(Ordering::SeqCst),
        );
        assert!(first.wake() && second.wake());
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(
            before,
            (
                first_runs.load(Ordering::SeqCst),
                second_runs.load(Ordering::SeqCst)
            ),
            "claimed bodies cannot run another turn"
        );
        second.cancel();
        assert!(
            !claim.wait(Duration::ZERO),
            "cancellation invalidates the pair"
        );
        assert_eq!(
            second_drops.load(Ordering::SeqCst),
            0,
            "a claimed body cannot be torn down"
        );
        drop(claim);
        let deadline = Instant::now() + Duration::from_secs(1);
        while first_runs.load(Ordering::SeqCst) == before.0
            || second_drops.load(Ordering::SeqCst) == 0
        {
            assert!(
                Instant::now() < deadline,
                "release did not resume and retire realms"
            );
            std::thread::yield_now();
        }
        first.cancel();
    }

    struct CountedTurn {
        first_lane: std::thread::ThreadId,
        seen: Arc<Mutex<Vec<std::thread::ThreadId>>>,
        remaining: usize,
    }

    impl PinnedTurn for CountedTurn {
        fn run_turn(&mut self, _waker: &Waker) -> TurnOutcome {
            let lane = std::thread::current().id();
            assert_eq!(lane, self.first_lane);
            self.seen.lock().unwrap().push(lane);
            self.remaining -= 1;
            if self.remaining == 0 {
                TurnOutcome::Done
            } else {
                TurnOutcome::Ready
            }
        }
    }

    #[test]
    fn many_pinned_realms_share_two_lanes_without_moving() {
        let scheduler = PinnedTurnScheduler::new(2).expect("lanes start");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let realms = (0..200)
            .map(|_| {
                let seen = Arc::clone(&seen);
                scheduler.spawn_pinned(move |_, _| CountedTurn {
                    first_lane: std::thread::current().id(),
                    seen,
                    remaining: 3,
                })
            })
            .collect::<Vec<_>>();
        let deadline = Instant::now() + Duration::from_secs(5);
        while realms
            .iter()
            .any(|realm| realm.status() == TurnStatus::Active)
        {
            assert!(Instant::now() < deadline, "pinned turns complete");
            std::thread::yield_now();
        }
        assert!(
            realms
                .iter()
                .all(|realm| realm.status() == TurnStatus::Complete)
        );
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 600);
        assert_eq!(
            seen.iter()
                .copied()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            2
        );
    }

    #[test]
    fn cancellation_drops_a_parked_body_on_its_owner_lane() {
        struct Parked {
            owner: std::thread::ThreadId,
            dropped: std::sync::mpsc::Sender<std::thread::ThreadId>,
        }
        impl PinnedTurn for Parked {
            fn run_turn(&mut self, _: &Waker) -> TurnOutcome {
                TurnOutcome::Parked
            }
        }
        impl Drop for Parked {
            fn drop(&mut self) {
                let lane = std::thread::current().id();
                assert_eq!(lane, self.owner);
                self.dropped.send(lane).expect("drop observed");
            }
        }

        let scheduler = PinnedTurnScheduler::new(1).expect("lane starts");
        let (created, ready) = std::sync::mpsc::channel();
        let (dropped, observed) = std::sync::mpsc::channel();
        let handle = scheduler.spawn_pinned(move |_, _| {
            let owner = std::thread::current().id();
            created.send(owner).expect("owner observed");
            Parked { owner, dropped }
        });
        let owner = ready
            .recv_timeout(Duration::from_secs(1))
            .expect("body created");
        handle.cancel();
        assert_eq!(
            observed
                .recv_timeout(Duration::from_secs(1))
                .expect("body dropped"),
            owner
        );
        assert_eq!(handle.status(), TurnStatus::Cancelled);
    }

    #[test]
    fn one_panicking_realm_does_not_stop_its_lane() {
        let scheduler = PinnedTurnScheduler::new(1).expect("lane starts");
        let bad = scheduler.spawn_pinned(|_, _| {
            |_waker: &Waker| -> TurnOutcome { panic!("deliberate scheduler test panic") }
        });
        let good = scheduler.spawn_pinned(|_, _| |_waker: &Waker| TurnOutcome::Done);
        let deadline = Instant::now() + Duration::from_secs(1);
        while good.status() == TurnStatus::Active {
            assert!(Instant::now() < deadline, "healthy realm completes");
            std::thread::yield_now();
        }
        assert_eq!(bad.status(), TurnStatus::Panicked);
        assert_eq!(good.status(), TurnStatus::Complete);
    }

    #[test]
    fn parked_local_timers_wake_many_realms_on_two_lanes() {
        struct TimerTurn {
            executor: crate::Executor,
            fired: std::rc::Rc<std::cell::Cell<bool>>,
            owner: std::thread::ThreadId,
        }
        impl PinnedTurn for TimerTurn {
            fn run_turn(&mut self, waker: &Waker) -> TurnOutcome {
                assert_eq!(std::thread::current().id(), self.owner);
                self.executor.poll_local_once(waker);
                if self.fired.get() {
                    TurnOutcome::Done
                } else {
                    TurnOutcome::Parked
                }
            }
        }

        let scheduler = PinnedTurnScheduler::new(2).expect("lanes start");
        let realms = (0..100)
            .map(|_| {
                scheduler.spawn_pinned(|reactor, _| {
                    let executor = crate::Executor::on_reactor(reactor);
                    let fired = std::rc::Rc::new(std::cell::Cell::new(false));
                    executor.scope(|| {
                        let fired = std::rc::Rc::clone(&fired);
                        tokio::task::spawn_local(async move {
                            tokio::time::sleep(Duration::from_millis(2)).await;
                            fired.set(true);
                        });
                    });
                    TimerTurn {
                        executor,
                        fired,
                        owner: std::thread::current().id(),
                    }
                })
            })
            .collect::<Vec<_>>();
        let deadline = Instant::now() + Duration::from_secs(5);
        while realms
            .iter()
            .any(|realm| realm.status() == TurnStatus::Active)
        {
            assert!(Instant::now() < deadline, "all parked timers wake");
            std::thread::yield_now();
        }
        assert!(
            realms
                .iter()
                .all(|realm| realm.status() == TurnStatus::Complete)
        );
    }

    #[test]
    fn pinned_tcp_http_callbacks_stay_on_the_attachment_lane() {
        use crate::{HostTransport, HttpDispatch, HttpEvent, HttpEventKind, HttpServerEngine};
        use kiln_host_abi::ResourceId;
        use std::cell::{Cell, RefCell};
        use std::io::{Read, Write};
        use std::rc::Rc;

        struct HttpTurn {
            executor: crate::Executor,
            done: Rc<Cell<bool>>,
            owner: std::thread::ThreadId,
        }
        impl PinnedTurn for HttpTurn {
            fn run_turn(&mut self, waker: &Waker) -> TurnOutcome {
                assert_eq!(std::thread::current().id(), self.owner);
                self.executor.poll_local_once(waker);
                if self.done.get() {
                    TurnOutcome::Done
                } else {
                    TurnOutcome::Parked
                }
            }
        }

        struct LaneDispatch {
            engine: Rc<RefCell<HttpServerEngine>>,
            resource: ResourceId,
            done: Rc<Cell<bool>>,
            owner: std::thread::ThreadId,
            events: Arc<Mutex<Vec<HttpEventKind>>>,
        }
        impl HttpDispatch for LaneDispatch {
            fn event(&self, event: &HttpEvent<'_>) -> i32 {
                assert_eq!(std::thread::current().id(), self.owner);
                self.events.lock().expect("event lock").push(event.kind);
                if event.kind == HttpEventKind::RequestHead {
                    let mut engine = self.engine.borrow_mut();
                    assert!(engine.start_response(self.resource, event.id, 200, b"", 1));
                    engine
                        .end_response(self.resource, event.id, b"lane-owned")
                        .expect("response ends");
                }
                if event.kind == HttpEventKind::Closed {
                    self.done.set(true);
                }
                0
            }

            fn checkpoint(&self) -> i32 {
                assert_eq!(std::thread::current().id(), self.owner);
                0
            }
        }

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback listener");
        let mut client =
            std::net::TcpStream::connect(listener.local_addr().unwrap()).expect("loopback client");
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("client read timeout");
        client
            .set_write_timeout(Some(Duration::from_secs(5)))
            .expect("client write timeout");
        let (server, _) = listener.accept().expect("loopback server accepts");
        server.set_nonblocking(true).expect("nonblocking server");

        let scheduler = PinnedTurnScheduler::new(2).expect("lanes start");
        let events = Arc::new(Mutex::new(Vec::new()));
        let handle = scheduler.spawn_pinned({
            let events = Arc::clone(&events);
            move |reactor, _| {
                let owner = std::thread::current().id();
                let executor = crate::Executor::on_reactor(reactor);
                let engine = Rc::new(RefCell::new(HttpServerEngine::default()));
                let done = Rc::new(Cell::new(false));
                let resource = ResourceId::new(91);
                let dispatch = Rc::new(LaneDispatch {
                    engine: Rc::clone(&engine),
                    resource,
                    done: Rc::clone(&done),
                    owner,
                    events,
                });
                executor.scope(|| {
                    let server = tokio::net::TcpStream::from_std(server)
                        .expect("socket attaches to the lane reactor");
                    assert!(engine.borrow_mut().attach(
                        resource,
                        HostTransport(Box::new(server)),
                        false,
                        dispatch,
                    ));
                });
                HttpTurn {
                    executor,
                    done,
                    owner,
                }
            }
        });
        let response = std::thread::spawn(move || {
            client
                .write_all(b"GET / HTTP/1.1\r\nHost: kiln\r\nConnection: close\r\n\r\n")
                .expect("request writes");
            let mut bytes = Vec::new();
            client.read_to_end(&mut bytes).expect("response reads");
            bytes
        })
        .join()
        .expect("client thread completes");
        assert!(
            response
                .windows(b"lane-owned".len())
                .any(|w| w == b"lane-owned")
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while handle.status() == TurnStatus::Active {
            assert!(Instant::now() < deadline, "HTTP realm completes");
            std::thread::yield_now();
        }
        assert_eq!(handle.status(), TurnStatus::Complete);
        let events = events.lock().expect("event lock");
        assert!(events.contains(&HttpEventKind::RequestHead));
        assert!(events.contains(&HttpEventKind::Closed));
    }

    #[test]
    fn deterministic_scheduler_uses_one_pausable_lane() {
        let scheduler = PinnedTurnScheduler::deterministic().expect("lane starts");
        assert_eq!(scheduler.lane_count(), 1);
        let lanes = Arc::new(Mutex::new(Vec::new()));
        let realms = (0..20)
            .map(|_| {
                let lanes = Arc::clone(&lanes);
                scheduler.spawn_pinned(move |reactor, _| {
                    let executor = crate::Executor::on_reactor(reactor);
                    executor.pause_time();
                    move |_: &Waker| {
                        assert!(executor.is_time_paused());
                        lanes.lock().unwrap().push(std::thread::current().id());
                        TurnOutcome::Done
                    }
                })
            })
            .collect::<Vec<_>>();
        let deadline = Instant::now() + Duration::from_secs(1);
        while realms
            .iter()
            .any(|realm| realm.status() == TurnStatus::Active)
        {
            assert!(Instant::now() < deadline, "deterministic turns complete");
            std::thread::yield_now();
        }
        assert!(
            realms
                .iter()
                .all(|realm| realm.status() == TurnStatus::Complete)
        );
        let lanes = lanes.lock().unwrap();
        assert_eq!(lanes.len(), 20);
        assert!(lanes.iter().all(|lane| *lane == lanes[0]));
    }

    #[test]
    fn pure_deadlines_wake_many_parked_realms_on_fixed_lanes() {
        let scheduler = PinnedTurnScheduler::new(2).expect("lanes start");
        let realms = (0..100)
            .map(|_| {
                scheduler.spawn_pinned(|_, _| {
                    let owner = std::thread::current().id();
                    let mut first = true;
                    move |_: &Waker| {
                        assert_eq!(std::thread::current().id(), owner);
                        if first {
                            first = false;
                            TurnOutcome::ParkedUntil(Instant::now() + Duration::from_millis(2))
                        } else {
                            TurnOutcome::Done
                        }
                    }
                })
            })
            .collect::<Vec<_>>();
        let deadline = Instant::now() + Duration::from_secs(1);
        while realms
            .iter()
            .any(|realm| realm.status() == TurnStatus::Active)
        {
            assert!(Instant::now() < deadline, "all pure deadlines wake");
            std::thread::yield_now();
        }
        assert!(
            realms
                .iter()
                .all(|realm| realm.status() == TurnStatus::Complete)
        );
    }

    #[test]
    fn a_movable_body_can_run_successive_turns_on_different_lanes() {
        let scheduler = PinnedTurnScheduler::new(2).expect("lanes start");
        let (first_blocked_tx, first_blocked_rx) = std::sync::mpsc::channel();
        let (first_release_tx, first_release_rx) = std::sync::mpsc::channel();
        let first_blocker = scheduler.spawn_pinned(move |_, _| {
            move |_: &Waker| {
                first_blocked_tx.send(()).unwrap();
                first_release_rx
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap();
                TurnOutcome::Done
            }
        });
        first_blocked_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();

        // The second pinned spawn consumes lane one in round-robin order.
        let lane_one = scheduler.spawn_pinned(|_, _| |_: &Waker| TurnOutcome::Done);
        let deadline = Instant::now() + Duration::from_secs(1);
        while lane_one.status() == TurnStatus::Active {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }

        let (seen_tx, seen_rx) = std::sync::mpsc::channel();
        let mut first = true;
        let movable = scheduler.spawn_movable(move |_: &Waker| {
            seen_tx.send(std::thread::current().id()).unwrap();
            if first {
                first = false;
                TurnOutcome::Parked
            } else {
                TurnOutcome::Done
            }
        });
        let first_lane = seen_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let (second_blocked_tx, second_blocked_rx) = std::sync::mpsc::channel();
        let (second_release_tx, second_release_rx) = std::sync::mpsc::channel();
        let second_blocker = scheduler.spawn_pinned(move |_, _| {
            move |_: &Waker| {
                second_blocked_tx.send(()).unwrap();
                second_release_rx
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap();
                TurnOutcome::Done
            }
        });
        second_blocked_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        first_release_tx.send(()).unwrap();
        while first_blocker.status() == TurnStatus::Active {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(movable.wake());
        let second_lane = seen_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_ne!(first_lane, second_lane);
        second_release_tx.send(()).unwrap();
        while movable.status() == TurnStatus::Active {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(movable.status(), TurnStatus::Complete);
        drop(second_blocker);
    }

    #[test]
    fn cancellation_drops_a_movable_body_that_is_running() {
        struct RunningBody {
            entered: std::sync::mpsc::Sender<()>,
            release: std::sync::mpsc::Receiver<()>,
            drops: Arc<std::sync::atomic::AtomicUsize>,
        }

        impl PinnedTurn for RunningBody {
            fn run_turn(&mut self, _waker: &Waker) -> TurnOutcome {
                self.entered.send(()).unwrap();
                self.release.recv_timeout(Duration::from_secs(2)).unwrap();
                TurnOutcome::Parked
            }
        }

        impl Drop for RunningBody {
            fn drop(&mut self) {
                self.drops.fetch_add(1, Ordering::Release);
            }
        }

        let scheduler = PinnedTurnScheduler::new(1).expect("lane starts");
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let drops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let body = RunningBody {
            entered: entered_tx,
            release: release_rx,
            drops: Arc::clone(&drops),
        };
        let handle = scheduler.spawn_movable(body);
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        handle.cancel();
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while drops.load(Ordering::Acquire) == 0 {
            assert!(Instant::now() < deadline, "cancelled body was retained");
            std::thread::yield_now();
        }
        assert_eq!(handle.status(), TurnStatus::Cancelled);
    }

    #[test]
    fn a_blocked_lane_gets_one_bounded_replacement_for_movable_work() {
        let reactor = Reactor::shared_wall().expect("reactor starts");
        let scheduler = Arc::new(
            PinnedTurnScheduler::with_reactor(1, 1, reactor).expect("scheduler lanes start"),
        );
        let (blocked_tx, blocked_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocking_scheduler = Arc::clone(&scheduler);
        let blocker = scheduler.spawn_pinned(move |_, _| {
            move |_: &Waker| {
                let guard = blocking_scheduler
                    .enter_blocking()
                    .expect("replacement starts");
                blocked_tx.send(std::thread::current().id()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                drop(guard);
                TurnOutcome::Done
            }
        });
        let original_lane = blocked_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(
            scheduler.enter_blocking().is_err(),
            "replacement limit is hard"
        );
        let (moved_tx, moved_rx) = std::sync::mpsc::channel();
        let movable = scheduler.spawn_movable(move |_: &Waker| {
            moved_tx.send(std::thread::current().id()).unwrap();
            TurnOutcome::Done
        });
        let replacement_lane = moved_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_ne!(original_lane, replacement_lane);
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while blocker.status() == TurnStatus::Active
            || movable.status() == TurnStatus::Active
            || !scheduler.replacements.lock().unwrap()[0]
                .as_ref()
                .is_some_and(|thread| thread.is_finished())
        {
            assert!(Instant::now() < deadline, "replacement did not drain");
            std::thread::yield_now();
        }
        assert_eq!(blocker.status(), TurnStatus::Complete);
        assert_eq!(movable.status(), TurnStatus::Complete);
    }

    #[test]
    fn two_waiting_parents_starve_cross_routed_pinned_notifiers() {
        let reactor = Reactor::shared_wall().expect("reactor starts");
        let scheduler = Arc::new(
            PinnedTurnScheduler::with_reactor(2, 2, reactor).expect("scheduler lanes start"),
        );
        let (waiting_tx, waiting_rx) = std::sync::mpsc::channel();
        let (outcome_tx, outcome_rx) = std::sync::mpsc::channel();
        let start = Arc::new(std::sync::Barrier::new(3));
        let mut parents = Vec::new();
        for key in [(0x51, 0), (0x52, 0)] {
            let scheduler = Arc::clone(&scheduler);
            let waiting_tx = waiting_tx.clone();
            let outcome_tx = outcome_tx.clone();
            let start = Arc::clone(&start);
            parents.push(scheduler.clone().spawn_pinned(move |_, _| {
                move |_: &Waker| {
                    let _replacement = scheduler.enter_blocking().expect("replacement starts");
                    waiting_tx.send(std::thread::current().id()).unwrap();
                    start.wait();
                    let outcome = crate::workers::wait(key, || true, Some(Duration::from_secs(1)));
                    outcome_tx.send(outcome).unwrap();
                    TurnOutcome::Done
                }
            }));
        }
        drop(waiting_tx);
        drop(outcome_tx);
        let first_lane = waiting_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let second_lane = waiting_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_ne!(first_lane, second_lane);
        start.wait();

        // Round-robin assigns these children to lanes 0 and 1. Each would notify the parent
        // on the opposite lane, but both owner lanes are occupied by synchronous waits.
        let (notified_tx, notified_rx) = std::sync::mpsc::channel();
        let mut children = Vec::new();
        for key in [(0x52, 0), (0x51, 0)] {
            let notified_tx = notified_tx.clone();
            children.push(scheduler.spawn_pinned(move |_, _| {
                move |_: &Waker| {
                    notified_tx.send(crate::workers::notify(key, 1)).unwrap();
                    TurnOutcome::Done
                }
            }));
        }
        drop(notified_tx);

        let (movable_tx, movable_rx) = std::sync::mpsc::channel();
        let movable = scheduler.spawn_movable(move |_: &Waker| {
            movable_tx.send(std::thread::current().id()).unwrap();
            TurnOutcome::Done
        });
        let replacement_lane = movable_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_ne!(replacement_lane, first_lane);
        assert_ne!(replacement_lane, second_lane);
        assert!(
            notified_rx.recv_timeout(Duration::from_millis(20)).is_err(),
            "cross-routed pinned children ran while both parents waited"
        );

        for parent in &parents {
            let deadline = Instant::now() + Duration::from_secs(2);
            while parent.status() == TurnStatus::Active {
                assert!(Instant::now() < deadline, "parent wait never timed out");
                std::thread::yield_now();
            }
            assert_eq!(parent.status(), TurnStatus::Complete);
        }
        let outcomes = [
            outcome_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            outcome_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        ];
        assert!(outcomes.contains(&crate::workers::WaitOutcome::TimedOut));
        notified_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        notified_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(movable.status(), TurnStatus::Complete);
        let deadline = Instant::now() + Duration::from_secs(1);
        while children
            .iter()
            .any(|child| child.status() == TurnStatus::Active)
        {
            assert!(
                Instant::now() < deadline,
                "pinned notifiers did not complete"
            );
            std::thread::yield_now();
        }
        assert!(
            children
                .iter()
                .all(|child| child.status() == TurnStatus::Complete)
        );
    }

    #[test]
    fn a_draining_replacement_hands_off_a_movable_deadline() {
        let reactor = Reactor::shared_wall().expect("reactor starts");
        let scheduler = Arc::new(
            PinnedTurnScheduler::with_reactor(1, 1, reactor).expect("scheduler lanes start"),
        );
        let (blocked_tx, blocked_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocking_scheduler = Arc::clone(&scheduler);
        let blocker = scheduler.spawn_pinned(move |_, _| {
            move |_: &Waker| {
                let guard = blocking_scheduler
                    .enter_blocking()
                    .expect("replacement starts");
                blocked_tx.send(std::thread::current().id()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                drop(guard);
                TurnOutcome::Done
            }
        });
        let original_lane = blocked_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let (first_tx, first_rx) = std::sync::mpsc::channel();
        let (second_tx, second_rx) = std::sync::mpsc::channel();
        let mut first = true;
        let movable = scheduler.spawn_movable(move |_: &Waker| {
            if first {
                first = false;
                first_tx.send(std::thread::current().id()).unwrap();
                TurnOutcome::ParkedUntil(Instant::now() + Duration::from_millis(500))
            } else {
                second_tx.send(std::thread::current().id()).unwrap();
                TurnOutcome::Done
            }
        });
        let replacement_lane = first_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_ne!(original_lane, replacement_lane);
        release_tx.send(()).unwrap();
        let completion_deadline = Instant::now() + Duration::from_secs(2);
        assert_eq!(
            second_rx
                .recv_timeout(completion_deadline.saturating_duration_since(Instant::now()))
                .unwrap(),
            original_lane
        );
        while movable.status() == TurnStatus::Active {
            assert!(
                Instant::now() < completion_deadline,
                "movable turn did not complete after its Done outcome"
            );
            std::thread::yield_now();
        }
        assert_eq!(blocker.status(), TurnStatus::Complete);
        assert_eq!(movable.status(), TurnStatus::Complete);
    }

    #[test]
    fn a_replacement_stays_with_a_realm_that_pins_on_it() {
        struct PinningBody {
            ticket: Arc<std::sync::OnceLock<Arc<TurnTicket>>>,
            seen: std::sync::mpsc::Sender<std::thread::ThreadId>,
            first: bool,
        }

        impl PinnedTurn for PinningBody {
            fn run_turn(&mut self, _waker: &Waker) -> TurnOutcome {
                unreachable!("lane identity is needed for pinning")
            }

            fn run_turn_on(&mut self, _waker: &Waker, lane: usize) -> TurnOutcome {
                let ticket = loop {
                    if let Some(ticket) = self.ticket.get() {
                        break ticket;
                    }
                    std::thread::yield_now();
                };
                assert!(ticket.pin_to_lane(lane));
                self.seen.send(std::thread::current().id()).unwrap();
                if self.first {
                    self.first = false;
                    TurnOutcome::Parked
                } else {
                    TurnOutcome::Done
                }
            }
        }

        let reactor = Reactor::shared_wall().expect("reactor starts");
        let scheduler = Arc::new(
            PinnedTurnScheduler::with_reactor(1, 1, reactor).expect("scheduler lanes start"),
        );
        let (blocked_tx, blocked_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocking_scheduler = Arc::clone(&scheduler);
        let blocker = scheduler.spawn_pinned(move |_, _| {
            move |_: &Waker| {
                let guard = blocking_scheduler
                    .enter_blocking()
                    .expect("replacement starts");
                blocked_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                drop(guard);
                TurnOutcome::Done
            }
        });
        blocked_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let ticket = Arc::new(std::sync::OnceLock::new());
        let (seen_tx, seen_rx) = std::sync::mpsc::channel();
        let movable = scheduler.spawn_movable(PinningBody {
            ticket: Arc::clone(&ticket),
            seen: seen_tx,
            first: true,
        });
        assert!(ticket.set(Arc::clone(&movable.entry.ticket)).is_ok());
        let first_lane = seen_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while blocker.status() == TurnStatus::Active {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(
            !scheduler.replacements.lock().unwrap()[0]
                .as_ref()
                .expect("replacement exists")
                .is_finished(),
            "a pinned realm keeps its replacement lane"
        );
        assert!(movable.wake());
        assert_eq!(
            seen_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            first_lane
        );
        while movable.status() == TurnStatus::Active {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(movable.status(), TurnStatus::Complete);
    }

    #[test]
    fn construction_can_pin_a_movable_factory_before_its_first_wake() {
        let scheduler = PinnedTurnScheduler::new(2).expect("lanes start");
        let body = scheduler.spawn_movable_with(|_, ticket, lane| {
            assert!(ticket.pin_to_lane(lane));
            let owner = std::thread::current().id();
            let mut first = true;
            move |_: &Waker| {
                assert_eq!(std::thread::current().id(), owner);
                if first {
                    first = false;
                    TurnOutcome::Ready
                } else {
                    TurnOutcome::Done
                }
            }
        });
        let deadline = Instant::now() + Duration::from_secs(1);
        while body.status() == TurnStatus::Active {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(body.status(), TurnStatus::Complete);
    }

    #[test]
    fn a_body_that_pins_during_a_turn_is_dropped_on_that_lane() {
        struct PinThenPark {
            ticket: Arc<TurnTicket>,
            owner: Option<std::thread::ThreadId>,
            parked: std::sync::mpsc::Sender<std::thread::ThreadId>,
            dropped: std::sync::mpsc::Sender<std::thread::ThreadId>,
        }
        impl PinnedTurn for PinThenPark {
            fn run_turn(&mut self, _: &Waker) -> TurnOutcome {
                unreachable!("scheduler supplies the lane")
            }

            fn run_turn_on(&mut self, _: &Waker, lane: usize) -> TurnOutcome {
                let current = std::thread::current().id();
                if let Some(owner) = self.owner {
                    assert_eq!(current, owner);
                    self.parked.send(owner).unwrap();
                    TurnOutcome::Parked
                } else {
                    assert!(self.ticket.pin_to_lane(lane));
                    self.owner = Some(current);
                    TurnOutcome::Ready
                }
            }
        }
        impl Drop for PinThenPark {
            fn drop(&mut self) {
                self.dropped.send(std::thread::current().id()).unwrap();
            }
        }

        let scheduler = PinnedTurnScheduler::new(2).expect("lanes start");
        let (parked, parked_rx) = std::sync::mpsc::channel();
        let (dropped, dropped_rx) = std::sync::mpsc::channel();
        let handle = scheduler.spawn_movable_with(move |_, ticket, _| PinThenPark {
            ticket,
            owner: None,
            parked,
            dropped,
        });
        let owner = parked_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        handle.cancel();
        assert_eq!(
            dropped_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            owner
        );
        assert_eq!(handle.status(), TurnStatus::Cancelled);
    }

    #[test]
    fn live_claim_waits_for_busy_realm_and_cancelled_claim_does_not_take_it() {
        let scheduler = Arc::new(PinnedTurnScheduler::new(1).expect("lane starts"));
        let realm = scheduler.spawn_pinned(|_, _| |_waker: &Waker| TurnOutcome::Parked);
        let target = realm.pause_target();
        let first = scheduler.claim_one(&target).expect("first claim");
        assert!(first.wait(Duration::from_secs(1)));

        let (started, started_rx) = mpsc::channel();
        let (answer, answer_rx) = mpsc::channel();
        let waiting_scheduler = Arc::clone(&scheduler);
        let waiting_target = target.clone();
        let waiter = std::thread::spawn(move || {
            started.send(()).unwrap();
            let next = waiting_scheduler
                .claim_one_while(&waiting_target, || true)
                .expect("second live claim");
            let paused = next.wait_while(|| true);
            answer.send(paused).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(answer_rx.recv_timeout(Duration::from_millis(20)).is_err());
        drop(first);
        assert!(answer_rx.recv_timeout(Duration::from_secs(1)).unwrap());
        waiter.join().unwrap();

        let held = scheduler.claim_one(&target).expect("third claim");
        assert!(held.wait(Duration::from_secs(1)));
        let live = Arc::new(AtomicU8::new(1));
        let cancelled = Arc::clone(&live);
        let (started, started_rx) = mpsc::channel();
        let (answer, answer_rx) = mpsc::channel();
        let waiting_scheduler = Arc::clone(&scheduler);
        let waiting_target = target.clone();
        let waiter = std::thread::spawn(move || {
            started.send(()).unwrap();
            let result = waiting_scheduler
                .claim_one_while(&waiting_target, || cancelled.load(Ordering::Acquire) != 0);
            answer.send(result.err()).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(answer_rx.recv_timeout(Duration::from_millis(20)).is_err());
        live.store(0, Ordering::Release);
        assert_eq!(
            answer_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Some(TurnClaimError::Unavailable)
        );
        waiter.join().unwrap();
        drop(held);
    }
}
