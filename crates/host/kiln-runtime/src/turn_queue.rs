//! Coalesced ready tickets for the realm turn scheduler (ADR 0070).
//!
//! A ticket is the only cross-thread name for a realm. Its owner stores the realm itself behind
//! an exclusive turn lease; this queue never contains a pointer into language state. A wake that
//! races the end of a running turn is remembered and queues exactly one successor turn.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

/// One source of ready tickets, shared by scheduler lanes.
pub struct ReadyQueue {
    state: Mutex<QueueState>,
    changed: Vec<Condvar>,
}

struct QueueState {
    movable: VecDeque<(Arc<TurnTicket>, u64)>,
    pinned: Vec<VecDeque<(Arc<TurnTicket>, u64)>>,
    waiting: Vec<bool>,
    prefer_pinned: Vec<bool>,
    next_wake: usize,
    closed: bool,
}

impl ReadyQueue {
    /// Create an empty queue. The caller owns its lanes and closes it at shutdown.
    pub fn new() -> Arc<Self> {
        Self::with_lanes(1)
    }

    /// Create one ready source with a private queue and condition variable for each lane.
    /// Movable tickets can be claimed by any lane; pinned tickets wake their owner directly.
    pub fn with_lanes(lanes: usize) -> Arc<Self> {
        let lanes = lanes.max(1);
        Arc::new(Self {
            state: Mutex::new(QueueState {
                movable: VecDeque::new(),
                pinned: (0..lanes).map(|_| VecDeque::new()).collect(),
                waiting: vec![false; lanes],
                prefer_pinned: vec![true; lanes],
                next_wake: 0,
                closed: false,
            }),
            changed: (0..lanes).map(|_| Condvar::new()).collect(),
        })
    }

    /// Register an opaque realm identity. Its first turn is queued by calling `wake`.
    pub fn ticket(self: &Arc<Self>, realm_id: u64) -> Arc<TurnTicket> {
        self.make_ticket(realm_id, None)
    }

    /// Register a ticket whose owner may only run on `lane`.
    pub fn ticket_on_lane(self: &Arc<Self>, realm_id: u64, lane: usize) -> Arc<TurnTicket> {
        assert!(lane < self.changed.len(), "scheduler lane exists");
        self.make_ticket(realm_id, Some(lane))
    }

    fn make_ticket(self: &Arc<Self>, realm_id: u64, lane: Option<usize>) -> Arc<TurnTicket> {
        Arc::new(TurnTicket {
            realm_id,
            lane,
            queue: Arc::downgrade(self),
            state: Mutex::new(TicketState {
                generation: 1,
                phase: Phase::Parked,
                wake_pending: false,
            }),
        })
    }

    fn enqueue(&self, ticket: Arc<TurnTicket>, generation: u64) -> bool {
        let mut state = self.state.lock().expect("ready queue lock poisoned");
        if state.closed {
            return false;
        }
        if let Some(lane) = ticket.lane {
            state.pinned[lane].push_back((ticket, generation));
            self.changed[lane].notify_one();
        } else {
            state.movable.push_back((ticket, generation));
            let lanes = state.waiting.len();
            for offset in 0..lanes {
                let lane = (state.next_wake + offset) % lanes;
                if state.waiting[lane] {
                    state.next_wake = (lane + 1) % lanes;
                    self.changed[lane].notify_one();
                    break;
                }
            }
        }
        true
    }

    /// Wait for a ready ticket. Stale generations and cancelled tickets are discarded.
    pub fn take(&self) -> Option<TurnLease> {
        self.take_until(0, None)
    }

    /// Wait no longer than `timeout` for a ready ticket.
    pub fn take_timeout(&self, timeout: Duration) -> Option<TurnLease> {
        self.take_until(0, Some(Instant::now() + timeout))
    }

    /// Wait for either a movable ticket or one pinned to this lane.
    pub fn take_for_lane(&self, lane: usize) -> Option<TurnLease> {
        self.take_until(lane, None)
    }

    /// Wait no longer than `timeout` for a ticket runnable on this lane.
    pub fn take_timeout_for_lane(&self, lane: usize, timeout: Duration) -> Option<TurnLease> {
        self.take_until(lane, Some(Instant::now() + timeout))
    }

    fn take_until(&self, lane: usize, deadline: Option<Instant>) -> Option<TurnLease> {
        assert!(lane < self.changed.len(), "scheduler lane exists");
        loop {
            // Release the queue lock before taking the ticket lock. Wake takes the locks in the
            // other order, so holding both here would deadlock at a turn boundary.
            let entry = {
                let mut state = self.state.lock().expect("ready queue lock poisoned");
                loop {
                    if state.closed {
                        return None;
                    }
                    let entry = if state.prefer_pinned[lane] {
                        state.pinned[lane]
                            .pop_front()
                            .map(|entry| (entry, true))
                            .or_else(|| state.movable.pop_front().map(|entry| (entry, false)))
                    } else {
                        state
                            .movable
                            .pop_front()
                            .map(|entry| (entry, false))
                            .or_else(|| state.pinned[lane].pop_front().map(|entry| (entry, true)))
                    };
                    if let Some((entry, was_pinned)) = entry {
                        state.prefer_pinned[lane] = !was_pinned;
                        state.waiting[lane] = false;
                        break entry;
                    }
                    state.waiting[lane] = true;
                    if let Some(deadline) = deadline {
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        if remaining.is_zero() {
                            state.waiting[lane] = false;
                            return None;
                        }
                        let (next, _) = self.changed[lane]
                            .wait_timeout(state, remaining)
                            .expect("ready queue lock poisoned");
                        state = next;
                    } else {
                        state = self.changed[lane]
                            .wait(state)
                            .expect("ready queue lock poisoned");
                    }
                }
            };
            let (ticket, generation) = entry;
            let mut state = ticket.state.lock().expect("turn ticket lock poisoned");
            if state.generation != generation || state.phase != Phase::Queued {
                continue;
            }
            state.phase = Phase::Running;
            drop(state);
            return Some(TurnLease {
                ticket,
                generation,
                finished: false,
            });
        }
    }

    /// Stop accepting wakes and release parked lanes.
    pub fn close(&self) {
        let mut state = self.state.lock().expect("ready queue lock poisoned");
        state.closed = true;
        state.movable.clear();
        for pinned in &mut state.pinned {
            pinned.clear();
        }
        for changed in &self.changed {
            changed.notify_all();
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Parked,
    Queued,
    Running,
    Done,
}

struct TicketState {
    generation: u64,
    phase: Phase,
    wake_pending: bool,
}

/// An opaque realm identity and generation, never a language-state pointer.
pub struct TurnTicket {
    realm_id: u64,
    lane: Option<usize>,
    queue: Weak<ReadyQueue>,
    state: Mutex<TicketState>,
}

impl TurnTicket {
    /// The realm identity supplied at registration.
    pub fn realm_id(&self) -> u64 {
        self.realm_id
    }

    /// Queue a parked realm or retain one wake for its currently running turn.
    /// Repeated wakes coalesce until a lane claims the ticket.
    pub fn wake(self: &Arc<Self>) -> bool {
        let mut state = self.state.lock().expect("turn ticket lock poisoned");
        match state.phase {
            Phase::Parked => {
                let Some(queue) = self.queue.upgrade() else {
                    return false;
                };
                if queue.enqueue(Arc::clone(self), state.generation) {
                    state.phase = Phase::Queued;
                    true
                } else {
                    false
                }
            }
            Phase::Running => {
                state.wake_pending = true;
                true
            }
            Phase::Queued => true,
            Phase::Done => false,
        }
    }

    /// Retire a realm. Any copy of its old queued generation becomes stale.
    pub fn cancel(&self) {
        let mut state = self.state.lock().expect("turn ticket lock poisoned");
        state.generation = state.generation.wrapping_add(1).max(1);
        state.phase = Phase::Done;
        state.wake_pending = false;
    }
}

impl std::task::Wake for TurnTicket {
    fn wake(self: Arc<Self>) {
        let _ = TurnTicket::wake(&self);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        let _ = TurnTicket::wake(self);
    }
}

/// What a single language turn did.
pub enum TurnOutcome {
    /// Another turn is immediately ready.
    Ready,
    /// Wait for an external wake or deadline.
    Parked,
    /// Wake at this monotonic deadline unless an external event runs the realm first.
    ParkedUntil(Instant),
    /// Realm teardown may begin; later wakes are discarded.
    Done,
}

/// Exclusive permission to run one turn for a realm.
///
/// Dropping an unfinished lease retires the ticket: a panic cannot leave it permanently running
/// or accidentally schedule more language work after an incomplete turn.
pub struct TurnLease {
    ticket: Arc<TurnTicket>,
    generation: u64,
    finished: bool,
}

impl TurnLease {
    pub fn realm_id(&self) -> u64 {
        self.ticket.realm_id
    }

    /// The ticket to use as the realm's local-task waker while this turn runs.
    pub fn ticket(&self) -> Arc<TurnTicket> {
        Arc::clone(&self.ticket)
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn finish(mut self, outcome: TurnOutcome) {
        self.ticket.finish(self.generation, outcome);
        self.finished = true;
    }
}

impl Drop for TurnLease {
    fn drop(&mut self) {
        if !self.finished {
            self.ticket.finish(self.generation, TurnOutcome::Done);
        }
    }
}

impl TurnTicket {
    fn finish(self: &Arc<Self>, generation: u64, outcome: TurnOutcome) {
        let mut state = self.state.lock().expect("turn ticket lock poisoned");
        if state.generation != generation || state.phase != Phase::Running {
            return;
        }
        match outcome {
            TurnOutcome::Done => {
                state.generation = state.generation.wrapping_add(1).max(1);
                state.phase = Phase::Done;
                state.wake_pending = false;
            }
            TurnOutcome::Ready => {
                state.wake_pending = false;
                if let Some(queue) = self.queue.upgrade()
                    && queue.enqueue(Arc::clone(self), generation)
                {
                    state.phase = Phase::Queued;
                } else {
                    state.phase = Phase::Parked;
                }
            }
            TurnOutcome::Parked | TurnOutcome::ParkedUntil(_) => {
                if state.wake_pending {
                    state.wake_pending = false;
                    if let Some(queue) = self.queue.upgrade()
                        && queue.enqueue(Arc::clone(self), generation)
                    {
                        state.phase = Phase::Queued;
                    } else {
                        state.phase = Phase::Parked;
                    }
                } else {
                    state.phase = Phase::Parked;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wakes_coalesce_and_a_racing_wake_survives_the_turn() {
        let queue = ReadyQueue::new();
        let ticket = queue.ticket(17);
        for _ in 0..100 {
            assert!(ticket.wake());
        }
        let lease = queue
            .take_timeout(Duration::from_millis(10))
            .expect("first turn");
        assert_eq!(lease.realm_id(), 17);
        assert_eq!(lease.generation(), 1);
        assert!(queue.take_timeout(Duration::from_millis(1)).is_none());
        ticket.wake();
        ticket.wake();
        lease.finish(TurnOutcome::Parked);
        let lease = queue
            .take_timeout(Duration::from_millis(10))
            .expect("one successor");
        assert!(queue.take_timeout(Duration::from_millis(1)).is_none());
        lease.finish(TurnOutcome::Done);
        assert!(!ticket.wake());
    }

    #[test]
    fn cancelling_a_queued_generation_discards_it() {
        let queue = ReadyQueue::new();
        let first = queue.ticket(1);
        let second = queue.ticket(2);
        first.wake();
        first.cancel();
        second.wake();
        let lease = queue
            .take_timeout(Duration::from_millis(10))
            .expect("second ticket");
        assert_eq!(lease.realm_id(), 2);
        lease.finish(TurnOutcome::Done);
    }

    #[test]
    fn many_lanes_never_claim_the_same_turn() {
        let queue = ReadyQueue::new();
        let ticket = queue.ticket(42);
        ticket.wake();
        let claimed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let queue = Arc::clone(&queue);
                let claimed = Arc::clone(&claimed);
                scope.spawn(move || {
                    if let Some(lease) = queue.take_timeout(Duration::from_millis(20)) {
                        claimed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(25));
                        lease.finish(TurnOutcome::Parked);
                    }
                });
            }
        });
        assert_eq!(claimed.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn a_wake_concurrent_with_finish_is_never_lost_or_duplicated() {
        let queue = ReadyQueue::new();
        let ticket = queue.ticket(9);
        ticket.wake();
        let mut lease = queue.take().expect("first turn");
        for _ in 0..100 {
            let barrier = std::sync::Barrier::new(2);
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    barrier.wait();
                    ticket.wake();
                });
                barrier.wait();
                lease.finish(TurnOutcome::Parked);
            });
            lease = queue
                .take_timeout(Duration::from_millis(100))
                .expect("racing wake must queue one successor");
            assert!(queue.take_timeout(Duration::from_millis(1)).is_none());
        }
        lease.finish(TurnOutcome::Done);
    }

    #[test]
    fn pinned_tickets_stay_on_their_lane_and_movable_tickets_can_be_claimed_elsewhere() {
        let queue = ReadyQueue::with_lanes(3);
        let pinned = queue.ticket_on_lane(7, 2);
        let movable = queue.ticket(8);
        pinned.wake();
        assert!(
            queue
                .take_timeout_for_lane(0, Duration::from_millis(1))
                .is_none()
        );
        movable.wake();
        let lease = queue
            .take_timeout_for_lane(0, Duration::from_millis(10))
            .expect("movable ticket can run on lane zero");
        assert_eq!(lease.realm_id(), 8);
        lease.finish(TurnOutcome::Done);
        let lease = queue
            .take_timeout_for_lane(2, Duration::from_millis(10))
            .expect("pinned ticket reaches its owner");
        assert_eq!(lease.realm_id(), 7);
        lease.finish(TurnOutcome::Done);
    }

    #[test]
    fn movable_ticket_wakes_a_waiting_lane() {
        let queue = ReadyQueue::with_lanes(2);
        let ticket = queue.ticket(11);
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                let lease = queue
                    .take_timeout_for_lane(1, Duration::from_secs(1))
                    .expect("second lane is notified");
                assert_eq!(lease.realm_id(), 11);
                lease.finish(TurnOutcome::Done);
            });
            std::thread::sleep(Duration::from_millis(10));
            ticket.wake();
            worker.join().unwrap();
        });
    }
}
