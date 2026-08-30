//! Runtime host implementations for Kiln programs.
//!
//! [`RealmRuntime`] is the first slice of the shared realm runtime. It owns a current-thread
//! Tokio runtime and drives a [`Realm`] whose language-level queues remain explicit: Tokio wakes
//! the realm at the next deadline, while the realm decides the order in which JavaScript work
//! runs. Tokio types are deliberately private to this crate.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet, VecDeque};
use std::marker::PhantomData;
use std::rc::Rc;
use std::time::Duration;

use kiln_host_abi::{HostError, HostOp, HostOps, HostValue};

/// An opaque identity returned by [`Realm::set_timeout`] or [`Realm::set_interval`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TimerId(u64);

impl TimerId {
    /// Creates a timer identity from a caller-owned numeric representation.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the numeric representation suitable for an embedding boundary.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Opaque caller-owned work identity carried through the realm queues.
///
/// The scheduler never stores a language closure. An interpreter or native launcher can keep its
/// continuation in whatever representation it owns and use this token to ask the realm to run it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorkId(u64);

impl WorkId {
    /// Creates a work identity from a caller-owned stable value.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the caller-owned value.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// One unit selected by the realm scheduler for the caller to execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkItem<M = WorkId, T = WorkId> {
    /// A FIFO microtask.
    Microtask(M),
    /// A timer callback, including its opaque timer identity.
    Timer { id: TimerId, work: T },
}

/// Failure while constructing the deterministic realm runtime.
#[derive(Debug)]
pub struct RuntimeBuildError(std::io::Error);

impl std::fmt::Display for RuntimeBuildError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "failed to build realm runtime: {}", self.0)
    }
}

impl std::error::Error for RuntimeBuildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

/// A deterministic, current-thread runtime for realm tasks.
///
/// The runtime starts Tokio time paused on the first call to [`Self::drive`]. A drive advances
/// automatically to each realm's next timer only after all currently-ready microtasks have been
/// exhausted. This makes a timer's delay virtual: a one-hour timer costs no wall-clock hour.
///
/// A runtime is intentionally synchronous at this initial seam. Callers enter a realm through
/// [`Self::drive`]; a future async HostOps seam can add an asynchronous entry point without
/// exposing Tokio types or changing the ordering contract.
///
/// `drive` accepts generic caller-owned microtask and timer payloads. Timer payloads must be
/// [`Clone`] when driven because an interval re-arms the same callback before returning it.
pub struct RealmRuntime {
    runtime: tokio::runtime::Runtime,
    time_paused: bool,
    // A runtime owns a current-thread realm and must not accidentally become a cross-thread
    // executor. `Runtime` itself can be moved, but the realm's language state cannot.
    _not_send: PhantomData<Rc<()>>,
}

impl std::fmt::Debug for RealmRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RealmRuntime")
            .field("time_paused", &self.time_paused)
            .finish_non_exhaustive()
    }
}

impl RealmRuntime {
    /// Builds a current-thread runtime with Tokio's timer driver enabled.
    pub fn new() -> Result<Self, RuntimeBuildError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .map_err(RuntimeBuildError)?;
        Ok(Self {
            runtime,
            time_paused: false,
            _not_send: PhantomData,
        })
    }

    /// Runs one synchronous realm operation as the current Tokio task.
    ///
    /// Interpreted bytecode and native continuation calls use this entry rather than executing
    /// beside the runtime that owns their timers and future I/O. The operation stays synchronous
    /// until HostOps grows requests, but it is polled by the same explicit runtime owner now.
    pub fn enter<F, R>(&mut self, operation: F) -> R
    where
        F: FnOnce() -> R,
    {
        self.runtime.block_on(async move { operation() })
    }

    /// Drives `realm` until it has no microtasks or timers left.
    ///
    /// Tokio's paused clock is enabled lazily so a runtime can be constructed before any realm
    /// work is queued. This method must not be called from another Tokio runtime's synchronous
    /// context; the runtime is deliberately an explicit owner rather than an ambient handle.
    pub fn drive<M, T, F, C, E>(
        &mut self,
        realm: &mut Realm<M, T>,
        mut execute: F,
        mut checkpoint: C,
    ) -> Result<(), E>
    where
        T: Clone,
        F: FnMut(&mut Realm<M, T>, WorkItem<M, T>) -> Result<(), E>,
        C: FnMut(&mut Realm<M, T>) -> Result<(), E>,
    {
        if !self.time_paused {
            self.runtime.block_on(async {
                tokio::time::pause();
            });
            self.time_paused = true;
        }
        self.runtime
            .block_on(drive_realm(realm, &mut execute, &mut checkpoint))
    }

    /// Waits for and removes the next timer after the caller has drained its microtasks.
    ///
    /// This lower-level entry point is for machines whose work executor already owns `realm` as
    /// one of its fields. Returning the payload ends the field borrow before the machine invokes
    /// arbitrary language callbacks, so those callbacks can enqueue more work directly into the
    /// same realm. [`Self::drive`] remains the convenient all-in-one path for other callers.
    pub fn next_timer<M, T>(&mut self, realm: &mut Realm<M, T>) -> Option<WorkItem<M, T>>
    where
        T: Clone,
    {
        if !self.time_paused {
            self.runtime.block_on(async {
                tokio::time::pause();
            });
            self.time_paused = true;
        }
        self.runtime.block_on(next_timer(realm))
    }
}

/// A single-thread-owned realm scheduler.
///
/// Microtasks are FIFO. Timers are ordered by `(deadline, sequence)` and each timer is followed
/// by a complete microtask drain. The observable clock starts at the Unix epoch and is represented
/// as elapsed virtual time from that epoch.
pub struct Realm<M = WorkId, T = WorkId> {
    now: Duration,
    next_sequence: u64,
    next_timer_id: u64,
    microtasks: VecDeque<M>,
    timers: BinaryHeap<TimerEntry<T>>,
    cancelled_timers: HashSet<TimerId>,
    active_timers: HashSet<TimerId>,
    _not_send: PhantomData<Rc<()>>,
}

impl<M, T> Default for Realm<M, T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<M, T> std::fmt::Debug for Realm<M, T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Realm")
            .field("now", &self.now)
            .field("microtasks", &self.microtasks.len())
            .field("timers", &self.active_timers.len())
            .finish()
    }
}

impl<M, T> Realm<M, T> {
    /// Creates an empty realm with its observable clock at the Unix epoch.
    pub fn new() -> Self {
        Self {
            now: Duration::ZERO,
            next_sequence: 0,
            // JavaScript timer handles are positive numbers; zero remains an invalid identity.
            next_timer_id: 1,
            microtasks: VecDeque::new(),
            timers: BinaryHeap::new(),
            cancelled_timers: HashSet::new(),
            active_timers: HashSet::new(),
            _not_send: PhantomData,
        }
    }

    /// Returns the realm's current virtual time from the Unix epoch.
    pub fn now(&self) -> Duration {
        self.now
    }

    /// Adds one FIFO microtask. The caller retains ownership of the work represented by `work`.
    pub fn queue_microtask(&mut self, work: M) {
        self.microtasks.push_back(work);
    }

    /// Removes the oldest queued microtask for execution by the realm owner.
    pub fn pop_microtask(&mut self) -> Option<M> {
        self.microtasks.pop_front()
    }

    /// Schedules a one-shot timer relative to the realm's current virtual time.
    pub fn set_timeout(&mut self, delay: Duration, work: T) -> TimerId {
        self.schedule_timer(delay, work, None)
    }

    /// Schedules a repeating timer relative to the realm's current virtual time.
    ///
    /// The next occurrence is inserted before the current occurrence is returned to the caller,
    /// so clearing `id` from its callback prevents the next run. The timer keeps the same identity
    /// across occurrences.
    pub fn set_interval(&mut self, interval: Duration, work: T) -> TimerId
    where
        T: Clone,
    {
        // The first occurrence preserves the requested delay (including zero), while subsequent
        // occurrences advance by at least one millisecond. This matches the interpreter's
        // existing virtual-time rule and prevents a zero-delay interval from spinning forever at
        // one instant.
        self.schedule_timer(interval, work, Some(interval.max(Duration::from_millis(1))))
    }

    fn schedule_timer(&mut self, delay: Duration, work: T, interval: Option<Duration>) -> TimerId {
        // Saturating arithmetic gives callers the same well-defined behaviour as a timer delay
        // clamped to the largest representable duration, without allowing an integer overflow to
        // become a scheduler panic.
        let deadline = self.now.saturating_add(delay);
        let id = TimerId(self.next_timer_id);
        self.next_timer_id = self.next_timer_id.wrapping_add(1);
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.active_timers.insert(id);
        self.timers.push(TimerEntry {
            deadline,
            sequence,
            id,
            work,
            interval,
        });
        id
    }

    /// Cancels a pending timer, returning whether `id` was still active.
    pub fn clear_timeout(&mut self, id: TimerId) -> bool {
        if self.active_timers.remove(&id) {
            self.cancelled_timers.insert(id);
            true
        } else {
            false
        }
    }

    /// Returns whether this realm still has language work waiting to run.
    pub fn has_pending_work(&self) -> bool {
        !self.microtasks.is_empty() || !self.active_timers.is_empty()
    }

    fn next_timer_deadline(&mut self) -> Option<Duration> {
        self.discard_cancelled_timers();
        self.timers.peek().map(|timer| timer.deadline)
    }

    fn discard_cancelled_timers(&mut self) {
        while self
            .timers
            .peek()
            .is_some_and(|timer| self.cancelled_timers.contains(&timer.id))
        {
            let timer = self.timers.pop().expect("peeked timer exists");
            self.cancelled_timers.remove(&timer.id);
        }
    }

    fn run_next_timer(&mut self) -> Option<WorkItem<M, T>>
    where
        T: Clone,
    {
        self.discard_cancelled_timers();
        let timer = self.timers.pop()?;
        self.now = self.now.max(timer.deadline);
        let work = timer.work.clone();
        if let Some(interval) = timer.interval {
            // Rearm before handing control to the callback. `clear_timeout` can therefore cancel
            // the next occurrence from within the current callback.
            let sequence = self.next_sequence;
            self.next_sequence = self.next_sequence.wrapping_add(1);
            let deadline = timer.deadline.saturating_add(interval);
            self.timers.push(TimerEntry {
                deadline,
                sequence,
                id: timer.id,
                work: timer.work,
                interval: Some(interval),
            });
        } else {
            self.active_timers.remove(&timer.id);
        }
        Some(WorkItem::Timer { id: timer.id, work })
    }
}

struct TimerEntry<T> {
    deadline: Duration,
    sequence: u64,
    id: TimerId,
    work: T,
    interval: Option<Duration>,
}

impl<T> PartialEq for TimerEntry<T> {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline && self.sequence == other.sequence && self.id == other.id
    }
}

impl<T> Eq for TimerEntry<T> {}

impl<T> Ord for TimerEntry<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is a max-heap; reverse the semantic order so the earliest, then oldest,
        // timer is returned first.
        other
            .deadline
            .cmp(&self.deadline)
            .then_with(|| other.sequence.cmp(&self.sequence))
            .then_with(|| other.id.0.cmp(&self.id.0))
    }
}

impl<T> PartialOrd for TimerEntry<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

async fn drive_realm<M, T, F, C, E>(
    realm: &mut Realm<M, T>,
    execute: &mut F,
    checkpoint: &mut C,
) -> Result<(), E>
where
    T: Clone,
    F: FnMut(&mut Realm<M, T>, WorkItem<M, T>) -> Result<(), E>,
    C: FnMut(&mut Realm<M, T>) -> Result<(), E>,
{
    loop {
        while let Some(work) = realm.microtasks.pop_front() {
            execute(realm, WorkItem::Microtask(work))?;
        }
        checkpoint(realm)?;
        let Some(deadline) = realm.next_timer_deadline() else {
            return Ok(());
        };

        let delay = deadline.saturating_sub(realm.now);
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        // Tokio's paused clock has advanced to the sleep deadline. The realm's own clock remains
        // authoritative and advances only when the corresponding timer is selected below.
        let work = realm
            .run_next_timer()
            .expect("a timer was present when its deadline was selected");
        execute(realm, work)?;
    }
}

async fn next_timer<M, T>(realm: &mut Realm<M, T>) -> Option<WorkItem<M, T>>
where
    T: Clone,
{
    let deadline = realm.next_timer_deadline()?;
    let delay = deadline.saturating_sub(realm.now);
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
    realm.run_next_timer()
}

/// A deterministic host used by the interpreter, tests, and CLI execution mode.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CaptureHost {
    stdout: String,
}

impl CaptureHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stdout(&self) -> &str {
        &self.stdout
    }

    pub fn into_stdout(self) -> String {
        self.stdout
    }

    pub fn clear(&mut self) {
        self.stdout.clear();
    }
}

impl HostOps for CaptureHost {
    fn call(&mut self, operation: HostOp, arguments: &[HostValue]) -> Result<HostValue, HostError> {
        match operation {
            HostOp::WriteStdout => {
                if !arguments.is_empty() {
                    if !self.stdout.is_empty() && !self.stdout.ends_with('\n') {
                        self.stdout.push('\n');
                    }
                    for (index, argument) in arguments.iter().enumerate() {
                        if index > 0 {
                            self.stdout.push(' ');
                        }
                        self.stdout.push_str(&argument.to_console_string());
                    }
                }
                self.stdout.push('\n');
                Ok(HostValue::Undefined)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn entered_operations_run_under_the_owned_tokio_runtime() {
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        assert!(tokio::runtime::Handle::try_current().is_err());
        assert!(runtime.enter(|| tokio::runtime::Handle::try_current().is_ok()));
    }

    fn drive(realm: &mut Realm, events: &Rc<RefCell<Vec<String>>>) {
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        runtime
            .drive(
                realm,
                |realm, work| {
                    match work {
                        WorkItem::Microtask(id) => {
                            events.borrow_mut().push(format!("micro:{}", id.get()));
                            if id.get() == 1 {
                                // This is appended after the already queued work, proving the
                                // queue is FIFO rather than depth-first.
                                realm.queue_microtask(WorkId::new(3));
                            }
                        }
                        WorkItem::Timer { work, .. } => events.borrow_mut().push(format!(
                            "timer:{}@{}",
                            work.get(),
                            realm.now().as_millis()
                        )),
                    }
                    Ok::<_, ()>(())
                },
                |_| Ok::<_, ()>(()),
            )
            .expect("realm drive succeeds");
    }

    #[test]
    fn drains_fifo_microtasks_before_timers_and_between_callbacks() {
        let mut realm: Realm = Realm::new();
        let events = Rc::new(RefCell::new(Vec::new()));
        realm.queue_microtask(WorkId::new(1));
        realm.queue_microtask(WorkId::new(2));
        realm.set_timeout(Duration::from_millis(10), WorkId::new(4));
        realm.set_timeout(Duration::from_millis(10), WorkId::new(5));
        realm.set_timeout(Duration::from_millis(5), WorkId::new(6));

        drive(&mut realm, &events);

        assert_eq!(
            *events.borrow(),
            [
                "micro:1",
                "micro:2",
                "micro:3",
                "timer:6@5",
                "timer:4@10",
                "timer:5@10",
            ]
        );
        assert_eq!(realm.now(), Duration::from_millis(10));
        assert!(!realm.has_pending_work());
    }

    #[test]
    fn timer_callback_microtasks_run_before_the_next_timer() {
        let mut realm: Realm = Realm::new();
        let events = Rc::new(RefCell::new(Vec::new()));
        realm.set_timeout(Duration::from_millis(1), WorkId::new(1));
        realm.set_timeout(Duration::from_millis(1), WorkId::new(2));

        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        runtime
            .drive(
                &mut realm,
                |realm, work| {
                    if let WorkItem::Timer { work, .. } = work {
                        events.borrow_mut().push(format!("timer:{}", work.get()));
                        realm.queue_microtask(WorkId::new(work.get() + 10));
                    } else if let WorkItem::Microtask(id) = work {
                        events.borrow_mut().push(format!("micro:{}", id.get()));
                    }
                    Ok::<_, ()>(())
                },
                |_| Ok::<_, ()>(()),
            )
            .expect("realm drive succeeds");

        assert_eq!(
            *events.borrow(),
            ["timer:1", "micro:11", "timer:2", "micro:12"]
        );
    }

    #[test]
    fn interval_rearms_before_callback_and_can_be_cleared() {
        let mut realm: Realm = Realm::new();
        let interval = realm.set_interval(Duration::from_millis(3), WorkId::new(7));
        let mut runs = 0;
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        runtime
            .drive(
                &mut realm,
                |realm, work| {
                    if let WorkItem::Timer { id, .. } = work {
                        assert_eq!(id, interval);
                        runs += 1;
                        if runs == 3 {
                            assert!(realm.clear_timeout(id));
                        }
                    }
                    Ok::<_, ()>(())
                },
                |_| Ok::<_, ()>(()),
            )
            .expect("realm drive succeeds");

        assert_eq!(runs, 3);
        assert_eq!(realm.now(), Duration::from_millis(9));
        assert!(!realm.has_pending_work());
    }

    #[test]
    fn zero_interval_has_immediate_first_run_then_advances() {
        let mut realm: Realm = Realm::new();
        let interval = realm.set_interval(Duration::ZERO, WorkId::new(8));
        let mut times = Vec::new();
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        runtime
            .drive(
                &mut realm,
                |realm, work| {
                    if let WorkItem::Timer { id, .. } = work {
                        times.push(realm.now());
                        if times.len() == 3 {
                            assert!(realm.clear_timeout(id));
                        }
                        assert_eq!(id, interval);
                    }
                    Ok::<_, ()>(())
                },
                |_| Ok::<_, ()>(()),
            )
            .expect("realm drive succeeds");

        assert_eq!(
            times,
            [
                Duration::ZERO,
                Duration::from_millis(1),
                Duration::from_millis(2)
            ]
        );
    }

    #[test]
    fn caller_payloads_are_returned_without_a_scheduler_side_map() {
        let mut realm: Realm<String, String> = Realm::new();
        realm.queue_microtask("promise-reaction".to_owned());
        realm.set_timeout(Duration::ZERO, "timer-callback".to_owned());
        let mut seen = Vec::new();
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        runtime
            .drive(
                &mut realm,
                |_, work| {
                    seen.push(match work {
                        WorkItem::Microtask(payload) => format!("micro:{payload}"),
                        WorkItem::Timer { work: payload, .. } => format!("timer:{payload}"),
                    });
                    Ok::<_, ()>(())
                },
                |_| Ok::<_, ()>(()),
            )
            .expect("realm drive succeeds");

        assert_eq!(seen, ["micro:promise-reaction", "timer:timer-callback"]);
    }

    #[test]
    fn paused_time_advances_an_hour_without_wall_clock_waiting() {
        let mut realm: Realm = Realm::new();
        realm.set_timeout(Duration::from_secs(60 * 60), WorkId::new(9));
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        let mut seen = None;
        runtime
            .drive(
                &mut realm,
                |realm, work| {
                    if let WorkItem::Timer { work, .. } = work {
                        assert_eq!(work, WorkId::new(9));
                        seen = Some(realm.now());
                    }
                    Ok::<_, ()>(())
                },
                |_| Ok::<_, ()>(()),
            )
            .expect("realm drive succeeds");

        assert_eq!(seen, Some(Duration::from_secs(60 * 60)));
    }

    #[test]
    fn checkpoint_runs_after_each_microtask_drain() {
        let mut realm = Realm::new();
        let events = Rc::new(RefCell::new(Vec::new()));
        realm.queue_microtask(WorkId::new(1));
        realm.set_timeout(Duration::ZERO, WorkId::new(2));
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        runtime
            .drive(
                &mut realm,
                |_, work| {
                    if let WorkItem::Microtask(id) = work {
                        events.borrow_mut().push(format!("micro:{}", id.get()));
                    } else {
                        events.borrow_mut().push("timer".to_owned());
                    }
                    Ok::<_, ()>(())
                },
                |_| {
                    events.borrow_mut().push("checkpoint".to_owned());
                    Ok::<_, ()>(())
                },
            )
            .expect("realm drive succeeds");

        assert_eq!(
            *events.borrow(),
            ["micro:1", "checkpoint", "timer", "checkpoint"]
        );
    }

    #[test]
    fn captures_console_style_lines() {
        let mut host = CaptureHost::new();
        host.call(
            HostOp::WriteStdout,
            &[
                HostValue::String("answer".into()),
                HostValue::number(42.0),
                HostValue::Boolean(true),
                HostValue::Null,
            ],
        )
        .expect("write succeeds");
        host.call(HostOp::WriteStdout, &[HostValue::Undefined])
            .expect("write succeeds");

        assert_eq!(host.stdout(), "answer 42 true null\nundefined\n");
    }

    #[test]
    fn empty_write_still_terminates_a_line() {
        let mut host = CaptureHost::default();
        host.call(HostOp::WriteStdout, &[]).expect("write succeeds");
        assert_eq!(host.stdout(), "\n");
    }

    #[test]
    fn output_can_be_cleared_or_taken() {
        let mut host = CaptureHost::default();
        host.call(HostOp::WriteStdout, &[HostValue::String("one".into())])
            .expect("write succeeds");
        host.clear();
        assert!(host.stdout().is_empty());

        host.call(HostOp::WriteStdout, &[HostValue::String("two".into())])
            .expect("write succeeds");
        assert_eq!(host.into_stdout(), "two\n");
    }
}
/// Converts a JavaScript millisecond delay into the Realm's virtual duration.
///
/// JavaScript accepts arbitrarily large finite numbers. [`Duration`] has a finite ceiling, so
/// delays beyond it clamp deterministically instead of panicking at either runtime Adapter.
pub fn duration_from_milliseconds(milliseconds: f64) -> Duration {
    Duration::try_from_secs_f64(milliseconds / 1_000.0).unwrap_or(Duration::MAX)
}
