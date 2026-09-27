//! In-process multi-realm worker pool and actor hosting.
//!
//! Under ADR 0028, each JavaScript realm is owned by exactly one current-thread Tokio runtime on
//! an OS thread. [`WorkerPool`] is where the process gets those threads: every realm it hosts is
//! pinned to one of them (ADR 0070's 1:1 model, which stays as pinning once a scheduler exists),
//! and the pool counts them against its capacity.
//!
//! It hosts two kinds of realm. A [`RealmRuntime`] served from a mailbox
//! ([`WorkerPool::spawn_realm_with_factory`]), which the interpreter's actors use; and a realm
//! that owns its own executor and runs to completion on the thread it was made on
//! ([`WorkerPool::spawn_pinned`]) -- a native realm, which the provider archive creates, enters
//! and drives itself, several of them at once in one process (realms issue 05).
//!
//! Cross-realm communication is mediated by asynchronous mailboxes (`tokio::sync::mpsc`) and
//! boundary-safe types ([`HostValue`], [`HostError`]).

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use kiln_host_abi::{HostError, HostValue};

use crate::{RealmCapabilities, RealmHostProvider, RealmRuntime, TokioHost};

/// Default mailbox buffer size for inter-realm envelope queues.
pub const DEFAULT_MAILBOX_CAPACITY: usize = 1024;

/// Unique identifier for an actor instance hosted within a worker realm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ActorId(u64);

impl ActorId {
    /// Creates an actor identity from a raw numeric representation.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw numeric representation.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ActorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Actor({})", self.0)
    }
}

/// Errors arising from worker realm operations or actor messaging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerError {
    /// The worker pool has reached its maximum worker thread capacity.
    CapacityExceeded,
    /// The worker mailbox queue is full.
    QueueFull,
    /// The worker realm thread or communication channel was closed.
    Closed,
    /// The target actor was not found in the worker realm.
    ActorNotFound(ActorId),
    /// An error returned by the actor invocation.
    ActorError(HostError),
    /// A task panicked while executing on the worker realm thread.
    Panicked(String),
    /// Failed to spawn the worker thread or initialize the realm runtime.
    SpawnFailed(String),
}

impl fmt::Display for WorkerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CapacityExceeded => write!(f, "worker pool capacity exceeded"),
            Self::QueueFull => write!(f, "worker mailbox queue is full"),
            Self::Closed => write!(f, "worker realm channel is closed"),
            Self::ActorNotFound(id) => write!(f, "actor {id} not found"),
            Self::ActorError(err) => write!(f, "actor error: {err:?}"),
            Self::Panicked(msg) => write!(f, "worker task panicked: {msg}"),
            Self::SpawnFailed(msg) => write!(f, "failed to spawn worker realm: {msg}"),
        }
    }
}

impl std::error::Error for WorkerError {}

/// Handler for stateful actor message dispatch inside a worker realm.
///
/// An actor runs strictly single-threaded on its assigned worker realm thread and has direct
/// mutable access to the realm's [`RealmRuntime`].
pub trait ActorHandler<H: RealmHostProvider = TokioHost>: Send + 'static {
    /// Handles one method invocation on the actor.
    fn handle(
        &mut self,
        runtime: &mut RealmRuntime<H>,
        method: &str,
        args: &[HostValue],
    ) -> Result<HostValue, HostError>;
}

impl<H, F> ActorHandler<H> for F
where
    H: RealmHostProvider + 'static,
    F: FnMut(&mut RealmRuntime<H>, &str, &[HostValue]) -> Result<HostValue, HostError>
        + Send
        + 'static,
{
    fn handle(
        &mut self,
        runtime: &mut RealmRuntime<H>,
        method: &str,
        args: &[HostValue],
    ) -> Result<HostValue, HostError> {
        self(runtime, method, args)
    }
}

/// Internal envelope types sent across the worker thread mailbox channel.
enum WorkerEnvelope<H: RealmHostProvider + 'static> {
    Task(Box<dyn FnOnce(&mut RealmRuntime<H>) + Send + 'static>),
    CallActor {
        target: ActorId,
        method: String,
        args: Vec<HostValue>,
        reply: tokio::sync::oneshot::Sender<Result<HostValue, WorkerError>>,
    },
    RegisterActor {
        actor: Box<dyn ActorHandler<H>>,
        reply: tokio::sync::oneshot::Sender<ActorId>,
    },
    UnregisterActor {
        target: ActorId,
        reply: tokio::sync::oneshot::Sender<bool>,
    },
    Shutdown,
}

/// An owned handle for communicating with an in-process worker realm on a dedicated OS thread.
///
/// Handles are cheap to clone and can be shared across tasks and threads. All messages sent through
/// a handle arrive at the worker realm in FIFO order.
pub struct WorkerRealmHandle<H: RealmHostProvider + 'static = TokioHost> {
    id: u64,
    sender: tokio::sync::mpsc::Sender<WorkerEnvelope<H>>,
}

impl<H: RealmHostProvider + 'static> Clone for WorkerRealmHandle<H> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            sender: self.sender.clone(),
        }
    }
}

impl<H: RealmHostProvider + 'static> fmt::Debug for WorkerRealmHandle<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkerRealmHandle")
            .field("id", &self.id)
            .field("is_closed", &self.sender.is_closed())
            .finish()
    }
}

impl<H: RealmHostProvider + 'static> WorkerRealmHandle<H> {
    /// Returns the unique numeric identity of this worker realm.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Returns whether the worker realm thread or channel has closed.
    pub fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }

    /// Asynchronously executes a closure with mutable access to the worker's [`RealmRuntime`].
    pub async fn execute<F, R>(&self, f: F) -> Result<R, WorkerError>
    where
        F: FnOnce(&mut RealmRuntime<H>) -> R + Send + 'static,
        R: Send + 'static,
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let task = Box::new(move |runtime: &mut RealmRuntime<H>| {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(runtime)));
            let _ = tx.send(result);
        });

        self.sender
            .send(WorkerEnvelope::Task(task))
            .await
            .map_err(|_| WorkerError::Closed)?;

        match rx.await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(panic_payload)) => Err(WorkerError::Panicked(downcast_panic(panic_payload))),
            Err(_) => Err(WorkerError::Closed),
        }
    }

    /// Synchronously executes a closure on the worker realm, blocking the calling thread until completion.
    pub fn execute_blocking<F, R>(&self, f: F) -> Result<R, WorkerError>
    where
        F: FnOnce(&mut RealmRuntime<H>) -> R + Send + 'static,
        R: Send + 'static,
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let task = Box::new(move |runtime: &mut RealmRuntime<H>| {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(runtime)));
            let _ = tx.send(result);
        });

        self.sender
            .blocking_send(WorkerEnvelope::Task(task))
            .map_err(|_| WorkerError::Closed)?;

        match rx.blocking_recv() {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(panic_payload)) => Err(WorkerError::Panicked(downcast_panic(panic_payload))),
            Err(_) => Err(WorkerError::Closed),
        }
    }

    /// Asynchronously registers an actor handler within this worker realm, returning its [`ActorId`].
    pub async fn register_actor<A: ActorHandler<H>>(
        &self,
        actor: A,
    ) -> Result<ActorId, WorkerError> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        self.sender
            .send(WorkerEnvelope::RegisterActor {
                actor: Box::new(actor),
                reply,
            })
            .await
            .map_err(|_| WorkerError::Closed)?;

        rx.await.map_err(|_| WorkerError::Closed)
    }

    /// Synchronously registers an actor handler within this worker realm.
    pub fn register_actor_blocking<A: ActorHandler<H>>(
        &self,
        actor: A,
    ) -> Result<ActorId, WorkerError> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        self.sender
            .blocking_send(WorkerEnvelope::RegisterActor {
                actor: Box::new(actor),
                reply,
            })
            .map_err(|_| WorkerError::Closed)?;

        rx.blocking_recv().map_err(|_| WorkerError::Closed)
    }

    /// Asynchronously invokes a method on a registered actor.
    pub async fn call_actor(
        &self,
        target: ActorId,
        method: impl Into<String>,
        args: Vec<HostValue>,
    ) -> Result<HostValue, WorkerError> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        self.sender
            .send(WorkerEnvelope::CallActor {
                target,
                method: method.into(),
                args,
                reply,
            })
            .await
            .map_err(|_| WorkerError::Closed)?;

        match rx.await {
            Ok(result) => result,
            Err(_) => Err(WorkerError::Closed),
        }
    }

    /// Synchronously invokes a method on a registered actor.
    pub fn call_actor_blocking(
        &self,
        target: ActorId,
        method: impl Into<String>,
        args: Vec<HostValue>,
    ) -> Result<HostValue, WorkerError> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        self.sender
            .blocking_send(WorkerEnvelope::CallActor {
                target,
                method: method.into(),
                args,
                reply,
            })
            .map_err(|_| WorkerError::Closed)?;

        match rx.blocking_recv() {
            Ok(result) => result,
            Err(_) => Err(WorkerError::Closed),
        }
    }

    /// Asynchronously unregisters an actor from this worker realm.
    pub async fn unregister_actor(&self, target: ActorId) -> Result<bool, WorkerError> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        self.sender
            .send(WorkerEnvelope::UnregisterActor { target, reply })
            .await
            .map_err(|_| WorkerError::Closed)?;

        rx.await.map_err(|_| WorkerError::Closed)
    }

    /// Synchronously unregisters an actor from this worker realm.
    pub fn unregister_actor_blocking(&self, target: ActorId) -> Result<bool, WorkerError> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        self.sender
            .blocking_send(WorkerEnvelope::UnregisterActor { target, reply })
            .map_err(|_| WorkerError::Closed)?;

        rx.blocking_recv().map_err(|_| WorkerError::Closed)
    }

    /// Asynchronously signals the worker realm to terminate its turn loop and shut down.
    pub async fn shutdown(&self) -> Result<(), WorkerError> {
        self.sender
            .send(WorkerEnvelope::Shutdown)
            .await
            .map_err(|_| WorkerError::Closed)
    }

    /// Synchronously signals the worker realm to terminate its turn loop and shut down.
    pub fn shutdown_blocking(&self) -> Result<(), WorkerError> {
        self.sender
            .blocking_send(WorkerEnvelope::Shutdown)
            .map_err(|_| WorkerError::Closed)
    }
}

/// A realm running on its own thread of a [`WorkerPool`] ([`WorkerPool::spawn_pinned`]).
pub struct PinnedRealm<T> {
    id: u64,
    thread: std::thread::JoinHandle<T>,
}

impl<T> PinnedRealm<T> {
    /// The worker's identity, as its thread's name spells it.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Waits for the realm to finish and returns what it returned.
    pub fn join(self) -> Result<T, WorkerError> {
        self.thread
            .join()
            .map_err(|payload| WorkerError::Panicked(downcast_panic(payload)))
    }
}

struct WorkerDropGuard {
    active_count: Arc<AtomicUsize>,
}

impl Drop for WorkerDropGuard {
    fn drop(&mut self) {
        self.active_count.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Actor registry local to one worker realm thread.
struct ActorRegistry<H: RealmHostProvider + 'static> {
    next_id: u64,
    actors: BTreeMap<ActorId, Box<dyn ActorHandler<H>>>,
}

impl<H: RealmHostProvider + 'static> Default for ActorRegistry<H> {
    fn default() -> Self {
        Self {
            next_id: 1,
            actors: BTreeMap::new(),
        }
    }
}

/// A pool of background OS worker threads, each running an independent current-thread [`RealmRuntime`].
pub struct WorkerPool<H: RealmHostProvider + 'static = TokioHost> {
    capacity: usize,
    mailbox_capacity: usize,
    active_count: Arc<AtomicUsize>,
    next_worker_id: Arc<AtomicU64>,
    workers: Arc<std::sync::Mutex<Vec<tokio::sync::mpsc::Sender<WorkerEnvelope<H>>>>>,
}

impl<H: RealmHostProvider + 'static> fmt::Debug for WorkerPool<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkerPool")
            .field("capacity", &self.capacity)
            .field("active_count", &self.active_count.load(Ordering::Relaxed))
            .field("mailbox_capacity", &self.mailbox_capacity)
            .finish()
    }
}

impl<H: RealmHostProvider + 'static> Default for WorkerPool<H> {
    fn default() -> Self {
        let capacity = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        Self::new(capacity)
    }
}

impl WorkerPool<TokioHost> {
    /// Spawns a new worker realm with default capabilities and a standard TokioHost.
    pub fn spawn_realm(&self) -> Result<WorkerRealmHandle<TokioHost>, WorkerError> {
        self.spawn_realm_with(RealmCapabilities::default())
    }

    /// Spawns a new worker realm with explicit environmental capabilities.
    pub fn spawn_realm_with(
        &self,
        capabilities: RealmCapabilities,
    ) -> Result<WorkerRealmHandle<TokioHost>, WorkerError> {
        self.spawn_realm_with_factory(TokioHost::new, capabilities)
    }
}

impl<H: RealmHostProvider + 'static> WorkerPool<H> {
    /// Creates a new worker pool with a maximum worker thread capacity.
    pub fn new(capacity: usize) -> Self {
        Self::with_mailbox_capacity(capacity, DEFAULT_MAILBOX_CAPACITY)
    }

    /// Creates a new worker pool with explicit thread capacity and mailbox channel buffer size.
    pub fn with_mailbox_capacity(capacity: usize, mailbox_capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            mailbox_capacity: mailbox_capacity.max(1),
            active_count: Arc::new(AtomicUsize::new(0)),
            next_worker_id: Arc::new(AtomicU64::new(1)),
            workers: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    /// Returns the maximum worker capacity of this pool.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns the number of currently active worker realms.
    pub fn active_count(&self) -> usize {
        self.active_count.load(Ordering::SeqCst)
    }

    /// Spawns a new worker realm running on an independent OS thread with an explicit host instance (requires Host: Send).
    pub fn spawn_realm_with_host(
        &self,
        host: H,
        capabilities: RealmCapabilities,
    ) -> Result<WorkerRealmHandle<H>, WorkerError>
    where
        H: Send,
    {
        self.spawn_realm_with_factory(move || host, capabilities)
    }

    /// The one way the pool starts a thread: counted against its capacity until `body` returns,
    /// and named after the worker it is.
    fn spawn_thread<F, T>(&self, body: F) -> Result<(u64, std::thread::JoinHandle<T>), WorkerError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let current = self.active_count.fetch_add(1, Ordering::SeqCst);
        if current >= self.capacity {
            self.active_count.fetch_sub(1, Ordering::SeqCst);
            return Err(WorkerError::CapacityExceeded);
        }
        let worker_id = self.next_worker_id.fetch_add(1, Ordering::Relaxed);
        let guard = WorkerDropGuard {
            active_count: Arc::clone(&self.active_count),
        };
        std::thread::Builder::new()
            .name(format!("kiln-worker-{worker_id}"))
            .spawn(move || {
                let _guard = guard;
                body()
            })
            .map(|handle| (worker_id, handle))
            .map_err(|err| WorkerError::SpawnFailed(err.to_string()))
    }

    /// Runs `realm` on a thread of its own, counted against the pool's capacity: a realm pinned
    /// to that thread for its whole life (ADR 0070), which makes, enters, drives and destroys
    /// itself there -- a native realm owns its executor and never waits on a mailbox.
    pub fn spawn_pinned<F, T>(&self, realm: F) -> Result<PinnedRealm<T>, WorkerError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let (id, thread) = self.spawn_thread(realm)?;
        Ok(PinnedRealm { id, thread })
    }

    /// Spawns a new worker realm running on an independent OS thread with a host factory closure.
    pub fn spawn_realm_with_factory<F>(
        &self,
        host_factory: F,
        capabilities: RealmCapabilities,
    ) -> Result<WorkerRealmHandle<H>, WorkerError>
    where
        F: FnOnce() -> H + Send + 'static,
    {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(self.mailbox_capacity);
        let (init_tx, init_rx) = std::sync::mpsc::sync_channel(1);

        let spawn_result = self.spawn_thread(move || {
            let host = host_factory();
            let runtime_result = RealmRuntime::with_host_and_capabilities(host, capabilities);
            let mut runtime = match runtime_result {
                Ok(rt) => {
                    let _ = init_tx.send(Ok(()));
                    rt
                }
                Err(err) => {
                    let _ = init_tx.send(Err(WorkerError::SpawnFailed(err.to_string())));
                    return;
                }
            };

            let executor = std::rc::Rc::clone(runtime.executor());
            let mut registry = ActorRegistry::<H>::default();

            executor.block_on(async {
                while let Some(envelope) = receiver.recv().await {
                    match envelope {
                        WorkerEnvelope::Task(task) => {
                            task(&mut runtime);
                        }
                        WorkerEnvelope::CallActor {
                            target,
                            method,
                            args,
                            reply,
                        } => {
                            let res = match registry.actors.get_mut(&target) {
                                Some(actor) => match actor.handle(&mut runtime, &method, &args) {
                                    Ok(val) => Ok(val),
                                    Err(err) => Err(WorkerError::ActorError(err)),
                                },
                                None => Err(WorkerError::ActorNotFound(target)),
                            };
                            let _ = reply.send(res);
                        }
                        WorkerEnvelope::RegisterActor { actor, reply } => {
                            let id = ActorId::new(registry.next_id);
                            registry.next_id = registry.next_id.wrapping_add(1).max(1);
                            registry.actors.insert(id, actor);
                            let _ = reply.send(id);
                        }
                        WorkerEnvelope::UnregisterActor { target, reply } => {
                            let removed = registry.actors.remove(&target).is_some();
                            let _ = reply.send(removed);
                        }
                        WorkerEnvelope::Shutdown => {
                            break;
                        }
                    }
                }

                runtime.shutdown();
            });
        });

        let worker_id = match spawn_result {
            Ok((worker_id, _detached)) => worker_id,
            Err(err) => return Err(err),
        };

        match init_rx.recv() {
            Ok(Ok(())) => {
                let handle = WorkerRealmHandle {
                    id: worker_id,
                    sender: sender.clone(),
                };
                if let Ok(mut lock) = self.workers.lock() {
                    lock.push(sender);
                }
                Ok(handle)
            }
            Ok(Err(err)) => Err(err),
            Err(_) => Err(WorkerError::SpawnFailed(
                "worker thread panicked during initialization".into(),
            )),
        }
    }

    /// Signals all active worker realms to shut down.
    pub fn shutdown(&self) {
        if let Ok(mut lock) = self.workers.lock() {
            for sender in lock.drain(..) {
                let _ = sender.try_send(WorkerEnvelope::Shutdown);
            }
        }
    }
}

impl<H: RealmHostProvider + 'static> Drop for WorkerPool<H> {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn downcast_panic(payload: Box<dyn std::any::Any + Send + 'static>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "worker task panicked".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ScriptedHost, ScriptedTransport};
    use kiln_host_abi::ResourceId;
    use std::time::{Duration, Instant};

    #[test]
    fn spawn_worker_realm_and_execute_task() {
        let pool = WorkerPool::new(4);
        assert_eq!(pool.active_count(), 0);

        let handle = pool.spawn_realm().expect("failed to spawn realm");
        assert_eq!(pool.active_count(), 1);
        assert!(!handle.is_closed());

        let result = handle
            .execute_blocking(|runtime| {
                assert!(!runtime.is_shutdown());
                42u32
            })
            .expect("task execution failed");
        assert_eq!(result, 42);

        handle.shutdown_blocking().expect("shutdown failed");
        // Give thread a moment to terminate and decrement active count
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(pool.active_count(), 0);
    }

    #[test]
    fn spawning_is_fast_and_under_threshold() {
        let pool = WorkerPool::new(4);
        let start = Instant::now();
        let handle = pool.spawn_realm().expect("failed to spawn realm");
        let duration = start.elapsed();

        // Target is < 1ms; allow generous tolerance for loaded CI/concurrent builds (20ms)
        assert!(
            duration < Duration::from_millis(20),
            "spawning took too long: {duration:?}"
        );

        let res = handle.execute_blocking(|_| 100).unwrap();
        assert_eq!(res, 100);
        handle.shutdown_blocking().unwrap();
    }

    #[test]
    fn multiple_realms_have_disjoint_state() {
        let pool = WorkerPool::new(4);
        let realm1 = pool.spawn_realm().unwrap();
        let realm2 = pool.spawn_realm().unwrap();
        assert_eq!(pool.active_count(), 2);

        let res1 = realm1
            .execute_blocking(|runtime| {
                runtime.use_host_clock();
                1
            })
            .unwrap();

        let res2 = realm2
            .execute_blocking(|runtime| {
                // realm2 clock capability remains default (Virtual)
                assert_eq!(
                    runtime.capabilities().clock,
                    crate::ClockCapability::Virtual
                );
                2
            })
            .unwrap();

        assert_eq!(res1, 1);
        assert_eq!(res2, 2);

        realm1.shutdown_blocking().unwrap();
        realm2.shutdown_blocking().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(pool.active_count(), 0);
    }

    #[test]
    fn actor_registration_and_message_call() {
        let pool = WorkerPool::new(2);
        let realm = pool.spawn_realm().unwrap();

        // State machine actor
        struct CounterActor {
            count: i64,
        }

        impl ActorHandler<TokioHost> for CounterActor {
            fn handle(
                &mut self,
                _runtime: &mut RealmRuntime<TokioHost>,
                method: &str,
                args: &[HostValue],
            ) -> Result<HostValue, HostError> {
                match method {
                    "get" => Ok(HostValue::number(self.count as f64)),
                    "increment" => {
                        let amount = match args.first() {
                            Some(HostValue::Number(bits)) => f64::from_bits(*bits) as i64,
                            _ => 1,
                        };
                        self.count += amount;
                        Ok(HostValue::number(self.count as f64))
                    }
                    _ => Err(HostError::new("counter: unknown method")),
                }
            }
        }

        let actor_id = realm
            .register_actor_blocking(CounterActor { count: 10 })
            .expect("failed to register actor");

        let initial = realm
            .call_actor_blocking(actor_id, "get", vec![])
            .expect("get failed");
        assert_eq!(initial.as_number(), Some(10.0));

        let updated = realm
            .call_actor_blocking(actor_id, "increment", vec![HostValue::number(5.0)])
            .expect("increment failed");
        assert_eq!(updated.as_number(), Some(15.0));

        // Unregister
        let removed = realm
            .unregister_actor_blocking(actor_id)
            .expect("failed to unregister");
        assert!(removed);

        // Subsequent call returns ActorNotFound
        let err = realm
            .call_actor_blocking(actor_id, "get", vec![])
            .unwrap_err();
        assert_eq!(err, WorkerError::ActorNotFound(actor_id));

        realm.shutdown_blocking().unwrap();
    }

    #[test]
    fn panic_in_worker_task_is_safely_caught() {
        let pool = WorkerPool::new(2);
        let realm = pool.spawn_realm().unwrap();

        let err = realm
            .execute_blocking(|_| {
                panic!("deliberate panic inside task");
            })
            .unwrap_err();

        match err {
            WorkerError::Panicked(msg) => {
                assert!(msg.contains("deliberate panic inside task"));
            }
            other => panic!("expected Panicked, got: {other:?}"),
        }

        // Realm thread survived and can execute subsequent tasks!
        let ok = realm.execute_blocking(|_| "alive").unwrap();
        assert_eq!(ok, "alive");

        realm.shutdown_blocking().unwrap();
    }

    #[test]
    fn pinned_realms_run_at_once_each_on_its_own_counted_thread() {
        let pool = WorkerPool::<TokioHost>::new(3);
        // The three realms and this thread: none of them returns until all four are here.
        let barrier = Arc::new(std::sync::Barrier::new(4));
        let realms: Vec<_> = (0..3)
            .map(|index| {
                let barrier = Arc::clone(&barrier);
                pool.spawn_pinned(move || {
                    barrier.wait();
                    (index, std::thread::current().name().map(str::to_owned))
                })
                .expect("a pinned realm starts")
            })
            .collect();
        assert_eq!(
            pool.spawn_pinned(|| ()).map(|_| ()).unwrap_err(),
            WorkerError::CapacityExceeded
        );
        barrier.wait();
        for (expected, realm) in realms.into_iter().enumerate() {
            let id = realm.id();
            let (index, name) = realm.join().expect("the realm finishes");
            assert_eq!(index, expected);
            assert_eq!(name, Some(format!("kiln-worker-{id}")));
        }
        let failing = pool.spawn_pinned(|| panic!("deliberate")).expect("starts");
        assert!(matches!(failing.join(), Err(WorkerError::Panicked(_))));
    }

    #[test]
    fn capacity_limit_is_enforced() {
        let pool = WorkerPool::new(2);
        let r1 = pool.spawn_realm().unwrap();
        let r2 = pool.spawn_realm().unwrap();

        let r3_err = pool.spawn_realm().unwrap_err();
        assert_eq!(r3_err, WorkerError::CapacityExceeded);

        r1.shutdown_blocking().unwrap();
        std::thread::sleep(Duration::from_millis(20));

        // Now capacity is available
        let r4 = pool.spawn_realm().unwrap();
        r2.shutdown_blocking().unwrap();
        r4.shutdown_blocking().unwrap();
    }

    #[cfg(feature = "http")]
    #[test]
    fn worker_shutdown_drops_http_connection_before_first_poll() {
        let resource = ResourceId::new(901);
        let pool = WorkerPool::<ScriptedHost>::new(1);
        let realm = pool
            .spawn_realm_with_factory(
                move || {
                    ScriptedHost::new([])
                        .with_transport(resource, ScriptedTransport::new([], false))
                },
                RealmCapabilities::default(),
            )
            .expect("worker realm starts");

        realm
            .execute_blocking(move |runtime| {
                assert!(runtime.server_attach_queued(resource, false));
                // Shutdown immediately after admission, before the worker can receive another
                // mailbox turn. This exercises owner-drop cleanup rather than a protocol Close.
                assert!(runtime.shutdown());
                assert!(!runtime.http_owns(resource));
                assert!(!runtime.has_live_http_work());
            })
            .expect("attach and shutdown execute");
        realm.shutdown_blocking().expect("worker shutdown");
    }
}
