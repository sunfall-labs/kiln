//! Runtime host implementations for Kiln programs.
//!
//! [`RealmRuntime`] is the first slice of the shared realm runtime. It owns a current-thread
//! Tokio runtime and drives a [`Realm`] whose language-level queues remain explicit: Tokio wakes
//! the realm at the next deadline, while the realm decides the order in which JavaScript work
//! runs. Tokio types are deliberately private to this crate.
#![allow(
    clippy::type_complexity,
    clippy::enum_variant_names,
    clippy::wrong_self_convention
)]

mod process_wire;
pub use process_wire::{
    ProcessWireError, ProcessWireRequest, decode as decode_process_wire,
    encode as encode_process_wire,
};

#[cfg(feature = "http")]
mod http_body;
#[cfg(feature = "http")]
pub mod http_client;
#[cfg(feature = "http")]
pub mod http_engine;
#[cfg(feature = "http")]
pub use http_client::HttpClientEngine;
#[cfg(feature = "http")]
pub use http_engine::{HostTransport, HttpDispatch, HttpEvent, HttpEventKind, HttpServerEngine};
/// Hyper-free transport seam used when the `http` feature is off: the boxed Tokio
/// socket a host hands over has no Hyper in it, so it lives here rather than in the
/// gated engine.
#[cfg(not(feature = "http"))]
pub struct HostTransport(pub Box<dyn HostIo>);
/// What a transport must be without Hyper: nothing more than Tokio's I/O traits.
#[cfg(not(feature = "http"))]
pub trait HostIo: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin {}
#[cfg(not(feature = "http"))]
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin> HostIo for T {}
#[cfg(not(feature = "http"))]
impl std::fmt::Debug for HostTransport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("HostTransport")
    }
}
pub mod node_http;
pub mod node_http_agent;
pub mod node_net;
pub mod node_stream;
pub mod scripted_transport;
pub use scripted_transport::{ScriptedTransport, ScriptedTransportLog};
/// The Linux io_uring transport behind `take_transport` (ADR 0046); see the module docs.
#[cfg(target_os = "linux")]
pub mod uring;
pub mod worker_pool;
#[cfg(target_os = "linux")]
pub use uring::IoPreference;

/// One `KILN_HTTP_TRACE` line, available with or without the Hyper engine.
#[allow(dead_code)]
pub(crate) fn http_trace_line(line: std::fmt::Arguments<'_>) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let enabled = *ENABLED.get_or_init(|| {
        std::env::var_os("KILN_HTTP_TRACE").is_some_and(|value| !value.is_empty() && value != "0")
    });
    if enabled {
        eprintln!("[kiln-http] {line}");
    }
}
pub use worker_pool::{
    ActorHandler, ActorId, DEFAULT_MAILBOX_CAPACITY, WorkerError, WorkerPool, WorkerRealmHandle,
};

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashSet, VecDeque};
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::rc::Rc;
use std::time::Duration;

pub use kiln_artifact_cache::{CapabilityIdentity, RealmCapabilityIdentity};
use kiln_host_abi::{
    AcceptedTcpStream, ActiveAsyncHostOps, AsyncHostError, AsyncHostOp, AsyncHostOps,
    AsyncHostValue, Completion, FileStat, HostError, HostOp, HostOps, HostValue, ProcessOutput,
    RequestId, ResourceId, ResourceInfo, ResourceKind, ResourceState,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{
    TcpListener, TcpStream,
    tcp::{OwnedReadHalf, OwnedWriteHalf},
};
use tokio::sync::oneshot;
use tokio_util::task::JoinMap;

use crate::node_http_agent::{AgentDecision, AgentFailure, AgentRelease, HttpAgent, HttpOrigin};

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
/// Connections attach through the queued adapter (ADR 0046): the engines dispatch into a
/// per-resource queue and the adapter drains it with `take_events` / `take_client_events` at
/// its own checkpoint.
pub trait ActiveHttpRuntime {
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
    /// Attach the server engine to an accepted stream. False if the host cannot hand its
    /// transport over or the resource is already attached.
    fn start_connection(&mut self, resource: ResourceId, allow_half_close: bool) -> bool;
    /// [`Self::start_connection`] for a connection that takes requests whole
    /// ([`HttpDispatch::whole_requests`]) when `whole_requests` is set: the fetch-style
    /// `serve` server's connections.
    fn start_connection_with(
        &mut self,
        resource: ResourceId,
        allow_half_close: bool,
        whole_requests: bool,
    ) -> bool {
        if whole_requests {
            return false;
        }
        self.start_connection(resource, allow_half_close)
    }
    /// Stop a connection of either role. False once no engine owns it, which is not an error
    /// for a caller that heard `Closed` a moment ago.
    fn close_connection(&mut self, resource: ResourceId, mode: HttpCloseMode) -> bool;
    fn take_events(&mut self, resource: ResourceId) -> Vec<RuntimeHttpEvent>;
    fn start_response(
        &mut self,
        resource: ResourceId,
        id: u64,
        status: u16,
        headers: Vec<(String, String)>,
        high_water_mark: usize,
    ) -> bool;
    /// A fetch-style (`serve`) response whole: status, the header list in its wire form
    /// (`\nname:value` per field, `HttpServerEngine::respond`) and the complete body. False when
    /// the request is gone or the head is refused.
    fn fetch_respond(
        &mut self,
        resource: ResourceId,
        id: u64,
        status: u16,
        headers: &[u8],
        body: Vec<u8>,
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
    fn end_response_bytes(
        &mut self,
        resource: ResourceId,
        id: u64,
        data: bytes::Bytes,
    ) -> Option<node_stream::WriteDisposition> {
        self.end_response(resource, id, data.to_vec())
    }
    /// Attach the client engine to a connected stream.
    fn start_client_connection(&mut self, resource: ResourceId) -> bool;
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

/// One clock reading, in the milliseconds a program reads.
///
/// Every clock JavaScript can see ends in this conversion -- `Date.now`, `performance.now`,
/// `process.hrtime`, `Intl`'s default instant, and the native backend's `kiln_realm_now` -- and
/// until this function existed each of them spelled it itself. Two spellings were in the tree:
/// `as_secs_f64() * 1_000.0` in the interpreter and `as_millis() as f64` in the native FFI. The
/// second truncates, so `performance.now()` answered whole milliseconds under the native backend
/// and sub-milliseconds under the interpreter -- one call, two precisions, decided by which
/// backend you happened to run. That is the divergence ADR 0043 exists to prevent, and the cost
/// was concrete: the native clock could not measure anything shorter than a millisecond, which
/// is most of what a benchmark measures.
///
/// Duplicated arithmetic is how it drifted, so there is one copy and both engines call it.
pub fn epoch_milliseconds(reading: Duration) -> f64 {
    reading.as_secs_f64() * 1_000.0
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
    /// The HTTP engines queued protocol events for a loop that drains them at its checkpoint
    /// (ADR 0046's queued adapter). Nothing else happened; run the checkpoint.
    Protocol,
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
    /// Scripted connections the HTTP engine may take over (ADR 0046), and what it did with
    /// each one after taking it.
    transports: BTreeMap<ResourceId, ScriptedTransport>,
    transport_logs: BTreeMap<ResourceId, ScriptedTransportLog>,
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
    /// Let provider-owned tasks finish cooperative teardown while the realm executor is alive.
    fn drain_on_shutdown(&mut self, _executor: &DriveExecutor) {}

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
    /// completions without borrowing the host across the wait (ADR 0046: language callbacks
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

    /// Return the number of completions ready without parking.
    fn ready_completions_count(&self) -> usize {
        0
    }

    /// Drain all completions currently ready without parking.
    fn take_ready_completions(&mut self) -> Vec<Completion> {
        let mut completions = Vec::new();
        while let Some(completion) = self.next_completion() {
            completions.push(completion);
        }
        completions
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
            transports: BTreeMap::new(),
            transport_logs: BTreeMap::new(),
            #[cfg(test)]
            shutdown_observer: None,
        }
    }

    /// Script the bytes of one connection: the HTTP engine takes `transport` when the realm
    /// attaches `resource`, and [`Self::transport_log`] shows what it wrote.
    pub fn with_transport(mut self, resource: ResourceId, transport: ScriptedTransport) -> Self {
        self.transport_logs.insert(resource, transport.log());
        self.transports.insert(resource, transport);
        self
    }

    /// What the engine has done with a scripted connection so far.
    pub fn transport_log(&self, resource: ResourceId) -> Option<&ScriptedTransportLog> {
        self.transport_logs.get(&resource)
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

    fn take_transport(&mut self, resource: ResourceId) -> Option<HostTransport> {
        self.transports
            .remove(&resource)
            .map(|transport| HostTransport(Box::new(transport)))
    }

    fn release_transport(&mut self, resource: ResourceId) -> bool {
        // The log outlives the connection on purpose: a test reads what was written after
        // the engine has let the transport go.
        self.transport_logs.contains_key(&resource)
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

impl Executor {
    /// Run `operation` inside this executor: directly when the caller is already there (a
    /// language callback made from a task, or from `enter`), since Tokio panics on a nested
    /// `block_on`; through one `block_on` otherwise.
    ///
    /// This is the one place the runtime asks "am I in context". ADR 0040 deleted the
    /// `_in_context` twins that put that question on every caller; a runtime that is only
    /// ever entered through its own executor can answer it itself.
    fn run<R>(&self, operation: impl FnOnce() -> R) -> R {
        if tokio::runtime::Handle::try_current().is_ok() {
            operation()
        } else {
            self.local
                .block_on(&self.runtime, async move { operation() })
        }
    }
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
    http_agent: HttpAgent,
    /// Server connections that are Hyper tasks over host transports (ADR 0046).
    #[cfg(feature = "http")]
    http_server: HttpServerEngine,
    /// Client connections, likewise.
    #[cfg(feature = "http")]
    http_client: HttpClientEngine,
    /// Without `http` there is no engine to own a connection; the field stays so
    /// constructors and destructuring compile unchanged.
    #[cfg(not(feature = "http"))]
    #[allow(dead_code)]
    http_server: (),
    #[cfg(not(feature = "http"))]
    #[allow(dead_code)]
    http_client: (),
    /// Protocol events parked by the queued adapter for a loop that drains them at its
    /// checkpoint, and the signal that wakes that loop.
    queued_http: Rc<QueuedHttp>,
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

    pub fn with_fs(mut self, fs: Box<dyn kiln_host_abi::FileSystemOps>) -> Self {
        self.state.host.set_fs(fs);
        self
    }

    pub fn set_fs(&mut self, fs: Box<dyn kiln_host_abi::FileSystemOps>) {
        self.state.host.set_fs(fs);
    }
}

impl<H: RealmHostProvider> RealmRuntime<H> {
    /// Tear down the realm after giving provider tasks a chance to complete cooperative cleanup.
    pub fn shutdown(&mut self) -> bool {
        if self.state.is_shutdown() {
            return false;
        }
        let executor = self.drive_executor();
        self.state.host.drain_on_shutdown(&executor);
        self.state.shutdown()
    }

    /// Select the host clock before the realm is entered. This keeps Tokio time real for
    /// host-backed executions; deterministic realms retain paused virtual time.
    pub fn use_host_clock(&mut self) {
        self.state.capabilities.clock = ClockCapability::Host;
    }

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
                http_agent: HttpAgent::default(),
                #[cfg(feature = "http")]
                http_server: HttpServerEngine::default(),
                #[cfg(feature = "http")]
                http_client: HttpClientEngine::default(),
                #[cfg(not(feature = "http"))]
                http_server: (),
                #[cfg(not(feature = "http"))]
                http_client: (),
                queued_http: Rc::new(QueuedHttp::default()),
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
    /// ADR 0046).
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
        let host = &mut self.state.host;
        Some(
            self.executor
                .run(|| RealmHostProvider::start_on_realm(host, operation, referenced)),
        )
    }

    /// Starts one asynchronous host operation on this realm's Tokio runtime.
    pub fn start_host_op(&mut self, operation: AsyncHostOp, referenced: bool) -> RequestId {
        self.try_start_host_op(operation, referenced)
            .expect("a shut down realm cannot start host work")
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
        if self.state.capabilities.clock == ClockCapability::Virtual && !self.state.time_paused {
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
        let host_pending = self.state.host.has_pending_work();
        if !host_pending && !self.state.has_live_http_work() {
            return self.next_timer(realm).map(RuntimeTurn::Timer);
        }
        if self.state.queued_http.take_signalled() || self.state.queued_http.has_events() {
            return Some(RuntimeTurn::Protocol);
        }
        // Three things can end the wait: a host completion (only while one is pending, since
        // a provider with nothing in flight answers `None` at once), the HTTP engines queueing
        // an event for the caller's checkpoint, and the next timer coming due.
        let signal = Rc::clone(&self.state.queued_http.signal);
        let host = &mut self.state.host;
        self.executor.local.block_on(&self.executor.runtime, async {
            let completion = async {
                if host_pending {
                    host.next_completion_async().await
                } else {
                    std::future::pending().await
                }
            };
            let timer = async {
                match deadline {
                    Some(deadline) => tokio::time::sleep(deadline.saturating_sub(realm.now)).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                biased;
                completion = completion => match completion {
                    Ok(Some(completion)) => Some(RuntimeTurn::Completion(completion)),
                    Ok(None) => None,
                    Err(failure) => Some(RuntimeTurn::Failure(failure)),
                },
                _ = signal.notified() => Some(RuntimeTurn::Protocol),
                _ = timer => realm.run_next_timer().map(RuntimeTurn::Timer),
            }
        })
    }

    /// Select a turn only while referenced host work, an HTTP request in flight, or a realm
    /// timer can keep execution alive. Unreferenced I/O remains registered and may still
    /// complete at later checkpoints, but does not make a Node realm wait indefinitely on an
    /// idle keep-alive socket.
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
        if !self.state.host.has_referenced_work()
            && realm.next_timer_deadline().is_none()
            && !self.state.has_live_http_work()
        {
            return None;
        }
        self.next_turn(realm)
    }

    /// Attach the HTTP engine to an accepted stream (ADR 0046): the host hands over its
    /// transport and Hyper drives it, calling `dispatch` directly for every protocol event.
    #[cfg(feature = "http")]
    pub fn server_attach(
        &mut self,
        resource: ResourceId,
        allow_half_close: bool,
        dispatch: Rc<dyn HttpDispatch>,
    ) -> bool {
        if self.is_shutdown() || self.state.http_owns(resource) {
            return false;
        }
        let Some(transport) = self.state.host.take_transport(resource) else {
            return false;
        };
        let engine = &mut self.state.http_server;
        self.executor
            .run(|| engine.attach(resource, transport, allow_half_close, dispatch))
    }

    /// Attach the HTTP client engine to a connected stream (ADR 0046): the host hands over its
    /// transport and Hyper drives it, calling `dispatch` directly for every response event.
    #[cfg(feature = "http")]
    pub fn client_attach(&mut self, resource: ResourceId, dispatch: Rc<dyn HttpDispatch>) -> bool {
        if self.is_shutdown() || self.state.http_owns(resource) {
            return false;
        }
        let Some(transport) = self.state.host.take_transport(resource) else {
            return false;
        };
        let engine = &mut self.state.http_client;
        self.executor
            .run(|| engine.attach(resource, transport, dispatch))
    }

    /// Attach a server connection whose events are queued for
    /// [`RealmState::take_queued_server_events`]
    /// (the queued adapter of ADR 0046, for a loop that drains events at its checkpoint).
    #[cfg(feature = "http")]
    pub fn server_attach_queued(&mut self, resource: ResourceId, allow_half_close: bool) -> bool {
        self.server_attach_queued_with(resource, allow_half_close, false)
    }

    /// [`Self::server_attach_queued`], optionally taking requests whole
    /// ([`HttpDispatch::whole_requests`]), which is what a fetch-style `serve` handler consumes.
    #[cfg(feature = "http")]
    pub fn server_attach_queued_with(
        &mut self,
        resource: ResourceId,
        allow_half_close: bool,
        whole_requests: bool,
    ) -> bool {
        let Some(admission) = self.state.queued_http.admit(resource) else {
            return false;
        };
        let dispatch = Rc::new(QueuedDispatch {
            resource,
            admission,
            role: QueuedRole::Server,
            queue: Rc::clone(&self.state.queued_http),
            whole_requests,
        });
        if self.server_attach(resource, allow_half_close, dispatch) {
            true
        } else {
            false
        }
    }

    /// Attach a client connection whose events are queued for
    /// [`Self::take_http_client_events`].
    #[cfg(feature = "http")]
    pub fn client_attach_queued(&mut self, resource: ResourceId) -> bool {
        let Some(admission) = self.state.queued_http.admit(resource) else {
            return false;
        };
        let dispatch = Rc::new(QueuedDispatch {
            resource,
            admission,
            role: QueuedRole::Client,
            queue: Rc::clone(&self.state.queued_http),
            whole_requests: false,
        });
        if self.client_attach(resource, dispatch) {
            true
        } else {
            false
        }
    }

    /// The direct HTTP server engine.
    #[cfg(feature = "http")]
    pub fn http_server(&mut self) -> &mut HttpServerEngine {
        &mut self.state.http_server
    }

    /// The direct HTTP client engine.
    #[cfg(feature = "http")]
    pub fn http_client(&mut self) -> &mut HttpClientEngine {
        &mut self.state.http_client
    }

    /// Stop the connection on `resource`, whichever engine owns it. The transport is released
    /// once the engine has published `Closed`; a caller closing from outside an engine
    /// callback sees that on its next turn.
    #[cfg(feature = "http")]
    pub fn http_close(&mut self, resource: ResourceId, mode: HttpCloseMode) -> bool {
        let engine = &mut self.state;
        self.executor.run(|| {
            engine.http_server.close(resource, mode) || engine.http_client.close(resource, mode)
        })
    }

    /// Without `http` no engine can own a transport, so there is nothing to stop.
    #[cfg(not(feature = "http"))]
    pub fn http_close(&mut self, _resource: ResourceId, _mode: HttpCloseMode) -> bool {
        false
    }

    /// Whether any direct server connection is still open.
    #[cfg(feature = "http")]
    pub fn has_open_server_connections(&self) -> bool {
        self.state.http_server.open_connections() != 0
    }

    /// Without `http` no server connection can be open.
    #[cfg(not(feature = "http"))]
    pub fn has_open_server_connections(&self) -> bool {
        false
    }

    /// How many server connections are open. The boolean above answers the drive's question;
    /// this answers a person's, through the `SIGUSR1` dump.
    #[cfg(feature = "http")]
    pub fn open_server_connections(&self) -> usize {
        self.state.http_server.open_connections()
    }

    /// Without `http` no server connection can be open.
    #[cfg(not(feature = "http"))]
    pub fn open_server_connections(&self) -> usize {
        0
    }

    /// How many client connections are open, likewise.
    #[cfg(feature = "http")]
    pub fn open_client_connections(&self) -> usize {
        self.state.http_client.open_connections()
    }

    /// Without `http` no client connection can be open.
    #[cfg(not(feature = "http"))]
    pub fn open_client_connections(&self) -> usize {
        0
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
        if self.state.capabilities.clock == ClockCapability::Virtual && !self.state.time_paused {
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
        if self.state.queued_http.has_events() || self.state.queued_http.take_signalled() {
            return Some(RuntimeTurn::Protocol);
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

    /// Requests in flight on direct connections: server responses the language has not
    /// finished, and client requests whose response has not ended.
    #[cfg(feature = "http")]
    pub fn requests_in_flight(&self) -> usize {
        self.state.http_server.requests_in_flight() + self.state.http_client.requests_in_flight()
    }

    /// Without `http` no request can be in flight on a direct connection.
    #[cfg(not(feature = "http"))]
    pub fn requests_in_flight(&self) -> usize {
        0
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
        if self.state.capabilities.clock == ClockCapability::Virtual && !self.state.time_paused {
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
        if self.state.capabilities.clock == ClockCapability::Virtual && !self.state.time_paused {
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

        #[cfg(feature = "http")]
        self.http_server.shutdown();
        #[cfg(feature = "http")]
        self.http_client.shutdown();
        self.queued_http.clear();
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

    /// Whether an HTTP engine owns the transport of `resource`.
    #[cfg(feature = "http")]
    pub fn http_owns(&self, resource: ResourceId) -> bool {
        self.http_server.is_attached(resource) || self.http_client.is_attached(resource)
    }

    /// Without `http` no engine can own a transport.
    #[cfg(not(feature = "http"))]
    pub fn http_owns(&self, _resource: ResourceId) -> bool {
        false
    }

    /// Whether the HTTP engines can still produce something the language must hear: a
    /// request in flight on either side, or a queued event nobody has drained. A taken
    /// transport is unreferenced by design, so this is what keeps a loop waiting on HTTP.
    #[cfg(feature = "http")]
    pub fn has_live_http_work(&self) -> bool {
        self.http_server.requests_in_flight() != 0
            || self.http_client.requests_in_flight() != 0
            || self.queued_http.has_pending_admissions()
            || self.queued_http.has_events()
    }

    /// Without `http` no engine can hold a request in flight; only queued events (which
    /// nothing can queue, since every attach fails) can keep a loop waiting.
    #[cfg(not(feature = "http"))]
    pub fn has_live_http_work(&self) -> bool {
        self.queued_http.has_events()
    }

    /// Drain the server events the queued adapter parked for `resource`. A `Closed` here is
    /// the connection's last word; the transport goes back to the host with it.
    pub fn take_queued_server_events(&mut self, resource: ResourceId) -> Vec<RuntimeHttpEvent> {
        let events = self.queued_http.take_server(resource);
        if events.contains(&RuntimeHttpEvent::Closed) {
            let _ = self.host.release_transport(resource);
        }
        events
    }

    /// Drain the server events the queued adapter parked for all resources in a single batch.
    pub fn take_all_queued_server_events(&mut self) -> Vec<(ResourceId, Vec<RuntimeHttpEvent>)> {
        let all = self.queued_http.take_all_server();
        for (resource, events) in &all {
            if events.contains(&RuntimeHttpEvent::Closed) {
                let _ = self.host.release_transport(*resource);
            }
        }
        all
    }

    /// Whether any server or client protocol event is queued.
    pub fn has_queued_http_events(&self) -> bool {
        self.queued_http.has_events()
    }

    /// Drain all completions currently ready without parking.
    pub fn take_ready_completions(&mut self) -> Vec<Completion> {
        self.host.take_ready_completions()
    }

    /// Take the next completion currently ready without parking.
    pub fn take_next_ready_completion(&mut self) -> Option<Completion> {
        self.host.next_completion()
    }

    /// Return the number of completions ready without parking.
    pub fn ready_completions_count(&self) -> usize {
        self.host.ready_completions_count()
    }

    /// Forget queued events for every resource not in `known`, returning how many were
    /// dropped. A loop that no longer tracks a connection cannot drain its queue, and an
    /// undrained queue would keep that loop waiting forever; the events of a connection the
    /// language has let go of are of no use to it.
    pub fn discard_queued_http_events_except(&mut self, known: &[ResourceId]) -> usize {
        self.queued_http.retain_resources(known)
    }

    /// Drain the client events the queued adapter parked for `resource`.
    pub fn take_queued_client_events(
        &mut self,
        resource: ResourceId,
    ) -> Vec<RuntimeHttpClientEvent> {
        let events = self.queued_http.take_client(resource);
        if events.contains(&RuntimeHttpClientEvent::Closed) {
            let _ = self.host.release_transport(resource);
        }
        events
    }

    #[cfg(test)]
    fn host_mut(&mut self) -> &mut H {
        &mut self.host
    }

    /// Give a transport back to the host once its connection has ended: the engine has
    /// published `Closed` and forgotten the connection, and a table that kept the resource
    /// would grow with connections ever accepted rather than connections open.
    pub fn retire_http_connection(&mut self, resource: ResourceId) -> bool {
        self.host.release_transport(resource)
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

/// Protocol events parked for a loop that drains them at its own checkpoint: ADR 0046's
/// queued adapter, which the interpreter uses until its loop can take the direct one.
#[derive(Default)]
pub struct QueuedHttp {
    server: std::cell::RefCell<BTreeMap<ResourceId, VecDeque<RuntimeHttpEvent>>>,
    client: std::cell::RefCell<BTreeMap<ResourceId, VecDeque<RuntimeHttpClientEvent>>>,
    /// Fires when an event is queued, so a parked [`RealmRuntime::next_turn`] returns
    /// [`RuntimeTurn::Protocol`].
    signal: Rc<tokio::sync::Notify>,
    /// A permit the signal would otherwise hand to a wait that has not started yet.
    signalled: std::cell::Cell<bool>,
    #[cfg_attr(not(feature = "http"), allow(dead_code))]
    next_admission: std::cell::Cell<u64>,
    pending_admissions: std::cell::RefCell<BTreeMap<ResourceId, u64>>,
}

impl QueuedHttp {
    #[cfg_attr(not(feature = "http"), allow(dead_code))]
    fn admit(&self, resource: ResourceId) -> Option<u64> {
        let mut pending = self.pending_admissions.borrow_mut();
        if pending.contains_key(&resource) {
            return None;
        }
        let admission = self.next_admission.get().wrapping_add(1).max(1);
        self.next_admission.set(admission);
        pending.insert(resource, admission);
        Some(admission)
    }

    #[cfg_attr(not(feature = "http"), allow(dead_code))]
    fn cancel_admission_token(&self, resource: ResourceId, admission: u64) {
        if self.pending_admissions.borrow().get(&resource) == Some(&admission) {
            self.pending_admissions.borrow_mut().remove(&resource);
        }
    }

    #[cfg_attr(not(feature = "http"), allow(dead_code))]
    fn started(&self, resource: ResourceId, admission: u64) {
        if self.pending_admissions.borrow().get(&resource) == Some(&admission) {
            self.pending_admissions.borrow_mut().remove(&resource);
            self.signalled.set(true);
            self.signal.notify_one();
        }
    }

    #[cfg_attr(not(feature = "http"), allow(dead_code))]
    fn push_server(&self, resource: ResourceId, event: RuntimeHttpEvent) {
        self.server
            .borrow_mut()
            .entry(resource)
            .or_default()
            .push_back(event);
        self.signalled.set(true);
        self.signal.notify_one();
    }

    #[cfg_attr(not(feature = "http"), allow(dead_code))]
    fn push_client(&self, resource: ResourceId, event: RuntimeHttpClientEvent) {
        self.client
            .borrow_mut()
            .entry(resource)
            .or_default()
            .push_back(event);
        self.signalled.set(true);
        self.signal.notify_one();
    }

    fn take_server(&self, resource: ResourceId) -> Vec<RuntimeHttpEvent> {
        self.server
            .borrow_mut()
            .remove(&resource)
            .map(Vec::from)
            .unwrap_or_default()
    }

    fn take_all_server(&self) -> Vec<(ResourceId, Vec<RuntimeHttpEvent>)> {
        std::mem::take(&mut *self.server.borrow_mut())
            .into_iter()
            .map(|(resource, events)| (resource, Vec::from(events)))
            .collect()
    }

    fn take_client(&self, resource: ResourceId) -> Vec<RuntimeHttpClientEvent> {
        self.client
            .borrow_mut()
            .remove(&resource)
            .map(Vec::from)
            .unwrap_or_default()
    }

    fn has_events(&self) -> bool {
        !self.server.borrow().is_empty() || !self.client.borrow().is_empty()
    }

    #[cfg_attr(not(feature = "http"), allow(dead_code))]
    fn has_pending_admissions(&self) -> bool {
        !self.pending_admissions.borrow().is_empty()
    }

    fn retain_resources(&self, known: &[ResourceId]) -> usize {
        let mut server = self.server.borrow_mut();
        let mut client = self.client.borrow_mut();
        let before = server.len() + client.len();
        server.retain(|resource, _| known.contains(resource));
        client.retain(|resource, _| known.contains(resource));
        before - server.len() - client.len()
    }

    /// Whether an event was queued since the last wait, consuming the flag. The `Notify`
    /// keeps its own permit, which the next wait drains at once; this only saves that wait.
    fn take_signalled(&self) -> bool {
        self.signalled.replace(false)
    }

    fn clear(&self) {
        self.server.borrow_mut().clear();
        self.client.borrow_mut().clear();
        self.pending_admissions.borrow_mut().clear();
    }
}

#[cfg(feature = "http")]
#[derive(Clone, Copy)]
enum QueuedRole {
    Server,
    Client,
}

/// The queued adapter: one per connection, converting each engine event into the owned event
/// the interpreter already consumes and parking it for `take_queued_*_events`.
#[cfg(feature = "http")]
struct QueuedDispatch {
    resource: ResourceId,
    admission: u64,
    role: QueuedRole,
    queue: Rc<QueuedHttp>,
    whole_requests: bool,
}

#[cfg(feature = "http")]
impl Drop for QueuedDispatch {
    fn drop(&mut self) {
        self.queue
            .cancel_admission_token(self.resource, self.admission);
    }
}

/// `name NUL value NUL` pairs back into owned strings; a value that is not UTF-8 is dropped,
/// as the bridged decoder dropped it.
fn unflatten_headers(bytes: &[u8]) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    let mut fields = bytes.split(|byte| *byte == 0);
    while let Some(name) = fields.next() {
        if name.is_empty() {
            break;
        }
        let Some(value) = fields.next() else {
            break;
        };
        if let (Ok(name), Ok(value)) = (std::str::from_utf8(name), std::str::from_utf8(value)) {
            headers.push((name.to_owned(), value.to_owned()));
        }
    }
    headers
}

#[cfg(feature = "http")]
impl HttpDispatch for QueuedDispatch {
    fn started(&self) {
        self.queue.started(self.resource, self.admission);
    }

    fn event(&self, event: &HttpEvent<'_>) -> i32 {
        if event.kind == HttpEventKind::Closed {
            self.queue
                .cancel_admission_token(self.resource, self.admission);
        }
        match self.role {
            QueuedRole::Server => {
                let queued = match event.kind {
                    HttpEventKind::RequestHead => RuntimeHttpEvent::RequestHead {
                        id: event.id,
                        request: node_http::HttpRequest {
                            method: String::from_utf8_lossy(event.method).into_owned(),
                            target: String::from_utf8_lossy(event.target).into_owned(),
                            version: if event.version_minor == 0 {
                                node_http::HttpVersion::Http10
                            } else {
                                node_http::HttpVersion::Http11
                            },
                            headers: node_http::HttpHeaders::from_raw(event.headers.to_vec()),
                            // Empty unless the connection takes requests whole.
                            body: event.data.to_vec(),
                            keep_alive: event.keep_alive,
                        },
                    },
                    HttpEventKind::RequestData => RuntimeHttpEvent::RequestData {
                        id: event.id,
                        data: event.data.to_vec(),
                    },
                    HttpEventKind::RequestEnd => RuntimeHttpEvent::RequestEnd { id: event.id },
                    HttpEventKind::Drain => RuntimeHttpEvent::Drain { id: event.id },
                    HttpEventKind::Finish => RuntimeHttpEvent::Finish { id: event.id },
                    HttpEventKind::Error => RuntimeHttpEvent::Error {
                        id: event.id,
                        message: String::from_utf8_lossy(event.message).into_owned(),
                    },
                    HttpEventKind::Closed => RuntimeHttpEvent::Closed,
                    HttpEventKind::ResponseHead
                    | HttpEventKind::ResponseData
                    | HttpEventKind::ResponseEnd => return 0,
                };
                self.queue.push_server(self.resource, queued);
            }
            QueuedRole::Client => {
                let queued = match event.kind {
                    HttpEventKind::ResponseHead => RuntimeHttpClientEvent::ResponseHead {
                        id: event.id,
                        status: event.status,
                        reason: String::from_utf8_lossy(event.reason).into_owned(),
                        headers: unflatten_headers(event.headers),
                    },
                    HttpEventKind::ResponseData => RuntimeHttpClientEvent::ResponseData {
                        id: event.id,
                        data: event.data.to_vec(),
                    },
                    HttpEventKind::ResponseEnd => RuntimeHttpClientEvent::ResponseEnd {
                        id: event.id,
                        reusable: event.keep_alive,
                    },
                    HttpEventKind::Drain => RuntimeHttpClientEvent::Drain { id: event.id },
                    HttpEventKind::Finish => RuntimeHttpClientEvent::Finish { id: event.id },
                    HttpEventKind::Error => RuntimeHttpClientEvent::Error {
                        id: event.id,
                        message: String::from_utf8_lossy(event.message).into_owned(),
                    },
                    HttpEventKind::Closed => RuntimeHttpClientEvent::Closed,
                    HttpEventKind::RequestHead
                    | HttpEventKind::RequestData
                    | HttpEventKind::RequestEnd => return 0,
                };
                self.queue.push_client(self.resource, queued);
            }
        }
        0
    }

    fn checkpoint(&self) -> i32 {
        // The loop that drains the queue runs its own checkpoint after each event.
        0
    }

    fn whole_requests(&self) -> bool {
        self.whole_requests
    }
}

/// `name NUL value NUL` per header: the flattening the engines parse straight into Hyper's map.
#[cfg(feature = "http")]
fn flatten_headers(headers: &[(String, String)]) -> Vec<u8> {
    let mut flattened = Vec::with_capacity(
        headers
            .iter()
            .map(|(name, value)| name.len() + value.len() + 2)
            .sum(),
    );
    for (name, value) in headers {
        flattened.extend_from_slice(name.as_bytes());
        flattened.push(0);
        flattened.extend_from_slice(value.as_bytes());
        flattened.push(0);
    }
    flattened
}

impl<H: RealmHostProvider> ActiveHttpRuntime for RealmRuntime<H> {
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

    fn start_connection(&mut self, resource: ResourceId, allow_half_close: bool) -> bool {
        self.start_connection_with(resource, allow_half_close, false)
    }

    fn start_connection_with(
        &mut self,
        resource: ResourceId,
        allow_half_close: bool,
        whole_requests: bool,
    ) -> bool {
        #[cfg(feature = "http")]
        {
            return self.server_attach_queued_with(resource, allow_half_close, whole_requests);
        }
        #[cfg(not(feature = "http"))]
        {
            let _ = (resource, allow_half_close, whole_requests);
            return false;
        }
    }

    fn close_connection(&mut self, resource: ResourceId, mode: HttpCloseMode) -> bool {
        #[cfg(feature = "http")]
        {
            return self.http_close(resource, mode);
        }
        #[cfg(not(feature = "http"))]
        {
            let _ = (resource, mode);
            return false;
        }
    }

    fn take_events(&mut self, resource: ResourceId) -> Vec<RuntimeHttpEvent> {
        self.state.take_queued_server_events(resource)
    }

    fn start_response(
        &mut self,
        resource: ResourceId,
        id: u64,
        status: u16,
        headers: Vec<(String, String)>,
        high_water_mark: usize,
    ) -> bool {
        #[cfg(feature = "http")]
        {
            return self.state.http_server.start_response(
                resource,
                id,
                status,
                &flatten_headers(&headers),
                high_water_mark,
            );
        }
        #[cfg(not(feature = "http"))]
        {
            let _ = (resource, id, status, headers, high_water_mark);
            return false;
        }
    }

    fn fetch_respond(
        &mut self,
        resource: ResourceId,
        id: u64,
        status: u16,
        headers: &[u8],
        body: Vec<u8>,
    ) -> bool {
        #[cfg(feature = "http")]
        {
            return self
                .state
                .http_server
                .respond(resource, id, status, headers, body.into());
        }
        #[cfg(not(feature = "http"))]
        {
            let _ = (resource, id, status, headers, body);
            return false;
        }
    }

    fn write_response(
        &mut self,
        resource: ResourceId,
        id: u64,
        data: Vec<u8>,
    ) -> Option<node_stream::WriteDisposition> {
        #[cfg(feature = "http")]
        {
            return self.state.http_server.write_response(resource, id, &data);
        }
        #[cfg(not(feature = "http"))]
        {
            let _ = (resource, id, data);
            return None;
        }
    }

    fn end_response(
        &mut self,
        resource: ResourceId,
        id: u64,
        data: Vec<u8>,
    ) -> Option<node_stream::WriteDisposition> {
        #[cfg(feature = "http")]
        {
            return self.state.http_server.end_response_bytes(
                resource,
                id,
                bytes::Bytes::from(data),
            );
        }
        #[cfg(not(feature = "http"))]
        {
            let _ = (resource, id, data);
            return None;
        }
    }

    #[cfg(feature = "http")]
    fn end_response_bytes(
        &mut self,
        resource: ResourceId,
        id: u64,
        data: bytes::Bytes,
    ) -> Option<node_stream::WriteDisposition> {
        self.state
            .http_server
            .end_response_bytes(resource, id, data)
    }

    fn start_client_connection(&mut self, resource: ResourceId) -> bool {
        #[cfg(feature = "http")]
        {
            return self.client_attach_queued(resource);
        }
        #[cfg(not(feature = "http"))]
        {
            let _ = resource;
            return false;
        }
    }

    fn send_client_request(
        &mut self,
        resource: ResourceId,
        request: RuntimeHttpClientRequest,
        id: u64,
        high_water_mark: usize,
    ) -> bool {
        #[cfg(feature = "http")]
        {
            return self.state.http_client.send_request(
                resource,
                id,
                &request.method,
                &request.target,
                &request.headers,
                high_water_mark,
            );
        }
        #[cfg(not(feature = "http"))]
        {
            let _ = (resource, request, id, high_water_mark);
            return false;
        }
    }

    fn write_client_request(
        &mut self,
        resource: ResourceId,
        id: u64,
        data: Vec<u8>,
    ) -> Option<node_stream::WriteDisposition> {
        #[cfg(feature = "http")]
        {
            return self.state.http_client.write_request(resource, id, &data);
        }
        #[cfg(not(feature = "http"))]
        {
            let _ = (resource, id, data);
            return None;
        }
    }

    fn end_client_request(
        &mut self,
        resource: ResourceId,
        id: u64,
        data: Vec<u8>,
    ) -> Option<node_stream::WriteDisposition> {
        #[cfg(feature = "http")]
        {
            return self.state.http_client.end_request(resource, id, &data);
        }
        #[cfg(not(feature = "http"))]
        {
            let _ = (resource, id, data);
            return None;
        }
    }

    fn take_client_events(&mut self, resource: ResourceId) -> Vec<RuntimeHttpClientEvent> {
        self.state.take_queued_client_events(resource)
    }
}

impl<H: RealmHostProvider> Drop for RealmRuntime<H> {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl<H: RealmHostProvider> ActiveAsyncHostOps for RealmRuntime<H> {
    fn start(&mut self, operation: AsyncHostOp, referenced: bool) -> RequestId {
        self.start_host_op(operation, referenced)
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
    process_tasks: BTreeSet<RequestId>,
    process_cancellers: BTreeMap<RequestId, oneshot::Sender<()>>,
    completed: Rc<tokio::sync::Notify>,
    failure: Option<RuntimeFailure>,
    shutdown: bool,
    fs: Option<Box<dyn kiln_host_abi::FileSystemOps>>,
    /// The io_uring every taken transport goes on when that backend is selected.
    #[cfg(target_os = "linux")]
    uring: uring::HostRing,
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
        listener: HostListener,
        referenced: bool,
    },
    TcpStream {
        stream: TcpStream,
        peer: std::net::SocketAddr,
        referenced: bool,
        accepted: bool,
    },
    /// A connection the io_uring accepted: not registered with Tokio's reactor until a discrete
    /// read, write or shutdown needs it, and never if the HTTP engine takes it first.
    #[cfg(target_os = "linux")]
    TcpRawStream {
        stream: std::net::TcpStream,
        peer: std::net::SocketAddr,
        referenced: bool,
    },
}

/// A listening socket: Tokio's, or (with the io_uring backend) the ring's multishot accept.
enum HostListener {
    Tokio(TcpListener),
    #[cfg(target_os = "linux")]
    Ring(uring::RingListener),
}

impl HostListener {
    fn local_addr(&self) -> std::io::Result<std::net::SocketAddr> {
        match self {
            HostListener::Tokio(listener) => listener.local_addr(),
            #[cfg(target_os = "linux")]
            HostListener::Ring(listener) => listener.local_addr(),
        }
    }
}

enum HostResource {
    TcpListener {
        listener: Rc<HostListener>,
        info: ResourceInfo,
        referenced: bool,
    },
    /// An accepted stream still in its raw form (see `HostCompletionValue::TcpRawStream`).
    #[cfg(target_os = "linux")]
    TcpRaw {
        stream: std::net::TcpStream,
        info: ResourceInfo,
        referenced: bool,
    },
    /// A stream whose transport the HTTP engine owns now (ADR 0046). Discrete reads and
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
                        #[cfg(target_os = "linux")]
                        HostResource::TcpRaw { referenced, .. } => *referenced,
                    })
                    .count(),
            )
            .field("ready", &self.ready.len())
            .field("has_fs", &self.fs.is_some())
            .finish_non_exhaustive()
    }
}

impl TokioHost {
    /// Builds the request and completion state owned by a [`RealmRuntime`].
    pub fn new() -> Self {
        Self {
            next_request: 1,
            next_sequence: 0,
            next_resource: 1,
            pending: BTreeMap::new(),
            resources: BTreeMap::new(),
            ready: BTreeMap::new(),
            tasks: JoinMap::new(),
            process_tasks: BTreeSet::new(),
            process_cancellers: BTreeMap::new(),
            completed: Rc::new(tokio::sync::Notify::new()),
            failure: None,
            shutdown: false,
            fs: None,
            #[cfg(target_os = "linux")]
            uring: uring::HostRing::new(),
            _not_send: PhantomData,
        }
    }

    /// Choose the transport backend for connections handed to the HTTP engines, overriding
    /// `KILN_IO`. Only affects transports taken after the call.
    #[cfg(target_os = "linux")]
    pub fn set_io_preference(&mut self, preference: IoPreference) {
        self.uring.set_preference(preference);
    }

    pub fn with_fs(mut self, fs: Box<dyn kiln_host_abi::FileSystemOps>) -> Self {
        self.fs = Some(fs);
        self
    }

    pub fn set_fs(&mut self, fs: Box<dyn kiln_host_abi::FileSystemOps>) {
        self.fs = Some(fs);
    }

    pub fn fs(&mut self) -> Option<&mut (dyn kiln_host_abi::FileSystemOps + '_)> {
        self.fs
            .as_mut()
            .map(|b| &mut **b as &mut (dyn kiln_host_abi::FileSystemOps + '_))
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
            AsyncHostOp::ProcessRun {
                command,
                args,
                stdin,
            } => {
                let (cancel, cancelled) = oneshot::channel();
                self.process_cancellers.insert(request, cancel);
                self.process_tasks.insert(request);
                self.spawn_task(request, sequence, referenced, async move {
                    let result = execute_process(command, args, stdin, cancelled).await;
                    HostCompletion {
                        request,
                        sequence,
                        lifecycle: None,
                        result: result.map(HostCompletionValue::Public),
                    }
                });
                return request;
            }
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
            AsyncHostOp::ReadFile { path } => {
                if let Some(fs) = &mut self.fs {
                    if let Some(data) = fs.read_in_memory(std::path::Path::new(&path)) {
                        self.insert_immediate(
                            request,
                            sequence,
                            referenced,
                            Ok(AsyncHostValue::Bytes(data)),
                        );
                        return request;
                    }
                }
                const MAX_REALM_READ_SIZE: u64 = 128 * 1024; // 131,072 bytes
                match std::fs::File::open(&path) {
                    Ok(mut file) => {
                        let file_size = file.metadata().map(|m| m.len()).unwrap_or(u64::MAX);
                        if file_size <= MAX_REALM_READ_SIZE {
                            use std::io::Read;
                            let mut data = Vec::with_capacity(file_size as usize);
                            match file.read_to_end(&mut data) {
                                Ok(_) => {
                                    self.insert_immediate(
                                        request,
                                        sequence,
                                        referenced,
                                        Ok(AsyncHostValue::Bytes(data)),
                                    );
                                    return request;
                                }
                                Err(error) => {
                                    self.insert_immediate(
                                        request,
                                        sequence,
                                        referenced,
                                        Err(io_error("open", path, error)),
                                    );
                                    return request;
                                }
                            }
                        }
                    }
                    Err(error) => {
                        self.insert_immediate(
                            request,
                            sequence,
                            referenced,
                            Err(io_error("open", path, error)),
                        );
                        return request;
                    }
                }
                AsyncHostOp::ReadFile { path }
            }
            AsyncHostOp::Stat { path } => {
                if let Some(fs) = &mut self.fs {
                    if let Some(meta) = fs.metadata_in_memory(std::path::Path::new(&path)) {
                        self.insert_immediate(
                            request,
                            sequence,
                            referenced,
                            Ok(AsyncHostValue::FileStat(FileStat {
                                size: meta.len,
                                is_file: meta.is_file,
                                is_directory: meta.is_dir,
                                is_symlink: meta.is_symlink,
                            })),
                        );
                        return request;
                    }
                }
                let result = std::fs::metadata(&path)
                    .map(|metadata| {
                        AsyncHostValue::FileStat(FileStat {
                            size: metadata.len(),
                            is_file: metadata.is_file(),
                            is_directory: metadata.is_dir(),
                            is_symlink: metadata.file_type().is_symlink(),
                        })
                    })
                    .map_err(|error| io_error("stat", path, error));
                self.insert_immediate(request, sequence, referenced, result);
                return request;
            }
            AsyncHostOp::Mkdir { path, recursive } => {
                let result = if recursive {
                    std::fs::create_dir_all(&path)
                } else {
                    std::fs::create_dir(&path)
                }
                .map(|()| AsyncHostValue::Undefined)
                .map_err(|error| io_error("mkdir", path, error));
                self.insert_immediate(request, sequence, referenced, result);
                return request;
            }
            AsyncHostOp::WriteFile { path, data } => {
                if let Some(fs) = &mut self.fs {
                    if fs.is_in_memory(std::path::Path::new(&path)) {
                        let _ = fs.write(std::path::Path::new(&path), &data);
                        self.insert_immediate(
                            request,
                            sequence,
                            referenced,
                            Ok(AsyncHostValue::Undefined),
                        );
                        return request;
                    }
                }
                const MAX_REALM_WRITE_SIZE: usize = 64 * 1024; // 65,536 bytes
                if data.len() <= MAX_REALM_WRITE_SIZE {
                    let result = std::fs::write(&path, &data)
                        .map(|()| AsyncHostValue::Undefined)
                        .map_err(|error| io_error("open", path, error));
                    self.insert_immediate(request, sequence, referenced, result);
                    return request;
                }
                AsyncHostOp::WriteFile { path, data }
            }
            AsyncHostOp::AppendFile { path, data } => {
                if let Some(fs) = &mut self.fs {
                    if fs.is_in_memory(std::path::Path::new(&path)) {
                        let _ = fs.append(std::path::Path::new(&path), &data);
                        self.insert_immediate(
                            request,
                            sequence,
                            referenced,
                            Ok(AsyncHostValue::Undefined),
                        );
                        return request;
                    }
                }
                const MAX_REALM_WRITE_SIZE: usize = 64 * 1024; // 65,536 bytes
                if data.len() <= MAX_REALM_WRITE_SIZE {
                    let result = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&path)
                        .and_then(|mut file| std::io::Write::write_all(&mut file, &data))
                        .map(|()| AsyncHostValue::Undefined)
                        .map_err(|error| io_error("open", path, error));
                    self.insert_immediate(request, sequence, referenced, result);
                    return request;
                }
                AsyncHostOp::AppendFile { path, data }
            }
            AsyncHostOp::TcpListen { host, port } => {
                // With the io_uring backend the ring accepts this listener's connections
                // (multishot), and the socket is never registered with Tokio's reactor.
                #[cfg(target_os = "linux")]
                let ring = self.uring.ring();
                self.spawn_task(request, sequence, referenced, async move {
                    let result = bind_tcp_listener_std(host.as_str(), port)
                        .and_then(|listener| {
                            #[cfg(target_os = "linux")]
                            if let Some(ring) = ring {
                                return match ring.listen(std::os::fd::OwnedFd::from(listener)) {
                                    Ok(listener) => Ok(HostListener::Ring(listener)),
                                    Err(fd) => {
                                        TcpListener::from_std(fd.into()).map(HostListener::Tokio)
                                    }
                                };
                            }
                            TcpListener::from_std(listener).map(HostListener::Tokio)
                        })
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
                #[cfg(target_os = "linux")]
                if let HostListener::Ring(ring_listener) = &*listener {
                    let accept = ring_listener.accept();
                    self.spawn_task(request, sequence, referenced, async move {
                        let result = async {
                            let stream = std::net::TcpStream::from(accept.await?);
                            // A multishot accept reports no address; ask for it once.
                            let peer = stream.peer_addr()?;
                            Ok(HostCompletionValue::TcpRawStream {
                                stream,
                                peer,
                                referenced: accepted_referenced,
                            })
                        }
                        .await
                        .map_err(|error| socket_error("accept", error));
                        HostCompletion {
                            request,
                            sequence,
                            lifecycle: None,
                            result,
                        }
                    });
                    return request;
                }
                self.spawn_task(request, sequence, referenced, async move {
                    let listener = match &*listener {
                        HostListener::Tokio(listener) => listener,
                        #[cfg(target_os = "linux")]
                        HostListener::Ring(_) => unreachable!("ring listeners are handled above"),
                    };
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
                #[cfg(target_os = "linux")]
                self.materialize_raw(stream);
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
                #[cfg(target_os = "linux")]
                self.materialize_raw(stream);
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
                #[cfg(target_os = "linux")]
                self.materialize_raw(stream);
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
            AsyncHostOp::StdinRead { max_bytes } => AsyncHostOp::StdinRead { max_bytes },
            AsyncHostOp::Lstat { path } => {
                if let Some(fs) = &mut self.fs {
                    if let Some(meta) = fs.metadata_in_memory(std::path::Path::new(&path)) {
                        let result = Ok(AsyncHostValue::FileStat(FileStat {
                            size: meta.len,
                            is_file: meta.is_file,
                            is_directory: meta.is_dir,
                            is_symlink: meta.is_symlink,
                        }));
                        self.insert_immediate(request, sequence, referenced, result);
                        return request;
                    }
                }
                AsyncHostOp::Lstat { path }
            }
            AsyncHostOp::ReadDir { path, with_types } => {
                if let Some(fs) = &mut self.fs {
                    if let Some(entries) = fs.read_dir_in_memory(std::path::Path::new(&path)) {
                        self.insert_immediate(
                            request,
                            sequence,
                            referenced,
                            Ok(AsyncHostValue::DirEntries(entries)),
                        );
                        return request;
                    }
                }
                AsyncHostOp::ReadDir { path, with_types }
            }
            AsyncHostOp::Unlink { path } => {
                if let Some(fs) = &mut self.fs {
                    if fs.is_in_memory(std::path::Path::new(&path)) {
                        let _ = fs.remove_file(std::path::Path::new(&path));
                        self.insert_immediate(
                            request,
                            sequence,
                            referenced,
                            Ok(AsyncHostValue::Undefined),
                        );
                        return request;
                    }
                }
                AsyncHostOp::Unlink { path }
            }
            AsyncHostOp::Rm {
                path,
                recursive,
                force,
            } => {
                if let Some(fs) = &mut self.fs {
                    if fs.is_in_memory(std::path::Path::new(&path)) {
                        let _ = fs.remove_file(std::path::Path::new(&path));
                        self.insert_immediate(
                            request,
                            sequence,
                            referenced,
                            Ok(AsyncHostValue::Undefined),
                        );
                        return request;
                    }
                }
                AsyncHostOp::Rm {
                    path,
                    recursive,
                    force,
                }
            }
            AsyncHostOp::CopyFile { .. }
            | AsyncHostOp::Rename { .. }
            | AsyncHostOp::Realpath { .. }
            | AsyncHostOp::Readlink { .. }
            | AsyncHostOp::Access { .. }
            | AsyncHostOp::Chmod { .. }
            | AsyncHostOp::Rmdir { .. } => operation,
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
        // An immediate completion still has to reach a drive that is parked on the completion
        // signal: `server.close()` issued from inside a connection callback is exactly that.
        self.completed.notify_one();
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
            #[cfg(target_os = "linux")]
            HostResource::TcpRaw {
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
            #[cfg(target_os = "linux")]
            HostResource::TcpRaw {
                info, referenced, ..
            } => ResourceState {
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
            self.process_tasks.remove(&request);
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

    /// Give a raw accepted stream its Tokio halves before a discrete read, write or shutdown.
    #[cfg(target_os = "linux")]
    fn materialize_raw(&mut self, resource: ResourceId) {
        if !matches!(
            self.resources.get(&resource),
            Some(HostResource::TcpRaw { .. })
        ) {
            return;
        }
        let Some(HostResource::TcpRaw {
            stream,
            info,
            referenced,
        }) = self.resources.remove(&resource)
        else {
            return;
        };
        // Outside a runtime there is no reactor to register with (and Tokio would panic); the
        // operation that asked then fails as an invalid resource.
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let Ok(stream) = TcpStream::from_std(stream) else {
            return;
        };
        let (reader, writer) = stream.into_split();
        self.resources.insert(
            resource,
            HostResource::TcpStream {
                reader: Rc::new(tokio::sync::Mutex::new(reader)),
                writer: Rc::new(tokio::sync::Mutex::new(writer)),
                info,
                referenced,
                read_closed: false,
                write_closed: false,
            },
        );
    }

    fn promote_completion(&mut self, completion: HostCompletion) {
        self.process_cancellers.remove(&completion.request);
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
            #[cfg(target_os = "linux")]
            HostCompletionValue::TcpRawStream {
                stream,
                peer,
                referenced,
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
                self.resources.insert(
                    id,
                    HostResource::TcpRaw {
                        stream,
                        info: resource.clone(),
                        referenced,
                    },
                );
                AsyncHostValue::AcceptedTcpStream(AcceptedTcpStream {
                    resource,
                    peer_address: peer.ip().to_string(),
                    peer_port: peer.port(),
                })
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

    fn take_ready_completions(&mut self) -> Vec<Completion> {
        self.collect_ready_nonblocking();
        let mut completions = Vec::with_capacity(self.ready.len());
        while let Some(completion) = self.take_ready() {
            completions.push(completion);
        }
        completions
    }
}

impl AsyncHostOps for TokioHost {
    fn start(&mut self, operation: AsyncHostOp, referenced: bool) -> RequestId {
        self.start_on(operation, referenced)
    }

    fn cancel(&mut self, request: RequestId) -> bool {
        let Some(pending) = self.pending.remove(&request) else {
            return false;
        };
        if let Some(cancel) = self.process_cancellers.remove(&request) {
            let _ = cancel.send(());
        } else {
            self.tasks.abort(&request);
        }
        self.ready.remove(&pending.sequence);
        true
    }

    fn has_referenced_work(&self) -> bool {
        self.pending.values().any(|request| request.referenced)
            || self.resources.values().any(|resource| match resource {
                HostResource::TcpListener { referenced, .. }
                | HostResource::TcpStream { referenced, .. }
                | HostResource::TcpTaken { referenced, .. } => *referenced,
                #[cfg(target_os = "linux")]
                HostResource::TcpRaw { referenced, .. } => *referenced,
            })
    }

    fn next_completion(&mut self) -> Option<Completion> {
        self.collect_ready_nonblocking();
        self.take_ready()
    }
}

impl RealmHostProvider for TokioHost {
    fn drain_on_shutdown(&mut self, executor: &DriveExecutor) {
        for (_, cancel) in std::mem::take(&mut self.process_cancellers) {
            let _ = cancel.send(());
        }
        // Abort unrelated resource work first; process tasks own explicit kill-and-wait cleanup
        // and are joined deterministically below.
        for request in self.pending.keys().copied().collect::<Vec<_>>() {
            if !self.process_tasks.contains(&request) {
                self.tasks.abort(&request);
            }
        }
        while !self.process_tasks.is_empty() {
            let Some((request, _result)) = executor.block_on(self.tasks.join_next()) else {
                break;
            };
            self.process_tasks.remove(&request);
        }
        self.tasks.abort_all();
    }

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
        #[cfg(target_os = "linux")]
        self.uring.shutdown();
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

    fn ready_completions_count(&self) -> usize {
        self.ready.len()
    }

    fn take_ready_completions(&mut self) -> Vec<Completion> {
        TokioHost::take_ready_completions(self)
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
        // A connection the ring accepted goes straight onto the ring: it was never registered
        // with Tokio's reactor, so there is nothing to leave.
        #[cfg(target_os = "linux")]
        if let Some(HostResource::TcpRaw { .. }) = self.resources.get(&resource)
            && let Some(ring) = self.uring.ring()
            && let Some(HostResource::TcpRaw {
                stream,
                info,
                referenced: _,
            }) = self.resources.remove(&resource)
        {
            let _ = stream.set_nodelay(true);
            self.resources.insert(
                resource,
                HostResource::TcpTaken {
                    info,
                    referenced: false,
                },
            );
            return match ring.register(std::os::fd::OwnedFd::from(stream)) {
                Ok(stream) => Some(HostTransport(Box::new(stream))),
                Err(fd) => {
                    if tokio::runtime::Handle::try_current().is_err() {
                        return None;
                    }
                    TcpStream::from_std(std::net::TcpStream::from(fd))
                        .ok()
                        .map(|stream| HostTransport(Box::new(stream)))
                }
            };
        }
        // Both halves must be held by the table alone: a read or write still in flight owns a
        // clone and the socket cannot be reunited under it. The entry is rewritten only once
        // the socket is whole again, so a refusal leaves the table exactly as it was.
        let Some(HostResource::TcpStream {
            reader,
            writer,
            info,
            referenced,
            read_closed,
            write_closed,
        }) = self.resources.remove(&resource)
        else {
            return None;
        };
        let restore = |this: &mut Self, reader, writer| {
            this.resources.insert(
                resource,
                HostResource::TcpStream {
                    reader,
                    writer,
                    info: info.clone(),
                    referenced,
                    read_closed,
                    write_closed,
                },
            );
        };
        let reader = match Rc::try_unwrap(reader) {
            Ok(reader) => reader.into_inner(),
            Err(reader) => {
                restore(self, reader, writer);
                return None;
            }
        };
        let writer = match Rc::try_unwrap(writer) {
            Ok(writer) => writer.into_inner(),
            Err(writer) => {
                restore(self, Rc::new(tokio::sync::Mutex::new(reader)), writer);
                return None;
            }
        };
        let stream = match reader.reunite(writer) {
            Ok(stream) => {
                // Node sets TCP_NODELAY on HTTP sockets; a small response must not wait for
                // Nagle behind the previous segment's acknowledgement.
                let _ = stream.set_nodelay(true);
                stream
            }
            Err(tokio::net::tcp::ReuniteError(reader, writer)) => {
                restore(
                    self,
                    Rc::new(tokio::sync::Mutex::new(reader)),
                    Rc::new(tokio::sync::Mutex::new(writer)),
                );
                return None;
            }
        };
        // A connection the engine drives does not keep the realm alive; the listener does, as
        // with the bridged path, where the accepted socket is unreferenced on attach.
        self.resources.insert(
            resource,
            HostResource::TcpTaken {
                info,
                referenced: false,
            },
        );
        #[cfg(target_os = "linux")]
        if let Some(ring) = self.uring.ring() {
            // Leave Tokio's reactor (one EPOLL_CTL_DEL) and hand the socket to the ring.
            return match stream.into_std() {
                Ok(stream) => match ring.register(std::os::fd::OwnedFd::from(stream)) {
                    Ok(stream) => Some(HostTransport(Box::new(stream))),
                    Err(fd) => TcpStream::from_std(std::net::TcpStream::from(fd))
                        .ok()
                        .map(|stream| HostTransport(Box::new(stream))),
                },
                Err(_) => None,
            };
        }
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
                self.process_tasks.remove(&request);
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
        AsyncHostOp::AppendFile { path, data } => async {
            let mut file = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .await?;
            file.write_all(&data).await?;
            Ok::<(), std::io::Error>(())
        }
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
                    is_symlink: metadata.file_type().is_symlink(),
                })
            })
            .map_err(|error| io_error("stat", path, error)),
        AsyncHostOp::Lstat { path } => tokio::fs::symlink_metadata(&path)
            .await
            .map(|metadata| {
                AsyncHostValue::FileStat(FileStat {
                    size: metadata.len(),
                    is_file: metadata.is_file(),
                    is_directory: metadata.is_dir(),
                    is_symlink: metadata.file_type().is_symlink(),
                })
            })
            .map_err(|error| io_error("lstat", path, error)),
        AsyncHostOp::Mkdir { path, recursive } => {
            let res = if recursive {
                tokio::fs::create_dir_all(&path).await
            } else {
                tokio::fs::create_dir(&path).await
            };
            res.map(|()| AsyncHostValue::Undefined)
                .map_err(|error| io_error("mkdir", path, error))
        }
        AsyncHostOp::ReadDir {
            path,
            with_types: _,
        } => match tokio::fs::read_dir(&path).await {
            Ok(mut read_dir) => {
                let mut entries = Vec::new();
                loop {
                    match read_dir.next_entry().await {
                        Ok(Some(entry)) => {
                            let name = entry.file_name().to_string_lossy().into_owned();
                            let file_type = entry.file_type().await.ok();
                            entries.push(kiln_host_abi::DirEntry {
                                name,
                                is_file: file_type.as_ref().map_or(false, |ft| ft.is_file()),
                                is_directory: file_type.as_ref().map_or(false, |ft| ft.is_dir()),
                                is_symlink: file_type.as_ref().map_or(false, |ft| ft.is_symlink()),
                            });
                        }
                        Ok(None) => break,
                        Err(error) => return Err(io_error("scandir", path, error)),
                    }
                }
                Ok(AsyncHostValue::DirEntries(entries))
            }
            Err(error) => Err(io_error("scandir", path, error)),
        },
        AsyncHostOp::CopyFile { from, to } => tokio::fs::copy(&from, &to)
            .await
            .map(|_| AsyncHostValue::Undefined)
            .map_err(|error| io_error("copyfile", from, error)),
        AsyncHostOp::Rename { from, to } => tokio::fs::rename(&from, &to)
            .await
            .map(|()| AsyncHostValue::Undefined)
            .map_err(|error| io_error("rename", from, error)),
        AsyncHostOp::Unlink { path } => tokio::fs::remove_file(&path)
            .await
            .map(|()| AsyncHostValue::Undefined)
            .map_err(|error| io_error("unlink", path, error)),
        AsyncHostOp::Rm {
            path,
            recursive,
            force,
        } => match tokio::fs::symlink_metadata(&path).await {
            Ok(meta) => {
                let res = if meta.is_dir() {
                    if recursive {
                        tokio::fs::remove_dir_all(&path).await
                    } else {
                        tokio::fs::remove_dir(&path).await
                    }
                } else {
                    tokio::fs::remove_file(&path).await
                };
                match res {
                    Ok(()) => Ok(AsyncHostValue::Undefined),
                    Err(err) if force && err.kind() == std::io::ErrorKind::NotFound => {
                        Ok(AsyncHostValue::Undefined)
                    }
                    Err(err) => Err(io_error("rm", path, err)),
                }
            }
            Err(err) => {
                if force && err.kind() == std::io::ErrorKind::NotFound {
                    Ok(AsyncHostValue::Undefined)
                } else {
                    Err(io_error("rm", path, err))
                }
            }
        },
        AsyncHostOp::Realpath { path } => tokio::fs::canonicalize(&path)
            .await
            .map(|p| AsyncHostValue::String(p.to_string_lossy().into_owned()))
            .map_err(|error| io_error("realpath", path, error)),
        AsyncHostOp::Readlink { path } => tokio::fs::read_link(&path)
            .await
            .map(|p| AsyncHostValue::String(p.to_string_lossy().into_owned()))
            .map_err(|error| io_error("readlink", path, error)),
        AsyncHostOp::Access { path, mode: _ } => match tokio::fs::metadata(&path).await {
            Ok(_) => Ok(AsyncHostValue::Undefined),
            Err(error) => Err(io_error("access", path, error)),
        },
        AsyncHostOp::Chmod { path, mode } => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
                    .await
                    .map(|()| AsyncHostValue::Undefined)
                    .map_err(|error| io_error("chmod", path, error))
            }
            #[cfg(not(unix))]
            {
                let _ = (path, mode);
                Ok(AsyncHostValue::Undefined)
            }
        }
        AsyncHostOp::Rmdir { path } => tokio::fs::remove_dir(&path)
            .await
            .map(|()| AsyncHostValue::Undefined)
            .map_err(|error| io_error("rmdir", path, error)),
        AsyncHostOp::StdinRead { max_bytes } => {
            let mut bytes = vec![0; max_bytes];
            let length = tokio::io::stdin()
                .read(&mut bytes)
                .await
                .map_err(|error| io_error("read", "stdin".to_string(), error))?;
            bytes.truncate(length);
            Ok(AsyncHostValue::Bytes(bytes))
        }
        AsyncHostOp::ProcessRun { .. } => {
            unreachable!("process operations are handled by start_on")
        }
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

async fn execute_process(
    command: String,
    args: Vec<String>,
    stdin: Option<Vec<u8>>,
    mut cancelled: oneshot::Receiver<()>,
) -> Result<AsyncHostValue, AsyncHostError> {
    use std::process::Stdio;
    let mut process = tokio::process::Command::new(&command);
    process
        .args(&args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = process
        .spawn()
        .map_err(|error| io_error("spawn", command.clone(), error))?;
    let mut child_stdout = child.stdout.take().ok_or_else(|| {
        AsyncHostError::new(
            "EIO",
            "spawn",
            Some(command.clone()),
            "child stdout was not piped",
        )
    })?;
    let mut child_stderr = child.stderr.take().ok_or_else(|| {
        AsyncHostError::new(
            "EIO",
            "spawn",
            Some(command.clone()),
            "child stderr was not piped",
        )
    })?;
    let child_stdin = child.stdin.take();
    let stdout_task = tokio::task::spawn_local(async move {
        let mut bytes = Vec::new();
        child_stdout.read_to_end(&mut bytes).await.map(|_| bytes)
    });
    let stderr_task = tokio::task::spawn_local(async move {
        let mut bytes = Vec::new();
        child_stderr.read_to_end(&mut bytes).await.map(|_| bytes)
    });
    let stdin_task = tokio::task::spawn_local(async move {
        if let (Some(data), Some(mut pipe)) = (stdin, child_stdin) {
            pipe.write_all(&data).await?;
        }
        Ok::<(), std::io::Error>(())
    });
    let status = tokio::select! {
        status = child.wait() => status,
        _ = &mut cancelled => {
            // Cancellation owns the child until wait completes. This explicit kill-and-wait
            // path reaps the process instead of relying on JoinMap task abort/drop semantics.
            let _ = child.start_kill();
            child.wait().await
        }
    };
    let (stdout, stderr, stdin_result) = tokio::join!(stdout_task, stderr_task, stdin_task);
    let stdout = stdout
        .map_err(|error| {
            io_error(
                "read",
                format!("{command}:stdout"),
                std::io::Error::other(error.to_string()),
            )
        })?
        .map_err(|error| io_error("read", format!("{command}:stdout"), error))?;
    let stderr = stderr
        .map_err(|error| {
            io_error(
                "read",
                format!("{command}:stderr"),
                std::io::Error::other(error.to_string()),
            )
        })?
        .map_err(|error| io_error("read", format!("{command}:stderr"), error))?;
    let _stdin_result = stdin_result
        .map_err(|error| {
            io_error(
                "write",
                format!("{command}:stdin"),
                std::io::Error::other(error.to_string()),
            )
        })?
        .map_err(|error| io_error("write", format!("{command}:stdin"), error))?;
    let status = status.map_err(|error| io_error("wait", command.clone(), error))?;
    #[cfg(unix)]
    let signal = std::os::unix::process::ExitStatusExt::signal(&status);
    #[cfg(not(unix))]
    let signal = None;
    Ok(AsyncHostValue::ProcessOutput(ProcessOutput {
        stdout,
        stderr,
        exit_code: status.code(),
        signal,
    }))
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

fn bind_tcp_listener_std(host: &str, port: u16) -> std::io::Result<std::net::TcpListener> {
    use std::net::ToSocketAddrs;
    use std::os::fd::FromRawFd;

    let address = (host, port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid address"))?;

    let (domain, sockaddr_storage, socklen) = match address {
        std::net::SocketAddr::V4(v4) => {
            let mut sin: libc::sockaddr_in = unsafe { std::mem::zeroed() };
            sin.sin_family = libc::AF_INET as libc::sa_family_t;
            sin.sin_port = v4.port().to_be();
            sin.sin_addr.s_addr = u32::from_ne_bytes(v4.ip().octets());
            let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
            unsafe {
                std::ptr::copy_nonoverlapping(
                    &sin as *const _ as *const u8,
                    &mut storage as *mut _ as *mut u8,
                    std::mem::size_of::<libc::sockaddr_in>(),
                );
            }
            (
                libc::AF_INET,
                storage,
                std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
            )
        }
        std::net::SocketAddr::V6(v6) => {
            let mut sin6: libc::sockaddr_in6 = unsafe { std::mem::zeroed() };
            sin6.sin6_family = libc::AF_INET6 as libc::sa_family_t;
            sin6.sin6_port = v6.port().to_be();
            sin6.sin6_addr.s6_addr = v6.ip().octets();
            let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
            unsafe {
                std::ptr::copy_nonoverlapping(
                    &sin6 as *const _ as *const u8,
                    &mut storage as *mut _ as *mut u8,
                    std::mem::size_of::<libc::sockaddr_in6>(),
                );
            }
            (
                libc::AF_INET6,
                storage,
                std::mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t,
            )
        }
    };

    let fd = unsafe {
        // `SOCK_NONBLOCK` and `SOCK_CLOEXEC` are Linux extensions to `socket(2)`; the BSDs and
        // macOS have neither, so the two flags are set afterwards with `fcntl`. Doing it in two
        // steps races only with a `fork` in another thread, and the only fork this process makes
        // is the shipped launcher's, which happens before any realm exists and so before any
        // listener does.
        let fd = libc::socket(domain, libc::SOCK_STREAM, 0);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK) < 0
            || libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) < 0
        {
            let err = std::io::Error::last_os_error();
            libc::close(fd);
            return Err(err);
        }
        let optval: libc::c_int = 1;
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_REUSEADDR,
            &optval as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        );
        #[cfg(all(unix, not(target_os = "solaris"), not(target_os = "illumos")))]
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_REUSEPORT,
            &optval as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        );
        let sockaddr_ptr = &sockaddr_storage as *const _ as *const libc::sockaddr;
        if libc::bind(fd, sockaddr_ptr, socklen) < 0 {
            let err = std::io::Error::last_os_error();
            libc::close(fd);
            return Err(err);
        }
        if libc::listen(fd, 1024) < 0 {
            let err = std::io::Error::last_os_error();
            libc::close(fd);
            return Err(err);
        }
        fd
    };

    Ok(unsafe { std::net::TcpListener::from_raw_fd(fd) })
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

    /// The same reading in ECMAScript milliseconds.
    pub fn now_milliseconds(&self) -> f64 {
        epoch_milliseconds(self.now())
    }

    /// Moves the clock forward to `now` when that is later than it reads. A wall-clock realm
    /// (ADR 0044) does this at every turn boundary so timers armed by the callbacks that follow
    /// measure from the present; the virtual clock never calls it, and advances only when a
    /// timer runs.
    pub fn advance_to(&mut self, now: Duration) {
        self.now = self.now.max(now);
    }

    /// Whether any next-tick or microtask is queued.
    pub fn has_queued_work(&self) -> bool {
        !self.next_ticks.is_empty() || !self.microtasks.is_empty()
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

/// Where [`CaptureHost`]'s clock starts: 2020-01-01T00:00:00Z, obviously synthetic and stable.
const CAPTURE_HOST_CLOCK_START_MS: f64 = 1_577_836_800_000.0;
/// How far it moves per read. One millisecond keeps ordinary elapsed-time code sane.
const CAPTURE_HOST_CLOCK_STEP_MS: f64 = 1.0;

/// A deterministic host used by the interpreter, tests, and CLI execution mode.
pub struct CaptureHost {
    stdout: String,
    /* Separate from `stdout`, because which stream a line went to is observable: a caller that
     * redirects one and not the other sees the difference, and node puts `console.error` on this
     * one. Kept as its own buffer rather than interleaved so a test can assert either alone. */
    stderr: String,
    /// Epoch milliseconds this host will answer `ClockNow` with next.
    ///
    /// A deterministic host still has to *have* a clock. `--node` asks for
    /// [`ClockCapability::Host`], because a Node program expects real time (ADR 0044), and a host
    /// that refuses the question leaves the realm asking for time from something that has none --
    /// which is how `fastify` and `express` came to be unrunnable here: any real library
    /// timestamps something.
    ///
    /// Fixed start, fixed step, so a run is reproducible and two reads never return the same
    /// value. Monotonic rather than constant because code that waits for the clock to move is
    /// ordinary, and against a constant clock it would wait forever.
    clock_ms: f64,
    fs: Option<Box<dyn kiln_host_abi::FileSystemOps>>,
}

impl std::fmt::Debug for CaptureHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaptureHost")
            .field("stdout", &self.stdout)
            .field("stderr", &self.stderr)
            .field("clock_ms", &self.clock_ms)
            .field("has_fs", &self.fs.is_some())
            .finish()
    }
}

impl Default for CaptureHost {
    fn default() -> Self {
        Self {
            stdout: String::new(),
            stderr: String::new(),
            clock_ms: CAPTURE_HOST_CLOCK_START_MS,
            fs: None,
        }
    }
}

impl CaptureHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_fs(mut self, fs: Box<dyn kiln_host_abi::FileSystemOps>) -> Self {
        self.fs = Some(fs);
        self
    }

    pub fn set_fs(&mut self, fs: Box<dyn kiln_host_abi::FileSystemOps>) {
        self.fs = Some(fs);
    }

    pub fn stdout(&self) -> &str {
        &self.stdout
    }

    pub fn into_stdout(self) -> String {
        self.stdout
    }

    pub fn stderr(&self) -> &str {
        &self.stderr
    }

    pub fn into_stderr(self) -> String {
        self.stderr
    }

    pub fn clear(&mut self) {
        self.stdout.clear();
        self.stderr.clear();
    }
}

impl HostOps for CaptureHost {
    fn call(&mut self, operation: HostOp, arguments: &[HostValue]) -> Result<HostValue, HostError> {
        match operation {
            HostOp::WriteStdout | HostOp::WriteStderr => {
                let sink = if operation == HostOp::WriteStdout {
                    &mut self.stdout
                } else {
                    &mut self.stderr
                };
                if !arguments.is_empty() {
                    if !sink.is_empty() && !sink.ends_with('\n') {
                        sink.push('\n');
                    }
                    for (index, argument) in arguments.iter().enumerate() {
                        if index > 0 {
                            sink.push(' ');
                        }
                        sink.push_str(&argument.to_console_string());
                    }
                }
                sink.push('\n');
                Ok(HostValue::Undefined)
            }
            HostOp::ClockNow => {
                let now = self.clock_ms;
                self.clock_ms += CAPTURE_HOST_CLOCK_STEP_MS;
                Ok(HostValue::number(now))
            }
            HostOp::TransferRegion => {
                let ptr = arguments.first().cloned().unwrap_or(HostValue::Undefined);
                Ok(ptr)
            }
            HostOp::EntropyU64 | HostOp::TimeZone | HostOp::Locale => Err(HostError::new(format!(
                "deterministic host does not provide {operation:?}"
            ))),
        }
    }

    fn fs(&mut self) -> Option<&mut (dyn kiln_host_abi::FileSystemOps + '_)> {
        self.fs
            .as_mut()
            .map(|b| &mut **b as &mut (dyn kiln_host_abi::FileSystemOps + '_))
    }

    fn clone_fs(&self) -> Option<Box<dyn kiln_host_abi::FileSystemOps>> {
        self.fs.as_ref().and_then(|f| f.clone_box())
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
    #[cfg(unix)]
    #[test]
    fn tokio_host_runs_process_with_bidirectional_pipes_and_preserves_status() {
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        let request = runtime.start_host_op(
            AsyncHostOp::ProcessRun {
                command: "sh".into(),
                args: vec![
                    "-c".into(),
                    "head -c 131072 /dev/zero & cat >/dev/null; wait; printf stderr >&2".into(),
                ],
                stdin: Some(vec![b'x'; 131072]),
            },
            false,
        );
        let completion = next_host_completion(&mut runtime).expect("process completes");
        assert_eq!(completion.request, request);
        let AsyncHostValue::ProcessOutput(output) = completion.result.expect("process succeeds")
        else {
            panic!("expected process output")
        };
        assert_eq!(output.stdout.len(), 131072);
        assert_eq!(output.stderr, b"stderr");
        assert_eq!(output.exit_code, Some(0));
        assert_eq!(output.signal, None);
    }

    #[cfg(unix)]
    #[test]
    fn tokio_host_process_result_preserves_signal_termination() {
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        runtime.start_host_op(
            AsyncHostOp::ProcessRun {
                command: "sh".into(),
                args: vec!["-c".into(), "kill -TERM $$".into()],
                stdin: None,
            },
            false,
        );
        let completion = next_host_completion(&mut runtime).expect("process completes");
        let AsyncHostValue::ProcessOutput(output) = completion.result.expect("process result")
        else {
            panic!("expected process output")
        };
        assert_eq!(output.exit_code, None);
        assert_eq!(output.signal, Some(libc::SIGTERM));
    }

    #[cfg(unix)]
    #[test]
    fn tokio_host_process_reports_nonzero_exit_and_missing_command() {
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        runtime.start_host_op(
            AsyncHostOp::ProcessRun {
                command: "sh".into(),
                args: vec!["-c".into(), "printf failed >&2; exit 7".into()],
                stdin: None,
            },
            false,
        );
        let completion = next_host_completion(&mut runtime).expect("process completes");
        let AsyncHostValue::ProcessOutput(output) = completion.result.expect("process result")
        else {
            panic!("expected process output")
        };
        assert_eq!(output.exit_code, Some(7));
        assert_eq!(output.stderr, b"failed");

        runtime.start_host_op(
            AsyncHostOp::ProcessRun {
                command: "kiln-process-command-that-does-not-exist".into(),
                args: Vec::new(),
                stdin: None,
            },
            false,
        );
        let completion = next_host_completion(&mut runtime).expect("spawn failure completes");
        let error = completion.result.expect_err("missing command fails");
        assert_eq!(error.syscall, "spawn");
    }

    #[cfg(unix)]
    #[test]
    fn tokio_host_process_cancellation_kills_and_reaps_the_child() {
        use std::process::Command;
        use std::thread;

        let directory = TestDirectory::new("process-cancel");
        let marker = directory.join("pid");
        let marker_string = marker.to_string_lossy().into_owned();
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        let request = runtime.start_host_op(
            AsyncHostOp::ProcessRun {
                command: "sh".into(),
                args: vec![
                    "-c".into(),
                    "echo \"$$\" > \"$1\"; exec sleep 30".into(),
                    "kiln-process-test".into(),
                    marker_string.clone(),
                ],
                stdin: None,
            },
            true,
        );
        let executor = runtime.drive_executor();
        let mut pid = None;
        for _ in 0..100 {
            executor.block_on(yield_to_tasks());
            if let Ok(contents) = fs::read_to_string(&marker) {
                pid = Some(contents.trim().parse::<i32>().expect("child PID"));
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        let pid = pid.expect("child published its PID");
        assert!(AsyncHostOps::cancel(runtime.host(), request));

        for _ in 0..100 {
            executor.block_on(yield_to_tasks());
            let status = Command::new("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .expect("ps is available");
            if status.stdout.iter().all(u8::is_ascii_whitespace) {
                return;
            }
            assert!(
                status.status.success(),
                "ps failed while inspecting live child: {:?}: {}",
                status.status,
                String::from_utf8_lossy(&status.stderr)
            );
            thread::sleep(Duration::from_millis(5));
        }
        panic!("cancelled child PID {pid} was not reaped");
    }

    #[cfg(unix)]
    #[test]
    fn tokio_host_shutdown_kills_and_reaps_a_pending_process() {
        use std::process::Command;
        use std::thread;

        let directory = TestDirectory::new("process-shutdown");
        let marker = directory.join("pid");
        let marker_string = marker.to_string_lossy().into_owned();
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        runtime.start_host_op(
            AsyncHostOp::ProcessRun {
                command: "sh".into(),
                args: vec![
                    "-c".into(),
                    "echo \"$$\" > \"$1\"; exec sleep 30".into(),
                    "kiln-process-test".into(),
                    marker_string,
                ],
                stdin: None,
            },
            true,
        );
        let mut pid = None;
        for _ in 0..100 {
            runtime.drive_executor().block_on(yield_to_tasks());
            if let Ok(contents) = fs::read_to_string(&marker) {
                pid = Some(contents.trim().parse::<i32>().expect("child PID"));
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        let pid = pid.expect("child published its PID");
        assert!(runtime.shutdown());
        for _ in 0..100 {
            let status = Command::new("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .expect("ps is available");
            if status.stdout.iter().all(u8::is_ascii_whitespace) {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("shutdown child PID {pid} was not reaped");
    }

    #[cfg(unix)]
    #[test]
    fn tokio_host_shutdown_drains_a_process_after_last_op_cancellation() {
        use std::process::Command;
        use std::thread;

        let directory = TestDirectory::new("process-cancel-shutdown");
        let marker = directory.join("pid");
        let marker_string = marker.to_string_lossy().into_owned();
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        let request = runtime.start_host_op(
            AsyncHostOp::ProcessRun {
                command: "sh".into(),
                args: vec![
                    "-c".into(),
                    "echo \"$$\" > \"$1\"; exec sleep 30".into(),
                    "kiln-process-test".into(),
                    marker_string,
                ],
                stdin: None,
            },
            true,
        );
        let mut pid = None;
        for _ in 0..100 {
            runtime.drive_executor().block_on(yield_to_tasks());
            if let Ok(contents) = fs::read_to_string(&marker) {
                pid = Some(contents.trim().parse::<i32>().expect("child PID"));
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        let pid = pid.expect("child published its PID");
        assert!(AsyncHostOps::cancel(runtime.host(), request));
        assert!(runtime.shutdown());
        for _ in 0..100 {
            let status = Command::new("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .expect("ps is available");
            if status.stdout.iter().all(u8::is_ascii_whitespace) {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("cancelled shutdown child PID {pid} was not reaped");
    }

    /// A clock reading keeps the part of it that is shorter than a millisecond.
    ///
    /// The native FFI once spelled this conversion `as_millis() as f64`, which truncates, while
    /// the interpreter spelled it `as_secs_f64() * 1_000.0`. So `performance.now()` answered
    /// whole milliseconds under one backend and sub-milliseconds under the other -- one call,
    /// two precisions, chosen by which engine ran it. It made the native clock unable to measure
    /// anything shorter than a millisecond, which is most of what a benchmark measures, and it
    /// is the kind of two-engine divergence ADR 0043 exists to forbid.
    #[test]
    fn a_clock_reading_keeps_its_sub_millisecond_part() {
        let reading = super::Duration::from_nanos(1_500_250);
        let milliseconds = super::epoch_milliseconds(reading);
        assert!(
            (milliseconds - 1.500_25).abs() < 1e-9,
            "expected 1.50025 ms, got {milliseconds}"
        );
        assert!(
            milliseconds.fract() != 0.0,
            "resolution collapsed to whole milliseconds"
        );
    }

    use super::*;

    fn next_host_completion<H: RealmHostProvider>(
        runtime: &mut RealmRuntime<H>,
    ) -> Option<Completion> {
        runtime
            .next_host_completion()
            .expect("realm runtime remains healthy")
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
                is_symlink: false,
            }))
        );
    }

    #[test]
    fn tokio_host_handles_large_file_reads_and_writes() {
        let directory = TestDirectory::new("async-large-files");
        let path = directory.join("large.bin");
        let path_str = path.to_string_lossy().into_owned();
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");

        // Write > 64 KiB (e.g. 100 KiB) to exercise thread pool write fallback
        let large_data = vec![42u8; 100 * 1024];
        let write = runtime.start_host_op(
            AsyncHostOp::WriteFile {
                path: path_str.clone(),
                data: large_data.clone(),
            },
            true,
        );
        assert_eq!(
            next_host_completion(&mut runtime)
                .expect("large write completes")
                .request,
            write
        );

        // Read > 128 KiB: write a 200 KiB file directly and read it via start_host_op
        let read_path = directory.join("read_large.bin");
        let read_path_str = read_path.to_string_lossy().into_owned();
        let read_data = vec![7u8; 200 * 1024];
        std::fs::write(&read_path, &read_data).expect("seed large file");

        let read = runtime.start_host_op(
            AsyncHostOp::ReadFile {
                path: read_path_str,
            },
            true,
        );
        let completion = next_host_completion(&mut runtime).expect("large read completes");
        assert_eq!(completion.request, read);
        assert_eq!(completion.result, Ok(AsyncHostValue::Bytes(read_data)));
    }

    #[test]
    fn tokio_host_executes_extended_filesystem_operations() {
        let directory = TestDirectory::new("async-extended-fs");
        let dir_str = directory.0.to_string_lossy().into_owned();
        let file_a = directory.join("file_a.txt");
        let file_b = directory.join("file_b.txt");
        let file_c = directory.join("file_c.txt");
        let file_a_str = file_a.to_string_lossy().into_owned();
        let file_b_str = file_b.to_string_lossy().into_owned();
        let file_c_str = file_c.to_string_lossy().into_owned();

        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");

        // Write file_a
        runtime.start_host_op(
            AsyncHostOp::WriteFile {
                path: file_a_str.clone(),
                data: b"hello world".to_vec(),
            },
            true,
        );
        let _ = next_host_completion(&mut runtime).expect("write completes");

        // Copy file_a -> file_b
        runtime.start_host_op(
            AsyncHostOp::CopyFile {
                from: file_a_str.clone(),
                to: file_b_str.clone(),
            },
            true,
        );
        let res = next_host_completion(&mut runtime)
            .expect("copy completes")
            .result;
        assert_eq!(res, Ok(AsyncHostValue::Undefined));

        // Rename file_b -> file_c
        runtime.start_host_op(
            AsyncHostOp::Rename {
                from: file_b_str.clone(),
                to: file_c_str.clone(),
            },
            true,
        );
        let res = next_host_completion(&mut runtime)
            .expect("rename completes")
            .result;
        assert_eq!(res, Ok(AsyncHostValue::Undefined));

        // ReadDir
        runtime.start_host_op(
            AsyncHostOp::ReadDir {
                path: dir_str.clone(),
                with_types: true,
            },
            true,
        );
        let res = next_host_completion(&mut runtime)
            .expect("readdir completes")
            .result;
        match res {
            Ok(AsyncHostValue::DirEntries(entries)) => {
                let names: Vec<String> = entries.into_iter().map(|e| e.name).collect();
                assert!(names.contains(&"file_a.txt".to_string()));
                assert!(names.contains(&"file_c.txt".to_string()));
                assert!(!names.contains(&"file_b.txt".to_string()));
            }
            other => panic!("expected DirEntries, got {other:?}"),
        }

        // Lstat file_a
        runtime.start_host_op(
            AsyncHostOp::Lstat {
                path: file_a_str.clone(),
            },
            true,
        );
        let res = next_host_completion(&mut runtime)
            .expect("lstat completes")
            .result;
        assert_eq!(
            res,
            Ok(AsyncHostValue::FileStat(FileStat {
                size: 11,
                is_file: true,
                is_directory: false,
                is_symlink: false,
            }))
        );

        // Unlink file_a
        runtime.start_host_op(
            AsyncHostOp::Unlink {
                path: file_a_str.clone(),
            },
            true,
        );
        let res = next_host_completion(&mut runtime)
            .expect("unlink completes")
            .result;
        assert_eq!(res, Ok(AsyncHostValue::Undefined));

        // Rm file_c
        runtime.start_host_op(
            AsyncHostOp::Rm {
                path: file_c_str.clone(),
                recursive: false,
                force: false,
            },
            true,
        );
        let res = next_host_completion(&mut runtime)
            .expect("rm completes")
            .result;
        assert_eq!(res, Ok(AsyncHostValue::Undefined));
    }

    #[test]
    fn tokio_host_fast_fails_missing_files_and_stats_on_realm_thread() {
        let mut runtime = RealmRuntime::new().expect("Tokio runtime builds");
        let missing_path = "/path/to/definitely/nonexistent/file.bin".to_string();

        let stat_req = runtime.start_host_op(
            AsyncHostOp::Stat {
                path: missing_path.clone(),
            },
            true,
        );
        let stat_comp = next_host_completion(&mut runtime).expect("stat completes");
        assert_eq!(stat_comp.request, stat_req);
        let stat_err = stat_comp.result.expect_err("missing file stat fails");
        assert_eq!(stat_err.code, "ENOENT");
        assert_eq!(stat_err.syscall, "stat");

        let read_req = runtime.start_host_op(AsyncHostOp::ReadFile { path: missing_path }, true);
        let read_comp = next_host_completion(&mut runtime).expect("read completes");
        assert_eq!(read_comp.request, read_req);
        let read_err = read_comp.result.expect_err("missing file read fails");
        assert_eq!(read_err.code, "ENOENT");
        assert_eq!(read_err.syscall, "open");
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
            RuntimeTurn::Protocol => panic!("no HTTP engine queued anything here"),
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
            RuntimeTurn::Protocol => panic!("no HTTP engine queued anything here"),
        };
        let second_completion = match runtime.next_turn(&mut realm).expect("second completion") {
            RuntimeTurn::Completion(completion) => completion,
            RuntimeTurn::Timer(_) => panic!("a ready completion must beat a due timer"),
            RuntimeTurn::Failure(failure) => panic!("realm failed unexpectedly: {failure}"),
            RuntimeTurn::Protocol => panic!("no HTTP engine queued anything here"),
        };
        assert_eq!(first_completion.request, first);
        assert_eq!(second_completion.request, second);
        assert!(first_completion.sequence < second_completion.sequence);
        assert!(runtime.host_mut().is_exhausted());
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

    /// With the io_uring backend a listener is the ring's, its connections arrive raw, and a
    /// discrete read or write gives one its Tokio halves on demand.
    #[cfg(target_os = "linux")]
    #[test]
    fn uring_listener_accepts_raw_streams_that_serve_discrete_io() {
        let mut host = TokioHost::new();
        host.set_io_preference(IoPreference::Uring);
        let mut runtime = RealmRuntime::with_host(host).expect("Tokio runtime builds");
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
        let on_ring = matches!(
            runtime.host().resources.get(&listener.id),
            Some(HostResource::TcpListener { listener, .. })
                if matches!(**listener, HostListener::Ring(_))
        );
        if !on_ring {
            eprintln!("skipped: io_uring unavailable, the listener fell back to epoll");
            return;
        }
        let port = listener.port.expect("port");
        let accept = runtime.start_host_op(
            AsyncHostOp::TcpAccept {
                listener: listener.id,
            },
            true,
        );
        let mut peer = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let accepted = next_host_completion(&mut runtime).expect("accept completes");
        assert_eq!(accepted.request, accept);
        let AsyncHostValue::AcceptedTcpStream(accepted) = accepted.result.expect("accepted") else {
            panic!("expected an accepted stream");
        };
        assert_eq!(accepted.peer_address, "127.0.0.1");
        assert_eq!(accepted.peer_port, peer.local_addr().unwrap().port());
        assert_eq!(accepted.resource.port, Some(port));
        assert!(matches!(
            runtime.host().resources.get(&accepted.resource.id),
            Some(HostResource::TcpRaw { .. })
        ));
        std::io::Write::write_all(&mut peer, b"raw bytes").unwrap();
        runtime.start_host_op(
            AsyncHostOp::TcpRead {
                stream: accepted.resource.id,
                max_bytes: 64,
            },
            true,
        );
        assert_eq!(
            next_host_completion(&mut runtime).expect("read").result,
            Ok(AsyncHostValue::Bytes(b"raw bytes".to_vec()))
        );
        assert!(matches!(
            runtime.host().resources.get(&accepted.resource.id),
            Some(HostResource::TcpStream { .. })
        ));
        runtime.start_host_op(
            AsyncHostOp::TcpWrite {
                stream: accepted.resource.id,
                data: b"back".to_vec(),
            },
            true,
        );
        next_host_completion(&mut runtime).expect("write");
        let mut back = [0_u8; 4];
        std::io::Read::read_exact(&mut peer, &mut back).unwrap();
        assert_eq!(&back, b"back");
        for resource in [accepted.resource.id, listener.id] {
            runtime.start_host_op(AsyncHostOp::CloseResource { resource }, true);
            next_host_completion(&mut runtime).expect("close");
        }
        // The listener is really closed once the ring cancels its accept.
        runtime.drive_executor().block_on(async {
            tokio::time::sleep(Duration::from_millis(20)).await;
        });
        assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err());
        assert!(runtime.shutdown());
    }

    #[test]
    fn tcp_listeners_support_reuseport_across_runtimes() {
        let mut runtime1 = RealmRuntime::new().expect("runtime 1");
        let mut runtime2 = RealmRuntime::new().expect("runtime 2");
        runtime1.start_host_op(
            AsyncHostOp::TcpListen {
                host: "127.0.0.1".into(),
                port: 0,
            },
            true,
        );
        let listener1 = match next_host_completion(&mut runtime1)
            .expect("listen 1 completes")
            .result
            .expect("listen 1 succeeds")
        {
            AsyncHostValue::Resource(resource) => resource,
            other => panic!("expected listener resource, got {other:?}"),
        };
        let port = listener1.port.expect("port");

        // runtime2 listens on the exact same port thanks to SO_REUSEPORT
        runtime2.start_host_op(
            AsyncHostOp::TcpListen {
                host: "127.0.0.1".into(),
                port,
            },
            true,
        );
        let listener2 = match next_host_completion(&mut runtime2)
            .expect("listen 2 completes")
            .result
            .expect("listen 2 succeeds with reuseport")
        {
            AsyncHostValue::Resource(resource) => resource,
            other => panic!("expected listener resource, got {other:?}"),
        };
        assert_eq!(listener2.port, Some(port));
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
            body: bytes::Bytes::from(request.body),
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
            RuntimeTurn::Protocol => panic!("no HTTP engine queued anything here"),
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

    // ------------------------------------------------------------------------------------
    // HTTP through the engines (ADR 0046): the host hands over a transport, the engine's task
    // drives Hyper over it, and the queued adapter parks events for these tests to drain.
    // A scripted run is "these bytes arrive, these bytes must leave".

    fn scripted_http_runtime(
        resource: ResourceId,
        inbound: &[&[u8]],
        peer_closes: bool,
    ) -> (RealmRuntime<ScriptedHost>, ScriptedTransportLog) {
        scripted_runtime_with(
            resource,
            ScriptedTransport::new(inbound.iter().map(|chunk| chunk.to_vec()), peer_closes),
        )
    }

    fn scripted_runtime_with(
        resource: ResourceId,
        transport: ScriptedTransport,
    ) -> (RealmRuntime<ScriptedHost>, ScriptedTransportLog) {
        let log = transport.log();
        let host = ScriptedHost::new([]).with_transport(resource, transport);
        let runtime = RealmRuntime::with_host(host).expect("scripted runtime builds");
        (runtime, log)
    }

    /// Let the runtime's real turn selector drive engine tasks and collect what they queued.
    fn drain_server_events<H: RealmHostProvider>(
        runtime: &mut RealmRuntime<H>,
        resource: ResourceId,
        done: impl Fn(&[RuntimeHttpEvent]) -> bool,
    ) -> Vec<RuntimeHttpEvent> {
        let mut events = Vec::new();
        let mut realm: Realm = Realm::new();
        loop {
            if done(&events) {
                break;
            }
            assert!(
                runtime.next_referenced_turn(&mut realm).is_some(),
                "HTTP server became idle before its requested event: {events:?} (host={}, http={})",
                runtime.has_referenced_host_work(),
                runtime.has_live_http_work()
            );
            events.extend(runtime.take_queued_server_events(resource));
        }
        events
    }

    fn drain_client_events<H: RealmHostProvider>(
        runtime: &mut RealmRuntime<H>,
        resource: ResourceId,
        done: impl Fn(&[RuntimeHttpClientEvent]) -> bool,
    ) -> Vec<RuntimeHttpClientEvent> {
        let mut events = Vec::new();
        let mut realm: Realm = Realm::new();
        loop {
            if done(&events) {
                break;
            }
            assert!(
                runtime.next_referenced_turn(&mut realm).is_some(),
                "HTTP client became idle before its requested event: {events:?}"
            );
            events.extend(runtime.take_queued_client_events(resource));
        }
        events
    }

    fn flat(headers: &[(&str, &str)]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for (name, value) in headers {
            bytes.extend_from_slice(name.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(value.as_bytes());
            bytes.push(0);
        }
        bytes
    }

    #[test]
    fn scripted_provider_drains_response_before_same_turn_server_close() {
        let resource = ResourceId::new(45);
        let (mut runtime, log) = scripted_http_runtime(
            resource,
            &[b"GET /same-turn HTTP/1.1\r\nHost: kiln\r\nConnection: close\r\n\r\n"],
            false,
        );
        assert!(runtime.server_attach_queued(resource, false));
        assert!(runtime.http_owns(resource));
        let events = drain_server_events(&mut runtime, resource, |events| {
            events
                .iter()
                .any(|event| matches!(event, RuntimeHttpEvent::RequestEnd { .. }))
        });
        let (id, request, body) = request_events(&events);
        assert_eq!(request.target, "/same-turn");
        assert!(!request.keep_alive);
        assert!(body.is_empty());
        assert_eq!(runtime.requests_in_flight(), 1);

        // This is the callback's ordering: response.end() commits the slot, then server.close()
        // runs before Tokio gets another poll of the connection task.
        assert!(runtime.http_server().start_response(
            resource,
            id,
            200,
            b"",
            node_stream::DEFAULT_HIGH_WATER_MARK
        ));
        assert!(
            runtime
                .http_server()
                .end_response(resource, id, b"same-turn")
                .expect("the response ends")
                .ready
        );
        assert!(runtime.http_close(resource, HttpCloseMode::Graceful));
        assert!(
            log.written().is_empty(),
            "nothing reaches the transport inside the caller"
        );

        let events = drain_server_events(&mut runtime, resource, |events| {
            events.contains(&RuntimeHttpEvent::Closed)
        });
        assert_eq!(
            events,
            vec![RuntimeHttpEvent::Finish { id }, RuntimeHttpEvent::Closed],
            "the response finishes before the connection closes"
        );
        assert_eq!(
            String::from_utf8(log.written()).expect("HTTP is ASCII"),
            "HTTP/1.1 200 OK\r\ncontent-length: 9\r\nconnection: close\r\n\r\nsame-turn"
        );
        assert!(
            log.is_dropped(),
            "a closed connection lets its transport go"
        );
        assert!(!runtime.http_owns(resource));
        assert_eq!(runtime.requests_in_flight(), 0);
        assert!(!runtime.has_live_http_work());
        assert!(runtime.retire_http_connection(resource));
    }

    #[test]
    fn queued_admission_wakes_once_then_idle_http_does_not_starve_timer() {
        let resource = ResourceId::new(48);
        let (mut runtime, _log) = scripted_http_runtime(resource, &[], false);
        assert!(runtime.server_attach_queued(resource, false));
        let mut realm: Realm = Realm::new();
        realm.set_timeout(Duration::ZERO, WorkId::new(480));
        assert!(matches!(
            runtime.next_referenced_turn(&mut realm),
            Some(RuntimeTurn::Protocol)
        ));
        assert!(matches!(
            runtime.next_referenced_turn(&mut realm),
            Some(RuntimeTurn::Timer(WorkItem::Timer { work, .. })) if work == WorkId::new(480)
        ));
        assert!(
            runtime.next_referenced_turn(&mut realm).is_none(),
            "an admitted but idle HTTP connection must not keep a realm alive"
        );
        assert!(runtime.shutdown());
    }

    #[test]
    fn an_abort_publishes_closed_from_a_task_and_never_inside_the_caller() {
        let resource = ResourceId::new(46);
        let (mut runtime, log) = scripted_http_runtime(resource, &[b"GET /"], false);
        assert!(runtime.server_attach_queued(resource, false));
        let mut realm: Realm = Realm::new();
        let _ = runtime.next_referenced_turn(&mut realm);
        assert!(runtime.http_close(resource, HttpCloseMode::Abort));
        assert!(
            runtime.take_queued_server_events(resource).is_empty(),
            "the caller does not hear Closed from inside its own close"
        );
        assert!(
            runtime.has_live_http_work(),
            "the abort counts as work until the language has heard it"
        );
        let events = drain_server_events(&mut runtime, resource, |events| {
            events.contains(&RuntimeHttpEvent::Closed)
        });
        assert_eq!(events, vec![RuntimeHttpEvent::Closed]);
        assert!(log.is_dropped());
        assert!(!runtime.http_owns(resource));
        assert!(!runtime.has_live_http_work());
    }

    #[test]
    fn realm_shutdown_aborts_engine_connections_and_rejects_new_admission() {
        let resource = ResourceId::new(47);
        let (mut runtime, log) =
            scripted_http_runtime(resource, &[b"GET / HTTP/1.1\r\nHost: kiln\r\n\r\n"], false);
        assert!(runtime.server_attach_queued(resource, false));
        assert!(
            !runtime.server_attach_queued(resource, false),
            "duplicate resource ownership is rejected"
        );
        assert!(
            runtime.has_live_http_work(),
            "a rejected duplicate attach must not erase the original admission"
        );
        assert!(
            !runtime.client_attach_queued(resource),
            "nor may the other engine take an owned resource"
        );
        let _ = drain_server_events(&mut runtime, resource, |events| {
            events
                .iter()
                .any(|event| matches!(event, RuntimeHttpEvent::RequestEnd { .. }))
        });
        assert!(runtime.has_live_http_work());

        assert!(runtime.shutdown());
        assert!(runtime.is_shutdown());
        assert!(!runtime.shutdown(), "terminal teardown is idempotent");
        assert!(!runtime.http_owns(resource));
        assert!(!runtime.has_live_http_work());
        assert!(runtime.take_queued_server_events(resource).is_empty());
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
        // An aborted task is dropped when its executor next runs, or goes away with it.
        drop(runtime);
        assert!(log.is_dropped(), "shutdown drops the transports it took");
    }

    #[test]
    fn direct_engine_close_settles_pre_first_poll_admissions() {
        let server_resource = ResourceId::new(60);
        let client_resource = ResourceId::new(61);
        let host = ScriptedHost::new([])
            .with_transport(server_resource, ScriptedTransport::new([], false))
            .with_transport(client_resource, ScriptedTransport::new([], false));
        let mut runtime = RealmRuntime::with_host(host).expect("scripted runtime builds");

        assert!(runtime.server_attach_queued(server_resource, false));
        let executor = &runtime.executor;
        let server_engine = &mut runtime.state.http_server;
        assert!(executor.local.block_on(&executor.runtime, async {
            server_engine.close(server_resource, HttpCloseMode::Abort)
        }));
        let mut realm: Realm = Realm::new();
        let _ = runtime.next_referenced_turn(&mut realm);
        assert_eq!(
            runtime.take_queued_server_events(server_resource),
            vec![RuntimeHttpEvent::Closed]
        );
        assert!(runtime.next_referenced_turn(&mut realm).is_none());

        assert!(runtime.client_attach_queued(client_resource));
        let executor = &runtime.executor;
        let client_engine = &mut runtime.state.http_client;
        assert!(executor.local.block_on(&executor.runtime, async {
            client_engine.close(client_resource, HttpCloseMode::Abort)
        }));
        let events = drain_client_events(&mut runtime, client_resource, |events| {
            events.contains(&RuntimeHttpClientEvent::Closed)
        });
        assert_eq!(events, vec![RuntimeHttpClientEvent::Closed]);
        assert!(runtime.next_referenced_turn(&mut realm).is_none());
    }

    /// Drive one scripted HTTP/1 request through the engine and return the bytes the
    /// response put on the transport. The head is what these tests are about.
    fn response_head_for(
        request: &[u8],
        status: u16,
        headers: &[(&str, &str)],
        chunks: &[&[u8]],
        body: &[u8],
    ) -> String {
        let resource = ResourceId::new(77);
        let (mut runtime, log) = scripted_http_runtime(resource, &[request], false);
        assert!(runtime.server_attach_queued(resource, false));
        let events = drain_server_events(&mut runtime, resource, |events| {
            events
                .iter()
                .any(|event| matches!(event, RuntimeHttpEvent::RequestEnd { .. }))
        });
        let (id, _, _) = request_events(&events);
        assert!(runtime.http_server().start_response(
            resource,
            id,
            status,
            &flat(headers),
            node_stream::DEFAULT_HIGH_WATER_MARK,
        ));
        for chunk in chunks {
            assert!(
                runtime
                    .http_server()
                    .write_response(resource, id, chunk)
                    .is_some()
            );
        }
        assert!(
            runtime
                .http_server()
                .end_response(resource, id, body)
                .is_some()
        );
        let _ = drain_server_events(&mut runtime, resource, |events| {
            events
                .iter()
                .any(|event| matches!(event, RuntimeHttpEvent::Finish { .. }))
        });
        String::from_utf8(log.written()).expect("an HTTP head is ASCII")
    }

    const GET: &[u8] = b"GET /framing HTTP/1.1\r\nHost: kiln\r\n\r\n";
    const HEAD: &[u8] = b"HEAD /framing HTTP/1.1\r\nHost: kiln\r\n\r\n";

    #[test]
    fn end_with_the_whole_body_declares_its_length_in_bytes() {
        // Case 1: nothing flushed, no prior write, no framing header the script chose.
        assert_eq!(
            response_head_for(GET, 200, &[], &[], b"kiln"),
            "HTTP/1.1 200 OK\r\ncontent-length: 4\r\n\r\nkiln"
        );
        // A length is bytes, not characters: five characters, six UTF-8 bytes.
        assert_eq!(
            response_head_for(GET, 200, &[], &[], "h\u{e9}llo".as_bytes()),
            "HTTP/1.1 200 OK\r\ncontent-length: 6\r\n\r\nh\u{e9}llo"
        );
    }

    #[test]
    fn end_with_no_body_declares_zero() {
        // Case 2.
        assert_eq!(
            response_head_for(GET, 200, &[], &[], b""),
            "HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n"
        );
    }

    #[test]
    fn a_framing_the_script_chose_is_left_alone() {
        // Case 3, both directions. A declared length is not recomputed, and a script that asked
        // for chunking still gets it even though the whole body arrived with `end`.
        assert_eq!(
            response_head_for(GET, 200, &[("content-length", "4")], &[], b"kiln"),
            "HTTP/1.1 200 OK\r\ncontent-length: 4\r\n\r\nkiln"
        );
        assert_eq!(
            response_head_for(GET, 200, &[("transfer-encoding", "chunked")], &[], b"kiln"),
            "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n4\r\nkiln\r\n0\r\n\r\n"
        );
        // The header name is matched without regard to case, as HTTP requires.
        assert_eq!(
            response_head_for(GET, 200, &[("Content-Length", "4")], &[], b"kiln"),
            "HTTP/1.1 200 OK\r\ncontent-length: 4\r\n\r\nkiln"
        );
    }

    #[test]
    fn a_chunk_before_end_keeps_the_body_chunked() {
        // Case 4: the length is not known when the head has to go out.
        assert_eq!(
            response_head_for(GET, 200, &[], &[b"ki"], b"ln"),
            "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n2\r\nki\r\n2\r\nln\r\n0\r\n\r\n"
        );
    }

    #[test]
    fn a_response_that_cannot_carry_a_body_declares_no_length() {
        // Case 5. A HEAD response and a bodiless status get neither header, which is what node
        // sends; hyper forwards a content-length here rather than dropping it, so the rule has to
        // decline to add one.
        assert_eq!(
            response_head_for(HEAD, 200, &[], &[], b"kiln"),
            "HTTP/1.1 200 OK\r\n\r\n"
        );
        assert_eq!(
            response_head_for(GET, 204, &[], &[], b""),
            "HTTP/1.1 204 No Content\r\n\r\n"
        );
        assert_eq!(
            response_head_for(GET, 304, &[], &[], b""),
            "HTTP/1.1 304 Not Modified\r\n\r\n"
        );
    }

    #[test]
    fn scripted_provider_dispatches_buffered_http1_pipeline_after_first_response() {
        let resource = ResourceId::new(44);
        let (mut runtime, log) = scripted_http_runtime(
            resource,
            &[b"GET /first HTTP/1.1\r\nHost: kiln\r\n\r\nGET /second HTTP/1.1\r\nHost: kiln\r\n\r\n"],
            false,
        );
        assert!(runtime.server_attach_queued(resource, false));
        let first_events = drain_server_events(&mut runtime, resource, |events| {
            events
                .iter()
                .any(|event| matches!(event, RuntimeHttpEvent::RequestEnd { .. }))
        });
        let (first_id, first, _) = request_events(&first_events);
        assert_eq!(first.target, "/first");
        assert!(
            !first_events
                .iter()
                .any(|event| matches!(event, RuntimeHttpEvent::RequestHead { request, .. } if request.target == "/second")),
            "the second request waits for the first response"
        );
        assert!(runtime.http_server().start_response(
            resource,
            first_id,
            200,
            b"",
            node_stream::DEFAULT_HIGH_WATER_MARK,
        ));
        assert!(
            runtime
                .http_server()
                .end_response(resource, first_id, b"/first")
                .is_some()
        );
        let second_events = drain_server_events(&mut runtime, resource, |events| {
            events.iter().any(|event| {
                matches!(
                    event,
                    RuntimeHttpEvent::RequestHead { request, .. } if request.target == "/second"
                )
            })
        });
        assert!(
            second_events.iter().any(|event| matches!(
                event,
                RuntimeHttpEvent::RequestHead { request, .. } if request.target == "/second"
            )),
            "buffered pipeline events: {second_events:?}"
        );
        assert!(
            log.written()
                .starts_with(b"HTTP/1.1 200 OK\r\ncontent-length: 6\r\n\r\n/first"),
            "the first response went out before the second request was dispatched"
        );
    }

    #[test]
    fn runtime_http_response_body_preserves_node_write_and_drain_semantics() {
        use crate::node_stream::WriteDisposition;

        let resource = ResourceId::new(92);
        let (mut runtime, _log) = scripted_http_runtime(
            resource,
            &[b"GET /stream HTTP/1.1\r\nhost: kiln\r\nconnection: close\r\n\r\n"],
            false,
        );
        assert!(runtime.server_attach_queued(resource, false));
        let events = drain_server_events(&mut runtime, resource, |events| {
            events
                .iter()
                .any(|event| matches!(event, RuntimeHttpEvent::RequestEnd { .. }))
        });
        let (id, _, body) = request_events(&events);
        assert!(body.is_empty());
        assert!(runtime.http_server().start_response(
            resource,
            id,
            200,
            &flat(&[("content-type", "text/plain")]),
            8,
        ));
        assert_eq!(
            runtime.http_server().write_response(resource, id, &[1; 4]),
            Some(WriteDisposition {
                ready: true,
                submit_now: true,
            })
        );
        assert_eq!(
            runtime.http_server().write_response(resource, id, &[2; 4]),
            Some(WriteDisposition {
                ready: false,
                submit_now: false,
            })
        );
        assert_eq!(
            runtime.http_server().end_response(resource, id, &[]),
            Some(WriteDisposition {
                ready: false,
                submit_now: false,
            })
        );
        let events = drain_server_events(&mut runtime, resource, |events| {
            events.contains(&RuntimeHttpEvent::Closed)
        });
        let drain_count = events
            .iter()
            .filter(|event| **event == RuntimeHttpEvent::Drain { id })
            .count();
        let finish_count = events
            .iter()
            .filter(|event| **event == RuntimeHttpEvent::Finish { id })
            .count();
        assert_eq!(drain_count, 1, "{events:?}");
        assert_eq!(finish_count, 1, "{events:?}");
    }

    #[test]
    fn a_scripted_client_connection_round_trips_through_the_queued_adapter() {
        let resource = ResourceId::new(51);
        // The reply is scripted, so it arrives once the request has left.
        let (mut runtime, log) = scripted_runtime_with(
            resource,
            ScriptedTransport::replying(
                [b"HTTP/1.1 201 Created\r\ncontent-type: text/plain\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok".to_vec()],
                true,
            ),
        );
        assert!(runtime.client_attach_queued(resource));
        assert!(runtime.http_client().send_request(
            resource,
            1,
            "POST",
            "/upload",
            &[("host".into(), "kiln".into())],
            node_stream::DEFAULT_HIGH_WATER_MARK,
        ));
        assert!(
            runtime
                .http_client()
                .end_request(resource, 1, b"kiln")
                .expect("the request ends")
                .ready
        );
        assert!(runtime.has_live_http_work());
        let events = drain_client_events(&mut runtime, resource, |events| {
            events.contains(&RuntimeHttpClientEvent::Closed)
        });
        let request = String::from_utf8(log.written()).expect("HTTP is ASCII");
        assert!(
            request.starts_with("POST /upload HTTP/1.1\r\n"),
            "{request}"
        );
        assert!(request.contains("content-length: 4\r\n"), "{request}");
        assert!(request.ends_with("\r\n\r\nkiln"), "{request}");
        assert_eq!(
            events,
            vec![
                RuntimeHttpClientEvent::Finish { id: 1 },
                RuntimeHttpClientEvent::ResponseHead {
                    id: 1,
                    status: 201,
                    reason: "Created".into(),
                    headers: vec![
                        ("content-type".into(), "text/plain".into()),
                        ("content-length".into(), "2".into()),
                        ("connection".into(), "close".into()),
                    ],
                },
                RuntimeHttpClientEvent::ResponseData {
                    id: 1,
                    data: b"ok".to_vec(),
                },
                RuntimeHttpClientEvent::ResponseEnd {
                    id: 1,
                    reusable: false,
                },
                RuntimeHttpClientEvent::Closed,
            ]
        );
        assert!(!runtime.has_live_http_work());
        assert!(!runtime.http_owns(resource));
    }

    #[test]
    fn a_live_tokio_stream_is_handed_to_the_engine_and_served() {
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
        assert!(runtime.server_attach_queued(server.id, false));
        let waiting_accept = runtime.start_host_op(
            AsyncHostOp::TcpAccept {
                listener: listener.id,
            },
            true,
        );
        assert!(
            runtime.has_referenced_host_work(),
            "the referenced listener accept keeps the realm alive while waiting for request data"
        );
        runtime.start_host_op(
            AsyncHostOp::TcpWrite {
                stream: client.id,
                data: b"POST /health HTTP/1.1\r\nhost: kiln\r\ncontent-length: 4\r\nconnection: close\r\n\r\nping".to_vec(),
            },
            false,
        );
        next_host_completion(&mut runtime).expect("the client write completes");
        let events = drain_server_events(&mut runtime, server.id, |events| {
            events
                .iter()
                .any(|event| matches!(event, RuntimeHttpEvent::RequestEnd { .. }))
        });
        let (id, request, body) = request_events(&events);
        assert_eq!(request.method, "POST");
        assert_eq!(request.target, "/health");
        assert_eq!(body, b"ping");
        assert!(runtime.cancel_host_op(waiting_accept));
        assert!(
            !runtime.has_referenced_host_work(),
            "once the server listener is unrefed, idle transports do not keep the realm alive"
        );
        assert!(runtime.http_server().start_response(
            server.id,
            id,
            200,
            &flat(&[("content-type", "text/plain")]),
            node_stream::DEFAULT_HIGH_WATER_MARK,
        ));
        assert!(
            runtime
                .http_server()
                .end_response(server.id, id, b"kiln")
                .is_some()
        );
        runtime.start_host_op(
            AsyncHostOp::TcpRead {
                stream: client.id,
                max_bytes: 4096,
            },
            false,
        );
        let response = match next_host_completion(&mut runtime)
            .expect("the client read completes")
            .result
            .expect("the read succeeds")
        {
            AsyncHostValue::Bytes(bytes) => String::from_utf8(bytes).expect("ASCII"),
            other => panic!("expected bytes, got {other:?}"),
        };
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        assert!(response.ends_with("\r\n\r\nkiln"), "{response}");
        assert!(!response.to_ascii_lowercase().contains("\r\ndate:"));
        let events = drain_server_events(&mut runtime, server.id, |events| {
            events.contains(&RuntimeHttpEvent::Closed)
        });
        assert!(
            events.contains(&RuntimeHttpEvent::Finish { id }),
            "{events:?}"
        );
        assert!(events.contains(&RuntimeHttpEvent::Closed), "{events:?}");
        for resource in [client.id, listener.id] {
            runtime.start_host_op(AsyncHostOp::CloseResource { resource }, false);
            next_host_completion(&mut runtime).expect("close completes");
        }
    }

    #[test]
    fn tokio_host_intercepts_vfs_overlay_operations_fast_path() {
        use kiln_host_abi::{DirEntry, FileStat, OverlayFileSystem};
        use std::collections::HashMap;
        use std::path::PathBuf;

        let mut overlay = HashMap::new();
        overlay.insert(
            PathBuf::from("/vfs/app.js"),
            b"console.log('hello vfs');".to_vec(),
        );
        let vfs = OverlayFileSystem::new(kiln_host_abi::OsFileSystem, overlay);

        let mut host = TokioHost::new().with_fs(Box::new(vfs));

        // 1. ReadFile fast path
        let req = kiln_host_abi::AsyncHostOps::start(
            &mut host,
            AsyncHostOp::ReadFile {
                path: "/vfs/app.js".to_string(),
            },
            true,
        );
        // Completed immediately into ready table!
        let comp = host.next_completion().expect("ready immediately");
        assert_eq!(comp.request, req);
        assert_eq!(
            comp.result,
            Ok(AsyncHostValue::Bytes(b"console.log('hello vfs');".to_vec()))
        );

        // 2. Stat fast path
        let req = kiln_host_abi::AsyncHostOps::start(
            &mut host,
            AsyncHostOp::Stat {
                path: "/vfs/app.js".to_string(),
            },
            true,
        );
        let comp = host.next_completion().expect("ready immediately");
        assert_eq!(comp.request, req);
        assert_eq!(
            comp.result,
            Ok(AsyncHostValue::FileStat(FileStat {
                size: 25,
                is_file: true,
                is_directory: false,
                is_symlink: false,
            }))
        );

        // 3. Lstat fast path
        let req = kiln_host_abi::AsyncHostOps::start(
            &mut host,
            AsyncHostOp::Lstat {
                path: "/vfs/app.js".to_string(),
            },
            true,
        );
        let comp = host.next_completion().expect("ready immediately");
        assert_eq!(comp.request, req);
        assert_eq!(
            comp.result,
            Ok(AsyncHostValue::FileStat(FileStat {
                size: 25,
                is_file: true,
                is_directory: false,
                is_symlink: false,
            }))
        );

        // 4. ReadDir fast path
        let req = kiln_host_abi::AsyncHostOps::start(
            &mut host,
            AsyncHostOp::ReadDir {
                path: "/vfs".to_string(),
                with_types: false,
            },
            true,
        );
        let comp = host.next_completion().expect("ready immediately");
        assert_eq!(comp.request, req);
        assert_eq!(
            comp.result,
            Ok(AsyncHostValue::DirEntries(vec![DirEntry {
                name: "app.js".to_string(),
                is_file: true,
                is_directory: false,
                is_symlink: false,
            }]))
        );
    }
}
