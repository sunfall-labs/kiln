//! Runtime host implementations for Kiln programs.
//!
//! [`RealmRuntime`] is the first slice of the shared realm runtime. It owns a current-thread
//! Tokio runtime and drives a [`Realm`] whose language-level queues remain explicit: Tokio wakes
//! the realm at the next deadline, while the realm decides the order in which JavaScript work
//! runs. Tokio types are deliberately private to this crate.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap, HashSet, VecDeque};
use std::marker::PhantomData;
use std::rc::Rc;
use std::time::Duration;

pub use kiln_artifact_cache::{CapabilityIdentity, RealmCapabilityIdentity};
use kiln_host_abi::{
    AsyncHostError, AsyncHostOp, AsyncHostOps, AsyncHostValue, Completion, FileStat, HostError,
    HostOp, HostOps, HostValue, RequestId,
};
use tokio::sync::mpsc;

/// Default deterministic entropy seed from ADR 0026.
pub const DEFAULT_ENTROPY_SEED: u64 = 0x9E37_79B9_7F4A_7C15;

/// How one realm obtains its observable clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClockCapability {
    /// Virtual time beginning at the Unix epoch and advancing only for scheduled work.
    #[default]
    Virtual,
    /// Time supplied explicitly through [`HostOp::ClockNow`].
    Host,
}

/// How one realm obtains entropy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntropyCapability {
    /// SplitMix64 with ADR 0026's fixed seed.
    Deterministic,
    /// SplitMix64 with a caller-selected seed.
    Seeded(u64),
    /// Entropy supplied explicitly through [`HostOp::EntropyU64`].
    Host,
}

impl Default for EntropyCapability {
    fn default() -> Self {
        Self::Deterministic
    }
}

/// How one realm selects local civil time.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TimeZoneCapability {
    /// UTC, preserving ADR 0015.
    #[default]
    Utc,
    /// A caller-selected IANA zone name.
    Named(String),
    /// A zone name supplied explicitly through [`HostOp::TimeZone`].
    Host,
}

/// How one realm selects its default locale.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LocaleCapability {
    /// `kiln-intl`'s deterministic default locale.
    #[default]
    Deterministic,
    /// A caller-selected Unicode locale identifier.
    Named(String),
    /// A locale tag supplied explicitly through [`HostOp::Locale`].
    Host,
}

/// Explicit environmental inputs for one realm.
///
/// `Default` is intentionally the pre-ADR-0031 realm: virtual epoch clock, fixed entropy, UTC,
/// and the deterministic `kiln-intl` locale. Runtime values for host-backed variants still arrive
/// through HostOps; this value records policy, never ambient state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RealmCapabilities {
    pub clock: ClockCapability,
    pub entropy: EntropyCapability,
    pub timezone: TimeZoneCapability,
    pub locale: LocaleCapability,
}

impl RealmCapabilities {
    /// Typed policy identity for artifact keys. Host *values* are deliberately absent.
    pub fn artifact_identity(&self) -> RealmCapabilityIdentity {
        RealmCapabilityIdentity {
            clock: match self.clock {
                ClockCapability::Virtual => CapabilityIdentity::Virtual,
                ClockCapability::Host => CapabilityIdentity::Host,
            },
            entropy: match self.entropy {
                EntropyCapability::Deterministic => CapabilityIdentity::Fixed,
                EntropyCapability::Seeded(seed) => CapabilityIdentity::Seeded(seed),
                EntropyCapability::Host => CapabilityIdentity::Host,
            },
            timezone: match &self.timezone {
                TimeZoneCapability::Utc => CapabilityIdentity::Utc,
                TimeZoneCapability::Named(name) => CapabilityIdentity::Named(name.clone()),
                TimeZoneCapability::Host => CapabilityIdentity::Host,
            },
            locale: match &self.locale {
                LocaleCapability::Deterministic => CapabilityIdentity::Default,
                LocaleCapability::Named(tag) => CapabilityIdentity::Named(tag.clone()),
                LocaleCapability::Host => CapabilityIdentity::Host,
            },
        }
    }

    pub fn entropy_seed(&self) -> Option<u64> {
        match self.entropy {
            EntropyCapability::Deterministic => Some(DEFAULT_ENTROPY_SEED),
            EntropyCapability::Seeded(seed) => Some(seed),
            EntropyCapability::Host => None,
        }
    }
}

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
    host: TokioHost,
    capabilities: RealmCapabilities,
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
        Self::with_capabilities(RealmCapabilities::default())
    }

    /// Builds a current-thread runtime with explicit environmental capabilities.
    pub fn with_capabilities(capabilities: RealmCapabilities) -> Result<Self, RuntimeBuildError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .map_err(RuntimeBuildError)?;
        Ok(Self {
            runtime,
            time_paused: false,
            host: TokioHost::new(),
            capabilities,
            _not_send: PhantomData,
        })
    }

    pub fn capabilities(&self) -> &RealmCapabilities {
        &self.capabilities
    }

    /// Returns the asynchronous host owned by this realm runtime.
    ///
    /// Host requests are spawned by [`Self::start_host_op`] so their futures always run on this
    /// runtime. Keeping the request table separately borrowable lets the interpreter settle a
    /// completion without exposing Tokio handles or constructing a second executor.
    pub fn host(&mut self) -> &mut TokioHost {
        &mut self.host
    }

    /// Starts one asynchronous host operation on this realm's Tokio runtime.
    pub fn start_host_op(&mut self, operation: AsyncHostOp, referenced: bool) -> RequestId {
        let spawn = self.runtime.handle().clone();
        self.host.start_on(&spawn, operation, referenced)
    }

    /// Waits for the next asynchronous host completion on this realm's Tokio runtime.
    pub fn next_host_completion(&mut self) -> Option<Completion> {
        if self.host.pending.is_empty() {
            return None;
        }
        if self.host.ready.is_empty() {
            let completion = self.runtime.block_on(self.host.receiver.recv())?;
            self.host.collect_ready(completion);
        }
        self.host.take_ready()
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

    /// Gives a caller exclusive access to the realm-owned host state between Tokio polls.
    ///
    /// This is useful for installing the host into a synchronous realm machine whose lifetime
    /// spans several calls to [`Self::enter`] and [`Self::next_host_completion`].
    pub fn host_mut(&mut self) -> &mut TokioHost {
        &mut self.host
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

/// Tokio-owned asynchronous host operations for one realm.
///
/// Requests are assigned a monotonic sequence at submission. Tokio may finish them in any order,
/// but completions that are already ready at the same realm checkpoint are delivered in sequence
/// order. This keeps executor scheduling from becoming observable JavaScript semantics while
/// allowing independent filesystem work to execute concurrently.
pub struct TokioHost {
    next_request: u64,
    next_sequence: u64,
    pending: BTreeMap<RequestId, PendingRequest>,
    ready: BTreeMap<u64, Completion>,
    sender: mpsc::UnboundedSender<Completion>,
    receiver: mpsc::UnboundedReceiver<Completion>,
    _not_send: PhantomData<Rc<()>>,
}

struct PendingRequest {
    sequence: u64,
    referenced: bool,
    task: tokio::task::JoinHandle<()>,
}

impl std::fmt::Debug for TokioHost {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TokioHost")
            .field("pending", &self.pending.len())
            .field("ready", &self.ready.len())
            .finish_non_exhaustive()
    }
}

impl TokioHost {
    /// Builds the request and completion state owned by a [`RealmRuntime`].
    fn new() -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        Self {
            next_request: 1,
            next_sequence: 0,
            pending: BTreeMap::new(),
            ready: BTreeMap::new(),
            sender,
            receiver,
            _not_send: PhantomData,
        }
    }

    fn start_on(
        &mut self,
        runtime: &tokio::runtime::Handle,
        operation: AsyncHostOp,
        referenced: bool,
    ) -> RequestId {
        let request = RequestId::new(self.next_request);
        self.next_request = self.next_request.wrapping_add(1).max(1);
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        let sender = self.sender.clone();
        let task = runtime.spawn(async move {
            let result = execute_async_host_op(operation).await;
            let _ = sender.send(Completion {
                request,
                sequence,
                result,
            });
        });
        self.pending.insert(
            request,
            PendingRequest {
                sequence,
                referenced,
                task,
            },
        );
        request
    }

    /// Starts a request while this host is entered through its owning [`RealmRuntime`].
    pub fn start(&mut self, operation: AsyncHostOp, referenced: bool) -> RequestId {
        <Self as AsyncHostOps>::start(self, operation, referenced)
    }

    fn collect_ready(&mut self, completion: Completion) {
        self.ready.insert(completion.sequence, completion);
        while let Ok(completion) = self.receiver.try_recv() {
            self.ready.insert(completion.sequence, completion);
        }
    }

    fn collect_ready_nonblocking(&mut self) {
        while let Ok(completion) = self.receiver.try_recv() {
            self.ready.insert(completion.sequence, completion);
        }
    }

    fn take_ready(&mut self) -> Option<Completion> {
        let sequence = *self.ready.keys().next()?;
        let completion = self.ready.remove(&sequence)?;
        self.pending.remove(&completion.request);
        Some(completion)
    }
}

impl Drop for TokioHost {
    fn drop(&mut self) {
        for request in self.pending.values() {
            request.task.abort();
        }
        self.pending.clear();
    }
}

impl AsyncHostOps for TokioHost {
    fn start(&mut self, operation: AsyncHostOp, referenced: bool) -> RequestId {
        let runtime = tokio::runtime::Handle::try_current()
            .expect("TokioHost requests must start inside their owning RealmRuntime");
        self.start_on(&runtime, operation, referenced)
    }

    fn cancel(&mut self, request: RequestId) -> bool {
        let Some(pending) = self.pending.remove(&request) else {
            return false;
        };
        pending.task.abort();
        self.ready.remove(&pending.sequence);
        true
    }

    fn has_referenced_work(&self) -> bool {
        self.pending.values().any(|request| request.referenced)
    }

    fn next_completion(&mut self) -> Option<Completion> {
        self.collect_ready_nonblocking();
        self.take_ready()
    }
}

async fn execute_async_host_op(operation: AsyncHostOp) -> Result<AsyncHostValue, AsyncHostError> {
    match operation {
        AsyncHostOp::ReadFile { path } => tokio::fs::read(&path)
            .await
            .map(AsyncHostValue::Bytes)
            .map_err(|error| io_error("open", path, error)),
        AsyncHostOp::WriteFile { path, data } => tokio::fs::write(&path, data)
            .await
            .map(|()| AsyncHostValue::Undefined)
            .map_err(|error| io_error("open", path, error)),
        AsyncHostOp::Stat { path } => tokio::fs::metadata(&path)
            .await
            .map(|metadata| {
                AsyncHostValue::FileStat(FileStat {
                    size: metadata.len(),
                    is_file: metadata.is_file(),
                    is_directory: metadata.is_dir(),
                })
            })
            .map_err(|error| io_error("stat", path, error)),
    }
}

fn io_error(syscall: &str, path: String, error: std::io::Error) -> AsyncHostError {
    let code = match error.kind() {
        std::io::ErrorKind::NotFound => "ENOENT",
        std::io::ErrorKind::PermissionDenied => "EACCES",
        std::io::ErrorKind::AlreadyExists => "EEXIST",
        std::io::ErrorKind::InvalidInput => "EINVAL",
        std::io::ErrorKind::IsADirectory => "EISDIR",
        std::io::ErrorKind::NotADirectory => "ENOTDIR",
        _ => "EIO",
    };
    AsyncHostError::new(code, syscall, Some(path), error.to_string())
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
            HostOp::ClockNow | HostOp::EntropyU64 | HostOp::TimeZone | HostOp::Locale => Err(
                HostError::new(format!("deterministic host does not provide {operation:?}")),
            ),
        }
    }
}

/// Converts a JavaScript millisecond delay into the Realm's virtual duration.
///
/// JavaScript accepts arbitrarily large finite numbers. [`Duration`] has a finite ceiling, so
/// delays beyond it clamp deterministically instead of panicking at either runtime Adapter.
pub fn duration_from_milliseconds(milliseconds: f64) -> Duration {
    Duration::try_from_secs_f64(milliseconds / 1_000.0).unwrap_or(Duration::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::fs;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let id = NEXT.fetch_add(1, AtomicOrdering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("kiln-runtime-{label}-{}-{id}", std::process::id()));
            fs::create_dir_all(&path).expect("create test directory");
            Self(path)
        }

        fn join(&self, path: &str) -> PathBuf {
            self.0.join(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn entered_operations_run_under_the_owned_tokio_runtime() {
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        assert!(tokio::runtime::Handle::try_current().is_err());
        assert!(runtime.enter(|| tokio::runtime::Handle::try_current().is_ok()));
    }

    #[test]
    fn default_capabilities_preserve_the_deterministic_realm() {
        let runtime = RealmRuntime::new().expect("Tokio runtime builds");
        let capabilities = runtime.capabilities();
        assert_eq!(capabilities, &RealmCapabilities::default());
        assert_eq!(capabilities.entropy_seed(), Some(DEFAULT_ENTROPY_SEED));
        assert_eq!(
            capabilities.artifact_identity(),
            RealmCapabilityIdentity::default()
        );
    }

    #[test]
    fn capability_identity_is_unambiguous_and_excludes_host_values() {
        let host = RealmCapabilities {
            clock: ClockCapability::Host,
            entropy: EntropyCapability::Host,
            timezone: TimeZoneCapability::Host,
            locale: LocaleCapability::Host,
        };
        assert_eq!(
            host.artifact_identity(),
            RealmCapabilityIdentity {
                clock: CapabilityIdentity::Host,
                entropy: CapabilityIdentity::Host,
                timezone: CapabilityIdentity::Host,
                locale: CapabilityIdentity::Host,
            }
        );
        assert_eq!(host.entropy_seed(), None);

        let named = RealmCapabilities {
            entropy: EntropyCapability::Seeded(7),
            timezone: TimeZoneCapability::Named("UTC;locale=host".into()),
            locale: LocaleCapability::Named("en-US".into()),
            ..RealmCapabilities::default()
        };
        assert_eq!(
            named.artifact_identity(),
            RealmCapabilityIdentity {
                clock: CapabilityIdentity::Virtual,
                entropy: CapabilityIdentity::Seeded(7),
                timezone: CapabilityIdentity::Named("UTC;locale=host".into()),
                locale: CapabilityIdentity::Named("en-US".into()),
            }
        );
    }

    #[test]
    fn tokio_host_reads_and_writes_owned_bytes() {
        let directory = TestDirectory::new("async-files");
        let path = directory.join("value.bin");
        let path = path.to_string_lossy().into_owned();
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        let write = runtime.start_host_op(
            AsyncHostOp::WriteFile {
                path: path.clone(),
                data: vec![0, 1, 2, 255],
            },
            true,
        );
        assert!(runtime.host().has_referenced_work());
        assert_eq!(
            runtime
                .next_host_completion()
                .expect("write completes")
                .request,
            write
        );
        assert!(!runtime.host().has_referenced_work());

        let read = runtime.start_host_op(AsyncHostOp::ReadFile { path }, true);
        let completion = runtime.next_host_completion().expect("read completes");
        assert_eq!(completion.request, read);
        assert_eq!(
            completion.result,
            Ok(AsyncHostValue::Bytes(vec![0, 1, 2, 255]))
        );
    }

    #[test]
    fn tokio_host_returns_portable_stat_metadata() {
        let directory = TestDirectory::new("async-stat");
        let file = directory.join("value.txt");
        fs::write(&file, b"kiln").expect("seed file");
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        runtime.start_host_op(
            AsyncHostOp::Stat {
                path: file.to_string_lossy().into_owned(),
            },
            true,
        );
        assert_eq!(
            runtime
                .next_host_completion()
                .expect("stat completes")
                .result,
            Ok(AsyncHostValue::FileStat(FileStat {
                size: 4,
                is_file: true,
                is_directory: false,
            }))
        );
    }

    #[test]
    fn tokio_host_maps_missing_paths_to_node_error_fields() {
        let directory = TestDirectory::new("async-missing");
        let path = directory.join("missing.txt").to_string_lossy().into_owned();
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        runtime.start_host_op(AsyncHostOp::ReadFile { path: path.clone() }, true);
        let error = runtime
            .next_host_completion()
            .expect("read completes")
            .result
            .expect_err("missing path fails");
        assert_eq!(error.code, "ENOENT");
        assert_eq!(error.syscall, "open");
        assert_eq!(error.path.as_deref(), Some(path.as_str()));
    }

    #[test]
    fn unreferenced_requests_do_not_keep_the_realm_alive() {
        let directory = TestDirectory::new("async-unref");
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        runtime.start_host_op(
            AsyncHostOp::ReadFile {
                path: directory.join("missing").to_string_lossy().into_owned(),
            },
            false,
        );
        assert!(!runtime.host().has_referenced_work());
        assert!(runtime.next_host_completion().is_some());
    }

    #[test]
    fn cancellation_removes_request_and_liveness() {
        let directory = TestDirectory::new("async-cancel");
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        let request = runtime.start_host_op(
            AsyncHostOp::ReadFile {
                path: directory.join("missing").to_string_lossy().into_owned(),
            },
            true,
        );
        assert!(runtime.host().cancel(request));
        assert!(!runtime.host().cancel(request));
        assert!(!runtime.host().has_referenced_work());
        assert!(runtime.next_host_completion().is_none());
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
