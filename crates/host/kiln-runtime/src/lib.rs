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
mod timer_queue;
pub mod turn_queue;
pub mod turn_scheduler;
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
pub use http_engine::{
    HostIo, HostTransport, HttpDispatch, HttpEvent, HttpEventKind, HttpServerEngine,
    UpgradeWriteCompletion, UpgradeWriteFenceBoundary,
};
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
pub mod listen_group;
pub use listen_group::join_listen_group;
pub mod node_http;
pub mod node_http_agent;
pub mod node_net;
pub mod node_stream;
pub mod scripted_transport;
pub mod signals;
pub use scripted_transport::{ScriptedTransport, ScriptedTransportLog};
/// The Linux io_uring transport behind `take_transport` (ADR 0046); see the module docs.
#[cfg(target_os = "linux")]
pub mod uring;
pub mod worker_pool;
pub mod workers;
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
    ActorHandler, ActorId, DEFAULT_MAILBOX_CAPACITY, PinnedRealm, WorkerError, WorkerPool,
    WorkerRealmHandle,
};

use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use timer_queue::{SequenceBuild, TimerEntry, TimerQueue};

pub use kiln_artifact_cache::{CapabilityIdentity, RealmCapabilityIdentity};
use kiln_host_abi::{
    AcceptedTcpStream, ActiveAsyncHostOps, AsyncHostError, AsyncHostOp, AsyncHostOps,
    AsyncHostValue, Completion, FileStat, HostError, HostOp, HostOps, HostValue,
    PendingCancellation, ProcessOutput, RequestId, ResourceId, ResourceInfo, ResourceKind,
    ResourceState,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{
    TcpListener, TcpStream,
    tcp::{OwnedReadHalf, OwnedWriteHalf},
};
use tokio::sync::oneshot;
use tokio_util::task::JoinMap;

use crate::node_http_agent::{
    AgentCancellation, AgentDecision, AgentFailure, AgentRelease, HttpAgent, HttpOrigin,
};

/// A protocol event selected by the runtime-owned HTTP engine for the realm to publish as Node
/// objects and stream events. No Hyper or Tokio type crosses this seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeHttpEvent {
    RequestHead {
        id: u64,
        request: node_http::HttpRequest,
    },
    UpgradeRequest {
        id: u64,
        request: node_http::HttpRequest,
        head: Vec<u8>,
    },
    UpgradeReady {
        head: Vec<u8>,
    },
    UpgradeWriteComplete {
        id: u64,
        fence: u64,
        error: Option<String>,
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

/// Result of accepting one queued HTTP upgrade write, including its per-append completion key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpgradeWriteReceipt {
    pub disposition: node_stream::WriteDisposition,
    pub fence: u64,
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
    Upgrade {
        id: u64,
        status: u16,
        reason: String,
        headers: Vec<(String, String)>,
        head: Vec<u8>,
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
    /// Inspect retention without releasing or reserving a connection across language callbacks.
    fn client_connection_can_retain(&self, resource: ResourceId, reusable: bool) -> bool;
    fn release_client_connection(&mut self, resource: ResourceId, reusable: bool) -> AgentRelease;
    fn client_connection_failed(&mut self, origin: &HttpOrigin, request: u64) -> AgentFailure;
    fn cancel_client_connection(&mut self, origin: &HttpOrigin, request: u64) -> AgentCancellation;
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
    /// Bytes written by a server's `upgrade` listener before Hyper has transferred the stream.
    /// The runtime parses one raw HTTP response head and hands any trailing bytes to the socket
    /// after the 101 response commits.
    fn write_upgrade(
        &mut self,
        _resource: ResourceId,
        _id: u64,
        _data: &[u8],
    ) -> Option<UpgradeWriteReceipt> {
        None
    }
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
    fn drain_on_shutdown(&mut self, _executor: &Executor) {}

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

    /// Replace the HTTP engine's `TcpTaken` ownership with an ordinary stream resource after a
    /// protocol upgrade. The resource identity is preserved for Node's existing socket object.
    #[cfg(feature = "http")]
    fn adopt_upgrade_transport(
        &mut self,
        _resource: ResourceId,
        _transport: HostTransport,
        _pending_write: Vec<u8>,
        _write_fences: Vec<http_engine::UpgradeWriteFenceBoundary>,
        _write_completion: std::sync::Arc<UpgradeWriteCompletion>,
    ) -> bool {
        false
    }

    /// A signal that fires whenever a host operation completes, so a drive can wait for
    /// completions without borrowing the host across the wait (ADR 0046: language callbacks
    /// run inside I/O tasks while the drive is parked, and they borrow the realm).
    fn completion_signal(&self) -> Option<Arc<tokio::sync::Notify>> {
        None
    }

    /// Forget a transport handed over earlier, once its connection has ended.
    fn release_transport(&mut self, _resource: ResourceId) -> bool {
        false
    }

    fn has_pending_work(&self) -> bool;

    /// Whether pending work needs the I/O driver turned to make progress (sockets, tasks), as
    /// opposed to work that is woken directly through the completion signal (port waits). A
    /// park that waits outside Tokio (ADR 0074) must poll the driver while this holds.
    fn needs_io_driver(&self) -> bool {
        self.has_pending_work()
    }

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

    fn cancel_if_pending(&mut self, request: RequestId) -> PendingCancellation {
        // Scripted operations already own their terminal result at submission.
        if self.pending.contains_key(&request) {
            PendingCancellation::CompletionReady
        } else {
            PendingCancellation::Unknown
        }
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

/// The I/O and timer runtime a scheduler lane may share with other lanes.
///
/// This owns no realm or local task set. A wall-clock scheduler uses one process-wide instance;
/// deterministic mode uses a separate current-thread instance whose clock may be paused.
pub struct Reactor {
    runtime: tokio::runtime::Runtime,
    /// The current-thread deterministic reactor may pause time. A shared wall-clock reactor
    /// never does: pausing it would change every realm using it.
    pause_supported: bool,
    time_paused: AtomicBool,
}

impl Reactor {
    /// A reactor for one deterministic lane. It is driven only while that lane enters it.
    pub fn current_thread() -> Result<Arc<Self>, RuntimeBuildError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .map_err(RuntimeBuildError)?;
        Ok(Arc::new(Self {
            runtime,
            pause_supported: true,
            time_paused: AtomicBool::new(false),
        }))
    }

    /// One process-wide wall-clock I/O service for scheduler lanes. Its only Tokio worker
    /// drives Send I/O futures; a lane polls its realm's local tasks on that lane itself.
    pub fn shared_wall() -> Result<Arc<Self>, RuntimeBuildError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_io()
            .enable_time()
            .build()
            .map_err(RuntimeBuildError)?;
        Ok(Arc::new(Self {
            runtime,
            pause_supported: false,
            time_paused: AtomicBool::new(false),
        }))
    }
}

/// The lane-local executor: one `LocalSet` run on a selected reactor.
///
/// Separate from [`RealmState`], which owns everything a drive reads and writes, so that a drive
/// can hold the state for its whole lifetime while the executor runs it. The previous single
/// struct could not express that, and the native adapter re-entered the executor for every small
/// step instead of running inside it once (ADR 0040).
///
/// An executor owns no realm. It can be made on a shared reactor while `RealmState` remains
/// separately owned. Its `LocalSet` cannot move to another thread while tasks are alive;
/// scheduler migration recreates an empty task set on the destination lane (ADR 0070).
pub struct Executor {
    reactor: Arc<Reactor>,
    local: tokio::task::LocalSet,
    runtime_metrics: std::cell::RefCell<tokio_metrics::RuntimeIntervals>,
    // Current-thread by construction: the LocalSet and the realms it drives are thread-bound.
    _not_send: PhantomData<Rc<()>>,
}

impl Executor {
    /// Builds a current-thread runtime with Tokio's I/O and timer drivers enabled.
    pub fn new() -> Result<Self, RuntimeBuildError> {
        Ok(Self::on_reactor(Reactor::current_thread()?))
    }

    /// Make a lane-local task set on an already-owned reactor. A local task set is never sent
    /// to another thread while it has live tasks; the scheduler recreates an empty one when a
    /// movable realm resumes on a different lane.
    pub fn on_reactor(reactor: Arc<Reactor>) -> Self {
        let runtime_metrics =
            tokio_metrics::RuntimeMonitor::new(reactor.runtime.handle()).intervals();
        Self {
            reactor,
            local: tokio::task::LocalSet::new(),
            runtime_metrics: std::cell::RefCell::new(runtime_metrics),
            _not_send: PhantomData,
        }
    }

    /// Run `future` to completion on this runtime and `LocalSet`: the one place a realm's
    /// thread parks. A drive enters here once per program run; Tokio panics on a nested entry,
    /// which is what makes a stray one loud instead of a deadlock.
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.local.block_on(&self.reactor.runtime, future)
    }

    /// Poll this lane's local tasks once without parking it. The scheduler supplies a waker
    /// that queues the owning realm's next turn when a local task becomes ready.
    ///
    /// Tokio's `LocalSet::run_until` retains unfinished local tasks after its borrowed future
    /// is dropped. Polling a never-ready future once therefore drives a bounded task batch and
    /// registers `waker` with the LocalSet while leaving the scheduler in charge of parking.
    pub fn poll_local_once(&self, waker: &std::task::Waker) {
        if self.reactor.pause_supported {
            // A current-thread reactor has no background I/O driver. Entering its runtime
            // for one bounded tick lets socket readiness reach local tasks; merely polling
            // LocalSet::run_until with Handle::enter does not drive the reactor.
            self.local
                .block_on(&self.reactor.runtime, tokio::task::yield_now());
        }
        let _runtime = self.reactor.runtime.enter();
        let mut future = std::pin::pin!(self.local.run_until(std::future::pending::<()>()));
        let mut context = std::task::Context::from_waker(waker);
        let _ = future.as_mut().poll(&mut context);
    }

    /// Make this executor ambient for `operation` without running it or parking the thread.
    ///
    /// Realm operations that spawn onto the `LocalSet` or register with the I/O driver
    /// ([`RealmState::start_host_op`], the HTTP attaches) need the executor's context. Inside a
    /// drive that context is already current; a caller outside one, such as the interpreter
    /// between turns, enters it here. Both guards nest, so the question "am I in context" that
    /// the deleted `_in_context` twins put on every caller does not arise: this is correct in
    /// either case and never calls `block_on`. Spawned tasks run at the next entry.
    pub fn scope<R>(&self, operation: impl FnOnce() -> R) -> R {
        let _runtime = self.reactor.runtime.enter();
        let _local = self.local.enter();
        operation()
    }

    /// Queue a lane-local task without exposing Tokio's task set to the native provider.
    /// The task is polled on this executor's lane and may wake its realm's scheduler ticket.
    pub fn spawn_local(&self, future: impl Future<Output = ()> + 'static) {
        self.scope(|| {
            tokio::task::spawn_local(future);
        });
    }

    /// Move a deterministic reactor's paused clock to its next realm timer without waiting
    /// for wall time. The scheduler polls local tasks again afterward so the timer can wake
    /// the parked realm ticket.
    pub fn advance_virtual_time(&self, duration: Duration) {
        assert!(
            self.reactor.pause_supported,
            "only deterministic reactors advance virtual time"
        );
        self.block_on(tokio::time::advance(duration));
    }

    /// Pause Tokio time for a deterministic clock, once for this runtime.
    pub fn pause_time(&self) {
        assert!(
            self.reactor.pause_supported,
            "a shared wall-clock reactor cannot pause time"
        );
        if !self.reactor.time_paused.load(Ordering::Acquire) {
            let _entered = self.reactor.runtime.enter();
            tokio::time::pause();
            self.reactor.time_paused.store(true, Ordering::Release);
        }
    }

    /// Whether Tokio time has been paused on this runtime.
    pub fn is_time_paused(&self) -> bool {
        self.reactor.time_paused.load(Ordering::Acquire)
    }

    /// Samples stable whole-runtime counters accumulated since the previous call.
    ///
    /// These are developer diagnostics, not JavaScript-observable time or scheduler state. The
    /// sample's `elapsed` and durations use wall time even though realm timers use paused Tokio
    /// time for deterministic execution.
    pub fn sample_runtime_metrics(&self) -> RuntimeMetricsSnapshot {
        let metrics = self
            .runtime_metrics
            .borrow_mut()
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

    /// Tear down `state` after giving its provider's tasks a chance to complete cooperative
    /// cleanup on this executor. Terminal and idempotent: `false` when teardown had already
    /// begun. Must be called outside any drive, since the drain parks.
    pub fn shut_down<H: RealmHostProvider>(&self, state: &mut RealmState<H>) -> bool {
        if state.is_shutdown() {
            return false;
        }
        state.host.drain_on_shutdown(self);
        state.shutdown()
    }
}

/// Wake signal a realm drive may hold across a turn boundary.
pub type DriveSignal = Arc<tokio::sync::Notify>;

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
pub async fn wait_for_turn(signal: Arc<tokio::sync::Notify>, delay: Option<Duration>) -> DriveWake {
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

/// A foreign event loop the main-thread realm parks inside (ADR 0074): the GUI's run loop.
///
/// `wait` blocks in that loop for up to `timeout_ms` (negative: until something happens) and
/// returns non-zero when it received events for the program. `wake` may be called from any
/// thread and makes a running `wait` return. `port` is a worker port the program waits on: an
/// event-bearing wait enqueues a message there, which wakes the language like any completion.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ParkHook {
    pub wait: extern "C" fn(timeout_ms: f64) -> i32,
    pub wake: extern "C" fn(),
    pub port: u64,
}

thread_local! {
    static PARK_HOOK: std::cell::Cell<Option<ParkHook>> = const { std::cell::Cell::new(None) };
}

/// Install (or with `None`, remove) the park hook for the realm driven on this thread.
pub fn set_park_hook(hook: Option<ParkHook>) {
    PARK_HOOK.with(|cell| cell.set(hook));
}

pub fn park_hook() -> Option<ParkHook> {
    PARK_HOOK.with(|cell| cell.get())
}

/// How long a hooked park waits in the foreign loop before turning the I/O driver again, while
/// work that needs the driver (sockets, tasks) is pending. With none pending, it waits for the
/// exact deadline or an event and uses no CPU.
const HOOKED_IO_SLICE: Duration = Duration::from_millis(4);

struct HookWaker(extern "C" fn());

impl std::task::Wake for HookWaker {
    fn wake(self: Arc<Self>) {
        (self.0)();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        (self.0)();
    }
}

/// [`wait_for_turn`] inside a foreign event loop. The completion signal is polled with a waker
/// that interrupts the foreign wait, so a completion from any thread ends the park at once;
/// timers end it at their exact deadline; foreign events are posted to the hook's port.
pub async fn wait_for_turn_in(
    hook: ParkHook,
    signal: Arc<tokio::sync::Notify>,
    delay: Option<Duration>,
    needs_io_driver: impl Fn() -> bool,
) -> DriveWake {
    let deadline = delay.map(|delay| std::time::Instant::now() + delay);
    let waker = std::task::Waker::from(Arc::new(HookWaker(hook.wake)));
    let mut notified = std::pin::pin!(signal.notified());
    loop {
        // Run woken tasks and give the I/O driver a zero-timeout turn.
        yield_to_tasks().await;
        if notified
            .as_mut()
            .poll(&mut std::task::Context::from_waker(&waker))
            .is_ready()
        {
            return DriveWake::Completion;
        }
        let remaining = deadline.map(|d| d.saturating_duration_since(std::time::Instant::now()));
        if remaining == Some(Duration::ZERO) {
            return DriveWake::Timer;
        }
        let slice = if needs_io_driver() {
            Some(remaining.map_or(HOOKED_IO_SLICE, |r| r.min(HOOKED_IO_SLICE)))
        } else {
            remaining
        };
        let timeout_ms = slice.map_or(-1.0, |s| s.as_secs_f64() * 1000.0);
        if (hook.wait)(timeout_ms) != 0 {
            workers::enqueue(
                hook.port,
                workers::Message::new(workers::kind::USER, Vec::new()),
            );
        }
    }
}

/// Let every task woken so far run before the caller parks.
pub async fn yield_to_tasks() {
    tokio::task::yield_now().await;
}

/// Everything a realm owns that is not the executor: the host, its capabilities, the HTTP
/// engines and agent, and the queued-event adapter. See ADR 0040.
///
/// Built without an executor and holding no handle to one, so it is an owned value that
/// whichever executor runs a turn can drive. Its operations never park the thread; those that
/// spawn or register I/O expect to run inside an executor's context (a drive, or
/// [`Executor::scope`]).
pub struct RealmState<H> {
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
    /// Accepted HTTP closes remain referenced until their terminal event reaches the adapter.
    #[cfg(feature = "http")]
    closing_http_resources: HashSet<ResourceId>,
    lifecycle: RealmRuntimeLifecycle,
}

/// A deterministic, current-thread runtime for realm tasks: a realm's state plus the executor
/// that drives it, for callers that are entered from outside the executor (the interpreter,
/// the daemon, conformance, the worker pool).
///
/// The runtime starts Tokio time paused on the first call to [`Self::drive`]. A drive advances
/// automatically to each realm's next timer only after all currently-ready microtasks have been
/// exhausted. This makes a timer's delay virtual: a one-hour timer costs no wall-clock hour.
///
/// `drive` accepts generic caller-owned microtask and timer payloads. Timer payloads must be
/// [`Clone`] when driven because an interval re-arms the same callback before returning it.
///
/// Every operation that needs the executor's context enters it with [`Executor::scope`] and
/// delegates to [`RealmState`]; a caller already running inside a drive, like the native
/// adapter, holds the two separately and calls the state directly.
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
            .field("time_paused", &self.executor.is_time_paused())
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
        self.executor.shut_down(&mut self.state)
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
        Ok(Self::from_parts(
            Rc::new(Executor::new()?),
            RealmState::new(host, capabilities),
        ))
    }

    /// A runtime over an existing executor and realm state.
    pub fn from_parts(executor: Rc<Executor>, state: RealmState<H>) -> Self {
        Self {
            executor,
            state,
            _not_send: PhantomData,
        }
    }

    /// The executor this runtime drives its realm on. Shared, so a caller can run one future
    /// for the life of a loop while that loop borrows the runtime.
    pub fn executor(&self) -> &Rc<Executor> {
        &self.executor
    }

    /// Pause Tokio time once, if this realm keeps the deterministic clock.
    fn pause_virtual_time(&self) {
        if self.state.capabilities.clock == ClockCapability::Virtual {
            self.executor.pause_time();
        }
    }

    /// Try to start one host operation, rejecting admission after teardown begins.
    pub fn try_start_host_op(
        &mut self,
        operation: AsyncHostOp,
        referenced: bool,
    ) -> Option<RequestId> {
        let state = &mut self.state;
        self.executor
            .scope(|| state.try_start_host_op(operation, referenced))
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
        self.executor
            .block_on(self.state.host.next_completion_async())
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
        self.pause_virtual_time();
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
        self.executor.block_on(async {
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

    /// [`RealmState::server_attach`], entered from outside the executor.
    #[cfg(feature = "http")]
    pub fn server_attach(
        &mut self,
        resource: ResourceId,
        allow_half_close: bool,
        dispatch: Rc<dyn HttpDispatch>,
    ) -> bool {
        let state = &mut self.state;
        self.executor
            .scope(|| state.server_attach(resource, allow_half_close, dispatch))
    }

    /// [`RealmState::client_attach`], entered from outside the executor.
    #[cfg(feature = "http")]
    pub fn client_attach(&mut self, resource: ResourceId, dispatch: Rc<dyn HttpDispatch>) -> bool {
        let state = &mut self.state;
        self.executor
            .scope(|| state.client_attach(resource, dispatch))
    }

    /// [`RealmState::server_attach_queued`], entered from outside the executor.
    #[cfg(feature = "http")]
    pub fn server_attach_queued(&mut self, resource: ResourceId, allow_half_close: bool) -> bool {
        self.server_attach_queued_with(resource, allow_half_close, false)
    }

    /// [`RealmState::server_attach_queued_with`], entered from outside the executor.
    #[cfg(feature = "http")]
    pub fn server_attach_queued_with(
        &mut self,
        resource: ResourceId,
        allow_half_close: bool,
        whole_requests: bool,
    ) -> bool {
        let state = &mut self.state;
        self.executor
            .scope(|| state.server_attach_queued_with(resource, allow_half_close, whole_requests))
    }

    /// [`RealmState::client_attach_queued`], entered from outside the executor.
    #[cfg(feature = "http")]
    pub fn client_attach_queued(&mut self, resource: ResourceId) -> bool {
        let state = &mut self.state;
        self.executor.scope(|| state.client_attach_queued(resource))
    }

    /// [`RealmState::http_close`], entered from outside the executor.
    pub fn http_close(&mut self, resource: ResourceId, mode: HttpCloseMode) -> bool {
        let state = &mut self.state;
        self.executor.scope(|| state.http_close(resource, mode))
    }

    /// Pause Tokio time for the deterministic clock, once.
    pub fn pause_time(&mut self) {
        self.pause_virtual_time();
    }

    /// Samples stable whole-runtime counters accumulated since the previous call.
    pub fn sample_runtime_metrics(&mut self) -> RuntimeMetricsSnapshot {
        self.executor.sample_runtime_metrics()
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
        self.executor.block_on(async move { operation() })
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
        self.pause_virtual_time();
        self.executor
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
        if self.is_shutdown() {
            return None;
        }
        self.pause_virtual_time();
        self.executor.block_on(next_timer(realm))
    }
}

/// Realm operations that need no executor.
///
/// Splitting these off is what makes the nesting rule structural rather than a runtime
/// check: a drive holding `&mut RealmState` cannot name a method that would park the
/// thread, because those live on [`RealmRuntime`] and need its executor. See ADR 0040.
///
/// Some of these spawn onto the `LocalSet` or register sockets with the I/O driver, and so
/// expect the executor's context to be current: they are called from inside a drive (the
/// native adapter) or through the [`RealmRuntime`] wrapper of the same name, which enters it
/// with [`Executor::scope`]. None of them parks.
impl<H: RealmHostProvider> RealmState<H> {
    /// A realm's state, built without any executor. It is driven by whichever executor runs
    /// its turns, through [`RealmRuntime::from_parts`] or a drive that holds both.
    pub fn new(host: H, capabilities: RealmCapabilities) -> Self {
        Self {
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
            #[cfg(feature = "http")]
            closing_http_resources: HashSet::new(),
            lifecycle: RealmRuntimeLifecycle::Open,
        }
    }

    /// Select the host clock before the realm is entered. This keeps Tokio time real for
    /// host-backed executions; deterministic realms retain paused virtual time.
    pub fn use_host_clock(&mut self) {
        self.capabilities.clock = ClockCapability::Host;
    }

    /// Try to start one host operation, rejecting admission after teardown begins.
    ///
    /// Expects the executor's context (see the block comment).
    pub fn try_start_host_op(
        &mut self,
        operation: AsyncHostOp,
        referenced: bool,
    ) -> Option<RequestId> {
        if self.is_shutdown() {
            return None;
        }
        Some(RealmHostProvider::start_on_realm(
            &mut self.host,
            operation,
            referenced,
        ))
    }

    /// Starts one asynchronous host operation. Expects the executor's context.
    pub fn start_host_op(&mut self, operation: AsyncHostOp, referenced: bool) -> RequestId {
        self.try_start_host_op(operation, referenced)
            .expect("a shut down realm cannot start host work")
    }

    /// Attach the HTTP engine to an accepted stream (ADR 0046): the host hands over its
    /// transport and Hyper drives it, calling `dispatch` directly for every protocol event.
    /// Expects the executor's context: the connection is a task on its `LocalSet`.
    #[cfg(feature = "http")]
    pub fn server_attach(
        &mut self,
        resource: ResourceId,
        allow_half_close: bool,
        dispatch: Rc<dyn HttpDispatch>,
    ) -> bool {
        if self.is_shutdown() || self.http_owns(resource) {
            return false;
        }
        let Some(transport) = self.host.take_transport(resource) else {
            return false;
        };
        self.http_server
            .attach(resource, transport, allow_half_close, dispatch)
    }

    /// Attach the HTTP client engine to a connected stream (ADR 0046): the host hands over its
    /// transport and Hyper drives it, calling `dispatch` directly for every response event.
    /// Expects the executor's context.
    #[cfg(feature = "http")]
    pub fn client_attach(&mut self, resource: ResourceId, dispatch: Rc<dyn HttpDispatch>) -> bool {
        if self.is_shutdown() || self.http_owns(resource) {
            return false;
        }
        let Some(transport) = self.host.take_transport(resource) else {
            return false;
        };
        self.http_client.attach(resource, transport, dispatch)
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
        let Some(admission) = self.queued_http.admit(resource) else {
            return false;
        };
        let dispatch = Rc::new(QueuedDispatch {
            resource,
            admission,
            role: QueuedRole::Server,
            queue: Rc::clone(&self.queued_http),
            whole_requests,
        });
        self.server_attach(resource, allow_half_close, dispatch)
    }

    /// Attach a client connection whose events are queued for
    /// [`Self::take_queued_client_events`].
    #[cfg(feature = "http")]
    pub fn client_attach_queued(&mut self, resource: ResourceId) -> bool {
        let Some(admission) = self.queued_http.admit(resource) else {
            return false;
        };
        let dispatch = Rc::new(QueuedDispatch {
            resource,
            admission,
            role: QueuedRole::Client,
            queue: Rc::clone(&self.queued_http),
            whole_requests: false,
        });
        self.client_attach(resource, dispatch)
    }

    /// The direct HTTP server engine.
    #[cfg(feature = "http")]
    pub fn http_server(&mut self) -> &mut HttpServerEngine {
        &mut self.http_server
    }

    /// The direct HTTP client engine.
    #[cfg(feature = "http")]
    pub fn http_client(&mut self) -> &mut HttpClientEngine {
        &mut self.http_client
    }

    /// Atomically return an upgraded server stream to the host under its existing resource id.
    /// Called before the language sees `UpgradeReady`, so socket operations observe the raw
    /// stream immediately and HTTP close authority has already ended.
    #[cfg(feature = "http")]
    pub fn adopt_server_upgrade(&mut self, resource: ResourceId) -> bool {
        let Some(upgrade) = self.http_server.take_upgrade(resource) else {
            return false;
        };
        let completion = Arc::clone(&upgrade.write_completion);
        let adopted = self.host.adopt_upgrade_transport(
            resource,
            upgrade.transport,
            upgrade.pending_write,
            upgrade.write_fences,
            Arc::clone(&completion),
        );
        if !adopted {
            completion.complete(Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "HTTP upgrade transport could not be adopted",
            )));
        }
        adopted
    }

    #[cfg(feature = "http")]
    pub fn adopt_client_upgrade(&mut self, resource: ResourceId) -> bool {
        let Some(upgrade) = self.http_client.take_upgrade(resource) else {
            return false;
        };
        let completion = Arc::clone(&upgrade.write_completion);
        let adopted = self.host.adopt_upgrade_transport(
            resource,
            upgrade.transport,
            upgrade.pending_write,
            upgrade.write_fences,
            Arc::clone(&completion),
        );
        if !adopted {
            completion.complete(Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "HTTP upgrade transport could not be adopted",
            )));
        }
        adopted
    }

    /// Stop the connection on `resource`, whichever engine owns it. The transport is released
    /// once the engine has published `Closed`; a caller closing from outside an engine
    /// callback sees that on its next turn. Expects the executor's context.
    #[cfg(feature = "http")]
    pub fn http_close(&mut self, resource: ResourceId, mode: HttpCloseMode) -> bool {
        let accepted =
            self.http_server.close(resource, mode) || self.http_client.close(resource, mode);
        if accepted {
            self.closing_http_resources.insert(resource);
        }
        accepted
    }

    /// Without `http` no engine can own a transport, so there is nothing to stop.
    #[cfg(not(feature = "http"))]
    pub fn http_close(&mut self, _resource: ResourceId, _mode: HttpCloseMode) -> bool {
        false
    }

    /// Whether any direct server connection is still open.
    #[cfg(feature = "http")]
    pub fn has_open_server_connections(&self) -> bool {
        self.http_server.open_connections() != 0
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
        self.http_server.open_connections()
    }

    /// Without `http` no server connection can be open.
    #[cfg(not(feature = "http"))]
    pub fn open_server_connections(&self) -> usize {
        0
    }

    /// How many client connections are open, likewise.
    #[cfg(feature = "http")]
    pub fn open_client_connections(&self) -> usize {
        self.http_client.open_connections()
    }

    /// Without `http` no client connection can be open.
    #[cfg(not(feature = "http"))]
    pub fn open_client_connections(&self) -> usize {
        0
    }

    /// The host's completion signal, for a drive that waits outside any borrow of this state.
    pub fn completion_signal(&self) -> Option<Arc<tokio::sync::Notify>> {
        self.host.completion_signal()
    }

    /// Wake a parked drive: something it was not waiting for has changed what it should wait
    /// for, such as a timer armed from inside a connection callback.
    pub fn wake_drive(&self) {
        if let Some(signal) = self.host.completion_signal() {
            signal.notify_one();
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
        if let Some(failure) = self.host.runtime_failure() {
            return Some(RuntimeTurn::Failure(failure.clone()));
        }
        if let Some(completion) = AsyncHostOps::next_completion(&mut self.host) {
            return Some(RuntimeTurn::Completion(completion));
        }
        if let Some(failure) = self.host.runtime_failure() {
            return Some(RuntimeTurn::Failure(failure.clone()));
        }
        if self.queued_http.has_events() || self.queued_http.take_signalled() {
            return Some(RuntimeTurn::Protocol);
        }
        None
    }

    /// Requests in flight on direct connections: server responses the language has not
    /// finished, and client requests whose response has not ended.
    #[cfg(feature = "http")]
    pub fn requests_in_flight(&self) -> usize {
        self.http_server.requests_in_flight() + self.http_client.requests_in_flight()
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
        format!("{:?}", self.host)
    }

    /// Ask the HTTP agent for a connection to `origin` (node's `http.Agent` pool).
    pub fn acquire_client_connection(&mut self, origin: HttpOrigin, request: u64) -> AgentDecision {
        assert!(
            !self.is_shutdown(),
            "a shut down realm cannot acquire an HTTP connection"
        );
        let decision = self.http_agent.acquire(origin, request);
        if let AgentDecision::Reuse(resource) = decision {
            let _ = self.host.set_resource_referenced_now(resource, true);
        }
        decision
    }

    /// Record that the agent's pending connect for `request` produced `resource`.
    pub fn client_connection_connected(
        &mut self,
        origin: HttpOrigin,
        resource: ResourceId,
        request: u64,
    ) -> bool {
        if self.is_shutdown() {
            return false;
        }
        self.http_agent.connected(origin, resource, request)
    }

    /// Whether completing this response can retain its Agent-owned connection. This is a pure
    /// query: callbacks must run before release publishes the resource or reserves a waiter.
    pub fn client_connection_can_retain(&self, resource: ResourceId, reusable: bool) -> bool {
        !self.is_shutdown() && reusable && self.http_agent.has_active_request(resource)
    }

    /// Return a client connection to the agent after its response.
    pub fn release_client_connection(
        &mut self,
        resource: ResourceId,
        reusable: bool,
    ) -> AgentRelease {
        if self.is_shutdown() {
            return AgentRelease::Retire;
        }
        let release = self.http_agent.release(resource, reusable);
        if release == AgentRelease::Idle {
            let _ = self.host.set_resource_referenced_now(resource, false);
        }
        release
    }

    /// Tell the agent a connect to `origin` failed.
    pub fn client_connection_failed(&mut self, origin: &HttpOrigin, request: u64) -> AgentFailure {
        if self.is_shutdown() {
            return AgentFailure::Empty;
        }
        self.http_agent.connect_failed(origin, request)
    }

    pub fn cancel_client_connection(
        &mut self,
        origin: &HttpOrigin,
        request: u64,
    ) -> AgentCancellation {
        if self.is_shutdown() {
            return AgentCancellation::Missing;
        }
        self.http_agent.cancel_connect(origin, request)
    }

    /// Tell the agent a client connection closed.
    pub fn client_connection_closed(&mut self, resource: ResourceId) -> Option<u64> {
        if self.is_shutdown() {
            return None;
        }
        self.http_agent.closed(resource)
    }

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

    /// Whether a park outside Tokio must keep turning the I/O driver: host tasks, or HTTP work
    /// on a direct connection.
    pub fn needs_io_driver(&self) -> bool {
        self.host.needs_io_driver() || self.requests_in_flight() != 0 || self.has_live_http_work()
    }

    /// Cancels one asynchronous host request owned by this realm.
    pub fn cancel_host_op(&mut self, request: RequestId) -> bool {
        RealmHostProvider::cancel_on_realm(&mut self.host, request)
    }

    /// Cancel unfinished host work without discarding a result awaiting language delivery.
    pub fn cancel_host_op_if_pending(&mut self, request: RequestId) -> PendingCancellation {
        AsyncHostOps::cancel_if_pending(&mut self.host, request)
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
            || !self.closing_http_resources.is_empty()
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
        let mut events = self.queued_http.take_server(resource);
        #[cfg(feature = "http")]
        if events
            .iter()
            .any(|event| matches!(event, RuntimeHttpEvent::UpgradeReady { .. }))
        {
            let adopted = self
                .http_server
                .take_upgrade(resource)
                .is_some_and(|upgrade| {
                    let completion = Arc::clone(&upgrade.write_completion);
                    let adopted = self.host.adopt_upgrade_transport(
                        resource,
                        upgrade.transport,
                        upgrade.pending_write,
                        upgrade.write_fences,
                        Arc::clone(&completion),
                    );
                    if !adopted {
                        completion.complete(Err(std::io::Error::new(
                            std::io::ErrorKind::NotConnected,
                            "HTTP upgrade transport could not be adopted",
                        )));
                    }
                    adopted
                });
            if !adopted {
                let _ = self.host.release_transport(resource);
                events.clear();
                events.push(RuntimeHttpEvent::Error {
                    id: 0,
                    message: "HTTP upgrade transport could not be adopted".into(),
                });
                events.push(RuntimeHttpEvent::Closed);
            }
        }
        if events.contains(&RuntimeHttpEvent::Closed) {
            let _ = self.host.release_transport(resource);
            #[cfg(feature = "http")]
            self.closing_http_resources.remove(&resource);
        }
        events
    }

    /// Drain the server events the queued adapter parked for all resources in a single batch.
    pub fn take_all_queued_server_events(&mut self) -> Vec<(ResourceId, Vec<RuntimeHttpEvent>)> {
        let mut all = self.queued_http.take_all_server();
        for (resource, events) in &mut all {
            #[cfg(feature = "http")]
            if events
                .iter()
                .any(|event| matches!(event, RuntimeHttpEvent::UpgradeReady { .. }))
            {
                let adopted = self
                    .http_server
                    .take_upgrade(*resource)
                    .is_some_and(|upgrade| {
                        let completion = Arc::clone(&upgrade.write_completion);
                        let adopted = self.host.adopt_upgrade_transport(
                            *resource,
                            upgrade.transport,
                            upgrade.pending_write,
                            upgrade.write_fences,
                            Arc::clone(&completion),
                        );
                        if !adopted {
                            completion.complete(Err(std::io::Error::new(
                                std::io::ErrorKind::NotConnected,
                                "HTTP upgrade transport could not be adopted",
                            )));
                        }
                        adopted
                    });
                if !adopted {
                    let _ = self.host.release_transport(*resource);
                    events.clear();
                    events.push(RuntimeHttpEvent::Error {
                        id: 0,
                        message: "HTTP upgrade transport could not be adopted".into(),
                    });
                    events.push(RuntimeHttpEvent::Closed);
                }
            }
            if events.contains(&RuntimeHttpEvent::Closed) {
                let _ = self.host.release_transport(*resource);
                #[cfg(feature = "http")]
                self.closing_http_resources.remove(resource);
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
        let discarded = self.queued_http.retain_resources(known);
        #[cfg(feature = "http")]
        self.closing_http_resources
            .retain(|resource| known.contains(resource));
        discarded
    }

    /// Drain the client events the queued adapter parked for `resource`.
    pub fn take_queued_client_events(
        &mut self,
        resource: ResourceId,
    ) -> Vec<RuntimeHttpClientEvent> {
        let mut events = self.queued_http.take_client(resource);
        #[cfg(feature = "http")]
        if events
            .iter()
            .any(|event| matches!(event, RuntimeHttpClientEvent::Upgrade { .. }))
        {
            let adopted = self
                .http_client
                .take_upgrade(resource)
                .is_some_and(|upgrade| {
                    let completion = Arc::clone(&upgrade.write_completion);
                    let adopted = self.host.adopt_upgrade_transport(
                        resource,
                        upgrade.transport,
                        upgrade.pending_write,
                        upgrade.write_fences,
                        Arc::clone(&completion),
                    );
                    if !adopted {
                        completion.complete(Err(std::io::Error::new(
                            std::io::ErrorKind::NotConnected,
                            "HTTP upgrade transport could not be adopted",
                        )));
                    }
                    adopted
                });
            if !adopted {
                let _ = self.host.release_transport(resource);
                let id = events
                    .iter()
                    .find_map(|event| match event {
                        RuntimeHttpClientEvent::Upgrade { id, .. } => Some(*id),
                        _ => None,
                    })
                    .unwrap_or(0);
                events.clear();
                events.push(RuntimeHttpClientEvent::Error {
                    id,
                    message: "HTTP upgrade transport could not be adopted".into(),
                });
                events.push(RuntimeHttpClientEvent::Closed);
            }
        }
        if events.contains(&RuntimeHttpClientEvent::Closed) {
            let _ = self.host.release_transport(resource);
            #[cfg(feature = "http")]
            self.closing_http_resources.remove(&resource);
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
        #[cfg(feature = "http")]
        self.closing_http_resources.remove(&resource);
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
                    HttpEventKind::UpgradeRequest => RuntimeHttpEvent::UpgradeRequest {
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
                            body: Vec::new(),
                            keep_alive: event.keep_alive,
                        },
                        head: event.data.to_vec(),
                    },
                    HttpEventKind::UpgradeReady => RuntimeHttpEvent::UpgradeReady {
                        head: event.data.to_vec(),
                    },
                    HttpEventKind::UpgradeWriteComplete => RuntimeHttpEvent::UpgradeWriteComplete {
                        id: event.id,
                        fence: event.write_fence,
                        error: (!event.message.is_empty())
                            .then(|| String::from_utf8_lossy(event.message).into_owned()),
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
                    | HttpEventKind::ResponseEnd
                    | HttpEventKind::UpgradeResponse => return 0,
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
                    HttpEventKind::UpgradeResponse => RuntimeHttpClientEvent::Upgrade {
                        id: event.id,
                        status: event.status,
                        reason: String::from_utf8_lossy(event.reason).into_owned(),
                        headers: unflatten_headers(event.headers),
                        head: event.data.to_vec(),
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
                    | HttpEventKind::UpgradeRequest
                    | HttpEventKind::UpgradeReady
                    | HttpEventKind::UpgradeWriteComplete
                    | HttpEventKind::RequestData
                    | HttpEventKind::RequestEnd => return 0,
                };
                self.queue.push_client(self.resource, queued);
            }
        }
        if matches!(
            event.kind,
            HttpEventKind::UpgradeReady | HttpEventKind::UpgradeResponse
        ) {
            self.queue
                .cancel_admission_token(self.resource, self.admission);
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
        self.state.acquire_client_connection(origin, request)
    }

    fn client_connection_connected(
        &mut self,
        origin: HttpOrigin,
        resource: ResourceId,
        request: u64,
    ) -> bool {
        self.state
            .client_connection_connected(origin, resource, request)
    }

    fn client_connection_can_retain(&self, resource: ResourceId, reusable: bool) -> bool {
        self.state.client_connection_can_retain(resource, reusable)
    }

    fn release_client_connection(&mut self, resource: ResourceId, reusable: bool) -> AgentRelease {
        self.state.release_client_connection(resource, reusable)
    }

    fn client_connection_failed(&mut self, origin: &HttpOrigin, request: u64) -> AgentFailure {
        self.state.client_connection_failed(origin, request)
    }

    fn cancel_client_connection(&mut self, origin: &HttpOrigin, request: u64) -> AgentCancellation {
        self.state.cancel_client_connection(origin, request)
    }

    fn client_connection_closed(&mut self, resource: ResourceId) -> Option<u64> {
        self.state.client_connection_closed(resource)
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

    fn write_upgrade(
        &mut self,
        resource: ResourceId,
        id: u64,
        data: &[u8],
    ) -> Option<UpgradeWriteReceipt> {
        #[cfg(feature = "http")]
        {
            return self.state.http_server.write_upgrade(resource, id, data);
        }
        #[cfg(not(feature = "http"))]
        {
            let _ = (resource, id, data);
            None
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

    fn cancel_if_pending(&mut self, request: RequestId) -> PendingCancellation {
        self.state.cancel_host_op_if_pending(request)
    }
}

impl ActiveAsyncHostOps for TokioHost {
    fn start(&mut self, operation: AsyncHostOp, referenced: bool) -> RequestId {
        self.start_on(operation, referenced)
    }

    fn cancel(&mut self, request: RequestId) -> bool {
        AsyncHostOps::cancel(self, request)
    }

    fn cancel_if_pending(&mut self, request: RequestId) -> PendingCancellation {
        AsyncHostOps::cancel_if_pending(self, request)
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
    port_waits: BTreeMap<RequestId, PortWait>,
    process_tasks: BTreeSet<RequestId>,
    process_cancellers: BTreeMap<RequestId, oneshot::Sender<()>>,
    completed: Arc<tokio::sync::Notify>,
    failure: Option<RuntimeFailure>,
    shutdown: bool,
    fs: Option<Box<dyn kiln_host_abi::FileSystemOps>>,
    /// The io_uring every taken transport goes on when that backend is selected.
    #[cfg(target_os = "linux")]
    uring: uring::HostRing,
    /// The shared-listener tickets of connections this realm accepted in `KILN_WORKERS` thread
    /// mode, keyed by the connection's resource and given back when it goes.
    listen_tickets: BTreeMap<ResourceId, listen_group::ListenTicket>,
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
        ticket: Option<listen_group::ListenTicket>,
    },
    /// A connection the io_uring accepted: not registered with Tokio's reactor until a discrete
    /// read, write or shutdown needs it, and never if the HTTP engine takes it first.
    #[cfg(target_os = "linux")]
    TcpRawStream {
        stream: std::net::TcpStream,
        peer: std::net::SocketAddr,
        referenced: bool,
        ticket: Option<listen_group::ListenTicket>,
    },
}

/// A listening socket: Tokio's, (with the io_uring backend) the ring's multishot accept, or in
/// `KILN_WORKERS` thread mode a member of the process's one listener for the address.
enum HostListener {
    Tokio(TcpListener),
    #[cfg(target_os = "linux")]
    Ring(uring::RingListener),
    Shared(listen_group::GroupMember),
}

impl HostListener {
    fn local_addr(&self) -> std::io::Result<std::net::SocketAddr> {
        match self {
            HostListener::Tokio(listener) => listener.local_addr(),
            #[cfg(target_os = "linux")]
            HostListener::Ring(listener) => listener.local_addr(),
            HostListener::Shared(listener) => listener.local_addr(),
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
        restore_referenced: bool,
    },
    TcpStream {
        reader: Rc<tokio::sync::Mutex<OwnedReadHalf>>,
        writer: Rc<tokio::sync::Mutex<OwnedWriteHalf>>,
        info: ResourceInfo,
        referenced: bool,
        read_closed: bool,
        write_closed: bool,
    },
    /// A Hyper-upgraded stream may be a rewind wrapper around the original socket rather than a
    /// concrete `TcpStream`. Keep it as two Tokio split halves and expose the same TCP operations.
    #[cfg(feature = "http")]
    TcpUpgraded {
        reader: Rc<tokio::sync::Mutex<tokio::io::ReadHalf<Box<dyn HostIo>>>>,
        writer: Rc<tokio::sync::Mutex<tokio::io::WriteHalf<Box<dyn HostIo>>>>,
        write_completion: Arc<UpgradeWriteCompletion>,
        prefix_abort: Option<tokio::task::AbortHandle>,
        info: ResourceInfo,
        referenced: bool,
        read_closed: bool,
        write_closed: bool,
    },
}

enum TcpReader {
    Native(Rc<tokio::sync::Mutex<OwnedReadHalf>>),
    #[cfg(feature = "http")]
    Upgraded(Rc<tokio::sync::Mutex<tokio::io::ReadHalf<Box<dyn HostIo>>>>),
}

impl TcpReader {
    async fn read(&self, bytes: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Native(reader) => reader.lock().await.read(bytes).await,
            #[cfg(feature = "http")]
            Self::Upgraded(reader) => reader.lock().await.read(bytes).await,
        }
    }
}

enum TcpWriter {
    Native(Rc<tokio::sync::Mutex<OwnedWriteHalf>>),
    #[cfg(feature = "http")]
    Upgraded {
        writer: Rc<tokio::sync::Mutex<tokio::io::WriteHalf<Box<dyn HostIo>>>>,
        write_completion: Arc<UpgradeWriteCompletion>,
    },
}

impl TcpWriter {
    async fn write_all(&self, bytes: &[u8]) -> std::io::Result<()> {
        match self {
            Self::Native(writer) => {
                let mut writer = writer.lock().await;
                // Attempt the real nonblocking write before yielding. Retain the lock across
                // any remaining asynchronous write so later submissions cannot overtake it.
                let written = match writer.try_write(bytes) {
                    Ok(written) => written,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => 0,
                    Err(error) => return Err(error),
                };
                writer.write_all(&bytes[written..]).await
            }
            #[cfg(feature = "http")]
            Self::Upgraded {
                writer,
                write_completion,
            } => {
                write_completion.wait().await?;
                writer.lock().await.write_all(bytes).await
            }
        }
    }
    async fn shutdown(&self) -> std::io::Result<()> {
        match self {
            Self::Native(writer) => writer.lock().await.shutdown().await,
            #[cfg(feature = "http")]
            Self::Upgraded {
                writer,
                write_completion,
            } => {
                write_completion.wait().await?;
                writer.lock().await.shutdown().await
            }
        }
    }
}

struct PendingRequest {
    sequence: u64,
    referenced: bool,
}

struct PortWait {
    port: u64,
    subscription: u64,
    sequence: u64,
}

struct AsyncAtomicsWait(u64);

impl Drop for AsyncAtomicsWait {
    fn drop(&mut self) {
        workers::wait_async_finish(self.0);
    }
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
                        #[cfg(feature = "http")]
                        HostResource::TcpUpgraded { referenced, .. } => *referenced,
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

impl Drop for TokioHost {
    fn drop(&mut self) {
        #[cfg(feature = "http")]
        self.abort_upgrade_prefixes();
        for (_, wait) in std::mem::take(&mut self.port_waits) {
            workers::unsubscribe(wait.port, wait.subscription);
        }
    }
}

impl TokioHost {
    #[cfg(feature = "http")]
    fn abort_upgrade_prefixes(&self) {
        for resource in self.resources.values() {
            if let HostResource::TcpUpgraded {
                prefix_abort: Some(abort),
                write_completion,
                ..
            } = resource
            {
                write_completion.complete(Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "realm shut down during HTTP upgrade write",
                )));
                abort.abort();
            }
        }
    }

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
            port_waits: BTreeMap::new(),
            process_tasks: BTreeSet::new(),
            process_cancellers: BTreeMap::new(),
            completed: Arc::new(tokio::sync::Notify::new()),
            failure: None,
            shutdown: false,
            fs: None,
            #[cfg(target_os = "linux")]
            uring: uring::HostRing::new(),
            listen_tickets: BTreeMap::new(),
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
        let completed = Arc::clone(&self.completed);
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
            AsyncHostOp::PortWait { port } => {
                // Register under the port lock, so a post cannot race the initial state check.
                // The port wakes this realm's drive directly; no Tokio task is needed per wait.
                let Some(subscription) = workers::subscribe(port, Arc::clone(&self.completed))
                else {
                    self.insert_immediate(
                        request,
                        sequence,
                        referenced,
                        Ok(AsyncHostValue::Undefined),
                    );
                    return request;
                };
                self.pending.insert(
                    request,
                    PendingRequest {
                        sequence,
                        referenced,
                    },
                );
                self.port_waits.insert(
                    request,
                    PortWait {
                        port,
                        subscription,
                        sequence,
                    },
                );
                return request;
            }
            AsyncHostOp::AtomicsWait { ticket, timeout } => {
                let Some(signal) = workers::wait_async_signal(ticket) else {
                    self.insert_immediate(
                        request,
                        sequence,
                        referenced,
                        Err(AsyncHostError::new(
                            "EINVAL",
                            "Atomics.waitAsync",
                            None,
                            "unknown async waiter ticket",
                        )),
                    );
                    return request;
                };
                let waiter = AsyncAtomicsWait(ticket);
                self.spawn_task(request, sequence, referenced, async move {
                    let notified = match timeout {
                        Some(duration) => tokio::time::timeout(duration, signal.notified())
                            .await
                            .is_ok(),
                        None => {
                            signal.notified().await;
                            true
                        }
                    };
                    let answer = if workers::wait_async_finish(ticket).unwrap_or(notified) {
                        "ok"
                    } else {
                        "timed-out"
                    };
                    drop(waiter);
                    HostCompletion {
                        request,
                        sequence,
                        lifecycle: None,
                        result: Ok(HostCompletionValue::Public(AsyncHostValue::String(
                            answer.into(),
                        ))),
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
                // A worker realm in `KILN_WORKERS` thread mode shares one socket per address
                // with the others, and the process balances accepts across them.
                let shared = listen_group::joined();
                self.spawn_task(request, sequence, referenced, async move {
                    let listen = || -> std::io::Result<HostListener> {
                        if shared && let Some(member) = listen_group::listen(host.as_str(), port)? {
                            return Ok(HostListener::Shared(member));
                        }
                        let listener = bind_tcp_listener_std(host.as_str(), port)?;
                        #[cfg(target_os = "linux")]
                        if let Some(ring) = &ring {
                            return match ring.listen(std::os::fd::OwnedFd::from(listener)) {
                                Ok(listener) => Ok(HostListener::Ring(listener)),
                                Err(fd) => {
                                    TcpListener::from_std(fd.into()).map(HostListener::Tokio)
                                }
                            };
                        }
                        TcpListener::from_std(listener).map(HostListener::Tokio)
                    };
                    let result = listen()
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
                if let HostListener::Shared(member) = &*listener {
                    let accept = member.accept();
                    #[cfg(target_os = "linux")]
                    let raw = self.uring.ring().is_some();
                    self.spawn_task(request, sequence, referenced, async move {
                        let result = async {
                            let (stream, ticket) = accept.await?;
                            let peer = stream.peer_addr()?;
                            // With the ring, the connection stays raw until the HTTP engine
                            // takes it onto the ring, exactly as a ring-accepted one does.
                            #[cfg(target_os = "linux")]
                            if raw {
                                return Ok(HostCompletionValue::TcpRawStream {
                                    stream,
                                    peer,
                                    referenced: accepted_referenced,
                                    ticket: Some(ticket),
                                });
                            }
                            Ok(HostCompletionValue::TcpStream {
                                stream: TcpStream::from_std(stream)?,
                                peer,
                                referenced: accepted_referenced,
                                accepted: true,
                                ticket: Some(ticket),
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
                                ticket: None,
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
                        HostListener::Shared(_) => {
                            unreachable!("shared listeners are handled above")
                        }
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
                                ticket: None,
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
                            ticket: None,
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
                let (reader, read_closed) = match self.resources.get(&stream) {
                    Some(HostResource::TcpStream {
                        reader,
                        read_closed,
                        ..
                    }) => (TcpReader::Native(Rc::clone(reader)), *read_closed),
                    #[cfg(feature = "http")]
                    Some(HostResource::TcpUpgraded {
                        reader,
                        read_closed,
                        ..
                    }) => (TcpReader::Upgraded(Rc::clone(reader)), *read_closed),
                    _ => {
                        self.insert_immediate(
                            request,
                            sequence,
                            referenced,
                            Err(invalid_resource("read", stream)),
                        );
                        return request;
                    }
                };
                if read_closed {
                    self.insert_immediate(
                        request,
                        sequence,
                        referenced,
                        Ok(AsyncHostValue::Bytes(Vec::new())),
                    );
                    return request;
                }
                self.spawn_task(request, sequence, referenced, async move {
                    let mut bytes = vec![0; max_bytes];
                    let result = async {
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
                let (writer, write_closed) = match self.resources.get(&stream) {
                    Some(HostResource::TcpStream {
                        writer,
                        write_closed,
                        ..
                    }) => (TcpWriter::Native(Rc::clone(writer)), *write_closed),
                    #[cfg(feature = "http")]
                    Some(HostResource::TcpUpgraded {
                        writer,
                        write_completion,
                        write_closed,
                        ..
                    }) => (
                        TcpWriter::Upgraded {
                            writer: Rc::clone(writer),
                            write_completion: Arc::clone(write_completion),
                        },
                        *write_closed,
                    ),
                    _ => {
                        self.insert_immediate(
                            request,
                            sequence,
                            referenced,
                            Err(invalid_resource("write", stream)),
                        );
                        return request;
                    }
                };
                if write_closed {
                    self.insert_immediate(
                        request,
                        sequence,
                        referenced,
                        Err(stream_closed("write", stream_id)),
                    );
                    return request;
                }
                let immediate_write = matches!(&writer, TcpWriter::Native(_));
                let mut write = Box::pin(async move {
                    let result = async {
                        writer.write_all(&data).await?;
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
                if immediate_write {
                    // A successful OS write is already complete even when its language callback
                    // has not run. Poll once and preserve the same future on Pending; dropping it
                    // would lose partial progress and release write-order authority.
                    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
                    if let std::task::Poll::Ready(completion) = write.as_mut().poll(&mut context) {
                        let result = completion.result.map(|value| match value {
                            HostCompletionValue::Public(value) => value,
                            _ => unreachable!("TCP writes only publish public results"),
                        });
                        self.insert_immediate(request, sequence, referenced, result);
                        return request;
                    }
                }
                self.spawn_task(request, sequence, referenced, write);
                return request;
            }
            AsyncHostOp::TcpShutdown { stream } => {
                #[cfg(target_os = "linux")]
                self.materialize_raw(stream);
                let stream_id = stream;
                let (writer, write_closed) = match self.resources.get(&stream) {
                    Some(HostResource::TcpStream {
                        writer,
                        write_closed,
                        ..
                    }) => (TcpWriter::Native(Rc::clone(writer)), *write_closed),
                    #[cfg(feature = "http")]
                    Some(HostResource::TcpUpgraded {
                        writer,
                        write_completion,
                        write_closed,
                        ..
                    }) => (
                        TcpWriter::Upgraded {
                            writer: Rc::clone(writer),
                            write_completion: Arc::clone(write_completion),
                        },
                        *write_closed,
                    ),
                    _ => {
                        self.insert_immediate(
                            request,
                            sequence,
                            referenced,
                            Err(invalid_resource("shutdown", stream)),
                        );
                        return request;
                    }
                };
                if write_closed {
                    self.insert_immediate(
                        request,
                        sequence,
                        referenced,
                        Ok(AsyncHostValue::Undefined),
                    );
                    return request;
                }
                self.spawn_task(request, sequence, referenced, async move {
                    let result = writer
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
        self.listen_tickets.remove(&resource);
        #[cfg(feature = "http")]
        if let Some(HostResource::TcpUpgraded {
            prefix_abort: Some(abort),
            write_completion,
            ..
        }) = self.resources.get(&resource)
        {
            write_completion.complete(Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "socket closed during HTTP upgrade write",
            )));
            abort.abort();
        }
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
            } => *current = referenced,
            HostResource::TcpTaken {
                referenced: current,
                restore_referenced,
                ..
            } => {
                *current = referenced;
                *restore_referenced = referenced;
            }
            #[cfg(feature = "http")]
            HostResource::TcpUpgraded {
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
            #[cfg(feature = "http")]
            HostResource::TcpUpgraded {
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
            HostResource::TcpTaken {
                info, referenced, ..
            } => ResourceState {
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
        let ready_ports: Vec<_> = self
            .port_waits
            .iter()
            .filter_map(|(&request, wait)| workers::readable(wait.port).then_some(request))
            .collect();
        for request in ready_ports {
            let wait = self
                .port_waits
                .remove(&request)
                .expect("ready port wait exists");
            workers::unsubscribe(wait.port, wait.subscription);
            self.promote_completion(HostCompletion {
                request,
                sequence: wait.sequence,
                lifecycle: None,
                result: Ok(HostCompletionValue::Public(AsyncHostValue::Undefined)),
            });
        }
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
        #[cfg(feature = "http")]
        self.abort_upgrade_prefixes();
        self.resources.clear();
        self.listen_tickets.clear();
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
                    #[cfg(feature = "http")]
                    if let Some(HostResource::TcpUpgraded { read_closed, .. }) =
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
                    #[cfg(feature = "http")]
                    if let Some(HostResource::TcpUpgraded { write_closed, .. }) =
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
                ticket,
            } => {
                let local = stream.local_addr().ok();
                let id = ResourceId::new(self.next_resource);
                self.next_resource = self.next_resource.wrapping_add(1).max(1);
                if let Some(ticket) = ticket {
                    self.listen_tickets.insert(id, ticket);
                }
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
                ticket,
            } => {
                let local = stream.local_addr().ok();
                let id = ResourceId::new(self.next_resource);
                self.next_resource = self.next_resource.wrapping_add(1).max(1);
                if let Some(ticket) = ticket {
                    self.listen_tickets.insert(id, ticket);
                }
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
        if let Some(wait) = self.port_waits.remove(&request) {
            workers::unsubscribe(wait.port, wait.subscription);
        } else if let Some(cancel) = self.process_cancellers.remove(&request) {
            let _ = cancel.send(());
        } else {
            self.tasks.abort(&request);
        }
        if let Some(completion) = self.ready.remove(&pending.sequence) {
            // Promotion allocates a resource before publishing its completion. Until the
            // language receives that completion, the request still owns the resource: cancelling
            // a queued accept/connect/listen must retire it along with the completion.
            let resource = match completion.result {
                Ok(AsyncHostValue::Resource(resource)) => Some(resource.id),
                Ok(AsyncHostValue::AcceptedTcpStream(accepted)) => Some(accepted.resource.id),
                _ => None,
            };
            if let Some(resource) = resource {
                let _ = self.close_resource(resource);
            }
        }
        true
    }

    fn cancel_if_pending(&mut self, request: RequestId) -> PendingCancellation {
        self.collect_ready_nonblocking();
        let Some(pending) = self.pending.get(&request) else {
            return PendingCancellation::Unknown;
        };
        if self.ready.contains_key(&pending.sequence) {
            return PendingCancellation::CompletionReady;
        }
        if AsyncHostOps::cancel(self, request) {
            PendingCancellation::Cancelled
        } else {
            PendingCancellation::Unknown
        }
    }

    fn has_referenced_work(&self) -> bool {
        self.pending.values().any(|request| request.referenced)
            || self.resources.values().any(|resource| match resource {
                HostResource::TcpListener { referenced, .. }
                | HostResource::TcpStream { referenced, .. }
                | HostResource::TcpTaken { referenced, .. } => *referenced,
                #[cfg(feature = "http")]
                HostResource::TcpUpgraded { referenced, .. } => *referenced,
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
    fn drain_on_shutdown(&mut self, executor: &Executor) {
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
        #[cfg(feature = "http")]
        self.abort_upgrade_prefixes();
    }

    fn shutdown_on_realm(&mut self) {
        if self.shutdown {
            return;
        }
        self.shutdown = true;
        self.tasks.abort_all();
        self.tasks = JoinMap::new();
        for (_, wait) in std::mem::take(&mut self.port_waits) {
            workers::unsubscribe(wait.port, wait.subscription);
        }
        self.pending.clear();
        self.ready.clear();
        #[cfg(feature = "http")]
        self.abort_upgrade_prefixes();
        self.resources.clear();
        self.listen_tickets.clear();
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

    fn needs_io_driver(&self) -> bool {
        // Port waits are woken by `completed` directly; only spawned tasks need the driver.
        !self.tasks.is_empty()
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

    fn completion_signal(&self) -> Option<Arc<tokio::sync::Notify>> {
        Some(Arc::clone(&self.completed))
    }

    fn release_transport(&mut self, resource: ResourceId) -> bool {
        match self.resources.get(&resource) {
            Some(HostResource::TcpTaken { .. }) => {
                self.listen_tickets.remove(&resource);
                self.resources.remove(&resource).is_some()
            }
            _ => false,
        }
    }

    #[cfg(feature = "http")]
    fn adopt_upgrade_transport(
        &mut self,
        resource: ResourceId,
        transport: HostTransport,
        pending_write: Vec<u8>,
        write_fences: Vec<http_engine::UpgradeWriteFenceBoundary>,
        write_completion: Arc<UpgradeWriteCompletion>,
    ) -> bool {
        let Some(HostResource::TcpTaken {
            info,
            referenced: _,
            restore_referenced,
        }) = self.resources.remove(&resource)
        else {
            write_completion.complete(Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "HTTP upgrade transport has no host resource",
            )));
            return false;
        };
        let (reader, writer) = tokio::io::split(transport.0);
        let writer = Rc::new(tokio::sync::Mutex::new(writer));
        let prefix_abort = if pending_write.is_empty() {
            write_completion.complete(Ok(()));
            None
        } else {
            let prefix_writer = Rc::clone(&writer);
            let prefix_completion = Arc::clone(&write_completion);
            let task = tokio::task::spawn_local(async move {
                let writing = async {
                    let mut writer = prefix_writer.lock().await;
                    let mut offset = 0usize;
                    for boundary in write_fences {
                        let end = boundary.offset;
                        if end < offset || end > pending_write.len() {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                "HTTP upgrade write fence offsets are out of order",
                            ));
                        }
                        writer.write_all(&pending_write[offset..end]).await?;
                        writer.flush().await?;
                        offset = end;
                        prefix_completion.complete_fence(boundary.fence, Ok(()));
                    }
                    writer.write_all(&pending_write[offset..]).await?;
                    writer.flush().await
                };
                tokio::pin!(writing);
                tokio::select! {
                    biased;
                    // An abort can complete the shared fence state while this task is blocked
                    // on a raw write. Dropping that future prevents later append bytes from
                    // escaping after their callbacks were failed. Check cancellation first,
                    // including when it was recorded before this task's initial poll.
                    result = prefix_completion.wait() => {
                        if result.is_ok() {
                            prefix_completion.complete(Ok(()));
                        }
                    },
                    result = &mut writing => prefix_completion.complete(result),
                }
            });
            Some(task.abort_handle())
        };
        self.resources.insert(
            resource,
            HostResource::TcpUpgraded {
                reader: Rc::new(tokio::sync::Mutex::new(reader)),
                writer,
                write_completion,
                prefix_abort,
                info,
                referenced: restore_referenced,
                read_closed: false,
                write_closed: false,
            },
        );
        true
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
                referenced,
            }) = self.resources.remove(&resource)
        {
            let _ = stream.set_nodelay(true);
            self.resources.insert(
                resource,
                HostResource::TcpTaken {
                    info,
                    referenced: false,
                    restore_referenced: referenced,
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
                restore_referenced: referenced,
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
                if self.tasks.is_empty() && self.port_waits.is_empty() {
                    return Ok(None);
                }
                if self.tasks.is_empty() {
                    self.completed.notified().await;
                    continue;
                }
                let (request, result) = tokio::select! {
                    joined = self.tasks.join_next() =>
                        joined.expect("a non-empty task map must yield a task exit"),
                    _ = self.completed.notified(), if !self.port_waits.is_empty() => continue,
                };
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
        AsyncHostOp::ProcessRun { .. }
        | AsyncHostOp::PortWait { .. }
        | AsyncHostOp::AtomicsWait { .. } => {
            unreachable!("process and port operations are handled by start_on")
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

pub(crate) fn bind_tcp_listener_std(
    host: &str,
    port: u16,
) -> std::io::Result<std::net::TcpListener> {
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
///
/// Macrotasks come from three lanes, taken in the order of one turn of node's event loop (libuv's
/// timers, poll and check phases): the timers that were due when the turn began, then the I/O
/// callbacks that were queued when the poll phase began (a message port's delivery), then the
/// `setImmediate` callbacks that were queued when the check phase began. Work a phase queues
/// for its own lane waits for the next turn, which is what keeps a ping-pong between two ports,
/// or an immediate that schedules an immediate, from starving the other lanes. Until
/// 2026-09-27 an immediate was a zero-delay timeout, so an immediate scheduled before a
/// `postMessage` ran before the message, and one scheduled from a timer callback after a
/// zero-delay timeout scheduled beside it; node runs both the other way round.
pub struct Realm<M = WorkId, T = WorkId> {
    now: Duration,
    next_sequence: u64,
    next_timer_id: u64,
    microtasks: VecDeque<M>,
    next_ticks: VecDeque<M>,
    timers: TimerQueue<T>,
    io: VecDeque<(TimerId, T)>,
    immediates: VecDeque<(TimerId, T)>,
    phase: Phase,
    cancelled_timers: HashSet<TimerId, SequenceBuild>,
    active_timers: HashSet<TimerId, SequenceBuild>,
    /// Whether a process warning has been written, after which node leaves off its
    /// `--trace-warnings` hint.
    warned: bool,
    _not_send: PhantomData<Rc<()>>,
}

/// Where a realm is in its turn (see [`Realm`]).
#[derive(Debug, Clone, Copy)]
enum Phase {
    /// Running the timers due at `time` and armed before `sequence`: a timer armed by one of
    /// them, even with no delay, belongs to the next turn, as libuv's does.
    Timers { time: Duration, sequence: u64 },
    /// Running the `remaining` I/O callbacks at the front of the lane.
    Io { remaining: usize },
    /// Running the `remaining` immediates at the front of the lane.
    Check { remaining: usize },
}

/// The macrotask a realm runs next.
#[derive(Debug, Clone, Copy)]
enum Next {
    Timer,
    Io,
    Immediate,
    /// Nothing runs until the clock reaches this timer's deadline.
    Wait(Duration),
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
            timers: TimerQueue::new(),
            io: VecDeque::new(),
            immediates: VecDeque::new(),
            warned: false,
            // The finished end of a turn, so the first question starts one.
            phase: Phase::Check { remaining: 0 },
            cancelled_timers: HashSet::default(),
            active_timers: HashSet::default(),
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

    /// Whether a Node next-tick callback is queued.
    pub fn has_queued_next_ticks(&self) -> bool {
        !self.next_ticks.is_empty()
    }

    /// Whether a microtask (a promise job included) is queued.
    pub fn has_queued_microtasks(&self) -> bool {
        !self.microtasks.is_empty()
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
        self.io
            .iter()
            .chain(&self.immediates)
            .for_each(|(_, work)| visit(work));
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

    /// Node's delay for `setTimeout` and `setInterval` (`lib/internal/timers.js`), and the
    /// warning it writes: anything but a number of milliseconds from 1 to 2^31 - 1 is 1 ms, and
    /// a NaN, a negative number or one too large says so on stderr. No delay at all (`undefined`)
    /// is 1 ms without a word. Both engines ask this before arming a timer, so a zero-delay
    /// timeout lands behind a 1 ms one armed first, as it does in node.
    pub fn node_timer_delay(&mut self, requested_ms: Option<f64>) -> (Duration, Option<String>) {
        const TIMEOUT_MAX_MS: f64 = 2_147_483_647.0;
        let Some(after) = requested_ms else {
            return (Duration::from_millis(1), None);
        };
        if (1.0..=TIMEOUT_MAX_MS).contains(&after) {
            return (duration_from_milliseconds(after), None);
        }
        let spelled = || {
            let mut digits = [0u8; kiln_number_core::MAX_FORMATTED_LEN];
            let written = kiln_number_core::format_number(after, &mut digits).unwrap_or(0);
            String::from_utf8_lossy(&digits[..written]).into_owned()
        };
        let warning = if after.is_nan() {
            Some(("TimeoutNaNWarning", "NaN is not a number.".to_owned()))
        } else if after > TIMEOUT_MAX_MS {
            Some((
                "TimeoutOverflowWarning",
                format!("{} does not fit into a 32-bit signed integer.", spelled()),
            ))
        } else if after < 0.0 {
            Some((
                "TimeoutNegativeWarning",
                format!("{} is a negative number.", spelled()),
            ))
        } else {
            None
        };
        let warning = warning.map(|(name, message)| {
            self.process_warning(name, &format!("{message}\nTimeout duration was set to 1."))
        });
        (Duration::from_millis(1), warning)
    }

    /// A process warning as node's default `warning` listener writes it to stderr: the
    /// `--trace-warnings` hint follows the first one only.
    pub fn process_warning(&mut self, name: &str, message: &str) -> String {
        let mut text = format!("(node:{}) {name}: {message}\n", crate::signals::pid());
        if !std::mem::replace(&mut self.warned, true) {
            text.push_str(
                "(Use `node --trace-warnings ...` to show where the warning was created)\n",
            );
        }
        text
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

    /// Queues a `setImmediate` callback: node's check phase, after this turn's I/O callbacks.
    /// Its identity is a timer's, so `clear_timeout` cancels it.
    pub fn set_immediate(&mut self, work: T) -> TimerId {
        let id = self.new_timer_id();
        self.immediates.push_back((id, work));
        id
    }

    /// Queues an I/O callback: node's poll phase, after the timers due when a turn begins and
    /// before its immediates. A message port's delivery is one; nothing else in a realm reaches
    /// it yet, since a Tokio completion is its own turn (`RuntimeTurn::Completion`).
    pub fn queue_io(&mut self, work: T) -> TimerId {
        let id = self.new_timer_id();
        self.io.push_back((id, work));
        id
    }

    fn new_timer_id(&mut self) -> TimerId {
        let id = TimerId(self.next_timer_id);
        self.next_timer_id = self.next_timer_id.wrapping_add(1);
        self.active_timers.insert(id);
        id
    }

    fn schedule_timer(&mut self, delay: Duration, work: T, interval: Option<Duration>) -> TimerId {
        // Saturating arithmetic gives callers the same well-defined behaviour as a timer delay
        // clamped to the largest representable duration, without allowing an integer overflow to
        // become a scheduler panic.
        let deadline = self.now.saturating_add(delay);
        let id = self.new_timer_id();
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.timers.push(
            delay,
            TimerEntry {
                deadline,
                sequence,
                id,
                work,
                interval,
            },
        );
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

    /// When the next macrotask can run: now, when one is ready, or the deadline of the timer
    /// the realm is waiting for. Cancelled entries are discarded on the way.
    pub fn next_timer_deadline(&mut self) -> Option<Duration> {
        match self.next()? {
            Next::Wait(deadline) => Some(deadline),
            Next::Timer | Next::Io | Next::Immediate => Some(self.now),
        }
    }

    /// Which lane the next macrotask comes from, moving through the turn's phases as each one
    /// runs out.
    fn next(&mut self) -> Option<Next> {
        loop {
            match self.phase {
                Phase::Timers { time, sequence } => {
                    self.discard_cancelled_timers();
                    if self
                        .timers
                        .peek_key()
                        .is_some_and(|(deadline, armed, _)| deadline <= time && armed < sequence)
                    {
                        return Some(Next::Timer);
                    }
                    self.phase = Phase::Io {
                        remaining: self.io.len(),
                    };
                }
                Phase::Io { remaining } => {
                    let remaining = discard_cancelled_front(
                        &mut self.io,
                        &mut self.cancelled_timers,
                        remaining,
                    );
                    if remaining > 0 {
                        self.phase = Phase::Io { remaining };
                        return Some(Next::Io);
                    }
                    self.phase = Phase::Check {
                        remaining: self.immediates.len(),
                    };
                }
                Phase::Check { remaining } => {
                    let remaining = discard_cancelled_front(
                        &mut self.immediates,
                        &mut self.cancelled_timers,
                        remaining,
                    );
                    if remaining > 0 {
                        self.phase = Phase::Check { remaining };
                        return Some(Next::Immediate);
                    }
                    self.phase = Phase::Check { remaining: 0 };
                    // The turn is over. The next one starts now if anything is ready at this
                    // time; otherwise the realm waits for its earliest timer.
                    if self.io.is_empty() && self.immediates.is_empty() {
                        self.discard_cancelled_timers();
                        let (deadline, _, _) = self.timers.peek_key()?;
                        if deadline > self.now {
                            return Some(Next::Wait(deadline));
                        }
                    }
                    self.begin_turn();
                }
            }
        }
    }

    fn begin_turn(&mut self) {
        self.phase = Phase::Timers {
            time: self.now,
            sequence: self.next_sequence,
        };
    }

    fn discard_cancelled_timers(&mut self) {
        if self.cancelled_timers.is_empty() {
            return;
        }
        while self
            .timers
            .peek_key()
            .is_some_and(|(_, _, id)| self.cancelled_timers.contains(&TimerId(id)))
        {
            let timer = self.timers.pop().expect("peeked timer exists");
            self.cancelled_timers.remove(&timer.id);
        }
    }

    /// The work of the timer expected to run `ahead` timers after the next one, for a cache hint
    /// (see `TimerQueue::upcoming`). Not an ordering promise.
    pub fn upcoming_timer_work(&self, ahead: usize) -> Option<&T> {
        self.timers.upcoming(ahead).map(|entry| &entry.work)
    }

    /// Take the next macrotask -- advancing virtual time to the earliest timer when nothing is
    /// ready before it -- re-arming an interval.
    pub fn run_next_timer(&mut self) -> Option<WorkItem<M, T>>
    where
        T: Clone,
    {
        let lane = match self.next()? {
            Next::Wait(deadline) => {
                self.now = self.now.max(deadline);
                self.begin_turn();
                return self.run_next_timer();
            }
            Next::Timer => return self.take_timer(),
            Next::Io => &mut self.io,
            Next::Immediate => &mut self.immediates,
        };
        // `next` has just counted this entry in the phase that runs it.
        let (id, work) = lane.pop_front().expect("the phase counted this entry");
        if let Phase::Io { remaining } | Phase::Check { remaining } = &mut self.phase {
            *remaining -= 1;
        }
        self.active_timers.remove(&id);
        Some(WorkItem::Timer { id, work })
    }

    fn take_timer(&mut self) -> Option<WorkItem<M, T>>
    where
        T: Clone,
    {
        let timer = self.timers.pop()?;
        self.now = self.now.max(timer.deadline);
        let TimerEntry {
            deadline,
            id,
            work,
            interval,
            ..
        } = timer;
        if let Some(interval) = interval {
            // Rearm before handing control to the callback. `clear_timeout` can therefore cancel
            // the next occurrence from within the current callback.
            let sequence = self.next_sequence;
            self.next_sequence = self.next_sequence.wrapping_add(1);
            let rearmed = work.clone();
            self.timers.push(
                interval,
                TimerEntry {
                    deadline: deadline.saturating_add(interval),
                    sequence,
                    id,
                    work: rearmed,
                    interval: Some(interval),
                },
            );
        } else {
            self.active_timers.remove(&id);
        }
        Some(WorkItem::Timer { id, work })
    }
}

/// Drops cancelled entries from the front of the first `remaining` of an I/O or check lane,
/// answering how many of those are left.
fn discard_cancelled_front<T>(
    lane: &mut VecDeque<(TimerId, T)>,
    cancelled: &mut HashSet<TimerId, SequenceBuild>,
    mut remaining: usize,
) -> usize {
    while remaining > 0
        && let Some((id, _)) = lane.front()
        && cancelled.remove(id)
    {
        lane.pop_front();
        remaining -= 1;
    }
    remaining
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
    /// When set, each write is handed here (`true` for stderr) the moment it happens and is not
    /// retained in `stdout`/`stderr`. A program that never finishes, a server, would otherwise
    /// say nothing until it was killed.
    stream: Option<Box<dyn FnMut(bool, &str)>>,
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
            stream: None,
        }
    }
}

impl CaptureHost {
    pub fn new() -> Self {
        Self::default()
    }

    /// Deliver standard-stream writes to `stream(is_stderr, text)` as they happen instead of
    /// buffering them; [`Self::stdout`] and [`Self::stderr`] then stay empty.
    pub fn with_stream(mut self, stream: Box<dyn FnMut(bool, &str)>) -> Self {
        self.stream = Some(stream);
        self
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
                let is_stderr = operation == HostOp::WriteStderr;
                let sink = if is_stderr {
                    &mut self.stderr
                } else {
                    &mut self.stdout
                };
                let start = sink.len();
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
                if let Some(stream) = self.stream.as_mut() {
                    stream(is_stderr, &sink[start..]);
                    sink.clear();
                }
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
mod tests;
