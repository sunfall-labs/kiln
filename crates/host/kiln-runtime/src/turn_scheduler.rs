//! Pinned scheduler lanes for realm turns.
//!
//! A lane constructs and owns its `!Send` realm bodies. Other threads hold only tickets and
//! send factories or cancellation commands. This is the first scheduling mode: migration waits
//! for the provider's realm-registry, GC and affinity audit (ADR 0070).

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Waker;
use std::time::Instant;

use crate::Reactor;
use crate::turn_queue::{ReadyQueue, TurnOutcome, TurnTicket};

/// Realm work executed only by its assigned scheduler lane.
pub trait PinnedTurn {
    /// Run one language turn, polling any local tasks with `waker` before parking.
    /// The implementation must return with no generated frame on its C stack.
    fn run_turn(&mut self, waker: &Waker) -> TurnOutcome;
}

impl<F> PinnedTurn for F
where
    F: FnMut(&Waker) -> TurnOutcome + 'static,
{
    fn run_turn(&mut self, waker: &Waker) -> TurnOutcome {
        self(waker)
    }
}

type Factory = Box<dyn FnOnce(Arc<Reactor>, Arc<TurnTicket>) -> Box<dyn PinnedTurn> + Send>;

enum Command {
    Install {
        realm_id: u64,
        ticket: Arc<TurnTicket>,
        status: Arc<AtomicU8>,
        factory: Factory,
    },
    Remove(u64),
}

struct LaneControl {
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

/// Observable state of a pinned scheduler realm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PinnedTurnStatus {
    Active = 0,
    Complete = 1,
    Cancelled = 2,
    Panicked = 3,
}

impl PinnedTurnStatus {
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

    pub fn status(&self) -> PinnedTurnStatus {
        PinnedTurnStatus::read(self.status.load(Ordering::Acquire))
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
                PinnedTurnStatus::Active as u8,
                PinnedTurnStatus::Cancelled as u8,
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

/// Fixed scheduler lanes sharing one Tokio reactor.
///
/// Each lane has a ready queue; pinned realms always requeue onto their owning lane. Migration
/// will route audited unpinned tickets through a process-wide queue after the affinity audit.
pub struct PinnedTurnScheduler {
    reactor: Arc<Reactor>,
    lanes: Vec<Arc<LaneControl>>,
    threads: Vec<std::thread::JoinHandle<()>>,
    next_realm_id: AtomicU64,
}

impl PinnedTurnScheduler {
    pub fn new(lane_count: usize) -> std::io::Result<Self> {
        let reactor = Reactor::shared_wall().map_err(|error| error.0)?;
        Self::with_reactor(lane_count.max(1), reactor)
    }

    /// One FIFO lane with its own pausable reactor for reproducible realm turns.
    /// The factory may pause this reactor before it schedules deterministic tasks.
    pub fn deterministic() -> std::io::Result<Self> {
        let reactor = Reactor::current_thread().map_err(|error| error.0)?;
        Self::with_reactor(1, reactor)
    }

    fn with_reactor(lane_count: usize, reactor: Arc<Reactor>) -> std::io::Result<Self> {
        let mut scheduler = Self {
            reactor,
            lanes: Vec::new(),
            threads: Vec::new(),
            next_realm_id: AtomicU64::new(1),
        };
        for index in 0..lane_count {
            let queue = ReadyQueue::new();
            let lane = Arc::new(LaneControl {
                control: queue.ticket(0),
                queue,
                commands: Mutex::new(VecDeque::new()),
            });
            let lane_for_thread = Arc::clone(&lane);
            let reactor_for_thread = Arc::clone(&scheduler.reactor);
            let thread = std::thread::Builder::new()
                .name(format!("kiln-turn-{index}"))
                .spawn(move || run_lane(lane_for_thread, reactor_for_thread))?;
            scheduler.lanes.push(lane);
            scheduler.threads.push(thread);
        }
        Ok(scheduler)
    }

    pub fn lane_count(&self) -> usize {
        self.lanes.len()
    }

    /// Construct a realm on one lane and queue its first turn. The factory is sent across
    /// threads; the returned body need not be `Send` and never leaves its lane.
    pub fn spawn_pinned<F, T>(&self, factory: F) -> PinnedTurnHandle
    where
        F: FnOnce(Arc<Reactor>, Arc<TurnTicket>) -> T + Send + 'static,
        T: PinnedTurn + 'static,
    {
        let id = self.next_realm_id.fetch_add(1, Ordering::Relaxed);
        let lane = Arc::clone(&self.lanes[(id as usize - 1) % self.lanes.len()]);
        let ticket = lane.queue.ticket(id);
        let status = Arc::new(AtomicU8::new(PinnedTurnStatus::Active as u8));
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
}

impl Drop for PinnedTurnScheduler {
    fn drop(&mut self) {
        for lane in &self.lanes {
            lane.queue.close();
        }
        for thread in self.threads.drain(..) {
            if thread.thread().id() != std::thread::current().id() {
                let _ = thread.join();
            }
        }
    }
}

fn run_lane(lane: Arc<LaneControl>, reactor: Arc<Reactor>) {
    let mut realms = HashMap::<u64, RealmEntry>::new();
    // One heap per lane schedules all pure realm timers without a Tokio task or OS thread per
    // realm. The epoch invalidates an old deadline when an external wake runs that realm first.
    let mut deadlines = BinaryHeap::<Reverse<(Instant, u64, u64)>>::new();
    loop {
        let now = Instant::now();
        while let Some(&Reverse((at, realm_id, epoch))) = deadlines.peek() {
            if at > now {
                break;
            }
            deadlines.pop();
            if let Some(realm) = realms.get(&realm_id)
                && realm.deadline_epoch == epoch
            {
                realm.ticket.wake();
            }
        }
        let lease = match deadlines.peek() {
            Some(&Reverse((at, _, _))) => {
                lane.queue.take_timeout(at.saturating_duration_since(now))
            }
            None => lane.queue.take(),
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
                        if status.load(Ordering::Acquire) != PinnedTurnStatus::Active as u8 {
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
                                status.store(PinnedTurnStatus::Panicked as u8, Ordering::Release);
                                ticket.cancel();
                            }
                        }
                    }
                    Command::Remove(realm_id) => {
                        realms.remove(&realm_id);
                    }
                }
            }
            lease.finish(TurnOutcome::Parked);
            continue;
        }
        let Some(realm) = realms.get_mut(&realm_id) else {
            lease.finish(TurnOutcome::Done);
            continue;
        };
        let waker = Waker::from(lease.ticket());
        let result = catch_unwind(AssertUnwindSafe(|| realm.body.run_turn(&waker)));
        match result {
            Ok(outcome) => {
                let done = matches!(outcome, TurnOutcome::Done);
                realm.deadline_epoch = realm.deadline_epoch.wrapping_add(1);
                if let TurnOutcome::ParkedUntil(at) = &outcome {
                    deadlines.push(Reverse((*at, realm_id, realm.deadline_epoch)));
                }
                if done {
                    let _ = realm.status.compare_exchange(
                        PinnedTurnStatus::Active as u8,
                        PinnedTurnStatus::Complete as u8,
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
                    .store(PinnedTurnStatus::Panicked as u8, Ordering::Release);
                lease.finish(TurnOutcome::Done);
                realms.remove(&realm_id);
            }
        }
    }
    // The thread that constructed each `!Send` body also drops it.
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
            .any(|realm| realm.status() == PinnedTurnStatus::Active)
        {
            assert!(Instant::now() < deadline, "pinned turns complete");
            std::thread::yield_now();
        }
        assert!(
            realms
                .iter()
                .all(|realm| realm.status() == PinnedTurnStatus::Complete)
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
        assert_eq!(handle.status(), PinnedTurnStatus::Cancelled);
    }

    #[test]
    fn one_panicking_realm_does_not_stop_its_lane() {
        let scheduler = PinnedTurnScheduler::new(1).expect("lane starts");
        let bad = scheduler.spawn_pinned(|_, _| {
            |_waker: &Waker| -> TurnOutcome { panic!("deliberate scheduler test panic") }
        });
        let good = scheduler.spawn_pinned(|_, _| |_waker: &Waker| TurnOutcome::Done);
        let deadline = Instant::now() + Duration::from_secs(1);
        while good.status() == PinnedTurnStatus::Active {
            assert!(Instant::now() < deadline, "healthy realm completes");
            std::thread::yield_now();
        }
        assert_eq!(bad.status(), PinnedTurnStatus::Panicked);
        assert_eq!(good.status(), PinnedTurnStatus::Complete);
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
            .any(|realm| realm.status() == PinnedTurnStatus::Active)
        {
            assert!(Instant::now() < deadline, "all parked timers wake");
            std::thread::yield_now();
        }
        assert!(
            realms
                .iter()
                .all(|realm| realm.status() == PinnedTurnStatus::Complete)
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
            .any(|realm| realm.status() == PinnedTurnStatus::Active)
        {
            assert!(Instant::now() < deadline, "deterministic turns complete");
            std::thread::yield_now();
        }
        assert!(
            realms
                .iter()
                .all(|realm| realm.status() == PinnedTurnStatus::Complete)
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
            .any(|realm| realm.status() == PinnedTurnStatus::Active)
        {
            assert!(Instant::now() < deadline, "all pure deadlines wake");
            std::thread::yield_now();
        }
        assert!(
            realms
                .iter()
                .all(|realm| realm.status() == PinnedTurnStatus::Complete)
        );
    }
}
