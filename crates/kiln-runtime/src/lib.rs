//! Runtime host implementations for Kiln programs.
//!
//! [`RealmRuntime`] is the first slice of the shared realm runtime. It owns a current-thread
//! Tokio runtime and drives a [`Realm`] whose language-level queues remain explicit: Tokio wakes
//! the realm at the next deadline, while the realm decides the order in which JavaScript work
//! runs. Tokio types are deliberately private to this crate.

mod http_body;
pub mod http_engine;
pub use http_engine::{
    HostTransport, HttpServerEngine, ServerCloseMode, ServerDispatch, ServerEvent, ServerEventKind,
};
mod http_transport;
mod hyper_http;
pub mod node_http;
pub mod node_http_agent;
pub mod node_net;
pub mod node_stream;

use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap, HashSet, VecDeque};
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::rc::Rc;
use std::time::Duration;

pub use kiln_artifact_cache::{CapabilityIdentity, RealmCapabilityIdentity};
use kiln_host_abi::{
    AcceptedTcpStream, ActiveAsyncHostOps, AsyncHostError, AsyncHostOp, AsyncHostOps,
    AsyncHostValue, Completion, FileStat, HostError, HostOp, HostOps, HostValue, RequestId,
    ResourceId, ResourceInfo, ResourceKind, ResourceState,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{
    TcpListener, TcpStream,
    tcp::{OwnedReadHalf, OwnedWriteHalf},
};
use tokio::sync::mpsc;
use tokio_util::task::JoinMap;

use crate::http_body::{BodyEvent, ResponseBodyDriver};
use crate::http_transport::HttpTransportDriver;
use crate::node_http_agent::{AgentDecision, AgentFailure, AgentRelease, HttpAgent, HttpOrigin};

pub use crate::http_transport::{
    HttpCloseCapability, HttpHostOp, HttpResourceId, HttpStreamAction,
};

/// A protocol event selected by the runtime-owned HTTP engine for the realm to publish as Node
/// objects and stream events. No Hyper or Tokio type crosses this seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeHttpEvent {
    RequestHead {
        id: u64,
        request: node_http::HttpRequest,
    },
    RequestData {
        id: u64,
        data: Vec<u8>,
    },
    RequestEnd {
        id: u64,
    },
    Drain {
        id: u64,
    },
    Finish {
        id: u64,
    },
    Error {
        id: u64,
        message: String,
    },
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeHttpClientRequest {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeHttpClientEvent {
    Drain {
        id: u64,
    },
    Finish {
        id: u64,
    },
    ResponseHead {
        id: u64,
        status: u16,
        reason: String,
        headers: Vec<(String, String)>,
    },
    ResponseData {
        id: u64,
        data: Vec<u8>,
    },
    ResponseEnd {
        id: u64,
        reusable: bool,
    },
    Error {
        id: u64,
        message: String,
    },
    Closed,
}

/// How an HTTP connection is retired by its owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpCloseMode {
    /// Drain protocol-selected response bytes before retiring the transport.
    Graceful,
    /// Cancel protocol work and retire the transport immediately.
    Abort,
}

/// Runtime-owned HTTP/1 control used by language adapters.
///
/// This stays separate from `HostOps`: TCP is a versioned capability boundary, while HTTP
/// framing and body backpressure are realm-runtime policy. Only Kiln types cross this seam.
pub trait ActiveHttpRuntime {
    /// Poll protocol tasks that may have become runnable without a new HostOps completion.
    fn poll_tasks(&mut self);
    fn acquire_client_connection(&mut self, origin: HttpOrigin, request: u64) -> AgentDecision;
    fn client_connection_connected(
        &mut self,
        origin: HttpOrigin,
        resource: ResourceId,
        request: u64,
    ) -> bool;
    fn release_client_connection(&mut self, resource: ResourceId, reusable: bool) -> AgentRelease;
    fn client_connection_failed(&mut self, origin: &HttpOrigin) -> AgentFailure;
    fn client_connection_closed(&mut self, resource: ResourceId) -> Option<u64>;
    fn start_connection(
        &mut self,
        resource: ResourceId,
        allow_half_close: bool,
    ) -> Vec<HttpStreamAction>;
    fn connection_started(
        &mut self,
        resource: ResourceId,
        operation: &HttpHostOp,
        request: RequestId,
    ) -> bool;
    /// Consume the only raw resource-close capability authorized for an HTTP-owned connection.
    fn start_connection_close(&mut self, close: HttpCloseCapability) -> Option<RequestId>;
    /// Consume an authorized close while already executing inside this realm runtime.
    fn start_connection_close_in_context(
        &mut self,
        close: HttpCloseCapability,
    ) -> Option<RequestId>;
    fn complete_connection(
        &mut self,
        resource: ResourceId,
        completion: Completion,
    ) -> Option<Vec<HttpStreamAction>>;
    fn poll_connection(&mut self, resource: ResourceId) -> Option<Vec<HttpStreamAction>>;
    fn poll_connection_in_context(&mut self, resource: ResourceId)
    -> Option<Vec<HttpStreamAction>>;
    fn close_connection(
        &mut self,
        resource: ResourceId,
        mode: HttpCloseMode,
    ) -> Option<Vec<HttpStreamAction>>;
    fn take_events(&mut self, resource: ResourceId) -> Vec<RuntimeHttpEvent>;
    fn start_response(
        &mut self,
        resource: ResourceId,
        id: u64,
        status: u16,
        headers: Vec<(String, String)>,
        high_water_mark: usize,
    ) -> bool;
    fn write_response(
        &mut self,
        resource: ResourceId,
        id: u64,
        data: Vec<u8>,
    ) -> Option<node_stream::WriteDisposition>;
    fn end_response(
        &mut self,
        resource: ResourceId,
        id: u64,
        data: Vec<u8>,
    ) -> Option<node_stream::WriteDisposition>;
    fn start_client_connection(&mut self, resource: ResourceId) -> Vec<HttpStreamAction>;
    fn send_client_request(
        &mut self,
        resource: ResourceId,
        request: RuntimeHttpClientRequest,
        id: u64,
        high_water_mark: usize,
    ) -> bool;
    fn write_client_request(
        &mut self,
        resource: ResourceId,
        id: u64,
        data: Vec<u8>,
    ) -> Option<node_stream::WriteDisposition>;
    fn end_client_request(
        &mut self,
        resource: ResourceId,
        id: u64,
        data: Vec<u8>,
    ) -> Option<node_stream::WriteDisposition>;
    fn take_client_events(&mut self, resource: ResourceId) -> Vec<RuntimeHttpClientEvent>;
}

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EntropyCapability {
    /// SplitMix64 with ADR 0026's fixed seed.
    #[default]
    Deterministic,
    /// SplitMix64 with a caller-selected seed.
    Seeded(u64),
    /// Entropy supplied explicitly through [`HostOp::EntropyU64`].
    Host,
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

/// The next externally-driven turn selected by a realm runtime.
///
/// Language code never observes Tokio's race winner directly: a ready host completion is
/// returned as an owned value, while a timer is removed through the realm's deterministic
/// `(deadline, sequence)` ordering.
#[derive(Debug)]
pub enum RuntimeTurn<M, T> {
    Completion(Completion),
    Timer(WorkItem<M, T>),
    /// The realm's asynchronous supervisor failed and the runtime can no longer make progress.
    Failure(RuntimeFailure),
}

/// One expected request and its caller-owned deterministic completion.
///
/// Scripts sit at the same portable HostOps boundary as [`TokioHost`]. They therefore exercise
/// the real stream, HTTP, module, and realm schedulers while replacing only external services.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptedHostStep {
    pub operation: AsyncHostOp,
    pub result: Result<AsyncHostValue, AsyncHostError>,
}

/// Strict deterministic asynchronous provider for compatibility and conformance tests.
///
/// Requests must match the transcript in order. Completions are released only when the realm asks
/// for one, use submission sequence as their tie-break, and retain the normal cancellation and
/// referenced-work semantics. An unexpected request becomes an explicit host error rather than
/// falling through to the machine.
#[derive(Debug, Default)]
pub struct ScriptedHost {
    expected: VecDeque<ScriptedHostStep>,
    pending: BTreeMap<RequestId, (u64, bool, Result<AsyncHostValue, AsyncHostError>)>,
    next_request: u64,
    next_sequence: u64,
    shutdown: bool,
    #[cfg(test)]
    shutdown_observer: Option<Rc<std::cell::Cell<usize>>>,
}

/// The provider boundary owned by [`RealmRuntime`].
///
/// [`AsyncHostOps`] is the portable operation protocol. This companion trait supplies the two
/// pieces needed by the realm pump: observing whether a request is outstanding and waiting for a
/// completion without exposing an executor or provider-specific readiness type. A provider may
/// return an already-scripted completion immediately, or await its own infrastructure (as the
/// Tokio provider does); the realm's timer-versus-completion arbitration remains shared.
pub trait RealmHostProvider: AsyncHostOps {
    /// Start an operation while the realm's LocalSet is active.
    ///
    /// Providers with resource tables may override this to keep resource creation on the realm
    /// task. The default is sufficient for providers whose [`AsyncHostOps::start`] is already
    /// context-independent.
    fn start_on_realm(&mut self, operation: AsyncHostOp, referenced: bool) -> RequestId {
        AsyncHostOps::start(self, operation, referenced)
    }

    /// Cancel an operation through the realm-owned provider boundary.
    fn cancel_on_realm(&mut self, request: RequestId) -> bool {
        AsyncHostOps::cancel(self, request)
    }

    /// Stop admitting work, cancel every outstanding request, and close every owned resource.
    ///
    /// This operation is terminal and idempotent. Requiring it on the provider trait makes realm
    /// teardown a compile-time obligation for every provider rather than relying on executor or
    /// field-drop order as an accidental cancellation protocol.
    fn shutdown_on_realm(&mut self);

    /// Update provider-owned resource liveness synchronously.
    ///
    /// This is metadata, not I/O: Agent idle/reuse transitions must work both inside a running
    /// LocalSet and in deterministic policy tests that never enter Tokio.
    fn set_resource_referenced_now(&mut self, _resource: ResourceId, _referenced: bool) -> bool {
        false
    }

    /// Whether the provider still owns a request that can complete.
    /// Hand the byte transport of an accepted or connected stream to the HTTP engine (ADR
    /// 0042). A provider that cannot (no such stream, or bytes still in flight through the
    /// discrete read/write operations) answers `None` and the caller keeps the bridged path.
    fn take_transport(&mut self, _resource: ResourceId) -> Option<HostTransport> {
        None
    }

    /// A signal that fires whenever a host operation completes, so a drive can wait for
    /// completions without borrowing the host across the wait (ADR 0042: language callbacks
    /// run inside I/O tasks while the drive is parked, and they borrow the realm).
    fn completion_signal(&self) -> Option<Rc<tokio::sync::Notify>> {
        None
    }

    /// Forget a transport handed over earlier, once its connection has ended.
    fn release_transport(&mut self, _resource: ResourceId) -> bool {
        false
    }

    fn has_pending_work(&self) -> bool;

    /// Returns the sticky realm failure, if the provider's asynchronous supervisor failed.
    fn runtime_failure(&self) -> Option<&RuntimeFailure> {
        None
    }

    /// Wait for the next completion in provider-defined infrastructure order.
    fn next_completion_async<'a>(
        &'a mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Completion>, RuntimeFailure>> + 'a>>;
}

impl ScriptedHost {
    pub fn new(steps: impl IntoIterator<Item = ScriptedHostStep>) -> Self {
        Self {
            expected: steps.into_iter().collect(),
            pending: BTreeMap::new(),
            next_request: 1,
            next_sequence: 0,
            shutdown: false,
            #[cfg(test)]
            shutdown_observer: None,
        }
    }

    /// Returns whether every expected operation was submitted and every completion consumed.
    pub fn is_exhausted(&self) -> bool {
        self.expected.is_empty() && self.pending.is_empty()
    }

    /// Number of transcript operations not yet submitted.
    pub fn remaining_steps(&self) -> usize {
        self.expected.len()
    }
}

impl AsyncHostOps for ScriptedHost {
    fn start(&mut self, operation: AsyncHostOp, referenced: bool) -> RequestId {
        assert!(!self.shutdown, "a shut down realm cannot start host work");
        let request = RequestId::new(self.next_request.max(1));
        self.next_request = self.next_request.wrapping_add(1).max(1);
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        let result = match self.expected.pop_front() {
            Some(step) if step.operation == operation => step.result,
            Some(step) => Err(AsyncHostError::new(
                "ERR_KILN_UNEXPECTED_HOST_OP",
                "script",
                None,
                format!("expected {:?}, received {operation:?}", step.operation),
            )),
            None => Err(AsyncHostError::new(
                "ERR_KILN_UNEXPECTED_HOST_OP",
                "script",
                None,
                format!("no scripted operation remains for {operation:?}"),
            )),
        };
        self.pending.insert(request, (sequence, referenced, result));
        request
    }

    fn cancel(&mut self, request: RequestId) -> bool {
        self.pending.remove(&request).is_some()
    }

    fn has_referenced_work(&self) -> bool {
        self.pending.values().any(|(_, referenced, _)| *referenced)
    }

    fn next_completion(&mut self) -> Option<Completion> {
        let (&request, _) = self
            .pending
            .iter()
            .min_by_key(|(_, (sequence, _, _))| *sequence)?;
        let (sequence, _, result) = self.pending.remove(&request)?;
        Some(Completion {
            request,
            sequence,
            result,
        })
    }
}

impl RealmHostProvider for ScriptedHost {
    fn shutdown_on_realm(&mut self) {
        if self.shutdown {
            return;
        }
        self.shutdown = true;
        self.pending.clear();
        #[cfg(test)]
        if let Some(observer) = &self.shutdown_observer {
            observer.set(observer.get() + 1);
        }
    }

    fn has_pending_work(&self) -> bool {
        !self.pending.is_empty()
    }

    fn set_resource_referenced_now(&mut self, _resource: ResourceId, _referenced: bool) -> bool {
        true
    }

    fn next_completion_async<'a>(
        &'a mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Completion>, RuntimeFailure>> + 'a>> {
        Box::pin(async move { Ok(<Self as AsyncHostOps>::next_completion(self)) })
    }
}

/// A fatal failure in the realm-owned asynchronous supervisor.
///
/// Unlike [`AsyncHostError`], this is not the result of one requested operation and must not be
/// exposed to language code as a catchable I/O rejection. Once present, it remains sticky for the
/// lifetime of the realm runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeFailure {
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for RuntimeFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for RuntimeFailure {}

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

/// One interval of stable Tokio runtime diagnostics.
///
/// The snapshot is deliberately made of Kiln-owned scalar types. Tokio and `tokio-metrics` remain
/// implementation details below the runtime seam, and sampling never changes realm scheduling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeMetricsSnapshot {
    /// Wall-clock time since the previous sample (or runtime construction for the first sample).
    pub elapsed: Duration,
    /// Number of worker threads. A realm runtime currently owns exactly one.
    pub workers: usize,
    /// Tasks alive at the instant of the sample.
    pub live_tasks: usize,
    /// Tasks waiting in Tokio's global injection queue.
    pub global_queue_depth: usize,
    /// Number of times runtime workers parked during the interval.
    pub park_count: u64,
    /// Wall-clock duration workers spent polling tasks during the interval.
    pub busy_duration: Duration,
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
/// The Tokio executor, owning nothing a drive mutates.
///
/// Separated from [`RealmState`] so that a drive can hold the state for its whole lifetime while
/// the executor is held alongside it. That is the one thing the previous single struct could not
/// express, and the reason the native adapter re-entered the executor for every small step
/// instead of running inside it once. See ADR 0040.
struct Executor {
    runtime: tokio::runtime::Runtime,
    local: tokio::task::LocalSet,
}

/// What ended a drive's wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveWake {
    /// A host operation completed.
    Completion,
    /// Virtual time reached the earliest timer.
    Timer,
}

/// Wait for a host completion or the next timer, whichever comes first, holding nothing. With
/// a paused clock the sleep costs no wall time once no task can make progress (ADR 0028).
pub async fn wait_for_turn(signal: Rc<tokio::sync::Notify>, delay: Option<Duration>) -> DriveWake {
    match delay {
        Some(delay) => {
            tokio::select! {
                biased;
                _ = signal.notified() => DriveWake::Completion,
                _ = tokio::time::sleep(delay) => DriveWake::Timer,
            }
        }
        None => {
            signal.notified().await;
            DriveWake::Completion
        }
    }
}

/// Let every task woken so far run before the caller parks.
pub async fn yield_to_tasks() {
    tokio::task::yield_now().await;
}

/// A handle on a realm's executor that owes nothing to the realm's borrow.
pub struct DriveExecutor(Rc<Executor>);

impl DriveExecutor {
    /// Run `future` to completion on the realm's current-thread runtime and `LocalSet`.
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.0.local.block_on(&self.0.runtime, future)
    }
}

/// Everything a drive reads and writes, owning nothing that runs futures.
/// Everything a realm owns that is not the executor: the host, its capabilities, the
/// HTTP connection table, and the metrics stream. See ADR 0040.
pub struct RealmState<H> {
    runtime_metrics: tokio_metrics::RuntimeIntervals,
    time_paused: bool,
    host: H,
    capabilities: RealmCapabilities,
    http_connections: BTreeMap<ResourceId, RuntimeHttpConnection>,
    http_agent: HttpAgent,
    /// Server connections that are Hyper tasks over host transports (ADR 0042).
    http_server: HttpServerEngine,
    lifecycle: RealmRuntimeLifecycle,
}

pub struct RealmRuntime<H: RealmHostProvider = TokioHost> {
    executor: Rc<Executor>,
    state: RealmState<H>,
    // A runtime owns a current-thread realm and must not accidentally become a cross-thread
    // executor. `Runtime` itself can be moved, but the realm's language state cannot.
    _not_send: PhantomData<Rc<()>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RealmRuntimeLifecycle {
    Open,
    Closing,
    Closed,
}

impl<H: RealmHostProvider> std::fmt::Debug for RealmRuntime<H> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RealmRuntime")
            .field("time_paused", &self.state.time_paused)
            .field("lifecycle", &self.state.lifecycle)
            .finish_non_exhaustive()
    }
}

impl RealmRuntime<TokioHost> {
    /// Builds a current-thread runtime with Tokio's timer driver enabled.
    pub fn new() -> Result<Self, RuntimeBuildError> {
        Self::with_capabilities(RealmCapabilities::default())
    }

    /// Builds a current-thread runtime with explicit environmental capabilities.
    pub fn with_capabilities(capabilities: RealmCapabilities) -> Result<Self, RuntimeBuildError> {
        Self::with_host_and_capabilities(TokioHost::new(), capabilities)
    }
}

impl<H: RealmHostProvider> RealmRuntime<H> {
    /// Builds a runtime around an explicit asynchronous host provider.
    ///
    /// The current-thread Tokio runtime and LocalSet remain the realm scheduler for every
    /// provider. Scripted providers therefore replace external services only; they do not create
    /// a second executor or bypass the normal completion ingress.
    pub fn with_host(host: H) -> Result<Self, RuntimeBuildError> {
        Self::with_host_and_capabilities(host, RealmCapabilities::default())
    }

    /// Builds a runtime around an explicit provider and capability policy.
    pub fn with_host_and_capabilities(
        host: H,
        capabilities: RealmCapabilities,
    ) -> Result<Self, RuntimeBuildError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .map_err(RuntimeBuildError)?;
        let runtime_metrics = tokio_metrics::RuntimeMonitor::new(runtime.handle()).intervals();
        Ok(Self {
            executor: Rc::new(Executor {
                runtime,
                local: tokio::task::LocalSet::new(),
            }),
            state: RealmState {
                runtime_metrics,
                time_paused: false,
                host,
                capabilities,
                http_connections: BTreeMap::new(),
                http_agent: HttpAgent::default(),
                http_server: HttpServerEngine::default(),
                lifecycle: RealmRuntimeLifecycle::Open,
            },
            _not_send: PhantomData,
        })
    }

    /// Borrow the executor and the realm state at once, disjointly.
    ///
    /// This is the whole point of the split, and the one thing the previous single struct could
    /// not express. A drive needs the state for its entire lifetime *and* needs the executor to
    /// run it; with both behind one `&mut self` those two borrows conflict, so the native adapter
    /// re-entered the executor for each small step. Two fields borrow independently, so a caller
    /// can hold `&mut RealmState` across an `await` while the executor drives it. See ADR 0040.
    ///
    /// Nothing here runs a future yet; step 1 of the sequence is the split alone.
    #[allow(dead_code)]
    fn split(&mut self) -> (&Executor, &mut RealmState<H>) {
        (&self.executor, &mut self.state)
    }

    /// The executor, detached from this runtime's borrow, so a drive can `block_on` one future
    /// for the life of a program while callbacks inside it borrow the runtime (ADR 0040 step 4,
    /// ADR 0042).
    pub fn drive_executor(&self) -> DriveExecutor {
        DriveExecutor(Rc::clone(&self.executor))
    }

    /// Try to start one host operation, rejecting admission after teardown begins.
    pub fn try_start_host_op(
        &mut self,
        operation: AsyncHostOp,
        referenced: bool,
    ) -> Option<RequestId> {
        if self.is_shutdown() {
            return None;
        }
        Some(self.executor.local.block_on(&self.executor.runtime, async {
            RealmHostProvider::start_on_realm(&mut self.state.host, operation, referenced)
        }))
    }

    /// Starts one asynchronous host operation on this realm's Tokio runtime.
    pub fn start_host_op(&mut self, operation: AsyncHostOp, referenced: bool) -> RequestId {
        self.try_start_host_op(operation, referenced)
            .expect("a shut down realm cannot start host work")
    }

    /// Starts a host operation while the caller is already executing inside this realm runtime.
    ///
    /// Language adapters use this path from native calls. It must not re-enter `block_on`; Tokio
    /// forbids nesting a runtime, and `spawn_local` already sees the active `LocalSet` here.
    pub fn start_host_op_in_context(
        &mut self,
        operation: AsyncHostOp,
        referenced: bool,
    ) -> RequestId {
        assert!(
            !self.is_shutdown(),
            "a shut down realm cannot start host work"
        );
        RealmHostProvider::start_on_realm(&mut self.state.host, operation, referenced)
    }

    /// Waits for the next asynchronous host completion on this realm's Tokio runtime.
    pub fn next_host_completion(&mut self) -> Result<Option<Completion>, RuntimeFailure> {
        if let Some(failure) = self.state.host.runtime_failure() {
            return Err(failure.clone());
        }
        if !self.state.host.has_pending_work() {
            return Ok(None);
        }
        self.executor.local.block_on(
            &self.executor.runtime,
            self.state.host.next_completion_async(),
        )
    }

    /// Wait for either the next host completion or the realm's next timer.
    ///
    /// This is the sole blocking ingress used by Node execution. Waiting on I/O first would
    /// starve timeouts behind an open socket; running virtual timers first would make every
    /// timeout beat already-ready I/O. Tokio polls both, and biased selection gives an already
    /// queued completion the checkpoint priority before virtual time advances.
    pub fn next_turn<M, T>(&mut self, realm: &mut Realm<M, T>) -> Option<RuntimeTurn<M, T>>
    where
        T: Clone,
    {
        if self.is_shutdown() {
            return None;
        }
        if !self.state.time_paused {
            self.executor.local.block_on(&self.executor.runtime, async {
                tokio::time::pause();
            });
            self.state.time_paused = true;
        }
        if let Some(failure) = self.state.host.runtime_failure() {
            return Some(RuntimeTurn::Failure(failure.clone()));
        }
        if let Some(completion) = AsyncHostOps::next_completion(&mut self.state.host) {
            return Some(RuntimeTurn::Completion(completion));
        }
        if let Some(failure) = self.state.host.runtime_failure() {
            return Some(RuntimeTurn::Failure(failure.clone()));
        }
        let deadline = realm.next_timer_deadline();
        if !self.state.host.has_pending_work() {
            return self.next_timer(realm).map(RuntimeTurn::Timer);
        }
        self.executor.local.block_on(&self.executor.runtime, async {
            match deadline {
                Some(deadline) => {
                    let delay = deadline.saturating_sub(realm.now);
                    tokio::select! {
                        biased;
                        completion = self.state.host.next_completion_async() => {
                            match completion {
                                Ok(Some(completion)) => Some(RuntimeTurn::Completion(completion)),
                                Ok(None) => None,
                                Err(failure) => Some(RuntimeTurn::Failure(failure)),
                            }
                        }
                        _ = tokio::time::sleep(delay) => {
                            realm.run_next_timer().map(RuntimeTurn::Timer)
                        }
                    }
                }
                None => match self.state.host.next_completion_async().await {
                    Ok(Some(completion)) => Some(RuntimeTurn::Completion(completion)),
                    Ok(None) => None,
                    Err(failure) => Some(RuntimeTurn::Failure(failure)),
                },
            }
        })
    }

    /// Select a turn only while referenced host work or a realm timer can keep execution alive.
    /// Unreferenced I/O remains registered and may still complete at later checkpoints, but does
    /// not make a Node realm wait indefinitely on an idle keep-alive socket.
    pub fn next_referenced_turn<M, T>(
        &mut self,
        realm: &mut Realm<M, T>,
    ) -> Option<RuntimeTurn<M, T>>
    where
        T: Clone,
    {
        if self.is_shutdown() {
            return None;
        }
        if let Some(failure) = self.state.host.runtime_failure() {
            return Some(RuntimeTurn::Failure(failure.clone()));
        }
        if !self.state.host.has_referenced_work() && realm.next_timer_deadline().is_none() {
            return None;
        }
        self.next_turn(realm)
    }

    /// Attach the HTTP engine to an accepted stream (ADR 0042): the host hands over its
    /// transport and Hyper drives it, calling `dispatch` directly for every protocol event.
    pub fn server_attach(
        &mut self,
        resource: ResourceId,
        allow_half_close: bool,
        dispatch: Rc<dyn ServerDispatch>,
    ) -> bool {
        if self.is_shutdown() || self.state.http_server.is_attached(resource) {
            return false;
        }
        let Some(transport) = self.state.host.take_transport(resource) else {
            return false;
        };
        self.state
            .http_server
            .attach(resource, transport, allow_half_close, dispatch)
    }

    /// The direct HTTP server engine.
    pub fn http_server(&mut self) -> &mut HttpServerEngine {
        &mut self.state.http_server
    }

    /// Whether any direct server connection is still open.
    pub fn has_open_server_connections(&self) -> bool {
        self.state.http_server.open_connections() != 0
    }

    /// The host's completion signal, for a drive that waits outside any borrow of this runtime.
    pub fn completion_signal(&self) -> Option<Rc<tokio::sync::Notify>> {
        self.state.host.completion_signal()
    }

    /// Wake a parked drive: something it was not waiting for has changed what it should wait
    /// for, such as a timer armed from inside a connection callback.
    pub fn wake_drive(&self) {
        if let Some(signal) = self.state.host.completion_signal() {
            signal.notify_one();
        }
    }

    /// Pause Tokio time for the deterministic clock, once.
    pub fn pause_time(&mut self) {
        if !self.state.time_paused {
            let _entered = self.executor.runtime.enter();
            tokio::time::pause();
            self.state.time_paused = true;
        }
    }

    /// A turn that is ready now, without parking: a failure, an already-completed host
    /// operation, or nothing. The caller decides how to wait when this is `None`.
    pub fn ready_turn<M, T>(&mut self) -> Option<RuntimeTurn<M, T>>
    where
        T: Clone,
    {
        if self.is_shutdown() {
            return None;
        }
        if let Some(failure) = self.state.host.runtime_failure() {
            return Some(RuntimeTurn::Failure(failure.clone()));
        }
        if let Some(completion) = AsyncHostOps::next_completion(&mut self.state.host) {
            return Some(RuntimeTurn::Completion(completion));
        }
        if let Some(failure) = self.state.host.runtime_failure() {
            return Some(RuntimeTurn::Failure(failure.clone()));
        }
        None
    }

    /// Whether a drive still has something to wait for: referenced host work, or a timer.
    pub fn has_referenced_host_work(&self) -> bool {
        self.state.host.has_referenced_work()
    }

    /// Whether any host operation is pending, referenced or not: whether a completion can
    /// still arrive.
    pub fn has_pending_host_work(&self) -> bool {
        self.state.host.has_pending_work()
    }

    /// Requests in flight on direct server connections.
    pub fn requests_in_flight(&self) -> usize {
        self.state.http_server.requests_in_flight()
    }

    /// A one-line description of the host's pending work, for `KILN_HTTP_TRACE`.
    pub fn describe_host_work(&self) -> String
    where
        H: std::fmt::Debug,
    {
        format!("{:?}", self.state.host)
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
        assert!(!self.is_shutdown(), "a shut down realm cannot be entered");
        self.executor
            .local
            .block_on(&self.executor.runtime, async move { operation() })
    }

    /// Install one Hyper HTTP/1 connection on an existing portable TCP resource.
    pub fn start_http1_connection(&mut self, resource: ResourceId) -> Vec<HttpStreamAction> {
        self.start_http1_connection_with_options(resource, false)
    }

    pub fn start_http1_connection_with_options(
        &mut self,
        resource: ResourceId,
        allow_half_close: bool,
    ) -> Vec<HttpStreamAction> {
        assert!(
            !self.is_shutdown(),
            "a shut down realm cannot start an HTTP connection"
        );
        assert!(
            !self.state.http_connections.contains_key(&resource),
            "an HTTP connection is already registered for resource {resource:?}"
        );
        // The listening server is the Node-visible resource that keeps the realm alive. Accepted
        // HTTP keep-alive sockets are Hyper-owned implementation resources; their reads remain
        // registered for reuse but must not outlive `server.close()` merely because they inherited
        // the listener's referenced flag at accept time. Referenced writes and close operations
        // still keep active work alive through their individual HostOps requests.
        let _ = self.state.host.set_resource_referenced_now(resource, false);
        let (io, mut transport) = HttpTransportDriver::new(HttpResourceId::new(resource));
        let (event_sender, event_receiver) = mpsc::unbounded_channel();
        let (response_sender, response_receiver) = mpsc::unbounded_channel();
        let task = self
            .executor
            .local
            .spawn_local(hyper_http::event_connection(
                io,
                allow_half_close,
                event_sender,
                response_receiver,
            ));
        let actions = transport.start();
        self.state.http_connections.insert(
            resource,
            RuntimeHttpConnection {
                transport,
                task,
                events: Some(event_receiver),
                responses: Some(response_sender),
                response_bodies: BTreeMap::new(),
                body_events: BTreeMap::new(),
                close_when_responses_finish: false,
                protocol_closed: false,
                close_after_response: HashSet::new(),
                pending_heads: BTreeMap::new(),
                head_requests: HashSet::new(),
                bodyless_responses: HashSet::new(),
                client_requests: None,
                client_events: None,
                client_bodies: BTreeMap::new(),
                client_body_events: BTreeMap::new(),
                completed_client_requests: HashSet::new(),
            },
        );
        actions
    }

    pub fn start_http1_client_connection(&mut self, resource: ResourceId) -> Vec<HttpStreamAction> {
        assert!(
            !self.is_shutdown(),
            "a shut down realm cannot start an HTTP connection"
        );
        assert!(
            !self.state.http_connections.contains_key(&resource),
            "an HTTP connection is already registered for resource {resource:?}"
        );
        let (io, mut transport) = HttpTransportDriver::new(HttpResourceId::new(resource));
        let (request_sender, request_receiver) = mpsc::unbounded_channel();
        let (event_sender, event_receiver) = mpsc::unbounded_channel();
        let task = self
            .executor
            .local
            .spawn_local(hyper_http::client_connection(
                io,
                request_receiver,
                event_sender,
            ));
        let actions = transport.start();
        self.state.http_connections.insert(
            resource,
            RuntimeHttpConnection {
                transport,
                task,
                events: None,
                responses: None,
                response_bodies: BTreeMap::new(),
                body_events: BTreeMap::new(),
                close_when_responses_finish: false,
                protocol_closed: false,
                close_after_response: HashSet::new(),
                pending_heads: BTreeMap::new(),
                head_requests: HashSet::new(),
                bodyless_responses: HashSet::new(),
                client_requests: Some(request_sender),
                client_events: Some(event_receiver),
                client_bodies: BTreeMap::new(),
                client_body_events: BTreeMap::new(),
                completed_client_requests: HashSet::new(),
            },
        );
        actions
    }

    /// Consume and register the raw close selected by an HTTP transport.
    ///
    /// HTTP actions cannot express `AsyncHostOp::CloseResource` directly. Keeping the conversion
    /// here ensures adapters cannot bypass the protocol drain/abort decision accidentally.
    #[doc(hidden)]
    pub fn start_http_connection_close(&mut self, close: HttpCloseCapability) -> Option<RequestId> {
        self.start_http_connection_close_with_context(close, false)
    }

    /// Consume an authorized HTTP close while already executing inside this realm runtime.
    #[doc(hidden)]
    pub fn start_http_connection_close_in_context(
        &mut self,
        close: HttpCloseCapability,
    ) -> Option<RequestId> {
        self.start_http_connection_close_with_context(close, true)
    }

    fn start_http_connection_close_with_context(
        &mut self,
        close: HttpCloseCapability,
        in_context: bool,
    ) -> Option<RequestId> {
        let resource = close.into_resource();
        let raw = resource.resource();
        if !self.state.http_connections.contains_key(&raw) {
            return None;
        }
        let operation = AsyncHostOp::CloseResource { resource: raw };
        let request = if in_context {
            self.start_host_op_in_context(operation, true)
        } else {
            self.start_host_op(operation, true)
        };
        let accepted = self
            .state
            .http_connections
            .get_mut(&raw)
            .is_some_and(|connection| connection.transport.close_started(request));
        if accepted {
            Some(request)
        } else {
            self.cancel_host_op(request);
            None
        }
    }

    /// Poll woken Hyper tasks and return bytes they selected for the existing HostOps stream.
    #[doc(hidden)]
    pub fn poll_http_connection(&mut self, resource: ResourceId) -> Option<Vec<HttpStreamAction>> {
        self.poll_http_tasks();
        self.state
            .http_connections
            .get_mut(&resource)
            .map(|connection| connection.transport.pump_write())
    }

    /// Return transport writes while the caller is already executing inside this realm runtime.
    ///
    /// Sending a response command wakes Hyper on the same `LocalSet`; the task is polled when the
    /// current synchronous language callback yields back to Tokio. This method must not nest
    /// `block_on`, so it only collects bytes already selected at the current checkpoint.
    pub fn poll_http_connection_in_context(
        &mut self,
        resource: ResourceId,
    ) -> Option<Vec<HttpStreamAction>> {
        self.state
            .http_connections
            .get_mut(&resource)
            .map(|connection| connection.transport.pump_write())
    }

    /// Poll every realm-owned HTTP task once before the event loop blocks on HostOps.
    ///
    /// A protocol task can become runnable without another socket completion: supplying the
    /// response head for one HTTP/1 request lets Hyper dispatch the next request already buffered
    /// in that connection. This checkpoint keeps that work under Tokio's scheduler while making
    /// it visible to the synchronous language adapter before it waits on an otherwise-idle read.
    /// One cooperative turn, for a caller that repeats until the checkpoint stops progressing.
    ///
    /// [`Self::poll_http_tasks`] drains a fixed four turns because its caller had no way to ask
    /// whether another was needed. A caller that loops on its own progress does, and one turn per
    /// round is both cheaper and strictly more thorough: it stops at quiescence rather than at
    /// four, and it keeps going past four when a connection still has work.
    /// Advances the local tasks that are already runnable, without parking the thread.
    ///
    /// The checkpoint calls this several times per request, so what it costs per call matters.
    /// `block_on` around a `yield_now` would do the same scheduling work, but it also arms and
    /// disarms the I/O driver each time, and that is a `kevent` per call. A run of 14,175
    /// requests parked for real 13 times: the driver was almost always being armed only to find
    /// work already waiting.
    ///
    /// Polling the `LocalSet` directly runs exactly the tasks that are ready and returns. It
    /// deliberately does not turn the reactor, so readiness that has not arrived yet will not be
    /// discovered here. That is not a gap the checkpoint has to cover. A round that applies no
    /// actions and drains no events ends the checkpoint, control returns to the drive, and the
    /// drive's own await parks and turns the reactor; the work resumes through
    /// [`Self::poll_http_tasks_after_wake`]. Progress that needs the kernel still waits for the
    /// kernel, in the one place that was already going to park.
    pub fn poll_http_tasks_once(&mut self) {
        // Polling the set as a future needs it exclusively. During an async drive (ADR 0042) a
        // DriveExecutor holds the executor and the set is already being run by that drive's
        // `block_on`, so there is nothing to do here; the drive yields instead.
        let Some(executor) = Rc::get_mut(&mut self.executor) else {
            return;
        };
        let _entered = executor.runtime.enter();
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        let _ = std::pin::Pin::new(&mut executor.local).poll(&mut context);
    }

    pub fn poll_http_tasks(&mut self) {
        if self.is_shutdown() {
            return;
        }
        self.executor.local.block_on(&self.executor.runtime, async {
            // Hyper commonly needs more than one cooperative turn: a body wake schedules the
            // connection, which schedules transport output or a service response. A zero-length
            // timeout races the first poll and made progress depend on executor polling details.
            // A small fixed drain advances no clock and keeps the checkpoint reproducible while
            // allowing each already-runnable layer to hand work to the next.
            //
            // Each yield here is a park_yield, a reactor turn. The interpreter's Node loop
            // depends on those turns: with fewer than four, or with direct LocalSet polls in
            // their place, it parks in `next_turn` and never wakes, which the conformance
            // coordinator reproduces on every server probe. The native drive has no such
            // dependency and uses `poll_http_tasks_after_wake`, which polls directly; why the two
            // loops differ here is not yet established and is worth its own measurement.
            for _ in 0..4 {
                tokio::task::yield_now().await;
            }
        });
    }

    /// Advance protocol tasks after the caller has just delivered a completion or command that
    /// woke them. Unlike [`Self::poll_http_tasks`], this path may await one scheduler yield: the
    /// concrete wake guarantees progress and the adapter immediately drains the resulting events.
    /// The native drive's checkpoint after a host completion: the four cooperative turns
    /// without a reactor turn.
    ///
    /// The drive reaches this from `next_turn`'s own park, which just delivered every readiness
    /// the kernel had, so there is nothing for a reactor turn to find. Not paying for one is the
    /// difference between a server whose throughput is flat from one connection to sixty-four
    /// and one whose throughput rises with concurrency: measured 2.1x / 7.4x / 4.9x at
    /// concurrency 1 / 16 / 64 on `bench/server` (2026-09-01).
    #[doc(hidden)]
    pub fn poll_http_tasks_after_wake(&mut self) {
        if self.is_shutdown() {
            return;
        }
        for _ in 0..4 {
            self.poll_http_tasks_once();
        }
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
        assert!(!self.is_shutdown(), "a shut down realm cannot be driven");
        if !self.state.time_paused {
            self.executor.local.block_on(&self.executor.runtime, async {
                tokio::time::pause();
            });
            self.state.time_paused = true;
        }
        self.executor.local.block_on(
            &self.executor.runtime,
            drive_realm(realm, &mut execute, &mut checkpoint),
        )
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
        if self.is_shutdown() {
            return None;
        }
        if !self.state.time_paused {
            self.executor.local.block_on(&self.executor.runtime, async {
                tokio::time::pause();
            });
            self.state.time_paused = true;
        }
        self.executor
            .local
            .block_on(&self.executor.runtime, next_timer(realm))
    }
}

/// Realm operations that need no executor.
///
/// Splitting these off is what makes the nesting rule structural rather than a runtime
/// check: a drive holding `&mut RealmState` cannot name a method that would park the
/// thread, because those live on [`RealmRuntime`] and need its executor. See ADR 0040.
impl<H: RealmHostProvider> RealmState<H> {
    /// Returns whether terminal realm teardown has begun.
    pub fn is_shutdown(&self) -> bool {
        self.lifecycle != RealmRuntimeLifecycle::Open
    }

    /// Tear down this realm's asynchronous infrastructure exactly once.
    ///
    /// Admission closes first. HTTP transports then select and retire their correlated
    /// cancellation identities before the provider aborts remaining work and drops its resource
    /// table. Hyper tasks and Agent state are discarded only after those protocol transitions.
    pub fn shutdown(&mut self) -> bool {
        if self.lifecycle != RealmRuntimeLifecycle::Open {
            return false;
        }
        self.lifecycle = RealmRuntimeLifecycle::Closing;

        let mut connections = std::mem::take(&mut self.http_connections);
        for connection in connections.values_mut() {
            connection.task.abort();
            for action in connection.transport.abort() {
                if let HttpStreamAction::Cancel(request) = action {
                    RealmHostProvider::cancel_on_realm(&mut self.host, request);
                }
            }
        }
        drop(connections);
        self.http_agent = HttpAgent::default();
        RealmHostProvider::shutdown_on_realm(&mut self.host);
        self.lifecycle = RealmRuntimeLifecycle::Closed;
        true
    }

    pub fn capabilities(&self) -> &RealmCapabilities {
        &self.capabilities
    }

    /// Samples stable whole-runtime counters accumulated since the previous call.
    ///
    /// These are developer diagnostics, not JavaScript-observable time or scheduler state. The
    /// sample's `elapsed` and durations use wall time even though realm timers use paused Tokio
    /// time for deterministic execution.
    pub fn sample_runtime_metrics(&mut self) -> RuntimeMetricsSnapshot {
        let metrics = self
            .runtime_metrics
            .next()
            .expect("Tokio runtime metric intervals are infinite");
        RuntimeMetricsSnapshot {
            elapsed: metrics.elapsed,
            workers: metrics.workers_count,
            live_tasks: metrics.live_tasks_count,
            global_queue_depth: metrics.global_queue_depth,
            park_count: metrics.total_park_count,
            busy_duration: metrics.total_busy_duration,
        }
    }

    #[cfg(test)]
    fn host(&mut self) -> &mut H {
        &mut self.host
    }

    /// Returns whether this realm still owns an asynchronous host request.
    ///
    /// The native ABI uses this to decide whether its realm drive must wait for a provider
    /// completion after language work is exhausted. The provider's resource liveness remains
    /// separate: this reports pending requests, not whether those requests keep the realm alive.
    pub fn has_pending_host_work(&self) -> bool {
        self.host.has_pending_work()
    }

    /// Returns whether pending requests or live resources should keep this realm running.
    ///
    /// Adapters retain bookkeeping for unreferenced sockets so future work can reuse them, but
    /// only the provider owns the authoritative resource-liveness bit.
    pub fn has_referenced_host_work(&self) -> bool {
        self.host.has_referenced_work()
    }

    /// Cancels one asynchronous host request owned by this realm.
    pub fn cancel_host_op(&mut self, request: RequestId) -> bool {
        RealmHostProvider::cancel_on_realm(&mut self.host, request)
    }

    #[cfg(test)]
    fn host_mut(&mut self) -> &mut H {
        &mut self.host
    }

    #[doc(hidden)]
    pub fn http_connection_started(
        &mut self,
        resource: ResourceId,
        operation: &HttpHostOp,
        request: RequestId,
    ) -> bool {
        self.http_connections
            .get_mut(&resource)
            .is_some_and(|connection| connection.transport.started(operation, request))
    }

    #[doc(hidden)]
    pub fn complete_http_connection(
        &mut self,
        resource: ResourceId,
        completion: Completion,
    ) -> Option<Vec<HttpStreamAction>> {
        let idle_client = self
            .http_connections
            .get(&resource)
            .is_some_and(|connection| connection.client_requests.is_some())
            && !self.http_agent.has_active_request(resource);
        let mut actions = self
            .http_connections
            .get_mut(&resource)?
            .transport
            .completed(completion)?;
        if idle_client {
            // A keep-alive connection may be reset after its response has completed and the Agent
            // has made it idle. There is no language request left to reject, so expose the close
            // transition but not the transport error. Busy connections still surface every error.
            actions.retain(|action| !matches!(action, HttpStreamAction::Error(_)));
        }
        Some(actions)
    }

    /// Forget a connection whose transport has reported Close.
    ///
    /// The Close event is the transport's last word for the resource: Hyper's task is done and
    /// every body driver has finished or been abandoned. A caller that has consumed that event
    /// retires the entry so the table stays proportional to open connections rather than to
    /// connections ever accepted, which is the difference a server measures over a long run.
    /// Retiring is the caller's call because its own checkpoint may still want to observe the
    /// closed connection once more before it lets go.
    pub fn retire_http_connection(&mut self, resource: ResourceId) -> bool {
        let released = self.host.release_transport(resource);
        self.http_connections.remove(&resource).is_some() || released
    }

    /// Retire an HTTP transport after Hyper has completed its connection future.
    pub fn close_http_connection(
        &mut self,
        resource: ResourceId,
        mode: HttpCloseMode,
    ) -> Option<Vec<HttpStreamAction>> {
        let connection = self.http_connections.get_mut(&resource)?;
        if mode == HttpCloseMode::Abort {
            return Some(connection.transport.abort());
        }
        // `server.close()` may run in the same JavaScript turn as `response.end()`. The response
        // command has only been sent to Hyper at that point; its task has not necessarily polled
        // yet, so the transport can have no queued write even though a body is active. Keep the
        // close request as a marker and let the normal checkpoint drain Hyper's writes. Idle
        // connections have no response body to drain and can be retired immediately.
        // A body that Hyper has already finished no longer holds the connection open, whether or
        // not the realm has drained its `finish` yet. Asking the driver rather than waiting for
        // `take_http_events` to empty the map keeps `close` from depending on who looked last.
        let responses_in_flight = connection
            .response_bodies
            .values()
            .any(|body| !body.finished());
        if !responses_in_flight || connection.protocol_closed {
            Some(connection.transport.close())
        } else {
            connection.close_when_responses_finish = true;
            Some(Vec::new())
        }
    }

    /// Drain protocol events already selected at this realm checkpoint.
    pub fn take_http_events(&mut self, resource: ResourceId) -> Vec<RuntimeHttpEvent> {
        let Some(connection) = self.http_connections.get_mut(&resource) else {
            return Vec::new();
        };
        let mut events = Vec::new();
        // Body events are drained first. Hyper can publish Closed immediately after consuming
        // the final response body, but Node observes finish before connection close.
        let mut finished_bodies = Vec::new();
        for (&id, receiver) in &mut connection.body_events {
            while let Ok(event) = receiver.try_recv() {
                if event == BodyEvent::Finish {
                    finished_bodies.push(id);
                }
                events.push(match event {
                    BodyEvent::Drain => RuntimeHttpEvent::Drain { id },
                    BodyEvent::Finish => {
                        if connection.close_after_response.remove(&id) {
                            connection.transport.close_requested();
                        }
                        RuntimeHttpEvent::Finish { id }
                    }
                });
            }
        }
        for id in finished_bodies {
            connection.response_bodies.remove(&id);
            connection.body_events.remove(&id);
            connection.head_requests.remove(&id);
            connection.bodyless_responses.remove(&id);
        }
        if connection.close_when_responses_finish
            && connection
                .response_bodies
                .values()
                .all(|body| body.finished())
        {
            connection.close_when_responses_finish = false;
            connection.transport.close_requested();
        }
        let Some(receiver) = connection.events.as_mut() else {
            return events;
        };
        while let Ok(event) = receiver.try_recv() {
            if matches!(event, hyper_http::EngineEvent::Closed) {
                connection.protocol_closed = true;
                // Hyper has finished the HTTP/1 state machine. Retire the portable TCP resource
                // after any bytes it already selected are acknowledged so the peer observes EOF
                // and raw `node:net` clients can complete their natural `end`/`close` lifecycle.
                connection.transport.close_requested();
            }
            events.push(match event {
                hyper_http::EngineEvent::RequestHead { id, request } => {
                    if !request.keep_alive {
                        connection.close_after_response.insert(id);
                    }
                    if request.method.eq_ignore_ascii_case("HEAD") {
                        connection.head_requests.insert(id);
                    }
                    RuntimeHttpEvent::RequestHead { id, request }
                }
                hyper_http::EngineEvent::RequestData { id, data } => {
                    RuntimeHttpEvent::RequestData { id, data }
                }
                hyper_http::EngineEvent::RequestEnd { id } => RuntimeHttpEvent::RequestEnd { id },
                hyper_http::EngineEvent::Error { id, message } => {
                    RuntimeHttpEvent::Error { id, message }
                }
                hyper_http::EngineEvent::Closed => RuntimeHttpEvent::Closed,
            });
        }
        events
    }

    /// Resume the matching Hyper service after the Node request listener finishes its turn.
    pub fn respond_http1(
        &mut self,
        resource: ResourceId,
        id: u64,
        response: node_http::HttpResponse,
    ) -> bool {
        let Some(connection) = self.http_connections.get_mut(&resource) else {
            return false;
        };
        let body_length = response.body.len();
        let mut headers = response.headers;
        if !headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        {
            headers.push(("content-length".into(), body_length.to_string()));
        }
        let mut body = ResponseBodyDriver::default();
        body.end(response.body);
        let (body_event_sender, body_event_receiver) = mpsc::unbounded_channel();
        let command = hyper_http::ResponseCommand {
            id,
            status: response.status,
            headers,
            body: body.body(body_event_sender),
        };
        if connection
            .responses
            .as_ref()
            .is_none_or(|responses| responses.send(command).is_err())
        {
            return false;
        }
        connection.response_bodies.insert(id, body);
        connection.body_events.insert(id, body_event_receiver);
        true
    }

    pub fn start_http1_response(
        &mut self,
        resource: ResourceId,
        id: u64,
        status: u16,
        headers: Vec<(String, String)>,
        high_water_mark: usize,
    ) -> bool {
        let Some(connection) = self.http_connections.get_mut(&resource) else {
            return false;
        };
        if connection.response_bodies.contains_key(&id) {
            return false;
        }
        if connection.responses.is_none() {
            return false;
        }
        let body = ResponseBodyDriver::new(high_water_mark);
        let (body_event_sender, body_event_receiver) = mpsc::unbounded_channel();
        // The head is parked rather than sent. Hyper decides framing from the head it is given,
        // and the framing Node would pick is not known until the script either streams a chunk or
        // ends the response with its whole body. `flush_response_head` sends it either way, and
        // always within the same JavaScript turn, because a realm only starts a response from
        // `write` or `end`.
        connection.pending_heads.insert(
            id,
            hyper_http::ResponseCommand {
                id,
                status,
                headers,
                body: body.body(body_event_sender),
            },
        );
        connection.response_bodies.insert(id, body);
        connection.body_events.insert(id, body_event_receiver);
        true
    }

    pub fn write_http1_response(
        &mut self,
        resource: ResourceId,
        id: u64,
        data: Vec<u8>,
    ) -> Option<node_stream::WriteDisposition> {
        let connection = self.http_connections.get_mut(&resource)?;
        // A chunk before `end` leaves the length unknown, so the head goes to Hyper as the realm
        // built it and Hyper chunks the body.
        if !flush_response_head(connection, id, None) {
            return None;
        }
        Some(connection.response_bodies.get_mut(&id)?.write(data))
    }

    pub fn end_http1_response(
        &mut self,
        resource: ResourceId,
        id: u64,
        data: Vec<u8>,
    ) -> Option<node_stream::WriteDisposition> {
        let connection = self.http_connections.get_mut(&resource)?;
        // `end` with the head still parked means this call carries the entire body, which is the
        // one case where Node declares a length instead of chunking.
        if !flush_response_head(connection, id, Some(data.len())) {
            return None;
        }
        let bodyless = connection.bodyless_responses.contains(&id);
        let body = connection.response_bodies.get_mut(&id)?;
        let disposition = body.end(data);
        if bodyless {
            body.finish_without_body();
        }
        Some(disposition)
    }

    pub fn send_http1_client_request(
        &mut self,
        resource: ResourceId,
        request: RuntimeHttpClientRequest,
        id: u64,
        high_water_mark: usize,
    ) -> bool {
        let Some(connection) = self.http_connections.get_mut(&resource) else {
            return false;
        };
        if connection.client_bodies.contains_key(&id) {
            return false;
        }
        let body = ResponseBodyDriver::new(high_water_mark);
        let (body_event_sender, body_event_receiver) = mpsc::unbounded_channel();
        let command = hyper_http::ClientRequestCommand {
            id,
            method: request.method,
            target: request.target,
            headers: request.headers,
            body: body.body(body_event_sender),
        };
        if connection
            .client_requests
            .as_ref()
            .is_none_or(|requests| requests.send(command).is_err())
        {
            return false;
        }
        connection.client_bodies.insert(id, body);
        connection
            .client_body_events
            .insert(id, body_event_receiver);
        true
    }

    pub fn write_http1_client_request(
        &mut self,
        resource: ResourceId,
        id: u64,
        data: Vec<u8>,
    ) -> Option<node_stream::WriteDisposition> {
        self.http_connections
            .get_mut(&resource)?
            .client_bodies
            .get_mut(&id)
            .map(|body| body.write(data))
    }

    pub fn end_http1_client_request(
        &mut self,
        resource: ResourceId,
        id: u64,
        data: Vec<u8>,
    ) -> Option<node_stream::WriteDisposition> {
        self.http_connections
            .get_mut(&resource)?
            .client_bodies
            .get_mut(&id)
            .map(|body| body.end(data))
    }

    pub fn take_http1_client_events(
        &mut self,
        resource: ResourceId,
    ) -> Vec<RuntimeHttpClientEvent> {
        if self
            .http_connections
            .get(&resource)
            .and_then(|connection| connection.client_events.as_ref())
            .is_none()
        {
            return Vec::new();
        }
        let mut events = Vec::new();
        let connection = self
            .http_connections
            .get_mut(&resource)
            .expect("checked above");
        let mut finished_bodies = Vec::new();
        for (&id, receiver) in &mut connection.client_body_events {
            while let Ok(event) = receiver.try_recv() {
                if event == BodyEvent::Finish {
                    finished_bodies.push(id);
                }
                events.push(match event {
                    BodyEvent::Drain => RuntimeHttpClientEvent::Drain { id },
                    BodyEvent::Finish => RuntimeHttpClientEvent::Finish { id },
                });
            }
        }
        for id in finished_bodies {
            connection.client_bodies.remove(&id);
            connection.client_body_events.remove(&id);
        }
        let Some(receiver) = connection.client_events.as_mut() else {
            return events;
        };
        while let Ok(event) = receiver.try_recv() {
            if let Some(event) =
                map_http1_client_event(event, &mut connection.completed_client_requests)
            {
                events.push(event);
            }
        }
        events
    }
}

/// A realm is its state plus the executor that drives it, so callers that only reach for
/// state read through the runtime unchanged. The conversion is deliberately one-way: state
/// cannot reach back to the executor.
impl<H: RealmHostProvider> std::ops::Deref for RealmRuntime<H> {
    type Target = RealmState<H>;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl<H: RealmHostProvider> std::ops::DerefMut for RealmRuntime<H> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

struct RuntimeHttpConnection {
    transport: HttpTransportDriver,
    task: tokio::task::JoinHandle<Result<(), hyper::Error>>,
    events: Option<mpsc::UnboundedReceiver<hyper_http::EngineEvent>>,
    responses: Option<mpsc::UnboundedSender<hyper_http::ResponseCommand>>,
    response_bodies: BTreeMap<u64, ResponseBodyDriver>,
    body_events: BTreeMap<u64, mpsc::UnboundedReceiver<BodyEvent>>,
    close_when_responses_finish: bool,
    protocol_closed: bool,
    close_after_response: HashSet<u64>,
    /// Response heads Hyper has not seen yet. Node picks a framing from what the script does
    /// after the head is built, so the runtime holds the head until `write` streams it or `end`
    /// completes it. See `flush_response_head`.
    pending_heads: BTreeMap<u64, hyper_http::ResponseCommand>,
    /// Requests whose method is HEAD. Node declares no length for them.
    head_requests: HashSet<u64>,
    /// Responses Hyper will never ask for a body from, decided when the head was flushed.
    bodyless_responses: HashSet<u64>,
    client_requests: Option<mpsc::UnboundedSender<hyper_http::ClientRequestCommand>>,
    client_events: Option<mpsc::UnboundedReceiver<hyper_http::ClientEvent>>,
    client_bodies: BTreeMap<u64, ResponseBodyDriver>,
    client_body_events: BTreeMap<u64, mpsc::UnboundedReceiver<BodyEvent>>,
    completed_client_requests: HashSet<u64>,
}

/// Send a parked response head to Hyper, declaring a body length when Node would.
///
/// This is the one place the Content-Length rule lives, so the interpreter and the native path
/// inherit it from the same code. `complete_body` is `Some(len)` only when the caller is `end`
/// and no chunk was streamed first, which is exactly Node's condition for computing a length
/// instead of chunking. A head the realm already committed carries its own framing header and is
/// left alone, and a response that cannot have a body never gets a length: Hyper drops
/// `Transfer-Encoding` for those itself, but it forwards a `Content-Length` that Node omits.
///
/// Returns false only when the connection can no longer accept a response.
fn flush_response_head(
    connection: &mut RuntimeHttpConnection,
    id: u64,
    complete_body: Option<usize>,
) -> bool {
    let Some(mut command) = connection.pending_heads.remove(&id) else {
        // Already flushed by an earlier `write`; the body driver owns the rest.
        return true;
    };
    let carries_body =
        response_status_allows_body(command.status) && !connection.head_requests.contains(&id);
    if !carries_body {
        connection.bodyless_responses.insert(id);
    }
    if let Some(length) = complete_body
        && carries_body
        && !declares_framing(&command.headers)
    {
        command
            .headers
            .push(("content-length".into(), length.to_string()));
    }
    connection
        .responses
        .as_ref()
        .is_some_and(|responses| responses.send(command).is_ok())
}

/// A head the realm already framed. Node leaves a script-supplied `Content-Length` or
/// `Transfer-Encoding` untouched, and so does Kiln.
fn declares_framing(headers: &[(String, String)]) -> bool {
    headers.iter().any(|(name, _)| {
        name.eq_ignore_ascii_case("content-length")
            || name.eq_ignore_ascii_case("transfer-encoding")
    })
}

/// Statuses that carry no body, and therefore no length. Matches Node's `_hasBody`.
fn response_status_allows_body(status: u16) -> bool {
    !matches!(status, 204 | 304) && !(100..200).contains(&status)
}

fn map_http1_client_event(
    event: hyper_http::ClientEvent,
    completed: &mut HashSet<u64>,
) -> Option<RuntimeHttpClientEvent> {
    let id = match &event {
        hyper_http::ClientEvent::ResponseHead { id, .. }
        | hyper_http::ClientEvent::ResponseData { id, .. }
        | hyper_http::ClientEvent::ResponseEnd { id, .. }
        | hyper_http::ClientEvent::Error { id, .. } => Some(*id),
        hyper_http::ClientEvent::Closed => None,
    };
    if id.is_some_and(|id| completed.contains(&id)) {
        return None;
    }
    let terminal = matches!(
        event,
        hyper_http::ClientEvent::ResponseEnd { .. } | hyper_http::ClientEvent::Error { .. }
    );
    let mapped = match event {
        hyper_http::ClientEvent::ResponseHead {
            id,
            status,
            reason,
            headers,
        } => RuntimeHttpClientEvent::ResponseHead {
            id,
            status,
            reason,
            headers,
        },
        hyper_http::ClientEvent::ResponseData { id, data } => {
            RuntimeHttpClientEvent::ResponseData { id, data }
        }
        hyper_http::ClientEvent::ResponseEnd { id, reusable } => {
            RuntimeHttpClientEvent::ResponseEnd { id, reusable }
        }
        hyper_http::ClientEvent::Error { id, message } => {
            RuntimeHttpClientEvent::Error { id, message }
        }
        hyper_http::ClientEvent::Closed => RuntimeHttpClientEvent::Closed,
    };
    if terminal && let Some(id) = id {
        completed.insert(id);
    }
    Some(mapped)
}

impl<H: RealmHostProvider> ActiveHttpRuntime for RealmRuntime<H> {
    fn poll_tasks(&mut self) {
        self.poll_http_tasks();
    }

    fn acquire_client_connection(&mut self, origin: HttpOrigin, request: u64) -> AgentDecision {
        assert!(
            !self.is_shutdown(),
            "a shut down realm cannot acquire an HTTP connection"
        );
        let decision = self.state.http_agent.acquire(origin, request);
        if let AgentDecision::Reuse(resource) = decision {
            let _ = self.state.host.set_resource_referenced_now(resource, true);
        }
        decision
    }

    fn client_connection_connected(
        &mut self,
        origin: HttpOrigin,
        resource: ResourceId,
        request: u64,
    ) -> bool {
        if self.is_shutdown() {
            return false;
        }
        self.state.http_agent.connected(origin, resource, request)
    }

    fn release_client_connection(&mut self, resource: ResourceId, reusable: bool) -> AgentRelease {
        if self.is_shutdown() {
            return AgentRelease::Retire;
        }
        let release = self.state.http_agent.release(resource, reusable);
        if release == AgentRelease::Idle {
            let _ = self.state.host.set_resource_referenced_now(resource, false);
        }
        release
    }

    fn client_connection_failed(&mut self, origin: &HttpOrigin) -> AgentFailure {
        if self.is_shutdown() {
            return AgentFailure::Empty;
        }
        self.state.http_agent.connect_failed(origin)
    }

    fn client_connection_closed(&mut self, resource: ResourceId) -> Option<u64> {
        if self.is_shutdown() {
            return None;
        }
        self.state.http_agent.closed(resource)
    }

    fn start_connection(
        &mut self,
        resource: ResourceId,
        allow_half_close: bool,
    ) -> Vec<HttpStreamAction> {
        self.start_http1_connection_with_options(resource, allow_half_close)
    }

    fn connection_started(
        &mut self,
        resource: ResourceId,
        operation: &HttpHostOp,
        request: RequestId,
    ) -> bool {
        self.http_connection_started(resource, operation, request)
    }

    fn start_connection_close(&mut self, close: HttpCloseCapability) -> Option<RequestId> {
        self.start_http_connection_close(close)
    }

    fn start_connection_close_in_context(
        &mut self,
        close: HttpCloseCapability,
    ) -> Option<RequestId> {
        self.start_http_connection_close_in_context(close)
    }

    fn complete_connection(
        &mut self,
        resource: ResourceId,
        completion: Completion,
    ) -> Option<Vec<HttpStreamAction>> {
        self.complete_http_connection(resource, completion)
    }

    fn poll_connection(&mut self, resource: ResourceId) -> Option<Vec<HttpStreamAction>> {
        self.poll_http_connection(resource)
    }

    fn poll_connection_in_context(
        &mut self,
        resource: ResourceId,
    ) -> Option<Vec<HttpStreamAction>> {
        self.poll_http_connection_in_context(resource)
    }

    fn close_connection(
        &mut self,
        resource: ResourceId,
        mode: HttpCloseMode,
    ) -> Option<Vec<HttpStreamAction>> {
        self.close_http_connection(resource, mode)
    }

    fn take_events(&mut self, resource: ResourceId) -> Vec<RuntimeHttpEvent> {
        self.take_http_events(resource)
    }

    fn start_response(
        &mut self,
        resource: ResourceId,
        id: u64,
        status: u16,
        headers: Vec<(String, String)>,
        high_water_mark: usize,
    ) -> bool {
        self.start_http1_response(resource, id, status, headers, high_water_mark)
    }

    fn write_response(
        &mut self,
        resource: ResourceId,
        id: u64,
        data: Vec<u8>,
    ) -> Option<node_stream::WriteDisposition> {
        self.write_http1_response(resource, id, data)
    }

    fn end_response(
        &mut self,
        resource: ResourceId,
        id: u64,
        data: Vec<u8>,
    ) -> Option<node_stream::WriteDisposition> {
        self.end_http1_response(resource, id, data)
    }

    fn start_client_connection(&mut self, resource: ResourceId) -> Vec<HttpStreamAction> {
        self.start_http1_client_connection(resource)
    }

    fn send_client_request(
        &mut self,
        resource: ResourceId,
        request: RuntimeHttpClientRequest,
        id: u64,
        high_water_mark: usize,
    ) -> bool {
        self.send_http1_client_request(resource, request, id, high_water_mark)
    }

    fn write_client_request(
        &mut self,
        resource: ResourceId,
        id: u64,
        data: Vec<u8>,
    ) -> Option<node_stream::WriteDisposition> {
        self.write_http1_client_request(resource, id, data)
    }

    fn end_client_request(
        &mut self,
        resource: ResourceId,
        id: u64,
        data: Vec<u8>,
    ) -> Option<node_stream::WriteDisposition> {
        self.end_http1_client_request(resource, id, data)
    }

    fn take_client_events(&mut self, resource: ResourceId) -> Vec<RuntimeHttpClientEvent> {
        self.take_http1_client_events(resource)
    }
}

impl Drop for RuntimeHttpConnection {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl<H: RealmHostProvider> Drop for RealmRuntime<H> {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl<H: RealmHostProvider> ActiveAsyncHostOps for RealmRuntime<H> {
    fn start(&mut self, operation: AsyncHostOp, referenced: bool) -> RequestId {
        self.start_host_op_in_context(operation, referenced)
    }

    fn cancel(&mut self, request: RequestId) -> bool {
        RealmHostProvider::cancel_on_realm(&mut self.state.host, request)
    }
}

impl ActiveAsyncHostOps for TokioHost {
    fn start(&mut self, operation: AsyncHostOp, referenced: bool) -> RequestId {
        self.start_on(operation, referenced)
    }

    fn cancel(&mut self, request: RequestId) -> bool {
        AsyncHostOps::cancel(self, request)
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
    next_resource: u64,
    pending: BTreeMap<RequestId, PendingRequest>,
    resources: BTreeMap<ResourceId, HostResource>,
    ready: BTreeMap<u64, Completion>,
    tasks: JoinMap<RequestId, HostCompletion>,
    completed: Rc<tokio::sync::Notify>,
    failure: Option<RuntimeFailure>,
    shutdown: bool,
    _not_send: PhantomData<Rc<()>>,
}

struct HostCompletion {
    request: RequestId,
    sequence: u64,
    lifecycle: Option<ResourceLifecycleUpdate>,
    result: Result<HostCompletionValue, AsyncHostError>,
}

enum ResourceLifecycleUpdate {
    ReadClosed(ResourceId),
    WriteClosed(ResourceId),
}

enum HostCompletionValue {
    Public(AsyncHostValue),
    TcpListener {
        listener: TcpListener,
        referenced: bool,
    },
    TcpStream {
        stream: TcpStream,
        peer: std::net::SocketAddr,
        referenced: bool,
        accepted: bool,
    },
}

enum HostResource {
    TcpListener {
        listener: Rc<TcpListener>,
        info: ResourceInfo,
        referenced: bool,
    },
    /// A stream whose transport the HTTP engine owns now (ADR 0042). Discrete reads and
    /// writes on it fail; close and referencing still resolve.
    TcpTaken {
        info: ResourceInfo,
        referenced: bool,
    },
    TcpStream {
        reader: Rc<tokio::sync::Mutex<OwnedReadHalf>>,
        writer: Rc<tokio::sync::Mutex<OwnedWriteHalf>>,
        info: ResourceInfo,
        referenced: bool,
        read_closed: bool,
        write_closed: bool,
    },
}

struct PendingRequest {
    sequence: u64,
    referenced: bool,
}

impl std::fmt::Debug for TokioHost {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TokioHost")
            .field("pending", &self.pending.len())
            .field(
                "referenced_pending",
                &self.pending.values().filter(|p| p.referenced).count(),
            )
            .field("resources", &self.resources.len())
            .field(
                "referenced_resources",
                &self
                    .resources
                    .values()
                    .filter(|r| match r {
                        HostResource::TcpListener { referenced, .. }
                        | HostResource::TcpStream { referenced, .. }
                        | HostResource::TcpTaken { referenced, .. } => *referenced,
                    })
                    .count(),
            )
            .field("ready", &self.ready.len())
            .finish_non_exhaustive()
    }
}

impl TokioHost {
    /// Builds the request and completion state owned by a [`RealmRuntime`].
    fn new() -> Self {
        Self {
            next_request: 1,
            next_sequence: 0,
            next_resource: 1,
            pending: BTreeMap::new(),
            resources: BTreeMap::new(),
            ready: BTreeMap::new(),
            tasks: JoinMap::new(),
            completed: Rc::new(tokio::sync::Notify::new()),
            failure: None,
            shutdown: false,
            _not_send: PhantomData,
        }
    }

    fn spawn_task<F>(&mut self, request: RequestId, sequence: u64, referenced: bool, task: F)
    where
        F: Future<Output = HostCompletion> + 'static,
    {
        let completed = Rc::clone(&self.completed);
        self.tasks.spawn_local(request, async move {
            let completion = task.await;
            completed.notify_one();
            completion
        });
        let replaced = self.pending.insert(
            request,
            PendingRequest {
                sequence,
                referenced,
            },
        );
        assert!(replaced.is_none(), "request identities must be unique");
    }

    fn start_on(&mut self, operation: AsyncHostOp, referenced: bool) -> RequestId {
        assert!(!self.shutdown, "a shut down realm cannot start host work");
        let request = RequestId::new(self.next_request);
        self.next_request = self.next_request.wrapping_add(1).max(1);
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        let operation = match operation {
            AsyncHostOp::CloseResource { resource } => {
                let result = self.close_resource(resource);
                self.insert_immediate(request, sequence, referenced, result);
                return request;
            }
            AsyncHostOp::SetResourceReferenced {
                resource,
                referenced,
            } => {
                let result = self.set_resource_referenced(resource, referenced);
                self.insert_immediate(request, sequence, referenced, result);
                return request;
            }
            AsyncHostOp::ResourceState { resource } => {
                let result = self.resource_state(resource);
                self.insert_immediate(request, sequence, referenced, result);
                return request;
            }
            AsyncHostOp::TcpListen { host, port } => {
                self.spawn_task(request, sequence, referenced, async move {
                    let result = TcpListener::bind((host.as_str(), port))
                        .await
                        .map(|listener| HostCompletionValue::TcpListener {
                            listener,
                            referenced,
                        })
                        .map_err(|error| socket_error("listen", error));
                    HostCompletion {
                        request,
                        sequence,
                        lifecycle: None,
                        result,
                    }
                });
                return request;
            }
            AsyncHostOp::TcpAccept { listener } => {
                let Some(HostResource::TcpListener {
                    listener,
                    referenced: resource_referenced,
                    ..
                }) = self.resources.get(&listener)
                else {
                    self.insert_immediate(
                        request,
                        sequence,
                        referenced,
                        Err(invalid_resource("accept", listener)),
                    );
                    return request;
                };
                let listener = Rc::clone(listener);
                let accepted_referenced = *resource_referenced;
                self.spawn_task(request, sequence, referenced, async move {
                    let result = listener
                        .accept()
                        .await
                        .map(|(stream, peer)| (stream, peer, accepted_referenced))
                        .map_err(|error| socket_error("accept", error));
                    HostCompletion {
                        request,
                        sequence,
                        lifecycle: None,
                        result: result.map(|(stream, peer, referenced)| {
                            HostCompletionValue::TcpStream {
                                stream,
                                peer,
                                referenced,
                                accepted: true,
                            }
                        }),
                    }
                });
                return request;
            }
            AsyncHostOp::TcpConnect { host, port } => {
                self.spawn_task(request, sequence, referenced, async move {
                    let result = async {
                        let stream = TcpStream::connect((host.as_str(), port)).await?;
                        let peer = stream.peer_addr()?;
                        Ok(HostCompletionValue::TcpStream {
                            stream,
                            peer,
                            referenced,
                            accepted: false,
                        })
                    }
                    .await
                    .map_err(|error| socket_error("connect", error));
                    HostCompletion {
                        request,
                        sequence,
                        lifecycle: None,
                        result,
                    }
                });
                return request;
            }
            AsyncHostOp::TcpRead { stream, max_bytes } => {
                let stream_id = stream;
                let Some(HostResource::TcpStream {
                    reader,
                    read_closed,
                    ..
                }) = self.resources.get(&stream)
                else {
                    self.insert_immediate(
                        request,
                        sequence,
                        referenced,
                        Err(invalid_resource("read", stream)),
                    );
                    return request;
                };
                if *read_closed {
                    self.insert_immediate(
                        request,
                        sequence,
                        referenced,
                        Ok(AsyncHostValue::Bytes(Vec::new())),
                    );
                    return request;
                }
                let reader = Rc::clone(reader);
                self.spawn_task(request, sequence, referenced, async move {
                    let mut bytes = vec![0; max_bytes];
                    let result = async {
                        let mut reader = reader.lock().await;
                        let length = reader.read(&mut bytes).await?;
                        bytes.truncate(length);
                        Ok(AsyncHostValue::Bytes(bytes))
                    }
                    .await
                    .map_err(|error| socket_error("read", error));
                    HostCompletion {
                        request,
                        sequence,
                        lifecycle: result
                            .as_ref()
                            .ok()
                            .filter(|value| matches!(value, AsyncHostValue::Bytes(bytes) if bytes.is_empty()))
                            .map(|_| ResourceLifecycleUpdate::ReadClosed(stream_id)),
                        result: result.map(HostCompletionValue::Public),
                    }
                });
                return request;
            }
            AsyncHostOp::TcpWrite { stream, data } => {
                let stream_id = stream;
                let Some(HostResource::TcpStream {
                    writer,
                    write_closed,
                    ..
                }) = self.resources.get(&stream)
                else {
                    self.insert_immediate(
                        request,
                        sequence,
                        referenced,
                        Err(invalid_resource("write", stream)),
                    );
                    return request;
                };
                if *write_closed {
                    self.insert_immediate(
                        request,
                        sequence,
                        referenced,
                        Err(stream_closed("write", stream_id)),
                    );
                    return request;
                }
                let writer = Rc::clone(writer);
                self.spawn_task(request, sequence, referenced, async move {
                    let result = async {
                        writer.lock().await.write_all(&data).await?;
                        Ok(AsyncHostValue::Undefined)
                    }
                    .await
                    .map_err(|error| socket_error("write", error));
                    HostCompletion {
                        request,
                        sequence,
                        lifecycle: None,
                        result: result.map(HostCompletionValue::Public),
                    }
                });
                return request;
            }
            AsyncHostOp::TcpShutdown { stream } => {
                let stream_id = stream;
                let Some(HostResource::TcpStream {
                    writer,
                    write_closed,
                    ..
                }) = self.resources.get(&stream)
                else {
                    self.insert_immediate(
                        request,
                        sequence,
                        referenced,
                        Err(invalid_resource("shutdown", stream)),
                    );
                    return request;
                };
                if *write_closed {
                    self.insert_immediate(
                        request,
                        sequence,
                        referenced,
                        Ok(AsyncHostValue::Undefined),
                    );
                    return request;
                }
                let writer = Rc::clone(writer);
                self.spawn_task(request, sequence, referenced, async move {
                    let result = writer
                        .lock()
                        .await
                        .shutdown()
                        .await
                        .map(|()| AsyncHostValue::Undefined)
                        .map_err(|error| socket_error("shutdown", error));
                    HostCompletion {
                        request,
                        sequence,
                        lifecycle: result
                            .as_ref()
                            .ok()
                            .map(|_| ResourceLifecycleUpdate::WriteClosed(stream_id)),
                        result: result.map(HostCompletionValue::Public),
                    }
                });
                return request;
            }
            operation => operation,
        };
        self.spawn_task(request, sequence, referenced, async move {
            let result = execute_async_host_op(operation).await;
            HostCompletion {
                request,
                sequence,
                lifecycle: None,
                result: result.map(HostCompletionValue::Public),
            }
        });
        request
    }

    fn insert_immediate(
        &mut self,
        request: RequestId,
        sequence: u64,
        referenced: bool,
        result: Result<AsyncHostValue, AsyncHostError>,
    ) {
        self.pending.insert(
            request,
            PendingRequest {
                sequence,
                referenced,
            },
        );
        self.ready.insert(
            sequence,
            Completion {
                request,
                sequence,
                result,
            },
        );
    }

    fn close_resource(&mut self, resource: ResourceId) -> Result<AsyncHostValue, AsyncHostError> {
        self.resources
            .remove(&resource)
            .map(|_| AsyncHostValue::Undefined)
            .ok_or_else(|| invalid_resource("close", resource))
    }

    fn set_resource_referenced(
        &mut self,
        resource: ResourceId,
        referenced: bool,
    ) -> Result<AsyncHostValue, AsyncHostError> {
        let Some(entry) = self.resources.get_mut(&resource) else {
            return Err(invalid_resource("ref", resource));
        };
        match entry {
            HostResource::TcpListener {
                referenced: current,
                ..
            }
            | HostResource::TcpStream {
                referenced: current,
                ..
            }
            | HostResource::TcpTaken {
                referenced: current,
                ..
            } => *current = referenced,
        }
        Ok(AsyncHostValue::Undefined)
    }

    fn resource_state(&self, resource: ResourceId) -> Result<AsyncHostValue, AsyncHostError> {
        let Some(entry) = self.resources.get(&resource) else {
            return Err(invalid_resource("fstat", resource));
        };
        let state = match entry {
            HostResource::TcpListener {
                info, referenced, ..
            } => ResourceState {
                info: info.clone(),
                referenced: *referenced,
                read_closed: false,
                write_closed: false,
            },
            HostResource::TcpStream {
                info,
                referenced,
                read_closed,
                write_closed,
                ..
            } => ResourceState {
                info: info.clone(),
                referenced: *referenced,
                read_closed: *read_closed,
                write_closed: *write_closed,
            },
            HostResource::TcpTaken { info, referenced } => ResourceState {
                info: info.clone(),
                referenced: *referenced,
                read_closed: false,
                write_closed: false,
            },
        };
        Ok(AsyncHostValue::ResourceState(state))
    }

    fn collect_ready_nonblocking(&mut self) {
        while let Some((request, result)) = self.tasks.try_join_next() {
            match result {
                Ok(completion) => {
                    debug_assert_eq!(completion.request, request);
                    self.promote_completion(completion);
                }
                Err(error) => self.handle_task_failure(request, error),
            }
        }
    }

    fn handle_task_failure(&mut self, request: RequestId, error: tokio::task::JoinError) {
        if error.is_cancelled() || !self.pending.contains_key(&request) {
            return;
        }

        // A task panic is a realm failure, not an individual I/O result. Stop every sibling,
        // discard anything that raced with the failure, and make the failure sticky so neither a
        // timer nor a later provider poll can make the damaged realm appear healthy again.
        self.tasks.abort_all();
        self.pending.clear();
        self.ready.clear();
        self.resources.clear();
        self.failure.get_or_insert_with(|| RuntimeFailure {
            code: "ERR_KILN_RUNTIME_TASK_PANIC",
            message: error.to_string(),
        });
    }

    fn promote_completion(&mut self, completion: HostCompletion) {
        // Cancellation removes the request before aborting its task. A task can already have
        // completed at that boundary; its stale result must not mutate resource lifecycle state
        // or become visible to the realm.
        if !self.pending.contains_key(&completion.request) {
            return;
        }
        if completion.result.is_ok() {
            match completion.lifecycle {
                Some(ResourceLifecycleUpdate::ReadClosed(resource)) => {
                    if let Some(HostResource::TcpStream { read_closed, .. }) =
                        self.resources.get_mut(&resource)
                    {
                        *read_closed = true;
                    }
                }
                Some(ResourceLifecycleUpdate::WriteClosed(resource)) => {
                    if let Some(HostResource::TcpStream { write_closed, .. }) =
                        self.resources.get_mut(&resource)
                    {
                        *write_closed = true;
                    }
                }
                None => {}
            }
        }
        let result = completion.result.map(|value| match value {
            HostCompletionValue::Public(value) => value,
            HostCompletionValue::TcpListener {
                listener,
                referenced,
            } => {
                let local = listener.local_addr().ok();
                let id = ResourceId::new(self.next_resource);
                self.next_resource = self.next_resource.wrapping_add(1).max(1);
                let info = ResourceInfo {
                    id,
                    kind: ResourceKind::TcpListener,
                    address: local.map(|address| address.ip().to_string()),
                    port: local.map(|address| address.port()),
                };
                self.resources.insert(
                    id,
                    HostResource::TcpListener {
                        listener: Rc::new(listener),
                        info: info.clone(),
                        referenced,
                    },
                );
                AsyncHostValue::Resource(info)
            }
            HostCompletionValue::TcpStream {
                stream,
                peer,
                referenced,
                accepted,
            } => {
                let local = stream.local_addr().ok();
                let id = ResourceId::new(self.next_resource);
                self.next_resource = self.next_resource.wrapping_add(1).max(1);
                let resource = ResourceInfo {
                    id,
                    kind: ResourceKind::TcpStream,
                    address: local.map(|address| address.ip().to_string()),
                    port: local.map(|address| address.port()),
                };
                let (reader, writer) = stream.into_split();
                self.resources.insert(
                    id,
                    HostResource::TcpStream {
                        reader: Rc::new(tokio::sync::Mutex::new(reader)),
                        writer: Rc::new(tokio::sync::Mutex::new(writer)),
                        info: resource.clone(),
                        referenced,
                        read_closed: false,
                        write_closed: false,
                    },
                );
                if accepted {
                    AsyncHostValue::AcceptedTcpStream(AcceptedTcpStream {
                        resource,
                        peer_address: peer.ip().to_string(),
                        peer_port: peer.port(),
                    })
                } else {
                    AsyncHostValue::Resource(resource)
                }
            }
        });
        self.ready.insert(
            completion.sequence,
            Completion {
                request: completion.request,
                sequence: completion.sequence,
                result,
            },
        );
    }

    fn take_ready(&mut self) -> Option<Completion> {
        let sequence = *self.ready.keys().next()?;
        let completion = self.ready.remove(&sequence)?;
        self.pending.remove(&completion.request);
        Some(completion)
    }
}

impl AsyncHostOps for TokioHost {
    fn start(&mut self, operation: AsyncHostOp, referenced: bool) -> RequestId {
        assert!(!self.shutdown, "a shut down realm cannot start host work");
        let request = RequestId::new(self.next_request);
        self.next_request = self.next_request.wrapping_add(1).max(1);
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.spawn_task(request, sequence, referenced, async move {
            let result = execute_async_host_op(operation)
                .await
                .map(HostCompletionValue::Public);
            HostCompletion {
                request,
                sequence,
                lifecycle: None,
                result,
            }
        });
        request
    }

    fn cancel(&mut self, request: RequestId) -> bool {
        let Some(pending) = self.pending.remove(&request) else {
            return false;
        };
        self.tasks.abort(&request);
        self.ready.remove(&pending.sequence);
        true
    }

    fn has_referenced_work(&self) -> bool {
        self.pending.values().any(|request| request.referenced)
            || self.resources.values().any(|resource| match resource {
                HostResource::TcpListener { referenced, .. }
                | HostResource::TcpStream { referenced, .. }
                | HostResource::TcpTaken { referenced, .. } => *referenced,
            })
    }

    fn next_completion(&mut self) -> Option<Completion> {
        self.collect_ready_nonblocking();
        self.take_ready()
    }
}

impl RealmHostProvider for TokioHost {
    fn shutdown_on_realm(&mut self) {
        if self.shutdown {
            return;
        }
        self.shutdown = true;
        self.tasks.abort_all();
        self.tasks = JoinMap::new();
        self.pending.clear();
        self.ready.clear();
        self.resources.clear();
    }

    fn start_on_realm(&mut self, operation: AsyncHostOp, referenced: bool) -> RequestId {
        self.start_on(operation, referenced)
    }

    fn cancel_on_realm(&mut self, request: RequestId) -> bool {
        AsyncHostOps::cancel(self, request)
    }

    fn has_pending_work(&self) -> bool {
        !self.pending.is_empty()
    }

    fn runtime_failure(&self) -> Option<&RuntimeFailure> {
        self.failure.as_ref()
    }

    fn set_resource_referenced_now(&mut self, resource: ResourceId, referenced: bool) -> bool {
        self.set_resource_referenced(resource, referenced).is_ok()
    }

    fn completion_signal(&self) -> Option<Rc<tokio::sync::Notify>> {
        Some(Rc::clone(&self.completed))
    }

    fn release_transport(&mut self, resource: ResourceId) -> bool {
        match self.resources.get(&resource) {
            Some(HostResource::TcpTaken { .. }) => self.resources.remove(&resource).is_some(),
            _ => false,
        }
    }

    fn take_transport(&mut self, resource: ResourceId) -> Option<HostTransport> {
        // Both halves must be held by the table alone: a read or write still in flight owns a
        // clone and the socket cannot be reunited under it. The entry stays so a later discrete
        // operation on this stream fails cleanly and referencing still resolves.
        let (reader, writer) = match self.resources.get(&resource) {
            Some(HostResource::TcpStream { reader, writer, .. })
                if Rc::strong_count(reader) == 1 && Rc::strong_count(writer) == 1 =>
            {
                (Rc::clone(reader), Rc::clone(writer))
            }
            _ => return None,
        };
        let Some(HostResource::TcpStream { info, .. }) = self.resources.remove(&resource) else {
            return None;
        };
        // A connection the engine drives does not keep the realm alive; the listener does, as
        // with the bridged path, where the accepted socket is unreferenced on attach.
        let taken = HostResource::TcpTaken {
            info,
            referenced: false,
        };
        self.resources.insert(resource, taken);
        let reader = Rc::try_unwrap(reader).ok()?.into_inner();
        let writer = Rc::try_unwrap(writer).ok()?.into_inner();
        let stream = reader.reunite(writer).ok()?;
        Some(HostTransport(Box::new(stream)))
    }

    fn next_completion_async<'a>(
        &'a mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Completion>, RuntimeFailure>> + 'a>> {
        Box::pin(async move {
            loop {
                self.collect_ready_nonblocking();
                if let Some(failure) = &self.failure {
                    return Err(failure.clone());
                }
                if let Some(completion) = self.take_ready() {
                    return Ok(Some(completion));
                }
                if self.tasks.is_empty() {
                    return Ok(None);
                }
                let (request, result) = self
                    .tasks
                    .join_next()
                    .await
                    .expect("a non-empty task map must yield a task exit");
                match result {
                    Ok(completion) => {
                        debug_assert_eq!(completion.request, request);
                        self.promote_completion(completion);
                    }
                    Err(error) => self.handle_task_failure(request, error),
                }
            }
        })
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
        AsyncHostOp::CloseResource { .. }
        | AsyncHostOp::SetResourceReferenced { .. }
        | AsyncHostOp::ResourceState { .. }
        | AsyncHostOp::TcpListen { .. }
        | AsyncHostOp::TcpAccept { .. }
        | AsyncHostOp::TcpConnect { .. }
        | AsyncHostOp::TcpRead { .. }
        | AsyncHostOp::TcpWrite { .. }
        | AsyncHostOp::TcpShutdown { .. } => {
            unreachable!("resource control operations are handled by the owning table")
        }
    }
}

fn invalid_resource(syscall: &str, resource: ResourceId) -> AsyncHostError {
    AsyncHostError::new(
        "EBADF",
        syscall,
        None,
        format!("unknown or closed resource {}", resource.get()),
    )
}

fn stream_closed(syscall: &str, resource: ResourceId) -> AsyncHostError {
    AsyncHostError::new(
        "EPIPE",
        syscall,
        None,
        format!("stream resource {} has ended", resource.get()),
    )
}

fn socket_error(syscall: &str, error: std::io::Error) -> AsyncHostError {
    let code = match error.kind() {
        std::io::ErrorKind::AddrInUse => "EADDRINUSE",
        std::io::ErrorKind::AddrNotAvailable => "EADDRNOTAVAIL",
        std::io::ErrorKind::ConnectionRefused => "ECONNREFUSED",
        std::io::ErrorKind::ConnectionReset => "ECONNRESET",
        std::io::ErrorKind::NotConnected => "ENOTCONN",
        std::io::ErrorKind::PermissionDenied => "EACCES",
        std::io::ErrorKind::TimedOut => "ETIMEDOUT",
        _ => "EIO",
    };
    AsyncHostError::new(code, syscall, None, error.to_string())
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
    next_ticks: VecDeque<M>,
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
            next_ticks: VecDeque::new(),
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

    /// Visits every queued next-tick and microtask, in queue order, without running or
    /// removing it. The native collector enumerates the roots queued work holds this way.
    pub fn for_each_queued(&self, mut visit: impl FnMut(&M)) {
        self.next_ticks.iter().for_each(&mut visit);
        self.microtasks.iter().for_each(&mut visit);
    }

    /// Visits every armed timer's work, including cancelled entries that have not been
    /// discarded yet, without running or removing it.
    pub fn for_each_timer(&self, mut visit: impl FnMut(&T)) {
        self.timers.iter().for_each(|entry| visit(&entry.work));
    }

    /// Adds one FIFO microtask. The caller retains ownership of the work represented by `work`.
    pub fn queue_microtask(&mut self, work: M) {
        self.microtasks.push_back(work);
    }

    /// Adds one FIFO Node next-tick callback.
    ///
    /// This is a language scheduling lane, not a Tokio task. The realm owner drains it ahead of
    /// Promise jobs at each checkpoint, preserving Node ordering without introducing another
    /// executor or allowing infrastructure scheduling to become observable.
    pub fn queue_next_tick(&mut self, work: M) {
        self.next_ticks.push_back(work);
    }

    /// Removes the oldest Node next-tick callback.
    pub fn pop_next_tick(&mut self) -> Option<M> {
        self.next_ticks.pop_front()
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
        !self.next_ticks.is_empty() || !self.microtasks.is_empty() || !self.active_timers.is_empty()
    }

    /// The earliest armed timer's virtual deadline, discarding cancelled entries.
    pub fn next_timer_deadline(&mut self) -> Option<Duration> {
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

    /// Advance virtual time to the earliest timer and take it, re-arming an interval.
    pub fn run_next_timer(&mut self) -> Option<WorkItem<M, T>>
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
        while let Some(work) = realm
            .next_ticks
            .pop_front()
            .or_else(|| realm.microtasks.pop_front())
        {
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

    fn next_host_completion<H: RealmHostProvider>(
        runtime: &mut RealmRuntime<H>,
    ) -> Option<Completion> {
        runtime
            .next_host_completion()
            .expect("realm runtime remains healthy")
    }

    fn start_http_test_operation<H: RealmHostProvider>(
        runtime: &mut RealmRuntime<H>,
        resource: ResourceId,
        operation: &HttpHostOp,
        referenced: bool,
    ) -> RequestId {
        let request = runtime.start_host_op(operation.clone().into_host_op(), referenced);
        assert!(runtime.http_connection_started(resource, operation, request));
        request
    }

    #[test]
    fn terminal_request_events_are_deduplicated() {
        let mut completed = HashSet::new();
        assert!(matches!(
            map_http1_client_event(
                hyper_http::ClientEvent::ResponseEnd {
                    id: 7,
                    reusable: true,
                },
                &mut completed,
            ),
            Some(RuntimeHttpClientEvent::ResponseEnd {
                id: 7,
                reusable: true,
            })
        ));
        assert!(
            map_http1_client_event(
                hyper_http::ClientEvent::ResponseData {
                    id: 7,
                    data: b"stale".to_vec(),
                },
                &mut completed,
            )
            .is_none()
        );
    }

    fn request_events(events: &[RuntimeHttpEvent]) -> (u64, &node_http::HttpRequest, Vec<u8>) {
        let [
            RuntimeHttpEvent::RequestHead { id, request },
            middle @ ..,
            RuntimeHttpEvent::RequestEnd { id: end_id },
        ] = events
        else {
            panic!("expected request head/data/end events: {events:?}")
        };
        assert_eq!(id, end_id);
        let mut body = Vec::new();
        for event in middle {
            let RuntimeHttpEvent::RequestData { id: data_id, data } = event else {
                panic!("unexpected event between request head and end: {event:?}")
            };
            assert_eq!(id, data_id);
            body.extend_from_slice(data);
        }
        (*id, request, body)
    }
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
    fn active_realm_host_protocol_admits_persistent_resources() {
        fn submit_listener(host: &mut dyn ActiveAsyncHostOps) -> RequestId {
            host.start(
                AsyncHostOp::TcpListen {
                    host: "127.0.0.1".into(),
                    port: 0,
                },
                true,
            )
        }

        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        // This is the same trait object the interpreter calls from inside `RealmRuntime::enter`.
        // Unlike the old Node-only seam, the generic host protocol admits resource operations.
        let request = runtime
            .executor
            .local
            .block_on(&runtime.executor.runtime, async {
                submit_listener(&mut runtime.state.host)
            });
        let resource = match next_host_completion(&mut runtime)
            .expect("listen completes")
            .result
            .expect("listen succeeds")
        {
            AsyncHostValue::Resource(resource) => resource,
            other => panic!("expected listener resource, got {other:?}"),
        };
        assert_eq!(resource.kind, ResourceKind::TcpListener);
        assert!(!ActiveAsyncHostOps::cancel(
            &mut runtime.state.host,
            request
        ));

        runtime.start_host_op(
            AsyncHostOp::CloseResource {
                resource: resource.id,
            },
            false,
        );
        next_host_completion(&mut runtime).expect("close completes");
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
    fn each_realm_runtime_owns_an_isolated_http_agent() {
        let origin = HttpOrigin::http("example.com", 80);
        let mut first = RealmRuntime::new().expect("runtime builds");
        let mut second = RealmRuntime::new().expect("runtime builds");

        assert_eq!(
            ActiveHttpRuntime::acquire_client_connection(&mut first, origin.clone(), 1),
            AgentDecision::Connect
        );
        assert!(ActiveHttpRuntime::client_connection_connected(
            &mut first,
            origin.clone(),
            ResourceId::new(41),
            1,
        ));
        assert_eq!(
            ActiveHttpRuntime::release_client_connection(&mut first, ResourceId::new(41), true),
            AgentRelease::Idle
        );
        assert_eq!(
            ActiveHttpRuntime::acquire_client_connection(&mut first, origin.clone(), 2),
            AgentDecision::Reuse(ResourceId::new(41))
        );
        assert_eq!(
            ActiveHttpRuntime::acquire_client_connection(&mut second, origin, 1),
            AgentDecision::Connect
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
        assert!(runtime.has_pending_host_work());
        assert!(runtime.host().has_referenced_work());
        assert_eq!(
            next_host_completion(&mut runtime)
                .expect("write completes")
                .request,
            write
        );
        assert!(!runtime.host().has_referenced_work());

        let read = runtime.start_host_op(AsyncHostOp::ReadFile { path }, true);
        let completion = next_host_completion(&mut runtime).expect("read completes");
        assert_eq!(completion.request, read);
        assert_eq!(
            completion.result,
            Ok(AsyncHostValue::Bytes(vec![0, 1, 2, 255]))
        );
        assert!(!runtime.has_pending_host_work());
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
            next_host_completion(&mut runtime)
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
        let error = next_host_completion(&mut runtime)
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
        assert!(next_host_completion(&mut runtime).is_some());
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
        assert!(AsyncHostOps::cancel(runtime.host(), request));
        assert!(!AsyncHostOps::cancel(runtime.host(), request));
        assert!(!runtime.host().has_referenced_work());
        assert!(next_host_completion(&mut runtime).is_none());
    }

    #[test]
    fn supervised_task_panic_fails_the_realm_and_cancels_siblings() {
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        let failed = RequestId::new(90);
        let sibling = RequestId::new(91);
        let executor = Rc::clone(&runtime.executor);
        let (local, tokio_runtime, host) =
            (&executor.local, &executor.runtime, &mut runtime.state.host);
        local.block_on(tokio_runtime, async {
            host.spawn_task(failed, 90, true, async {
                panic!("supervised task panic");
            });
            host.spawn_task(sibling, 91, true, std::future::pending());
        });

        let failure = runtime
            .next_host_completion()
            .expect_err("task panic fails the realm");
        assert_eq!(failure.code, "ERR_KILN_RUNTIME_TASK_PANIC");
        assert!(failure.message.contains("supervised task panic"));
        assert!(!runtime.has_pending_host_work());
        assert!(!runtime.has_referenced_host_work());
        assert_eq!(
            runtime
                .next_host_completion()
                .expect_err("realm failure remains sticky"),
            failure
        );

        let mut realm: Realm = Realm::new();
        assert!(matches!(
            runtime.next_turn(&mut realm),
            Some(RuntimeTurn::Failure(observed)) if observed == failure
        ));
    }

    #[test]
    fn runtime_metrics_are_exposed_as_a_kiln_owned_snapshot() {
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        let first = runtime.sample_runtime_metrics();
        assert_eq!(first.workers, 1);
        assert_eq!(first.global_queue_depth, 0);

        runtime.enter(|| {});
        let second = runtime.sample_runtime_metrics();
        assert_eq!(second.workers, 1);
        assert!(second.busy_duration <= second.elapsed);
    }

    #[test]
    fn stale_completion_queued_after_cancellation_is_rejected() {
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        let request = runtime.start_host_op(
            AsyncHostOp::ReadFile {
                path: "canceled.txt".into(),
            },
            false,
        );
        assert!(runtime.cancel_host_op(request));
        runtime.host_mut().promote_completion(HostCompletion {
            request,
            sequence: 0,
            lifecycle: None,
            result: Ok(HostCompletionValue::Public(AsyncHostValue::Bytes(
                b"stale".to_vec(),
            ))),
        });

        assert!(AsyncHostOps::next_completion(runtime.host_mut()).is_none());
    }

    #[test]
    fn stale_completion_does_not_end_a_later_turn() {
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        let stale = runtime.start_host_op(
            AsyncHostOp::ReadFile {
                path: "canceled.txt".into(),
            },
            false,
        );
        assert!(runtime.cancel_host_op(stale));
        runtime.start_host_op(
            AsyncHostOp::TcpListen {
                host: "127.0.0.1".into(),
                port: 0,
            },
            false,
        );
        let listener = match next_host_completion(&mut runtime)
            .expect("listen completes")
            .result
            .expect("listen succeeds")
        {
            AsyncHostValue::Resource(resource) => resource,
            other => panic!("expected listener, got {other:?}"),
        };
        let live = runtime.start_host_op(
            AsyncHostOp::TcpAccept {
                listener: listener.id,
            },
            false,
        );
        runtime.host_mut().promote_completion(HostCompletion {
            request: stale,
            sequence: 0,
            lifecycle: None,
            result: Ok(HostCompletionValue::Public(AsyncHostValue::Bytes(
                b"stale".to_vec(),
            ))),
        });
        let mut realm: Realm = Realm::new();
        realm.set_timeout(Duration::ZERO, WorkId::new(93));
        let turn = runtime
            .next_turn(&mut realm)
            .expect("stale completion must not end the drive");
        match turn {
            RuntimeTurn::Timer(WorkItem::Timer { work, .. }) => {
                assert_eq!(work, WorkId::new(93));
                assert!(runtime.cancel_host_op(live));
            }
            RuntimeTurn::Timer(WorkItem::Microtask(_)) => {
                panic!("next_turn must not select a microtask")
            }
            RuntimeTurn::Completion(_) => panic!("accept must remain pending without a client"),
            RuntimeTurn::Failure(failure) => panic!("realm failed unexpectedly: {failure}"),
        }
        runtime.start_host_op(
            AsyncHostOp::CloseResource {
                resource: listener.id,
            },
            false,
        );
        next_host_completion(&mut runtime).expect("close completes");
    }

    #[test]
    fn persistent_resources_own_liveness_until_unref_or_close() {
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        runtime.start_host_op(
            AsyncHostOp::TcpListen {
                host: "127.0.0.1".into(),
                port: 0,
            },
            true,
        );
        let resource = match next_host_completion(&mut runtime)
            .expect("listen completes")
            .result
            .expect("listen succeeds")
        {
            AsyncHostValue::Resource(resource) => resource,
            other => panic!("expected listener, got {other:?}"),
        };
        assert_eq!(resource.kind, ResourceKind::TcpListener);
        assert!(runtime.host().has_referenced_work());

        runtime.start_host_op(
            AsyncHostOp::SetResourceReferenced {
                resource: resource.id,
                referenced: false,
            },
            true,
        );
        assert_eq!(
            next_host_completion(&mut runtime)
                .expect("unref completes")
                .result,
            Ok(AsyncHostValue::Undefined)
        );
        assert!(!runtime.host().has_referenced_work());

        runtime.start_host_op(
            AsyncHostOp::CloseResource {
                resource: resource.id,
            },
            true,
        );
        assert_eq!(
            next_host_completion(&mut runtime)
                .expect("close completes")
                .result,
            Ok(AsyncHostValue::Undefined)
        );
    }

    #[test]
    fn closed_resource_id_fails_instead_of_aliasing_a_new_resource() {
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        runtime.start_host_op(
            AsyncHostOp::TcpListen {
                host: "127.0.0.1".into(),
                port: 0,
            },
            false,
        );
        let resource = match next_host_completion(&mut runtime)
            .expect("listen completes")
            .result
            .expect("listen succeeds")
        {
            AsyncHostValue::Resource(resource) => resource,
            other => panic!("expected listener, got {other:?}"),
        };
        runtime.start_host_op(
            AsyncHostOp::CloseResource {
                resource: resource.id,
            },
            true,
        );
        next_host_completion(&mut runtime).expect("first close completes");
        runtime.start_host_op(
            AsyncHostOp::CloseResource {
                resource: resource.id,
            },
            true,
        );
        let error = next_host_completion(&mut runtime)
            .expect("second close completes")
            .result
            .expect_err("closed resource is rejected");
        assert_eq!(error.code, "EBADF");
        assert_eq!(error.syscall, "close");
    }

    #[test]
    fn scripted_host_validates_requests_and_releases_in_submission_order() {
        let first_op = AsyncHostOp::ReadFile {
            path: "fixture/a.txt".into(),
        };
        let second_op = AsyncHostOp::WriteFile {
            path: "fixture/b.txt".into(),
            data: b"written".to_vec(),
        };
        let mut host = ScriptedHost::new([
            ScriptedHostStep {
                operation: first_op.clone(),
                result: Ok(AsyncHostValue::Bytes(b"first".to_vec())),
            },
            ScriptedHostStep {
                operation: second_op.clone(),
                result: Ok(AsyncHostValue::Undefined),
            },
        ]);

        let first = host.start(first_op, true);
        let second = host.start(second_op, false);
        assert!(host.has_referenced_work());
        assert_eq!(host.remaining_steps(), 0);
        assert_eq!(
            host.next_completion().expect("first checkpoint").request,
            first
        );
        assert!(!host.has_referenced_work());
        assert_eq!(
            host.next_completion().expect("second checkpoint").request,
            second
        );
        assert!(host.is_exhausted());
    }

    #[test]
    fn scripted_host_fails_closed_and_models_cancellation() {
        let mut host = ScriptedHost::new([ScriptedHostStep {
            operation: AsyncHostOp::ReadFile {
                path: "expected.txt".into(),
            },
            result: Ok(AsyncHostValue::Bytes(Vec::new())),
        }]);
        let mismatch = host.start(
            AsyncHostOp::ReadFile {
                path: "ambient.txt".into(),
            },
            true,
        );
        let error = host
            .next_completion()
            .expect("mismatch is released")
            .result
            .expect_err("unexpected machine access is rejected");
        assert_eq!(error.code, "ERR_KILN_UNEXPECTED_HOST_OP");
        assert!(error.message.contains("expected.txt"));
        assert!(error.message.contains("ambient.txt"));

        let unexpected = host.start(AsyncHostOp::Stat { path: "x".into() }, true);
        assert!(host.cancel(unexpected));
        assert!(!host.cancel(unexpected));
        assert!(host.next_completion().is_none());
        assert_ne!(mismatch, unexpected);
    }

    #[test]
    fn scripted_runtime_drives_the_stream_driver_through_fragmented_reads_and_queued_writes() {
        use crate::node_stream::{StreamAction, StreamDriver, StreamEvent};

        let resource = ResourceId::new(41);
        let connect = AsyncHostOp::TcpConnect {
            host: "127.0.0.1".into(),
            port: 8080,
        };
        let first_read = AsyncHostOp::TcpRead {
            stream: resource,
            max_bytes: 4,
        };
        let second_read = first_read.clone();
        let third_read = first_read.clone();
        let first_write = AsyncHostOp::TcpWrite {
            stream: resource,
            data: b"abcd".to_vec(),
        };
        let second_write = AsyncHostOp::TcpWrite {
            stream: resource,
            data: b"ef".to_vec(),
        };
        let mut runtime = RealmRuntime::with_host(ScriptedHost::new([
            ScriptedHostStep {
                operation: connect.clone(),
                result: Ok(AsyncHostValue::Resource(ResourceInfo {
                    id: resource,
                    kind: ResourceKind::TcpStream,
                    address: Some("127.0.0.1".into()),
                    port: Some(8080),
                })),
            },
            ScriptedHostStep {
                operation: first_read.clone(),
                result: Ok(AsyncHostValue::Bytes(b"ab".to_vec())),
            },
            ScriptedHostStep {
                operation: second_read,
                result: Ok(AsyncHostValue::Bytes(b"cd".to_vec())),
            },
            ScriptedHostStep {
                operation: third_read,
                result: Ok(AsyncHostValue::Bytes(Vec::new())),
            },
            ScriptedHostStep {
                operation: first_write.clone(),
                result: Ok(AsyncHostValue::Undefined),
            },
            ScriptedHostStep {
                operation: second_write.clone(),
                result: Ok(AsyncHostValue::Undefined),
            },
        ]))
        .expect("scripted runtime builds");
        let connect_request = runtime.start_host_op(connect, true);
        assert_eq!(
            next_host_completion(&mut runtime)
                .expect("connect completes")
                .request,
            connect_request
        );
        let mut driver = StreamDriver::new(resource, 4);

        let actions = driver.start_reading();
        let StreamAction::Start(operation) = &actions[0] else {
            panic!("expected first scripted read, got {actions:?}");
        };
        assert_eq!(operation, &first_read);
        let request = runtime.start_host_op(operation.clone(), false);
        assert!(driver.started(operation, request));

        let actions = driver
            .completed(next_host_completion(&mut runtime).expect("first read completes"))
            .expect("first read belongs to stream");
        assert_eq!(actions[0], StreamAction::Data(b"ab".to_vec()));
        let StreamAction::Start(operation) = &actions[1] else {
            panic!("expected second scripted read, got {actions:?}");
        };
        let request = runtime.start_host_op(operation.clone(), false);
        assert!(driver.started(operation, request));

        let actions = driver
            .completed(next_host_completion(&mut runtime).expect("second read completes"))
            .expect("second read belongs to stream");
        assert_eq!(actions[0], StreamAction::Data(b"cd".to_vec()));
        let StreamAction::Start(operation) = &actions[1] else {
            panic!("expected EOF read, got {actions:?}");
        };
        let request = runtime.start_host_op(operation.clone(), false);
        assert!(driver.started(operation, request));
        assert_eq!(
            driver.completed(next_host_completion(&mut runtime).expect("EOF completes")),
            Some(vec![StreamAction::Event(StreamEvent::End)])
        );

        let (first, actions) = driver.write(b"abcd".to_vec());
        assert!(!first.ready);
        let StreamAction::Start(operation) = &actions[0] else {
            panic!("expected first scripted write, got {actions:?}");
        };
        assert_eq!(operation, &first_write);
        let request = runtime.start_host_op(operation.clone(), false);
        assert!(driver.started(operation, request));
        let (second, actions) = driver.write(b"ef".to_vec());
        assert!(!second.ready);
        assert!(actions.is_empty());

        let actions = driver
            .completed(next_host_completion(&mut runtime).expect("first write completes"))
            .expect("first write belongs to stream");
        assert_eq!(actions[0], StreamAction::Event(StreamEvent::Drain));
        let StreamAction::Start(operation) = &actions[1] else {
            panic!("expected queued scripted write, got {actions:?}");
        };
        assert_eq!(operation, &second_write);
        let request = runtime.start_host_op(operation.clone(), false);
        assert!(driver.started(operation, request));
        assert!(
            driver
                .completed(next_host_completion(&mut runtime).expect("second write completes"))
                .expect("second write belongs to stream")
                .is_empty()
        );
        assert!(runtime.host_mut().is_exhausted());
    }

    #[test]
    fn scripted_runtime_cancellation_retires_stream_read_before_close() {
        use crate::node_stream::{StreamAction, StreamDriver, StreamEvent};

        let resource = ResourceId::new(42);
        let read = AsyncHostOp::TcpRead {
            stream: resource,
            max_bytes: 8,
        };
        let close = AsyncHostOp::CloseResource { resource };
        let mut runtime = RealmRuntime::with_host(ScriptedHost::new([
            ScriptedHostStep {
                operation: read.clone(),
                result: Ok(AsyncHostValue::Bytes(b"late".to_vec())),
            },
            ScriptedHostStep {
                operation: close.clone(),
                result: Ok(AsyncHostValue::Undefined),
            },
        ]))
        .expect("scripted runtime builds");
        let mut driver = StreamDriver::new(resource, 8);
        let actions = driver.start_reading();
        let StreamAction::Start(operation) = &actions[0] else {
            panic!("expected scripted read, got {actions:?}");
        };
        let request = runtime.start_host_op(operation.clone(), false);
        assert!(driver.started(operation, request));

        let actions = driver.close_actions();
        assert_eq!(actions[0], StreamAction::Cancel(request));
        let StreamAction::Start(close_operation) = &actions[1] else {
            panic!("expected close action, got {actions:?}");
        };
        assert_eq!(close_operation, &close);
        assert!(runtime.cancel_host_op(request));
        let close_request = runtime.start_host_op(close_operation.clone(), false);
        assert!(driver.started(close_operation, close_request));
        assert_eq!(
            driver.completed(next_host_completion(&mut runtime).expect("close completes")),
            Some(vec![StreamAction::Event(StreamEvent::Close)])
        );
        assert!(runtime.host_mut().is_exhausted());
    }

    #[test]
    fn scripted_runtime_keeps_equal_checkpoint_completions_in_submission_order() {
        let first_op = AsyncHostOp::ReadFile {
            path: "scripted/first".into(),
        };
        let second_op = AsyncHostOp::ReadFile {
            path: "scripted/second".into(),
        };
        let mut runtime = RealmRuntime::with_host(ScriptedHost::new([
            ScriptedHostStep {
                operation: first_op.clone(),
                result: Ok(AsyncHostValue::Bytes(b"first".to_vec())),
            },
            ScriptedHostStep {
                operation: second_op.clone(),
                result: Ok(AsyncHostValue::Bytes(b"second".to_vec())),
            },
        ]))
        .expect("scripted runtime builds");
        let mut realm = Realm::<(), ()>::new();
        realm.set_timeout(Duration::ZERO, ());
        let first = runtime.start_host_op(first_op, false);
        let second = runtime.start_host_op(second_op, false);
        let first_completion = match runtime.next_turn(&mut realm).expect("first completion") {
            RuntimeTurn::Completion(completion) => completion,
            RuntimeTurn::Timer(_) => panic!("a ready completion must beat a due timer"),
            RuntimeTurn::Failure(failure) => panic!("realm failed unexpectedly: {failure}"),
        };
        let second_completion = match runtime.next_turn(&mut realm).expect("second completion") {
            RuntimeTurn::Completion(completion) => completion,
            RuntimeTurn::Timer(_) => panic!("a ready completion must beat a due timer"),
            RuntimeTurn::Failure(failure) => panic!("realm failed unexpectedly: {failure}"),
        };
        assert_eq!(first_completion.request, first);
        assert_eq!(second_completion.request, second);
        assert!(first_completion.sequence < second_completion.sequence);
        assert!(runtime.host_mut().is_exhausted());
    }

    #[test]
    fn scripted_provider_drives_hyper_connection_through_the_same_stream_bridge() {
        use crate::node_http::HttpResponse;
        use crate::node_stream::DEFAULT_HIGH_WATER_MARK;

        let resource = ResourceId::new(43);
        let connect = AsyncHostOp::TcpConnect {
            host: "127.0.0.1".into(),
            port: 8080,
        };
        let connected = ResourceInfo {
            id: resource,
            kind: ResourceKind::TcpStream,
            address: Some("127.0.0.1".into()),
            port: Some(8080),
        };
        let read = AsyncHostOp::TcpRead {
            stream: resource,
            max_bytes: DEFAULT_HIGH_WATER_MARK,
        };
        let request = b"GET /health HTTP/1.1\r\nhost: kiln\r\nconnection: close\r\n\r\n";
        let write = AsyncHostOp::TcpWrite {
            stream: resource,
            data: b"HTTP/1.1 200 OK\r\nconnection: close\r\ncontent-length: 4\r\n\r\nkiln".to_vec(),
        };
        let mut runtime = RealmRuntime::with_host(ScriptedHost::new([
            ScriptedHostStep {
                operation: connect.clone(),
                result: Ok(AsyncHostValue::Resource(connected.clone())),
            },
            ScriptedHostStep {
                operation: read.clone(),
                result: Ok(AsyncHostValue::Bytes(request.to_vec())),
            },
            ScriptedHostStep {
                operation: write.clone(),
                result: Ok(AsyncHostValue::Undefined),
            },
        ]))
        .expect("scripted runtime builds");
        let connect_request = runtime.start_host_op(connect, false);
        assert_eq!(
            next_host_completion(&mut runtime)
                .expect("connect completes")
                .result,
            Ok(AsyncHostValue::Resource(connected))
        );
        let start = runtime.start_http1_connection(resource);
        let [HttpStreamAction::Start(read_operation)] = start.as_slice() else {
            panic!("HTTP connection starts one scripted read: {start:?}");
        };
        assert_eq!(read_operation.clone().into_host_op(), read);
        let read_request = start_http_test_operation(&mut runtime, resource, read_operation, false);
        let request_completion =
            next_host_completion(&mut runtime).expect("request read completes");
        let actions = runtime
            .complete_http_connection(resource, request_completion)
            .expect("request read belongs to HTTP connection");
        let [HttpStreamAction::Start(next_read)] = actions.as_slice() else {
            panic!("HTTP bridge starts one follow-up read: {actions:?}");
        };
        assert_eq!(next_read.clone().into_host_op(), read);

        let events = (0..8)
            .find_map(|_| {
                runtime.poll_http_connection(resource);
                let events = runtime.take_http_events(resource);
                (!events.is_empty()).then_some(events)
            })
            .unwrap_or_else(|| panic!("Hyper did not publish a request event"));
        let (id, request, body) = request_events(&events);
        assert_eq!(request.target, "/health");
        assert!(body.is_empty());
        assert!(runtime.respond_http1(
            resource,
            id,
            HttpResponse {
                status: 200,
                reason: Some("OK".into()),
                headers: vec![("connection".into(), "close".into())],
                body: b"kiln".to_vec(),
            }
        ));
        let writes = (0..8)
            .find_map(|_| {
                runtime
                    .poll_http_connection(resource)
                    .filter(|actions| !actions.is_empty())
            })
            .unwrap_or_else(|| panic!("Hyper did not select a response write"));
        let [HttpStreamAction::Start(write_operation)] = writes.as_slice() else {
            panic!("Hyper selects one scripted response write: {writes:?}");
        };
        assert_eq!(write_operation.clone().into_host_op(), write);
        let _write_request =
            start_http_test_operation(&mut runtime, resource, write_operation, false);
        let write_completion =
            next_host_completion(&mut runtime).expect("response write completes");
        let mut write_actions = runtime
            .complete_http_connection(resource, write_completion)
            .expect("response write belongs to HTTP connection");
        for _ in 0..8 {
            if runtime
                .take_http_events(resource)
                .contains(&RuntimeHttpEvent::Closed)
            {
                write_actions.extend(
                    runtime
                        .close_http_connection(resource, HttpCloseMode::Graceful)
                        .unwrap_or_default(),
                );
            }
            write_actions.extend(runtime.poll_http_connection(resource).unwrap_or_default());
            if write_actions
                .iter()
                .any(|action| matches!(action, HttpStreamAction::Close(_)))
            {
                break;
            }
        }
        assert!(write_actions.iter().any(|action| matches!(
            action,
            HttpStreamAction::Close(closed) if closed.resource() == resource
        )));
        assert!(
            !write_actions.iter().any(|action| matches!(
                action,
                HttpStreamAction::Start(HttpHostOp::TcpWrite { .. })
            ))
        );
        assert_ne!(connect_request, read_request);
        assert!(runtime.host_mut().is_exhausted());
    }

    #[test]
    fn scripted_provider_drains_response_before_same_turn_server_close() {
        let resource = ResourceId::new(45);
        let read = AsyncHostOp::TcpRead {
            stream: resource,
            max_bytes: node_stream::DEFAULT_HIGH_WATER_MARK,
        };
        let request = b"GET /same-turn HTTP/1.1\r\nHost: kiln\r\nConnection: close\r\n\r\n";
        let write = AsyncHostOp::TcpWrite {
            stream: resource,
            data: b"HTTP/1.1 200 OK\r\ncontent-length: 9\r\nconnection: close\r\n\r\nsame-turn"
                .to_vec(),
        };
        let close = AsyncHostOp::CloseResource { resource };
        let mut runtime = RealmRuntime::with_host(ScriptedHost::new([
            ScriptedHostStep {
                operation: read.clone(),
                result: Ok(AsyncHostValue::Bytes(request.to_vec())),
            },
            ScriptedHostStep {
                operation: write.clone(),
                result: Ok(AsyncHostValue::Undefined),
            },
            ScriptedHostStep {
                operation: close.clone(),
                result: Ok(AsyncHostValue::Undefined),
            },
        ]))
        .expect("scripted runtime builds");
        let start = runtime.start_http1_connection(resource);
        let [HttpStreamAction::Start(read_operation)] = start.as_slice() else {
            panic!("HTTP connection starts one scripted read: {start:?}");
        };
        assert_eq!(read_operation.clone().into_host_op(), read);
        let _read_request =
            start_http_test_operation(&mut runtime, resource, read_operation, false);
        let read_completion =
            next_host_completion(&mut runtime).expect("scripted request read completes");
        let actions = runtime
            .complete_http_connection(resource, read_completion)
            .expect("request read belongs to connection");
        assert!(actions.iter().any(|action| matches!(
            action,
            HttpStreamAction::Start(HttpHostOp::TcpRead { stream, .. })
                if stream.resource() == resource
        )));
        runtime.poll_http_connection(resource);
        let events = runtime.take_http_events(resource);
        let (id, _, body) = request_events(&events);
        assert!(body.is_empty());
        assert!(runtime.respond_http1(
            resource,
            id,
            node_http::HttpResponse {
                status: 200,
                reason: None,
                headers: Vec::new(),
                body: b"same-turn".to_vec(),
            },
        ));

        // This is the native callback's ordering: response.end() queues the Hyper command, then
        // server.close() runs before Tokio gets another poll of the connection task.
        let close_actions = runtime
            .close_http_connection(resource, HttpCloseMode::Graceful)
            .expect("server close belongs to HTTP connection");
        assert!(
            close_actions.is_empty(),
            "close must wait for the response body"
        );

        let writes = runtime
            .poll_http_connection(resource)
            .expect("connection remains while response drains");
        let [HttpStreamAction::Start(HttpHostOp::TcpWrite { stream, data })] = writes.as_slice()
        else {
            panic!("response write must precede close: {writes:?}");
        };
        assert_eq!(stream.resource(), resource);
        assert_eq!(
            &AsyncHostOp::TcpWrite {
                stream: resource,
                data: data.clone(),
            },
            &write
        );
        let write_operation = HttpHostOp::TcpWrite {
            stream: HttpResourceId::new(resource),
            data: data.clone(),
        };
        let _write_request =
            start_http_test_operation(&mut runtime, resource, &write_operation, false);
        let write_completion =
            next_host_completion(&mut runtime).expect("scripted response write completes");
        let completion = runtime
            .complete_http_connection(resource, write_completion)
            .expect("response write belongs to connection");
        assert!(
            completion.is_empty()
                || completion.iter().all(|action| matches!(
                    action,
                    HttpStreamAction::Start(HttpHostOp::TcpWrite { .. })
                        | HttpStreamAction::Event(node_stream::StreamEvent::Close)
                ))
        );

        let close_action = (0..8)
            .find_map(|_| {
                let _ = runtime.take_http_events(resource);
                runtime.poll_http_connection(resource).and_then(|actions| {
                    actions.into_iter().find(|action| {
                        matches!(
                            action,
                            HttpStreamAction::Close(closed)
                                if closed.resource() == resource
                        )
                    })
                })
            })
            .unwrap_or_else(|| panic!("response write must be acknowledged before close"));
        let HttpStreamAction::Close(close_action) = close_action else {
            panic!("response write must select an authorized close");
        };
        assert_eq!(close_action.resource(), resource);
        let close_request = runtime
            .start_http_connection_close(close_action)
            .expect("authorized HTTP close starts");
        let close_completion =
            next_host_completion(&mut runtime).expect("close follows the response write");
        assert_eq!(close_completion.request, close_request);
        let retired = runtime
            .complete_http_connection(resource, close_completion)
            .expect("the close completion belongs to the connection");
        assert!(retired.iter().any(|action| matches!(
            action,
            HttpStreamAction::Event(node_stream::StreamEvent::Close)
        )));
        assert!(runtime.retire_http_connection(resource));
        assert!(
            runtime.poll_http_connection(resource).is_none(),
            "a retired connection is gone from the table"
        );
        assert!(!runtime.retire_http_connection(resource));
        assert!(runtime.host_mut().is_exhausted());
    }

    #[test]
    fn scripted_runtime_aborts_http_connection_without_waiting_for_response_write() {
        let resource = ResourceId::new(46);
        let read = AsyncHostOp::TcpRead {
            stream: resource,
            max_bytes: node_stream::DEFAULT_HIGH_WATER_MARK,
        };
        let close = AsyncHostOp::CloseResource { resource };
        let mut runtime = RealmRuntime::with_host(ScriptedHost::new([
            ScriptedHostStep {
                operation: read.clone(),
                result: Ok(AsyncHostValue::Bytes(b"late".to_vec())),
            },
            ScriptedHostStep {
                operation: close.clone(),
                result: Ok(AsyncHostValue::Undefined),
            },
        ]))
        .expect("scripted runtime builds");

        let start = runtime.start_http1_connection(resource);
        let [HttpStreamAction::Start(read_operation)] = start.as_slice() else {
            panic!("HTTP connection starts one scripted read");
        };
        let read_request = start_http_test_operation(&mut runtime, resource, read_operation, true);

        let actions = runtime
            .close_http_connection(resource, HttpCloseMode::Abort)
            .expect("HTTP connection is registered");
        assert_eq!(actions.len(), 2);
        assert!(matches!(
            actions.first(),
            Some(HttpStreamAction::Cancel(request)) if *request == read_request
        ));
        assert!(runtime.cancel_host_op(read_request));
        let Some(HttpStreamAction::Close(close_action)) = actions.into_iter().nth(1) else {
            panic!("abort must select a portable close operation");
        };
        assert_eq!(close_action.resource(), resource);
        let close_request = runtime
            .start_http_connection_close(close_action)
            .expect("authorized HTTP close starts");
        assert_eq!(
            next_host_completion(&mut runtime)
                .expect("aborted close completes")
                .request,
            close_request
        );
        assert!(runtime.host_mut().is_exhausted());
    }

    #[test]
    fn realm_shutdown_aborts_http_transport_and_rejects_new_admission() {
        let resource = ResourceId::new(47);
        let read = AsyncHostOp::TcpRead {
            stream: resource,
            max_bytes: node_stream::DEFAULT_HIGH_WATER_MARK,
        };
        let mut runtime = RealmRuntime::with_host(ScriptedHost::new([ScriptedHostStep {
            operation: read,
            result: Ok(AsyncHostValue::Bytes(b"late".to_vec())),
        }]))
        .expect("scripted runtime builds");

        let start = runtime.start_http1_connection(resource);
        let [HttpStreamAction::Start(read_operation)] = start.as_slice() else {
            panic!("HTTP connection starts one scripted read");
        };
        start_http_test_operation(&mut runtime, resource, read_operation, true);
        assert!(runtime.has_pending_host_work());
        assert_eq!(runtime.http_connections.len(), 1);
        let duplicate = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            runtime.start_http1_client_connection(resource)
        }));
        assert!(
            duplicate.is_err(),
            "duplicate resource ownership is rejected"
        );
        assert_eq!(runtime.http_connections.len(), 1);
        assert_eq!(runtime.host_mut().pending.len(), 1);

        assert!(runtime.shutdown());
        assert!(runtime.is_shutdown());
        assert!(!runtime.shutdown(), "terminal teardown is idempotent");
        assert!(!runtime.has_pending_host_work());
        assert!(!runtime.has_referenced_host_work());
        assert!(runtime.http_connections.is_empty());
        assert!(runtime.host_mut().is_exhausted());
        assert_eq!(
            runtime
                .next_host_completion()
                .expect("shutdown provider remains healthy"),
            None
        );
        assert_eq!(
            runtime.try_start_host_op(
                AsyncHostOp::ReadFile {
                    path: "must-not-start".into(),
                },
                true,
            ),
            None
        );
        let origin = HttpOrigin::http("closed.example", 80);
        assert!(!runtime.client_connection_connected(origin.clone(), resource, 1));
        assert_eq!(
            runtime.release_client_connection(resource, true),
            AgentRelease::Retire
        );
        assert_eq!(
            runtime.client_connection_failed(&origin),
            AgentFailure::Empty
        );
        assert_eq!(runtime.client_connection_closed(resource), None);
    }

    #[test]
    fn dropping_realm_runtime_uses_the_idempotent_shutdown_protocol() {
        let observer = Rc::new(std::cell::Cell::new(0));
        let host = ScriptedHost {
            shutdown_observer: Some(Rc::clone(&observer)),
            ..ScriptedHost::default()
        };
        {
            let mut runtime = RealmRuntime::with_host(host).expect("scripted runtime builds");
            assert!(runtime.shutdown());
            assert!(!runtime.shutdown());
        }
        assert_eq!(observer.get(), 1, "Drop must reuse, not repeat, shutdown");

        let host = ScriptedHost {
            shutdown_observer: Some(Rc::clone(&observer)),
            ..ScriptedHost::default()
        };
        drop(RealmRuntime::with_host(host).expect("scripted runtime builds"));
        assert_eq!(observer.get(), 2, "Drop must initiate shutdown when needed");
    }

    /// Drive one scripted HTTP/1 request through the runtime and return the bytes the response
    /// selected for the transport. The head is what these tests are about, so the write is read
    /// off the action rather than started against the script.
    fn response_head_for(
        request: &[u8],
        status: u16,
        headers: Vec<(String, String)>,
        chunks: &[&[u8]],
        body: &[u8],
    ) -> String {
        use crate::node_stream::DEFAULT_HIGH_WATER_MARK;

        let resource = ResourceId::new(77);
        let read = AsyncHostOp::TcpRead {
            stream: resource,
            max_bytes: DEFAULT_HIGH_WATER_MARK,
        };
        let mut runtime = RealmRuntime::with_host(ScriptedHost::new([ScriptedHostStep {
            operation: read.clone(),
            result: Ok(AsyncHostValue::Bytes(request.to_vec())),
        }]))
        .expect("scripted runtime builds");
        let start = runtime.start_http1_connection(resource);
        let [HttpStreamAction::Start(read_operation)] = start.as_slice() else {
            panic!("HTTP connection starts one read")
        };
        let _read_request =
            start_http_test_operation(&mut runtime, resource, read_operation, false);
        let completion = next_host_completion(&mut runtime).expect("request read completes");
        runtime
            .complete_http_connection(resource, completion)
            .expect("read belongs to connection");
        let events = (0..8)
            .find_map(|_| {
                runtime.poll_http_connection(resource);
                let events = runtime.take_http_events(resource);
                (!events.is_empty()).then_some(events)
            })
            .expect("request events");
        let (id, _, _) = request_events(&events);

        assert!(runtime.start_http1_response(
            resource,
            id,
            status,
            headers,
            DEFAULT_HIGH_WATER_MARK,
        ));
        for chunk in chunks {
            assert!(
                runtime
                    .write_http1_response(resource, id, chunk.to_vec())
                    .is_some()
            );
        }
        assert!(
            runtime
                .end_http1_response(resource, id, body.to_vec())
                .is_some()
        );
        let actions = (0..8)
            .find_map(|_| {
                runtime
                    .poll_http_connection(resource)
                    .filter(|actions| !actions.is_empty())
            })
            .expect("the response selects a write");
        let [HttpStreamAction::Start(write)] = actions.as_slice() else {
            panic!("a response selects one write: {actions:?}")
        };
        let AsyncHostOp::TcpWrite { data, .. } = write.clone().into_host_op() else {
            panic!("a response writes bytes")
        };
        String::from_utf8(data).expect("an HTTP head is ASCII")
    }

    const GET: &[u8] = b"GET /framing HTTP/1.1\r\nHost: kiln\r\n\r\n";
    const HEAD: &[u8] = b"HEAD /framing HTTP/1.1\r\nHost: kiln\r\n\r\n";

    #[test]
    fn end_with_the_whole_body_declares_its_length_in_bytes() {
        // Case 1: nothing flushed, no prior write, no framing header the script chose.
        assert_eq!(
            response_head_for(GET, 200, Vec::new(), &[], b"kiln"),
            "HTTP/1.1 200 OK\r\ncontent-length: 4\r\n\r\nkiln"
        );
        // A length is bytes, not characters: five characters, six UTF-8 bytes.
        assert_eq!(
            response_head_for(GET, 200, Vec::new(), &[], "h\u{e9}llo".as_bytes()),
            "HTTP/1.1 200 OK\r\ncontent-length: 6\r\n\r\nh\u{e9}llo"
        );
    }

    #[test]
    fn end_with_no_body_declares_zero() {
        // Case 2.
        assert_eq!(
            response_head_for(GET, 200, Vec::new(), &[], b""),
            "HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n"
        );
    }

    #[test]
    fn a_framing_the_script_chose_is_left_alone() {
        // Case 3, both directions. A declared length is not recomputed, and a script that asked
        // for chunking still gets it even though the whole body arrived with `end`.
        assert_eq!(
            response_head_for(
                GET,
                200,
                vec![("content-length".into(), "4".into())],
                &[],
                b"kiln",
            ),
            "HTTP/1.1 200 OK\r\ncontent-length: 4\r\n\r\nkiln"
        );
        assert_eq!(
            response_head_for(
                GET,
                200,
                vec![("transfer-encoding".into(), "chunked".into())],
                &[],
                b"kiln",
            ),
            "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n4\r\nkiln\r\n0\r\n\r\n"
        );
        // The header name is matched without regard to case, as HTTP requires.
        assert_eq!(
            response_head_for(
                GET,
                200,
                vec![("Content-Length".into(), "4".into())],
                &[],
                b"kiln",
            ),
            "HTTP/1.1 200 OK\r\ncontent-length: 4\r\n\r\nkiln"
        );
    }

    #[test]
    fn a_chunk_before_end_keeps_the_body_chunked() {
        // Case 4: the length is not known when the head has to go out.
        assert_eq!(
            response_head_for(GET, 200, Vec::new(), &[b"ki"], b"ln"),
            "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n2\r\nki\r\n2\r\nln\r\n0\r\n\r\n"
        );
    }

    #[test]
    fn a_response_that_cannot_carry_a_body_declares_no_length() {
        // Case 5. A HEAD response and a bodiless status get neither header, which is what node
        // sends; hyper forwards a content-length here rather than dropping it, so the rule has to
        // decline to add one.
        assert_eq!(
            response_head_for(HEAD, 200, Vec::new(), &[], b"kiln"),
            "HTTP/1.1 200 OK\r\n\r\n"
        );
        assert_eq!(
            response_head_for(GET, 204, Vec::new(), &[], b""),
            "HTTP/1.1 204 No Content\r\n\r\n"
        );
        assert_eq!(
            response_head_for(GET, 304, Vec::new(), &[], b""),
            "HTTP/1.1 304 Not Modified\r\n\r\n"
        );
    }

    #[test]
    fn scripted_provider_dispatches_buffered_http1_pipeline_after_first_response() {
        use crate::node_stream::DEFAULT_HIGH_WATER_MARK;

        let resource = ResourceId::new(44);
        let read = AsyncHostOp::TcpRead {
            stream: resource,
            max_bytes: DEFAULT_HIGH_WATER_MARK,
        };
        let requests =
            b"GET /first HTTP/1.1\r\nHost: kiln\r\n\r\nGET /second HTTP/1.1\r\nHost: kiln\r\n\r\n";
        let first_write = AsyncHostOp::TcpWrite {
            stream: resource,
            // `end` carries the whole body, so the head declares a length instead of chunking.
            data: b"HTTP/1.1 200 OK\r\ncontent-length: 6\r\n\r\n/first".to_vec(),
        };
        let mut runtime = RealmRuntime::with_host(ScriptedHost::new([
            ScriptedHostStep {
                operation: read.clone(),
                result: Ok(AsyncHostValue::Bytes(requests.to_vec())),
            },
            ScriptedHostStep {
                operation: first_write.clone(),
                result: Ok(AsyncHostValue::Undefined),
            },
        ]))
        .expect("scripted runtime builds");
        let start = runtime.start_http1_connection(resource);
        let [HttpStreamAction::Start(read_operation)] = start.as_slice() else {
            panic!("HTTP connection starts one read")
        };
        let _read_request =
            start_http_test_operation(&mut runtime, resource, read_operation, false);
        let completion = next_host_completion(&mut runtime).expect("pipeline read completes");
        runtime
            .complete_http_connection(resource, completion)
            .expect("read belongs to connection");

        let first_events = (0..8)
            .find_map(|_| {
                runtime.poll_http_connection(resource);
                let events = runtime.take_http_events(resource);
                (!events.is_empty()).then_some(events)
            })
            .expect("first request events");
        let (first_id, first, _) = request_events(&first_events);
        assert_eq!(first.target, "/first");
        assert!(runtime.start_http1_response(
            resource,
            first_id,
            200,
            Vec::new(),
            DEFAULT_HIGH_WATER_MARK,
        ));
        assert!(
            runtime
                .end_http1_response(resource, first_id, b"/first".to_vec())
                .is_some()
        );

        let write_actions = (0..8)
            .find_map(|_| {
                runtime
                    .poll_http_connection(resource)
                    .filter(|actions| !actions.is_empty())
            })
            .expect("first response write");
        let [HttpStreamAction::Start(write_operation)] = write_actions.as_slice() else {
            panic!("first response selects one write: {write_actions:?}")
        };
        assert_eq!(write_operation.clone().into_host_op(), first_write);
        let _write_request =
            start_http_test_operation(&mut runtime, resource, write_operation, false);
        let completion = next_host_completion(&mut runtime).expect("first write completes");
        runtime
            .complete_http_connection(resource, completion)
            .expect("write belongs to connection");

        let mut second_events = Vec::new();
        for _ in 0..16 {
            runtime.poll_http_connection(resource);
            second_events.extend(runtime.take_http_events(resource));
            if second_events.iter().any(|event| {
                matches!(
                    event,
                    RuntimeHttpEvent::RequestHead { request, .. } if request.target == "/second"
                )
            }) {
                break;
            }
        }
        assert!(
            second_events.iter().any(|event| matches!(
                event,
                RuntimeHttpEvent::RequestHead { request, .. } if request.target == "/second"
            )),
            "buffered pipeline events: {second_events:?}"
        );
    }

    #[test]
    fn tokio_tcp_resources_exchange_bytes_and_shutdown_cleanly() {
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        runtime.start_host_op(
            AsyncHostOp::TcpListen {
                host: "127.0.0.1".into(),
                port: 0,
            },
            true,
        );
        let listener = match next_host_completion(&mut runtime)
            .expect("listen completes")
            .result
            .expect("listen succeeds")
        {
            AsyncHostValue::Resource(resource) => resource,
            other => panic!("expected listener resource, got {other:?}"),
        };
        assert_eq!(listener.kind, ResourceKind::TcpListener);
        assert_eq!(listener.address.as_deref(), Some("127.0.0.1"));
        let port = listener.port.expect("ephemeral port assigned");
        assert_ne!(port, 0);

        let accept = runtime.start_host_op(
            AsyncHostOp::TcpAccept {
                listener: listener.id,
            },
            true,
        );
        let connect = runtime.start_host_op(
            AsyncHostOp::TcpConnect {
                host: "127.0.0.1".into(),
                port,
            },
            true,
        );
        let first = next_host_completion(&mut runtime).expect("connection completes");
        let second = next_host_completion(&mut runtime).expect("accept completes");
        let mut accepted = None;
        let mut client = None;
        for completion in [first, second] {
            if completion.request == accept {
                accepted = Some(match completion.result.expect("accept succeeds") {
                    AsyncHostValue::AcceptedTcpStream(accepted) => accepted,
                    other => panic!("expected accepted stream, got {other:?}"),
                });
            } else if completion.request == connect {
                client = Some(match completion.result.expect("connect succeeds") {
                    AsyncHostValue::Resource(resource) => resource,
                    other => panic!("expected connected stream, got {other:?}"),
                });
            } else {
                panic!("unexpected connection request");
            }
        }
        let accepted = accepted.expect("accept result observed");
        let client = client.expect("connect result observed");
        assert_eq!(accepted.resource.kind, ResourceKind::TcpStream);
        assert_eq!(client.kind, ResourceKind::TcpStream);
        assert_eq!(accepted.peer_address, "127.0.0.1");

        runtime.start_host_op(
            AsyncHostOp::TcpWrite {
                stream: client.id,
                data: b"hello kiln".to_vec(),
            },
            true,
        );
        assert_eq!(
            next_host_completion(&mut runtime)
                .expect("write completes")
                .result,
            Ok(AsyncHostValue::Undefined)
        );
        runtime.start_host_op(
            AsyncHostOp::TcpRead {
                stream: accepted.resource.id,
                max_bytes: 64,
            },
            true,
        );
        assert_eq!(
            next_host_completion(&mut runtime)
                .expect("read completes")
                .result,
            Ok(AsyncHostValue::Bytes(b"hello kiln".to_vec()))
        );

        runtime.start_host_op(AsyncHostOp::TcpShutdown { stream: client.id }, true);
        next_host_completion(&mut runtime).expect("shutdown completes");
        runtime.start_host_op(
            AsyncHostOp::TcpRead {
                stream: accepted.resource.id,
                max_bytes: 64,
            },
            true,
        );
        assert_eq!(
            next_host_completion(&mut runtime)
                .expect("eof completes")
                .result,
            Ok(AsyncHostValue::Bytes(Vec::new()))
        );

        for resource in [client.id, accepted.resource.id, listener.id] {
            runtime.start_host_op(AsyncHostOp::CloseResource { resource }, true);
            assert_eq!(
                next_host_completion(&mut runtime)
                    .expect("close completes")
                    .result,
                Ok(AsyncHostValue::Undefined)
            );
        }
        assert!(!runtime.host().has_referenced_work());
    }

    #[test]
    fn pending_socket_read_does_not_block_a_write_on_the_same_stream() {
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        runtime.start_host_op(
            AsyncHostOp::TcpListen {
                host: "127.0.0.1".into(),
                port: 0,
            },
            false,
        );
        let listener = match next_host_completion(&mut runtime)
            .expect("listen completes")
            .result
            .expect("listen succeeds")
        {
            AsyncHostValue::Resource(resource) => resource,
            other => panic!("expected listener, got {other:?}"),
        };
        let accept = runtime.start_host_op(
            AsyncHostOp::TcpAccept {
                listener: listener.id,
            },
            false,
        );
        let connect = runtime.start_host_op(
            AsyncHostOp::TcpConnect {
                host: "127.0.0.1".into(),
                port: listener.port.expect("ephemeral port assigned"),
            },
            false,
        );
        let mut server = None;
        let mut client = None;
        for _ in 0..2 {
            let completion = next_host_completion(&mut runtime).expect("connection completes");
            if completion.request == accept {
                server = Some(match completion.result.expect("accept succeeds") {
                    AsyncHostValue::AcceptedTcpStream(accepted) => accepted.resource,
                    other => panic!("expected accepted stream, got {other:?}"),
                });
            } else if completion.request == connect {
                client = Some(match completion.result.expect("connect succeeds") {
                    AsyncHostValue::Resource(resource) => resource,
                    other => panic!("expected connected stream, got {other:?}"),
                });
            }
        }
        let server = server.expect("accepted stream observed");
        let client = client.expect("connected stream observed");

        let pending_read = runtime.start_host_op(
            AsyncHostOp::TcpRead {
                stream: server.id,
                max_bytes: 64,
            },
            false,
        );
        assert!(!runtime.host().has_referenced_work());
        let mut realm = Realm::<(), ()>::new();
        assert!(runtime.next_referenced_turn(&mut realm).is_none());
        let response_write = runtime.start_host_op(
            AsyncHostOp::TcpWrite {
                stream: server.id,
                data: b"response while reading".to_vec(),
            },
            false,
        );
        let completion = next_host_completion(&mut runtime)
            .expect("write completes without waiting for read EOF");
        assert_eq!(completion.request, response_write);
        assert_eq!(completion.result, Ok(AsyncHostValue::Undefined));

        runtime.start_host_op(
            AsyncHostOp::TcpRead {
                stream: client.id,
                max_bytes: 64,
            },
            false,
        );
        assert_eq!(
            next_host_completion(&mut runtime)
                .expect("peer reads response")
                .result,
            Ok(AsyncHostValue::Bytes(b"response while reading".to_vec()))
        );

        assert!(runtime.cancel_host_op(pending_read));
        for resource in [server.id, client.id, listener.id] {
            runtime.start_host_op(AsyncHostOp::CloseResource { resource }, false);
            next_host_completion(&mut runtime).expect("close completes");
        }
    }

    fn resource_state(runtime: &mut RealmRuntime, resource: ResourceId) -> ResourceState {
        runtime.start_host_op(AsyncHostOp::ResourceState { resource }, false);
        match next_host_completion(runtime)
            .expect("state completes")
            .result
            .expect("state succeeds")
        {
            AsyncHostValue::ResourceState(state) => state,
            other => panic!("expected resource state, got {other:?}"),
        }
    }

    #[test]
    fn resource_state_is_the_single_source_of_stream_lifecycle_truth() {
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        runtime.start_host_op(
            AsyncHostOp::TcpListen {
                host: "127.0.0.1".into(),
                port: 0,
            },
            true,
        );
        let listener = match next_host_completion(&mut runtime)
            .expect("listen completes")
            .result
            .expect("listen succeeds")
        {
            AsyncHostValue::Resource(resource) => resource,
            other => panic!("expected listener, got {other:?}"),
        };
        let listener_state = resource_state(&mut runtime, listener.id);
        assert_eq!(listener_state.info, listener);
        assert!(listener_state.referenced);
        assert!(!listener_state.read_closed);
        assert!(!listener_state.write_closed);

        runtime.start_host_op(
            AsyncHostOp::SetResourceReferenced {
                resource: listener.id,
                referenced: false,
            },
            false,
        );
        next_host_completion(&mut runtime).expect("unref completes");
        assert!(!resource_state(&mut runtime, listener.id).referenced);

        runtime.start_host_op(
            AsyncHostOp::CloseResource {
                resource: listener.id,
            },
            false,
        );
        next_host_completion(&mut runtime).expect("close completes");
    }

    #[test]
    fn stream_half_close_is_idempotent_and_persistently_observable() {
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        runtime.start_host_op(
            AsyncHostOp::TcpListen {
                host: "127.0.0.1".into(),
                port: 0,
            },
            false,
        );
        let listener = match next_host_completion(&mut runtime)
            .expect("listen completes")
            .result
            .expect("listen succeeds")
        {
            AsyncHostValue::Resource(resource) => resource,
            other => panic!("expected listener, got {other:?}"),
        };
        let port = listener.port.expect("ephemeral port assigned");
        let accept_request = runtime.start_host_op(
            AsyncHostOp::TcpAccept {
                listener: listener.id,
            },
            false,
        );
        let connect_request = runtime.start_host_op(
            AsyncHostOp::TcpConnect {
                host: "127.0.0.1".into(),
                port,
            },
            false,
        );
        let mut accepted = None;
        let mut client = None;
        for _ in 0..2 {
            let completion =
                next_host_completion(&mut runtime).expect("connection operation completes");
            if completion.request == accept_request {
                accepted = Some(match completion.result.expect("accept succeeds") {
                    AsyncHostValue::AcceptedTcpStream(accepted) => accepted.resource,
                    other => panic!("expected accepted stream, got {other:?}"),
                });
            } else if completion.request == connect_request {
                client = Some(match completion.result.expect("connect succeeds") {
                    AsyncHostValue::Resource(resource) => resource,
                    other => panic!("expected connected stream, got {other:?}"),
                });
            } else {
                panic!("unexpected connection request");
            }
        }
        let accepted = accepted.expect("accept result observed");
        let client = client.expect("connect result observed");

        for _ in 0..2 {
            runtime.start_host_op(AsyncHostOp::TcpShutdown { stream: client.id }, false);
            assert_eq!(
                next_host_completion(&mut runtime)
                    .expect("shutdown completes")
                    .result,
                Ok(AsyncHostValue::Undefined)
            );
        }
        assert!(resource_state(&mut runtime, client.id).write_closed);
        runtime.start_host_op(
            AsyncHostOp::TcpWrite {
                stream: client.id,
                data: b"too late".to_vec(),
            },
            false,
        );
        let error = next_host_completion(&mut runtime)
            .expect("write completes")
            .result
            .expect_err("write after end is rejected");
        assert_eq!(error.code, "EPIPE");

        runtime.start_host_op(
            AsyncHostOp::TcpRead {
                stream: accepted.id,
                max_bytes: 32,
            },
            false,
        );
        assert_eq!(
            next_host_completion(&mut runtime)
                .expect("EOF completes")
                .result,
            Ok(AsyncHostValue::Bytes(Vec::new()))
        );
        assert!(resource_state(&mut runtime, accepted.id).read_closed);
        runtime.start_host_op(
            AsyncHostOp::TcpRead {
                stream: accepted.id,
                max_bytes: 32,
            },
            false,
        );
        assert_eq!(
            next_host_completion(&mut runtime)
                .expect("cached EOF completes")
                .result,
            Ok(AsyncHostValue::Bytes(Vec::new()))
        );

        for resource in [client.id, accepted.id, listener.id] {
            runtime.start_host_op(AsyncHostOp::CloseResource { resource }, false);
            next_host_completion(&mut runtime).expect("close completes");
        }
    }

    #[test]
    fn node_stream_driver_runs_the_same_action_loop_on_live_tokio_sockets() {
        use crate::node_stream::{StreamAction, StreamDriver, StreamEvent};

        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        runtime.start_host_op(
            AsyncHostOp::TcpListen {
                host: "127.0.0.1".into(),
                port: 0,
            },
            false,
        );
        let listener = match next_host_completion(&mut runtime)
            .expect("listen completes")
            .result
            .expect("listen succeeds")
        {
            AsyncHostValue::Resource(resource) => resource,
            other => panic!("expected listener, got {other:?}"),
        };
        let port = listener.port.expect("ephemeral port assigned");
        let accept = runtime.start_host_op(
            AsyncHostOp::TcpAccept {
                listener: listener.id,
            },
            false,
        );
        let connect = runtime.start_host_op(
            AsyncHostOp::TcpConnect {
                host: "127.0.0.1".into(),
                port,
            },
            false,
        );
        let mut accepted = None;
        let mut client = None;
        for _ in 0..2 {
            let completion = next_host_completion(&mut runtime).expect("connection completes");
            if completion.request == accept {
                accepted = Some(match completion.result.expect("accept succeeds") {
                    AsyncHostValue::AcceptedTcpStream(accepted) => accepted.resource,
                    other => panic!("expected accepted stream, got {other:?}"),
                });
            } else if completion.request == connect {
                client = Some(match completion.result.expect("connect succeeds") {
                    AsyncHostValue::Resource(resource) => resource,
                    other => panic!("expected client stream, got {other:?}"),
                });
            }
        }
        let accepted = accepted.expect("accepted stream observed");
        let client = client.expect("client stream observed");
        let mut reader = StreamDriver::new(accepted.id, 16);
        let mut writer = StreamDriver::new(client.id, 4);

        let read = reader.start_reading().pop().expect("read action");
        let StreamAction::Start(read_op) = &read else {
            panic!("expected read operation");
        };
        let read_request = runtime.start_host_op(read_op.clone(), false);
        assert!(reader.started(read_op, read_request));

        let (_, actions) = writer.end(Some(b"hello".to_vec()));
        assert_eq!(actions.len(), 1);
        let StreamAction::Start(write_op) = &actions[0] else {
            panic!("expected write operation");
        };
        let write_request = runtime.start_host_op(write_op.clone(), false);
        assert!(writer.started(write_op, write_request));

        let mut bytes = Vec::new();
        let mut writer_events = Vec::new();
        let mut reader_events = Vec::new();
        while !writer.state().write_closed() || !reader.state().read_closed() {
            let completion = next_host_completion(&mut runtime).expect("stream work completes");
            let (owner_is_writer, actions) =
                if let Some(actions) = writer.completed(completion.clone()) {
                    (true, actions)
                } else {
                    (
                        false,
                        reader
                            .completed(completion)
                            .expect("completion belongs to a driver"),
                    )
                };
            for action in actions {
                match action {
                    StreamAction::Start(operation) => {
                        let request = runtime.start_host_op(operation.clone(), false);
                        assert!(
                            writer.started(&operation, request)
                                || reader.started(&operation, request)
                        );
                    }
                    StreamAction::Cancel(request) => {
                        panic!("unexpected cancellation of live stream request {request:?}");
                    }
                    StreamAction::Data(data) => bytes.extend(data),
                    StreamAction::Event(event) => {
                        if owner_is_writer {
                            writer_events.push(event);
                        } else {
                            reader_events.push(event);
                        }
                    }
                    StreamAction::Error(error) => panic!("unexpected stream error: {error}"),
                }
            }
        }
        assert_eq!(bytes, b"hello");
        assert_eq!(writer_events, [StreamEvent::Drain, StreamEvent::Finish]);
        assert_eq!(reader_events, [StreamEvent::End]);

        for resource in [client.id, accepted.id, listener.id] {
            runtime.start_host_op(AsyncHostOp::CloseResource { resource }, false);
            next_host_completion(&mut runtime).expect("close completes");
        }
    }

    #[test]
    fn node_server_driver_rearms_accept_on_live_tokio_listener() {
        use crate::node_net::{ServerAction, TcpServerDriver};

        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        let mut server = TcpServerDriver::default();
        let ServerAction::Start(listen) = server.listen("127.0.0.1".into(), 0) else {
            unreachable!()
        };
        let listen_request = runtime.start_host_op(listen.clone(), false);
        assert!(server.started(&listen, listen_request));
        let actions = server
            .completed(next_host_completion(&mut runtime).expect("listen completes"))
            .expect("listen belongs to server");
        let listener = match &actions[0] {
            ServerAction::Listening(listener) => listener.clone(),
            other => panic!("expected listening action, got {other:?}"),
        };
        let ServerAction::Start(accept) = &actions[1] else {
            panic!("expected accept action");
        };
        let accept_request = runtime.start_host_op(accept.clone(), false);
        assert!(server.started(accept, accept_request));

        let port = listener.port.expect("ephemeral port assigned");
        runtime.start_host_op(
            AsyncHostOp::TcpConnect {
                host: "127.0.0.1".into(),
                port,
            },
            false,
        );
        let mut accepted_resource = None;
        let mut client_resource = None;
        while accepted_resource.is_none() || client_resource.is_none() {
            let completion = next_host_completion(&mut runtime).expect("connection completes");
            if completion.request == accept_request {
                let actions = server
                    .completed(completion)
                    .expect("accept belongs to server");
                match &actions[0] {
                    ServerAction::Connection(accepted) => {
                        accepted_resource = Some(accepted.resource.id)
                    }
                    other => panic!("expected connection, got {other:?}"),
                }
                let ServerAction::Start(next_accept) = &actions[1] else {
                    panic!("expected rearmed accept");
                };
                let next_request = runtime.start_host_op(next_accept.clone(), false);
                assert!(server.started(next_accept, next_request));
                assert!(AsyncHostOps::cancel(runtime.host(), next_request));
            } else {
                client_resource = Some(match completion.result.expect("connect succeeds") {
                    AsyncHostValue::Resource(resource) => resource.id,
                    other => panic!("expected client stream, got {other:?}"),
                });
            }
        }

        let close_actions = server.close();
        let ServerAction::Start(close) = close_actions.last().expect("close starts") else {
            unreachable!()
        };
        let close_request = runtime.start_host_op(close.clone(), false);
        assert!(server.started(close, close_request));
        assert_eq!(
            server.completed(next_host_completion(&mut runtime).expect("close completes")),
            Some(vec![ServerAction::Close])
        );
        for resource in [accepted_resource.unwrap(), client_resource.unwrap()] {
            runtime.start_host_op(AsyncHostOp::CloseResource { resource }, false);
            next_host_completion(&mut runtime).expect("stream close completes");
        }
    }

    #[test]
    fn realm_shutdown_cancels_pending_tcp_work_before_closing_resources() {
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        runtime.start_host_op(
            AsyncHostOp::TcpListen {
                host: "127.0.0.1".into(),
                port: 0,
            },
            true,
        );
        let listener = match next_host_completion(&mut runtime)
            .expect("listen completes")
            .result
            .expect("listen succeeds")
        {
            AsyncHostValue::Resource(resource) => resource,
            other => panic!("expected listener, got {other:?}"),
        };

        let accept_request = runtime.start_host_op(
            AsyncHostOp::TcpAccept {
                listener: listener.id,
            },
            true,
        );
        let connect_request = runtime.start_host_op(
            AsyncHostOp::TcpConnect {
                host: "127.0.0.1".into(),
                port: listener.port.expect("ephemeral port assigned"),
            },
            true,
        );
        let mut accepted = None;
        let mut client = None;
        while accepted.is_none() || client.is_none() {
            let completion = next_host_completion(&mut runtime).expect("connection completes");
            if completion.request == accept_request {
                accepted = Some(match completion.result.expect("accept succeeds") {
                    AsyncHostValue::AcceptedTcpStream(stream) => stream.resource.id,
                    other => panic!("expected accepted stream, got {other:?}"),
                });
            } else {
                assert_eq!(completion.request, connect_request);
                client = Some(match completion.result.expect("connect succeeds") {
                    AsyncHostValue::Resource(resource) => resource.id,
                    other => panic!("expected client stream, got {other:?}"),
                });
            }
        }

        let pending_accept = runtime.start_host_op(
            AsyncHostOp::TcpAccept {
                listener: listener.id,
            },
            true,
        );
        let pending_read = runtime.start_host_op(
            AsyncHostOp::TcpRead {
                stream: accepted.expect("accepted resource"),
                max_bytes: 16,
            },
            true,
        );
        assert_ne!(pending_accept, pending_read);
        assert_eq!(runtime.host.pending.len(), 2);
        assert_eq!(runtime.host.resources.len(), 3);

        assert!(runtime.shutdown());
        assert!(runtime.host.shutdown);
        assert!(runtime.host.pending.is_empty());
        assert!(runtime.host.ready.is_empty());
        assert!(runtime.host.resources.is_empty());
        assert!(runtime.host.tasks.is_empty());

        runtime
            .executor
            .local
            .block_on(&runtime.executor.runtime, async {
                tokio::task::yield_now().await;
            });
        assert_eq!(
            runtime
                .next_host_completion()
                .expect("late task exits are discarded"),
            None
        );
        assert!(!runtime.shutdown());
        let _ = client;
    }

    #[test]
    fn http_core_composes_with_stream_driver_for_a_live_tokio_exchange() {
        use crate::node_http::{Http1Decoder, HttpResponse};

        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        runtime.start_host_op(
            AsyncHostOp::TcpListen {
                host: "127.0.0.1".into(),
                port: 0,
            },
            false,
        );
        let listener = match next_host_completion(&mut runtime)
            .expect("listen completes")
            .result
            .expect("listen succeeds")
        {
            AsyncHostValue::Resource(resource) => resource,
            other => panic!("expected listener, got {other:?}"),
        };
        let accept = runtime.start_host_op(
            AsyncHostOp::TcpAccept {
                listener: listener.id,
            },
            false,
        );
        let connect = runtime.start_host_op(
            AsyncHostOp::TcpConnect {
                host: "127.0.0.1".into(),
                port: listener.port.expect("ephemeral port assigned"),
            },
            false,
        );
        let mut server_stream = None;
        let mut client_stream = None;
        for _ in 0..2 {
            let completion = next_host_completion(&mut runtime).expect("connection completes");
            if completion.request == accept {
                server_stream = Some(match completion.result.expect("accept succeeds") {
                    AsyncHostValue::AcceptedTcpStream(accepted) => accepted.resource,
                    other => panic!("expected accepted stream, got {other:?}"),
                });
            } else if completion.request == connect {
                client_stream = Some(match completion.result.expect("connect succeeds") {
                    AsyncHostValue::Resource(resource) => resource,
                    other => panic!("expected client stream, got {other:?}"),
                });
            }
        }
        let server_stream = server_stream.unwrap();
        let client_stream = client_stream.unwrap();

        runtime.start_host_op(
            AsyncHostOp::TcpWrite {
                stream: client_stream.id,
                data: b"POST /echo HTTP/1.1\r\nHost: kiln\r\nContent-Length: 4\r\nConnection: close\r\n\r\nping".to_vec(),
            },
            false,
        );
        next_host_completion(&mut runtime).expect("request write completes");
        runtime.start_host_op(
            AsyncHostOp::TcpRead {
                stream: server_stream.id,
                max_bytes: 4096,
            },
            false,
        );
        let request_bytes = match next_host_completion(&mut runtime)
            .expect("request read completes")
            .result
            .expect("request read succeeds")
        {
            AsyncHostValue::Bytes(bytes) => bytes,
            other => panic!("expected request bytes, got {other:?}"),
        };
        let request = Http1Decoder::default()
            .push(&request_bytes)
            .expect("request parses")
            .pop()
            .expect("complete request");
        assert_eq!(request.method, "POST");
        assert_eq!(request.target, "/echo");
        assert_eq!(request.body, b"ping");

        let response = HttpResponse {
            status: 200,
            reason: None,
            headers: vec![("Content-Type".into(), "text/plain".into())],
            body: request.body,
        }
        .encode(request.keep_alive);
        runtime.start_host_op(
            AsyncHostOp::TcpWrite {
                stream: server_stream.id,
                data: response,
            },
            false,
        );
        next_host_completion(&mut runtime).expect("response write completes");
        runtime.start_host_op(
            AsyncHostOp::TcpShutdown {
                stream: server_stream.id,
            },
            false,
        );
        next_host_completion(&mut runtime).expect("response shutdown completes");
        runtime.start_host_op(
            AsyncHostOp::TcpRead {
                stream: client_stream.id,
                max_bytes: 4096,
            },
            false,
        );
        let response = match next_host_completion(&mut runtime)
            .expect("response read completes")
            .result
            .expect("response read succeeds")
        {
            AsyncHostValue::Bytes(bytes) => bytes,
            other => panic!("expected response bytes, got {other:?}"),
        };
        assert_eq!(
            response,
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 4\r\nConnection: close\r\n\r\nping"
        );

        for resource in [server_stream.id, client_stream.id, listener.id] {
            runtime.start_host_op(AsyncHostOp::CloseResource { resource }, false);
            next_host_completion(&mut runtime).expect("close completes");
        }
    }

    #[test]
    fn runtime_owned_hyper_connection_uses_live_hostops_transport() {
        use crate::node_http::HttpResponse;

        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        runtime.start_host_op(
            AsyncHostOp::TcpListen {
                host: "127.0.0.1".into(),
                port: 0,
            },
            false,
        );
        let listener = match next_host_completion(&mut runtime)
            .expect("listen completes")
            .result
            .expect("listen succeeds")
        {
            AsyncHostValue::Resource(resource) => resource,
            other => panic!("expected listener, got {other:?}"),
        };
        let accept = runtime.start_host_op(
            AsyncHostOp::TcpAccept {
                listener: listener.id,
            },
            false,
        );
        let connect = runtime.start_host_op(
            AsyncHostOp::TcpConnect {
                host: "127.0.0.1".into(),
                port: listener.port.expect("listener has a port"),
            },
            false,
        );
        let mut server = None;
        let mut client = None;
        for _ in 0..2 {
            let completion = next_host_completion(&mut runtime).expect("connect completes");
            if completion.request == accept {
                server = Some(match completion.result.expect("accept succeeds") {
                    AsyncHostValue::AcceptedTcpStream(stream) => stream.resource,
                    other => panic!("expected accepted stream, got {other:?}"),
                });
            } else if completion.request == connect {
                client = Some(match completion.result.expect("connect succeeds") {
                    AsyncHostValue::Resource(resource) => resource,
                    other => panic!("expected client stream, got {other:?}"),
                });
            }
        }
        let server = server.expect("accepted stream exists");
        let client = client.expect("client stream exists");
        let start = runtime.start_http1_connection(server.id);
        let [HttpStreamAction::Start(read)] = start.as_slice() else {
            panic!("HTTP connection starts one read")
        };
        let read_request = start_http_test_operation(&mut runtime, server.id, read, false);
        runtime.start_host_op(
            AsyncHostOp::TcpWrite {
                stream: client.id,
                data: b"POST /health HTTP/1.1\r\nhost: kiln\r\ncontent-length: 4\r\nconnection: close\r\n\r\nping".to_vec(),
            },
            false,
        );
        let mut read_completion = None;
        for _ in 0..2 {
            let completion = next_host_completion(&mut runtime).expect("I/O completes");
            if completion.request == read_request {
                read_completion = Some(completion);
            }
        }
        let actions = runtime
            .complete_http_connection(server.id, read_completion.expect("server read completes"))
            .expect("connection owns completion");
        for action in actions {
            if let HttpStreamAction::Start(operation) = action {
                start_http_test_operation(&mut runtime, server.id, &operation, false);
            }
        }
        runtime.poll_http_connection(server.id);
        let events = runtime.take_http_events(server.id);
        let (id, request, body) = request_events(&events);
        assert_eq!(request.method, "POST");
        assert_eq!(request.target, "/health");
        assert_eq!(body, b"ping");
        assert!(runtime.respond_http1(
            server.id,
            id,
            HttpResponse {
                status: 200,
                reason: Some("OK".into()),
                headers: vec![("content-type".into(), "text/plain".into())],
                body: b"kiln".to_vec(),
            }
        ));
        let writes = runtime
            .poll_http_connection(server.id)
            .expect("connection exists");
        let [HttpStreamAction::Start(write)] = writes.as_slice() else {
            panic!("Hyper selected one response write: {writes:?}")
        };
        let write_request = start_http_test_operation(&mut runtime, server.id, write, false);
        runtime.start_host_op(
            AsyncHostOp::TcpRead {
                stream: client.id,
                max_bytes: 4096,
            },
            false,
        );
        let mut response = None;
        for _ in 0..2 {
            let completion = next_host_completion(&mut runtime).expect("response I/O completes");
            if completion.request == write_request {
                runtime
                    .complete_http_connection(server.id, completion)
                    .expect("write completion belongs to connection");
            } else if let Ok(AsyncHostValue::Bytes(bytes)) = completion.result {
                response = Some(bytes);
            }
        }
        let response = String::from_utf8(response.expect("client receives response"))
            .expect("response is ASCII");
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.ends_with("\r\n\r\nkiln"));
        assert!(!response.to_ascii_lowercase().contains("\r\ndate:"));

        for resource in [server.id, client.id, listener.id] {
            runtime.start_host_op(AsyncHostOp::CloseResource { resource }, false);
            next_host_completion(&mut runtime).expect("close completes");
        }
    }

    #[test]
    fn runtime_owned_hyper_connection_is_deterministic_over_scripted_completions() {
        use crate::node_http::HttpResponse;

        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        let resource = ResourceId::new(91);
        let start = runtime.start_http1_connection(resource);
        let [HttpStreamAction::Start(read)] = start.as_slice() else {
            panic!("HTTP connection starts one read")
        };
        assert!(runtime.http_connection_started(resource, read, RequestId::new(1)));
        let actions = runtime
            .complete_http_connection(
                resource,
                Completion {
                    request: RequestId::new(1),
                    sequence: 0,
                    result: Ok(AsyncHostValue::Bytes(
                        b"POST /echo HTTP/1.1\r\nhost: kiln\r\ncontent-length: 4\r\nconnection: close\r\n\r\nping"
                            .to_vec(),
                    )),
                },
            )
            .expect("scripted completion belongs to connection");
        assert!(actions.iter().any(|action| matches!(
            action,
            HttpStreamAction::Start(HttpHostOp::TcpRead { stream, .. })
                if stream.resource() == resource
        )));
        runtime.poll_http_connection(resource);
        let events = runtime.take_http_events(resource);
        let (id, request, body) = request_events(&events);
        assert_eq!(request.target, "/echo");
        assert_eq!(body, b"ping");
        assert!(runtime.respond_http1(
            resource,
            id,
            HttpResponse {
                status: 201,
                reason: None,
                headers: vec![("content-type".into(), "text/plain".into())],
                body: body.clone(),
            }
        ));
        let writes = runtime
            .poll_http_connection(resource)
            .expect("connection exists");
        let [HttpStreamAction::Start(HttpHostOp::TcpWrite { stream, data })] = writes.as_slice()
        else {
            panic!("script selects one write: {writes:?}")
        };
        assert_eq!(stream.resource(), resource);
        let response = String::from_utf8(data.clone()).expect("HTTP is ASCII");
        assert!(response.starts_with("HTTP/1.1 201 Created\r\n"));
        assert!(response.contains("content-type: text/plain\r\n"));
        assert!(response.ends_with("\r\n\r\nping"));
        assert!(!response.to_ascii_lowercase().contains("\r\ndate:"));
        assert!(runtime.http_connection_started(
            resource,
            &HttpHostOp::TcpWrite {
                stream: HttpResourceId::new(resource),
                data: data.clone(),
            },
            RequestId::new(2),
        ));
        let mut after_write = runtime
            .complete_http_connection(
                resource,
                Completion {
                    request: RequestId::new(2),
                    sequence: 1,
                    result: Ok(AsyncHostValue::Undefined),
                },
            )
            .expect("write completion belongs to connection");
        for _ in 0..8 {
            if runtime
                .take_http_events(resource)
                .contains(&RuntimeHttpEvent::Closed)
            {
                after_write.extend(
                    runtime
                        .close_http_connection(resource, HttpCloseMode::Graceful)
                        .unwrap_or_default(),
                );
            }
            after_write.extend(runtime.poll_http_connection(resource).unwrap_or_default());
            if after_write
                .iter()
                .any(|action| matches!(action, HttpStreamAction::Close(_)))
            {
                break;
            }
        }
        assert!(after_write.iter().any(|action| matches!(
            action,
            HttpStreamAction::Close(closed) if closed.resource() == resource
        )));
        assert!(
            !after_write.iter().any(|action| matches!(
                action,
                HttpStreamAction::Start(HttpHostOp::TcpWrite { .. })
            ))
        );
    }

    #[test]
    fn runtime_http_response_body_preserves_node_write_and_drain_semantics() {
        use crate::node_stream::WriteDisposition;

        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        let resource = ResourceId::new(92);
        let start = runtime.start_http1_connection(resource);
        let [HttpStreamAction::Start(read)] = start.as_slice() else {
            panic!("HTTP connection starts one read")
        };
        assert!(runtime.http_connection_started(resource, read, RequestId::new(1)));
        runtime
            .complete_http_connection(
                resource,
                Completion {
                    request: RequestId::new(1),
                    sequence: 0,
                    result: Ok(AsyncHostValue::Bytes(
                        b"GET /stream HTTP/1.1\r\nhost: kiln\r\nconnection: close\r\n\r\n".to_vec(),
                    )),
                },
            )
            .expect("scripted completion belongs to connection");
        runtime.poll_http_connection(resource);
        let events = runtime.take_http_events(resource);
        let (id, _, body) = request_events(&events);
        assert!(body.is_empty());
        assert!(runtime.start_http1_response(
            resource,
            id,
            200,
            vec![("content-type".into(), "text/plain".into())],
            8,
        ));
        assert_eq!(
            runtime.write_http1_response(resource, id, vec![1; 4]),
            Some(WriteDisposition {
                ready: true,
                submit_now: true,
            })
        );
        assert_eq!(
            runtime.write_http1_response(resource, id, vec![2; 4]),
            Some(WriteDisposition {
                ready: false,
                submit_now: false,
            })
        );
        assert_eq!(
            runtime.end_http1_response(resource, id, Vec::new()),
            Some(WriteDisposition {
                ready: false,
                submit_now: false,
            })
        );

        let mut drain_count = 0;
        let mut finish_count = 0;
        for _ in 0..32 {
            runtime.poll_http_connection(resource);
            for event in runtime.take_http_events(resource) {
                drain_count += usize::from(event == RuntimeHttpEvent::Drain { id });
                finish_count += usize::from(event == RuntimeHttpEvent::Finish { id });
            }
            if finish_count == 1 {
                break;
            }
        }
        assert_eq!(drain_count, 1);
        assert_eq!(finish_count, 1);
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
    fn generic_drive_drains_next_ticks_before_microtasks() {
        let mut realm: Realm = Realm::new();
        realm.queue_next_tick(WorkId::new(1));
        realm.queue_microtask(WorkId::new(2));
        let mut events = Vec::new();
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");

        runtime
            .drive(
                &mut realm,
                |_realm, work| {
                    if let WorkItem::Microtask(work) = work {
                        events.push(work.get());
                    }
                    Ok::<_, ()>(())
                },
                |_| Ok::<_, ()>(()),
            )
            .expect("realm drive succeeds");

        assert_eq!(events, [1, 2]);
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
    fn a_virtual_timer_is_not_starved_by_a_pending_host_read() {
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        runtime.start_host_op(
            AsyncHostOp::TcpListen {
                host: "127.0.0.1".into(),
                port: 0,
            },
            false,
        );
        let listener = match next_host_completion(&mut runtime)
            .expect("listen completes")
            .result
            .expect("listen succeeds")
        {
            AsyncHostValue::Resource(resource) => resource,
            other => panic!("expected listener, got {other:?}"),
        };
        let accept = runtime.start_host_op(
            AsyncHostOp::TcpAccept {
                listener: listener.id,
            },
            false,
        );
        let connect = runtime.start_host_op(
            AsyncHostOp::TcpConnect {
                host: "127.0.0.1".into(),
                port: listener.port.expect("ephemeral port assigned"),
            },
            false,
        );
        let mut server = None;
        for _ in 0..2 {
            let completion = next_host_completion(&mut runtime).expect("connection completes");
            if completion.request == accept {
                server = Some(match completion.result.expect("accept succeeds") {
                    AsyncHostValue::AcceptedTcpStream(accepted) => accepted.resource,
                    other => panic!("expected accepted stream, got {other:?}"),
                });
            } else {
                assert_eq!(completion.request, connect);
            }
        }
        let server = server.expect("accepted stream observed");
        let pending_read = runtime.start_host_op(
            AsyncHostOp::TcpRead {
                stream: server.id,
                max_bytes: 64,
            },
            false,
        );
        let mut realm: Realm = Realm::new();
        realm.set_timeout(Duration::from_secs(60 * 60), WorkId::new(91));

        let turn = runtime
            .next_turn(&mut realm)
            .expect("timer advances while read remains pending");
        assert!(matches!(
            turn,
            RuntimeTurn::Timer(WorkItem::Timer { work, .. }) if work == WorkId::new(91)
        ));
        assert_eq!(realm.now(), Duration::from_secs(60 * 60));
        assert!(runtime.cancel_host_op(pending_read));
    }

    #[test]
    fn an_already_ready_host_completion_beats_a_due_timer() {
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        let request = runtime.start_host_op(
            AsyncHostOp::ResourceState {
                resource: ResourceId::new(999),
            },
            false,
        );
        let mut realm: Realm = Realm::new();
        realm.set_timeout(Duration::ZERO, WorkId::new(92));

        let turn = runtime
            .next_turn(&mut realm)
            .expect("ready host completion is selected");
        match turn {
            RuntimeTurn::Completion(completion) => {
                assert_eq!(completion.request, request);
                assert_eq!(
                    completion
                        .result
                        .expect_err("unknown resource is rejected")
                        .code,
                    "EBADF"
                );
            }
            RuntimeTurn::Timer(_) => panic!("a due timer ran before a ready completion"),
            RuntimeTurn::Failure(failure) => panic!("realm failed unexpectedly: {failure}"),
        }
        assert!(realm.has_pending_work());
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

    /// ADR 0040: state operations must not park the thread, because the drive calls them from
    /// inside a task that is already running on the executor and a nested park deadlocks.
    ///
    /// The type system carries most of this already — `RealmState` has no executor field, so it
    /// cannot reach `Runtime::block_on` through `self`. This closes the remaining door, which is
    /// reaching the ambient runtime through `Handle::current` or spawning onto it.
    #[test]
    fn realm_state_operations_never_park_the_thread() {
        let source = include_str!("lib.rs");
        let header = "impl<H: RealmHostProvider> RealmState<H> {";
        let start = source
            .find(header)
            .expect("the RealmState operations block should exist");
        let mut depth = 0usize;
        let mut end = start;
        for (offset, character) in source[start..].char_indices() {
            match character {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = start + offset;
                        break;
                    }
                }
                _ => {}
            }
        }
        assert!(end > start, "the RealmState operations block should close");
        let block = &source[start..end];

        for forbidden in ["block_on", "Handle::current", "self.executor"] {
            assert!(
                !block.contains(forbidden),
                "`{forbidden}` appears in the RealmState operations block. These operations run \
                 inside a task on the executor, so parking there deadlocks the drive. Either keep \
                 the operation non-blocking or move it onto RealmRuntime, which owns the executor \
                 and is only reachable from outside a turn. See ADR 0040."
            );
        }
    }
}
