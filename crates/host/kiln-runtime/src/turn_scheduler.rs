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
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Waker;
use std::time::Instant;

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
}

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

/// The cross-thread name of a realm hosted on one scheduler lane.
pub struct PinnedTurnHandle {
    ticket: Arc<TurnTicket>,
    lane: Arc<LaneControl>,
    status: Arc<AtomicU8>,
}

impl PinnedTurnHandle {
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
            self.lane.command(Command::Remove(self.id()));
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
}

impl MovableTurnHandle {
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
            if let Some(lane) = self.entry.ticket.pinned_lane() {
                self.lanes[lane].command(Command::Remove(self.id()));
            } else {
                // A running movable turn can put its body back just before it observes this
                // cancellation. Both paths use the body lock, so either cancellation takes the
                // returned body here or the turn sees Cancelled and drops it on its own lane.
                let body = self
                    .entry
                    .body
                    .lock()
                    .expect("movable body lock poisoned")
                    .take();
                drop(body);
                self.realms
                    .lock()
                    .expect("movable realm lock poisoned")
                    .remove(&self.id());
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
            let thread = std::thread::Builder::new()
                .name(format!("kiln-turn-{index}"))
                .spawn(move || {
                    run_lane(
                        lane_for_thread,
                        reactor_for_thread,
                        movable_for_thread,
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
        let queue_identity = Arc::as_ptr(&self.lanes[0].queue) as usize;
        let owner = ACTIVE_LANE.with(|slot| {
            slot.get()
                .and_then(|(identity, lane)| (identity == queue_identity).then_some(lane))
                .unwrap_or(0)
        });
        let handoff = Arc::clone(&self.lanes[owner]);
        let thread = std::thread::Builder::new()
            .name(format!("kiln-turn-replacement-{slot}"))
            .spawn(move || run_lane(lane_for_thread, reactor, movable, Some(handoff)))?;
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
        let id = self.next_realm_id.fetch_add(1, Ordering::Relaxed);
        let lane = Arc::clone(&self.lanes[(id as usize - 1) % self.base_lanes]);
        let ticket = lane.queue.ticket_on_lane(id, lane.index);
        let status = Arc::new(AtomicU8::new(TurnStatus::Active as u8));
        lane.command(Command::Install {
            realm_id: id,
            ticket: Arc::clone(&ticket),
            status: Arc::clone(&status),
            factory: Box::new(move |reactor, ticket| Box::new(factory(reactor, ticket))),
        });
        PinnedTurnHandle {
            ticket,
            lane,
            status,
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
        }
    }

    /// Construct a movable body on one scheduler lane. The factory may permanently pin its
    /// ticket there before the first wake if initialization publishes thread-affine state.
    pub fn spawn_movable_with<F, T>(&self, factory: F) -> MovableTurnHandle
    where
        F: FnOnce(Arc<Reactor>, Arc<TurnTicket>, usize) -> T + Send + 'static,
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
        lane.command(Command::InstallMovable {
            entry: Arc::clone(&entry),
            factory: Box::new(move |reactor, ticket, lane| {
                Box::new(factory(reactor, ticket, lane))
            }),
        });
        MovableTurnHandle {
            entry,
            realms: Arc::clone(&self.movable),
            lanes: Arc::clone(&self.lanes),
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
                    Command::Drain => draining = true,
                    Command::Deadlines(other) => deadlines.extend(other),
                }
            }
            lease.finish(TurnOutcome::Parked);
            continue;
        }
        let Some(realm) = realms.get_mut(&realm_id) else {
            run_movable(lease, lane.index, &movable, &mut realms, &mut deadlines);
            continue;
        };
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
    use std::time::{Duration, Instant};

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
        assert_eq!(
            second_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            original_lane
        );
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
}
