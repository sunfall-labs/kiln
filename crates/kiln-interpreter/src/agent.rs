//! A second agent: a thread of its own, a realm of its own, and a deterministic turn between
//! them.
//!
//! Test262's agent protocol is the one host capability that cannot be faked. `$262.agent.start`
//! runs source in a *new agent* -- a separate execution context with its own realm -- and the
//! two talk only through a `SharedArrayBuffer` and `Atomics`. Until there is a second agent,
//! `Atomics.wait` has nobody who could ever call `Atomics.notify`, which is why the interpreter
//! answered every wait "timed-out": that was the honest answer to a question with one asker.
//!
//! # Why the agents take turns
//!
//! An agent is a real OS thread here, because a realm's heap is `Rc`-based and cannot move
//! between threads (ADR 0028: "additional runtime threads create additional current-thread
//! runtimes rather than moving a live realm"). But the threads do not run *at the same time*.
//! Exactly one holds the baton, and the baton moves only at points this module decides:
//!
//!   * when the running agent blocks -- `Atomics.wait`, `$262.agent.sleep`, waiting for a
//!     broadcast; or
//!   * when it has executed [`QUANTUM_STEPS`] bytecodes without blocking, which is what lets a
//!     spin loop such as Test262's own `$262.agent.waitUntil` make progress.
//!
//! That is a scheduler, not a mutex, and it is the whole reason this can be landed at all.
//! ADR 0012 says a Kiln program's callback order is a function of the program alone, and ADR
//! 0020 says a conformance number must mean something; a wall-clock race between two OS threads
//! satisfies neither. `Atomics.notify(i32a, 0, 1)` must return exactly 1 on every run of
//! `notify-one.js`, and it does, because the interleaving is decided by an instruction count and
//! a virtual clock rather than by whatever else the machine was doing.
//!
//! # Why the clock is virtual too
//!
//! Time here is [`Sched::now_us`], and it advances by exactly ADR 0012's rule: only when no
//! agent can make progress, and only as far as the earliest deadline. A `$262.agent.sleep(1000)`
//! therefore costs no wall-clock second, an `Atomics.wait` timeout is a *position* rather than a
//! duration, and the whole 218-variant subtree runs in the time its instructions take.
//!
//! The one extension is [`SPIN_QUANTA`]: an agent that spins while every other agent is asleep
//! would otherwise hold the baton for ever, because a spinning agent is runnable and the rule
//! above never fires. After that many consecutive quanta the clock is allowed to move anyway.
//! The bound is a constant, so this stays a function of the program.
//!
//! # Why `std` and not `parking_lot` or `crossbeam` (ADR 0029, ADR 0038)
//!
//! Both crates are mature and both were considered. `parking_lot::Mutex` and its futex-backed
//! `park`/`unpark` map almost onto `Atomics.wait`/`notify` -- and that is the reason they are
//! wrong here, not the reason they are right: a futex hands the wake decision to the operating
//! system, and the wake decision is precisely the thing a conformance run has to decide for
//! itself. `crossbeam`'s channels would carry the report queue, but the queue is already inside
//! the one mutex that orders everything else, and a second synchronisation point is a second
//! ordering to keep honest. What is actually needed is a condition variable guarding one piece
//! of state, which `std::sync` has.

use std::cell::{Ref, RefCell, RefMut};
use std::collections::VecDeque;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use kiln_host_abi::HostOps;
use kiln_runtime::{CaptureHost, RealmRuntime};

use crate::{
    BufferData, EvalSource, ExecutionCapabilities, ExecutionError, ExecutionLimits, ObjectData,
    Value, Vm,
};

/// Bytecodes an agent may run before the baton moves on.
///
/// Large enough that an ordinary agent script finishes inside one or two, small enough that
/// Test262's `waitUntil` -- roughly a dozen instructions per turn of its spin -- hands over after
/// a few hundred iterations rather than a few million.
pub(crate) const QUANTUM_STEPS: u64 = 1_024;

/// Quanta the runnable agents may spend without any of them blocking, before the virtual clock
/// is allowed to move anyway.
///
/// Without this the clock could stop for good. ADR 0012 advances time only when nothing can
/// progress, and a *spinning* agent is progressing by that rule -- so two agents spinning on a
/// cell that only a sleeping third agent will ever write would hand the baton back and forth for
/// ever while the sleeper's deadline never arrived. Test262 writes exactly that program:
/// `notify-in-order.js` has three agents spinning while the main agent sleeps between the stores
/// that release them.
///
/// Patience rather than a heuristic about progress: after this many consecutive quanta in which
/// no agent blocked, woke or left, the clock moves to the earliest deadline. It is a constant,
/// so the interleaving is still a function of the program alone.
const SPIN_QUANTA: u32 = 24;

/// Agents one program may start, the main agent included.
///
/// Test262 asks for at most three. The cap is here because each agent is a thread and a Tokio
/// runtime, and a program that asked for ten thousand would take the machine down rather than
/// fail -- which ADR 0020 counts as worse than refusing.
const MAX_AGENTS: usize = 8;

/// The bytes of an `ArrayBuffer`, which a `SharedArrayBuffer`'s can outlive one thread.
///
/// Two shapes rather than one, because the two buffer types genuinely differ in who may hold the
/// bytes. An ordinary buffer belongs to one realm on one thread and a [`RefCell`] says so at no
/// cost. A shared one is reachable from every agent that was broadcast it, so its bytes live
/// behind an [`Arc`] and a lock -- the only part of the value graph that is allowed to.
///
/// The accessors are deliberately spelled `borrow`/`borrow_mut`: every reader of a buffer's bytes
/// was already written against `RefCell`, and a shared buffer is not a different *operation*.
/// There is no contention to pay for either, because the baton in [`AgentGroup`] means only one
/// agent is running when a lock is taken.
#[derive(Debug)]
pub(crate) enum Bytes {
    Local(RefCell<Vec<u8>>),
    Shared(SharedStore),
}

/// A shared buffer's bytes, and the identity `Atomics.wait` keys its waiter list on.
pub(crate) type SharedStore = Arc<Mutex<Vec<u8>>>;

impl Bytes {
    /// The bytes of a new buffer, held the way its `shared` flag requires.
    pub(crate) fn new(shared: bool, bytes: Vec<u8>) -> Self {
        if shared {
            Self::Shared(Arc::new(Mutex::new(bytes)))
        } else {
            Self::Local(RefCell::new(bytes))
        }
    }

    /// A second handle onto bytes another agent already owns. The whole point of `broadcast`.
    pub(crate) fn from_store(store: &SharedStore) -> Self {
        Self::Shared(Arc::clone(store))
    }

    /// The identity other agents know these bytes by, or `None` for an unshared buffer.
    pub(crate) fn store(&self) -> Option<&SharedStore> {
        match self {
            Self::Local(_) => None,
            Self::Shared(store) => Some(store),
        }
    }

    pub(crate) fn borrow(&self) -> BytesRef<'_> {
        match self {
            Self::Local(cell) => BytesRef::Local(cell.borrow()),
            Self::Shared(store) => BytesRef::Shared(lock(store)),
        }
    }

    pub(crate) fn borrow_mut(&self) -> BytesMut<'_> {
        match self {
            Self::Local(cell) => BytesMut::Local(cell.borrow_mut()),
            Self::Shared(store) => BytesMut::Shared(lock(store)),
        }
    }
}

/// Take a lock, ignoring poisoning.
///
/// An agent that panicked has already failed its own program; the bytes it was holding are still
/// bytes, and refusing every later read of them would turn one agent's failure into the whole
/// run's. The panic itself is reported where it happened.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) enum BytesRef<'a> {
    Local(Ref<'a, Vec<u8>>),
    Shared(MutexGuard<'a, Vec<u8>>),
}

impl Deref for BytesRef<'_> {
    type Target = Vec<u8>;
    fn deref(&self) -> &Vec<u8> {
        match self {
            Self::Local(value) => value,
            Self::Shared(value) => value,
        }
    }
}

pub(crate) enum BytesMut<'a> {
    Local(RefMut<'a, Vec<u8>>),
    Shared(MutexGuard<'a, Vec<u8>>),
}

impl Deref for BytesMut<'_> {
    type Target = Vec<u8>;
    fn deref(&self) -> &Vec<u8> {
        match self {
            Self::Local(value) => value,
            Self::Shared(value) => value,
        }
    }
}

impl DerefMut for BytesMut<'_> {
    fn deref_mut(&mut self) -> &mut Vec<u8> {
        match self {
            Self::Local(value) => value,
            Self::Shared(value) => value,
        }
    }
}

/// What one `SharedArrayBuffer` looks like to an agent that did not create it.
#[derive(Clone)]
pub(crate) struct Broadcast {
    store: SharedStore,
    max_byte_length: Option<usize>,
}

/// How an `Atomics.wait` ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum WaitOutcome {
    /// Another agent called `Atomics.notify` on this address.
    Woken,
    /// The deadline arrived first -- or nothing could ever have arrived, which this engine
    /// answers the same way for the reason [`Sched::break_deadlock`] gives.
    TimedOut,
}

impl WaitOutcome {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Woken => "ok",
            Self::TimedOut => "timed-out",
        }
    }
}

/// The group is shutting down and this agent must stop.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Terminated;

/// What an agent is doing when it is not the one running.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    /// Able to run: either running now, or waiting only for the baton.
    Runnable,
    /// `$262.agent.sleep`, until this instant on the virtual clock.
    Sleeping { until: u64 },
    /// `Atomics.wait` on `(store identity, byte offset)`, until this instant if there is one.
    Waiting {
        address: (usize, usize),
        until: Option<u64>,
    },
    /// `$262.agent.receiveBroadcast`, until somebody broadcasts.
    AwaitingBroadcast,
    /// `$262.agent.broadcast`, until every started agent has taken its copy.
    Broadcasting,
    /// The agent's script and event loop are finished.
    Finished,
}

impl State {
    /// The instant this state ends by itself, if it ends by itself.
    fn deadline(self) -> Option<u64> {
        match self {
            Self::Sleeping { until } => Some(until),
            Self::Waiting { until, .. } => until,
            _ => None,
        }
    }
}

/// Everything the agents share, under one lock.
struct Sched {
    /// The virtual clock, in microseconds since the run began. ADR 0012's clock, extended to
    /// order the agents against each other rather than only one realm's timers.
    now_us: u64,
    /// Whose turn it is. Every other agent's thread is parked on the condition variable.
    turn: usize,
    states: Vec<State>,
    outcomes: Vec<WaitOutcome>,
    /// Agents inside `Atomics.wait`, oldest first. The specification wakes a waiter list in
    /// arrival order and `waiterlist-order-of-operations-is-fifo.js` checks it.
    waiters: VecDeque<usize>,
    /// `$262.agent.report`, in the order the agents reported. One queue, because Test262's
    /// `getReport` is one queue.
    reports: VecDeque<String>,
    /// A broadcast each agent has not yet taken.
    mailboxes: Vec<Option<Broadcast>>,
    /// Consecutive quanta in which no agent blocked, was woken, or finished. Reset by any of
    /// those; see [`SPIN_QUANTA`].
    spun: u32,
    terminating: bool,
}

impl Sched {
    /// The next agent other than `me` that could run, searched round-robin from just after `me`.
    fn next_other(&self, me: usize) -> Option<usize> {
        let count = self.states.len();
        (1..count)
            .map(|step| (me + step) % count)
            .find(|&id| self.states[id] == State::Runnable)
    }

    fn any_runnable(&self) -> Option<usize> {
        self.states.iter().position(|&s| s == State::Runnable)
    }

    fn earliest_deadline(&self) -> Option<u64> {
        self.states.iter().filter_map(|s| s.deadline()).min()
    }

    /// Move every agent whose deadline has arrived back to runnable.
    fn expire(&mut self, instant: u64) {
        for id in 0..self.states.len() {
            match self.states[id] {
                State::Sleeping { until } if until <= instant => {
                    self.states[id] = State::Runnable;
                }
                State::Waiting {
                    until: Some(until), ..
                } if until <= instant => {
                    self.outcomes[id] = WaitOutcome::TimedOut;
                    self.states[id] = State::Runnable;
                    self.waiters.retain(|&waiter| waiter != id);
                }
                _ => {}
            }
        }
    }

    /// Nothing is runnable and nothing has a deadline: this program has genuinely deadlocked.
    ///
    /// Every waiter is answered "timed-out" and released. That is not the truth -- a wait with no
    /// timeout did not time out, it will simply never end -- but it is the same answer this
    /// engine already gave a single agent that waited on a buffer nobody else could reach, and
    /// it is the only answer that is not a hung process. A hang here would not fail one test; it
    /// would stop a 147-chunk sweep on a per-chunk timeout, and diagnose as a regression
    /// somewhere else entirely.
    fn break_deadlock(&mut self) {
        let mut released = false;
        for id in 0..self.states.len() {
            match self.states[id] {
                State::Waiting { .. } => {
                    self.outcomes[id] = WaitOutcome::TimedOut;
                    self.states[id] = State::Runnable;
                    released = true;
                }
                State::AwaitingBroadcast | State::Broadcasting => {
                    self.states[id] = State::Runnable;
                    released = true;
                }
                _ => {}
            }
        }
        self.waiters.clear();
        if !released {
            // Every agent has finished. Nothing is left to hand the baton to, and any thread
            // still parked is one that must stop.
            self.terminating = true;
        }
    }

    /// Whether every started agent has taken the broadcast, which is what `broadcast` waits for.
    fn broadcast_delivered(&self) -> bool {
        self.mailboxes
            .iter()
            .enumerate()
            .all(|(id, slot)| slot.is_none() || self.states[id] == State::Finished)
    }

    /// Hand the baton on, having just put `me` into `states[me]`.
    ///
    /// The loop is ADR 0012's rule with one extension: prefer another runnable agent; failing
    /// that let a runnable `me` keep going, up to [`SPIN_QUANTA`] times in a row; failing that
    /// move the clock to the earliest deadline; failing that admit the deadlock.
    fn schedule(&mut self, me: usize) {
        // Patience first: agents that have done nothing but spin for a while are agents waiting
        // on something only the clock can deliver.
        if self.spun > SPIN_QUANTA
            && let Some(instant) = self.earliest_deadline()
        {
            self.spun = 0;
            self.now_us = self.now_us.max(instant);
            self.expire(self.now_us);
        }
        if let Some(next) = self.next_other(me) {
            self.turn = next;
            return;
        }
        if self.states[me] == State::Runnable {
            self.turn = me;
            return;
        }
        // Nothing at all is runnable, which is ADR 0012's condition exactly: move the clock to
        // the earliest deadline, and no further.
        self.spun = 0;
        loop {
            match self.earliest_deadline() {
                Some(instant) => {
                    self.now_us = self.now_us.max(instant);
                    self.expire(self.now_us);
                }
                None => self.break_deadlock(),
            }
            if let Some(next) = self.any_runnable() {
                self.turn = next;
                return;
            }
            if self.terminating {
                self.turn = me;
                return;
            }
        }
    }
}

/// The agents of one program, and the turn between them.
pub(crate) struct AgentGroup {
    sched: Mutex<Sched>,
    baton: Condvar,
    /// Read without the lock on every quantum boundary, so a program with one agent pays a
    /// relaxed load and nothing else. False until `$262.agent.start` has actually started one.
    concurrent: AtomicBool,
    execution: ExecutionCapabilities,
    limits: ExecutionLimits,
    threads: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

impl AgentGroup {
    /// The group a main agent starts with: one agent, itself, holding the baton.
    pub(crate) fn new(execution: ExecutionCapabilities, limits: ExecutionLimits) -> Arc<Self> {
        Arc::new(Self {
            sched: Mutex::new(Sched {
                now_us: 0,
                turn: 0,
                states: vec![State::Runnable],
                outcomes: vec![WaitOutcome::TimedOut],
                waiters: VecDeque::new(),
                reports: VecDeque::new(),
                mailboxes: vec![None],
                spun: 0,
                terminating: false,
            }),
            baton: Condvar::new(),
            concurrent: AtomicBool::new(false),
            execution,
            limits,
            threads: Mutex::new(Vec::new()),
        })
    }

    /// Whether the quantum check in the interpreter's dispatch loop has anything to do.
    pub(crate) fn concurrent(&self) -> bool {
        self.concurrent.load(Ordering::Relaxed)
    }

    /// The virtual clock, in milliseconds -- `$262.agent.monotonicNow`.
    pub(crate) fn now_ms(&self) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        {
            lock(&self.sched).now_us as f64 / 1_000.0
        }
    }

    /// Park until this agent has the baton again.
    fn park(&self, guard: MutexGuard<'_, Sched>, me: usize) -> Result<(), Terminated> {
        let mut guard = guard;
        while guard.turn != me && !guard.terminating {
            guard = self
                .baton
                .wait(guard)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        if guard.terminating {
            return Err(Terminated);
        }
        Ok(())
    }

    /// Put `me` into `state` and give the baton up until `me` is runnable again.
    fn block(&self, me: usize, state: State) -> Result<(), Terminated> {
        let mut guard = lock(&self.sched);
        if guard.terminating {
            return Err(Terminated);
        }
        // A quantum boundary is the agents making no progress anybody else can see; anything
        // else is progress, and resets the patience that lets the clock move under a spin.
        if state == State::Runnable {
            guard.spun = guard.spun.saturating_add(1);
        } else {
            guard.spun = 0;
        }
        guard.states[me] = state;
        guard.schedule(me);
        self.baton.notify_all();
        self.park(guard, me)
    }

    /// The quantum boundary: stay runnable, but let somebody else have a turn.
    ///
    /// Called only when [`Self::concurrent`] is true, which is only after a second agent exists.
    pub(crate) fn tick(&self, me: usize) -> Result<(), Terminated> {
        self.block(me, State::Runnable)
    }

    /// `$262.agent.sleep(ms)` on the virtual clock: no wall-clock time passes.
    pub(crate) fn sleep(&self, me: usize, milliseconds: f64) -> Result<(), Terminated> {
        let until = lock(&self.sched)
            .now_us
            .saturating_add(microseconds(milliseconds));
        self.block(me, State::Sleeping { until })
    }

    /// `Atomics.wait`: join the waiter list for `address` and give the baton up.
    ///
    /// The caller has already compared the cell against the expected value while holding the
    /// bytes, which is what makes this the specification's atomic compare-and-block rather than
    /// two operations with a gap: no other agent can have run in between, because no other agent
    /// runs until this one gives the baton up here.
    pub(crate) fn wait(
        &self,
        me: usize,
        address: (usize, usize),
        timeout_ms: f64,
    ) -> Result<WaitOutcome, Terminated> {
        let mut guard = lock(&self.sched);
        if guard.terminating {
            return Err(Terminated);
        }
        let until = if timeout_ms.is_finite() {
            Some(guard.now_us.saturating_add(microseconds(timeout_ms)))
        } else {
            None
        };
        guard.outcomes[me] = WaitOutcome::TimedOut;
        guard.waiters.push_back(me);
        guard.states[me] = State::Waiting { address, until };
        guard.schedule(me);
        self.baton.notify_all();
        self.park(guard, me)?;
        Ok(lock(&self.sched).outcomes[me])
    }

    /// `Atomics.notify`: wake up to `count` waiters on `address`, oldest first.
    ///
    /// The count this answers is exactly how many agents were in the list, and it is the same on
    /// every run because the schedule that put them there is.
    pub(crate) fn notify(&self, address: (usize, usize), count: usize) -> usize {
        let mut guard = lock(&self.sched);
        let mut woken = Vec::new();
        for &id in &guard.waiters {
            if woken.len() >= count {
                break;
            }
            if matches!(guard.states[id], State::Waiting { address: at, .. } if at == address) {
                woken.push(id);
            }
        }
        for &id in &woken {
            guard.outcomes[id] = WaitOutcome::Woken;
            guard.states[id] = State::Runnable;
            guard.waiters.retain(|&waiter| waiter != id);
        }
        if !woken.is_empty() {
            guard.spun = 0;
            self.baton.notify_all();
        }
        woken.len()
    }

    /// `$262.agent.report(text)`.
    pub(crate) fn report(&self, text: String) {
        lock(&self.sched).reports.push_back(text);
    }

    /// `$262.agent.getReport()`, which answers `null` when there is nothing yet.
    pub(crate) fn take_report(&self) -> Option<String> {
        lock(&self.sched).reports.pop_front()
    }

    /// `$262.agent.broadcast(sab)`: give every started agent a handle onto the same bytes, and
    /// block until each has taken it.
    ///
    /// Blocking is what INTERPRETING.md specifies, and it is safe here because a broadcast no
    /// agent will ever take is a deadlock the scheduler detects rather than a hang.
    pub(crate) fn broadcast(
        &self,
        me: usize,
        store: &SharedStore,
        max_byte_length: Option<usize>,
    ) -> Result<(), Terminated> {
        {
            let mut guard = lock(&self.sched);
            let payload = Broadcast {
                store: Arc::clone(store),
                max_byte_length,
            };
            for id in 0..guard.mailboxes.len() {
                if id != me && guard.states[id] != State::Finished {
                    guard.mailboxes[id] = Some(payload.clone());
                    if guard.states[id] == State::AwaitingBroadcast {
                        guard.states[id] = State::Runnable;
                    }
                }
            }
            if guard.broadcast_delivered() {
                return Ok(());
            }
        }
        loop {
            self.block(me, State::Broadcasting)?;
            let guard = lock(&self.sched);
            if guard.broadcast_delivered() || guard.terminating {
                return Ok(());
            }
        }
    }

    /// `$262.agent.receiveBroadcast(callback)`: block until there is one, then take it.
    pub(crate) fn receive_broadcast(&self, me: usize) -> Result<Broadcast, Terminated> {
        loop {
            {
                let mut guard = lock(&self.sched);
                if guard.terminating {
                    return Err(Terminated);
                }
                if let Some(payload) = guard.mailboxes[me].take() {
                    // Whoever is waiting in `broadcast` may now be finished waiting.
                    for id in 0..guard.states.len() {
                        if guard.states[id] == State::Broadcasting && guard.broadcast_delivered() {
                            guard.states[id] = State::Runnable;
                        }
                    }
                    self.baton.notify_all();
                    return Ok(payload);
                }
            }
            self.block(me, State::AwaitingBroadcast)?;
        }
    }

    /// This agent's script and event loop are over.
    pub(crate) fn finish(&self, me: usize) {
        let mut guard = lock(&self.sched);
        if guard.terminating {
            return;
        }
        guard.spun = 0;
        guard.states[me] = State::Finished;
        // A `broadcast` blocked on an agent that has now left is no longer blocked.
        for id in 0..guard.states.len() {
            if guard.states[id] == State::Broadcasting && guard.broadcast_delivered() {
                guard.states[id] = State::Runnable;
            }
        }
        guard.schedule(me);
        self.baton.notify_all();
    }

    /// `$262.agent.start(source)`: register an agent, spawn its thread, and let it wait its turn.
    ///
    /// The source is the host's prelude and the program's text together, because a `$262` is the
    /// host's statement about what it can do and the new realm has not heard it yet. The prelude
    /// arrives as a capability rather than being spelled here, for ADR 0031's reason.
    pub(crate) fn start(self: &Arc<Self>, source: String) -> Result<(), ExecutionError> {
        let id = {
            let mut guard = lock(&self.sched);
            if guard.states.len() >= MAX_AGENTS {
                return Err(ExecutionError::TypeErrorMessage {
                    message: format!("kiln: at most {MAX_AGENTS} agents may run at once"),
                });
            }
            let id = guard.states.len();
            guard.states.push(State::Runnable);
            guard.outcomes.push(WaitOutcome::TimedOut);
            guard.mailboxes.push(None);
            id
        };
        let group = Arc::clone(self);
        let handle = std::thread::Builder::new()
            .name(format!("kiln-agent-{id}"))
            // An agent runs an ordinary program, so it needs an ordinary program's stack. The
            // interpreter recurses in Rust when a built-in calls back into JavaScript, bounded by
            // `max_native_depth` against a stack this size.
            .stack_size(16 * 1024 * 1024)
            .spawn(move || run_agent(&group, id, &source))
            .map_err(|error| ExecutionError::Host {
                instruction: 0,
                message: format!("kiln: could not start an agent: {error}"),
            })?;
        lock(&self.threads).push(handle);
        self.concurrent.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Stop every agent and join its thread.
    ///
    /// Called from [`AgentOwner::drop`], so it runs on every path out of the main agent -- a
    /// completed program, a thrown error, an unwind. A thread that outlived its case would be a
    /// thread holding a whole realm, and a conformance process runs tens of thousands of cases.
    fn shutdown(&self) {
        {
            let mut guard = lock(&self.sched);
            guard.terminating = true;
        }
        self.baton.notify_all();
        let handles = std::mem::take(&mut *lock(&self.threads));
        for handle in handles {
            // A panic in an agent has already been reported by the default hook; joining it here
            // is how its thread is reclaimed, and re-raising would take the whole run down for
            // one case.
            drop(handle.join());
        }
    }
}

/// The main agent's handle on its group, which shuts the group down when the machine goes away.
pub(crate) struct AgentOwner(pub(crate) Arc<AgentGroup>);

impl Drop for AgentOwner {
    fn drop(&mut self) {
        self.0.shutdown();
    }
}

/// Milliseconds a program asked for, as microseconds on the virtual clock.
fn microseconds(milliseconds: f64) -> u64 {
    if !milliseconds.is_finite() || milliseconds <= 0.0 {
        return 0;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        (milliseconds * 1_000.0).min(u64::MAX as f64) as u64
    }
}

/// One agent's whole life: wait for the baton, build a realm, run the source, leave.
fn run_agent(group: &Arc<AgentGroup>, id: usize, source: &str) {
    if group.park(lock(&group.sched), id).is_err() {
        return;
    }
    let outcome = run_agent_realm(group, id, source);
    // An agent that failed has failed its own program. The main agent finds out the way Test262
    // says it does -- a report that never arrives -- rather than through an error in a realm that
    // is not the one being tested.
    drop(outcome);
    group.finish(id);
}

fn run_agent_realm(group: &Arc<AgentGroup>, id: usize, source: &str) -> Result<(), ExecutionError> {
    let Some(compile) = group.execution.source_compiler() else {
        return Err(ExecutionError::NoSourceCompiler);
    };
    let (program, strictness) = compile(&EvalSource {
        source,
        outer: &[],
        strict: false,
        global_var_scope: true,
        script: true,
    })
    .map_err(|error| ExecutionError::Host {
        instruction: 0,
        message: format!("kiln: an agent's source did not compile: {error:?}"),
    })?;
    let mut runtime = RealmRuntime::new().map_err(|error| ExecutionError::Host {
        instruction: 0,
        message: error.to_string(),
    })?;
    let mut capture = CaptureHost::new();
    let host: &mut dyn HostOps = &mut capture;
    let program = Rc::new(program);
    let locals = vec![Value::Undefined; program.local_count as usize];
    let mut vm = Vm::with_entropy_seed(
        Rc::clone(&program),
        Rc::new(strictness),
        host,
        locals,
        group.limits,
        runtime.capabilities(),
        group.execution,
    )?;
    // The group this agent belongs to, in place of the empty one its machine made for itself.
    vm.agents = Some(Arc::clone(group));
    vm.agent_owner = None;
    vm.agent_id = id;
    vm.this_value = vm.global_object();
    let outcome = runtime
        .enter(|| vm.run())
        .and_then(|_| vm.run_event_loop(&mut runtime));
    vm.release_cycles();
    outcome
}

/// Build a `SharedArrayBuffer` in this realm over bytes another agent already holds.
pub(crate) fn adopt_broadcast(payload: &Broadcast) -> Value {
    Value::ArrayBuffer(Rc::new(BufferData {
        bytes: Bytes::from_store(&payload.store),
        max_byte_length: payload.max_byte_length,
        shared: true,
        detached: std::cell::Cell::new(false),
        immutable: false,
        properties: RefCell::new(ObjectData::default()),
    }))
}

/// The identity `Atomics.wait` and `Atomics.notify` agree on for one cell.
pub(crate) fn address(store: &SharedStore, byte_offset: usize) -> (usize, usize) {
    (Arc::as_ptr(store).cast::<()>() as usize, byte_offset)
}
