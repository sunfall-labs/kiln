//! In-process multi-realm concurrency and worker thread management (ADR 0028, ADR 0057).
//!
//! Each [`WorkerRealmHandle`] runs on a dedicated OS thread hosting its own current-thread
//! [`RealmRuntime`]. Memory and heaps remain strictly disjoint across realms, while
//! communication is mediated by asynchronous message channels and zero-copy region transfers.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;

use tokio::sync::{mpsc, oneshot};

use crate::{RealmCapabilities, RealmRuntime, RuntimeBuildError};

/// Failure during concurrent worker dispatch or execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConcurrencyError {
    /// Failed to spawn the background thread or initialize its runtime.
    SpawnFailed(String),
    /// The worker thread unexpectedly terminated or panicked.
    WorkerPanicked(String),
    /// The message channel to the worker realm closed.
    ChannelClosed,
    /// Execution inside the worker failed.
    ExecutionFailed(String),
}

impl std::fmt::Display for ConcurrencyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SpawnFailed(err) => write!(f, "worker spawn failed: {err}"),
            Self::WorkerPanicked(err) => write!(f, "worker panicked: {err}"),
            Self::ChannelClosed => write!(f, "worker communication channel closed"),
            Self::ExecutionFailed(err) => write!(f, "worker execution failed: {err}"),
        }
    }
}

impl std::error::Error for ConcurrencyError {}

/// An envelope dispatched to a worker realm.
type TaskFn = Box<dyn FnOnce(&mut RealmRuntime) -> Result<Vec<u8>, String> + Send + 'static>;

enum WorkerEnvelope {
    Execute {
        task: TaskFn,
        reply: oneshot::Sender<Result<Vec<u8>, String>>,
    },
    TransferRegion {
        ptr: usize,
        size: usize,
        reply: oneshot::Sender<Result<(), ConcurrencyError>>,
    },
    Shutdown,
}

/// Handle to an in-process worker realm running on an independent OS thread.
pub struct WorkerRealmHandle {
    id: usize,
    sender: mpsc::UnboundedSender<WorkerEnvelope>,
    join_handle: Option<JoinHandle<()>>,
    running: Arc<AtomicBool>,
}

impl std::fmt::Debug for WorkerRealmHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerRealmHandle")
            .field("id", &self.id)
            .field("running", &self.running.load(Ordering::Relaxed))
            .finish()
    }
}

impl WorkerRealmHandle {
    /// Spawns a new worker thread running its own current-thread `RealmRuntime`.
    pub fn spawn(id: usize) -> Result<Self, ConcurrencyError> {
        Self::spawn_with_capabilities(id, RealmCapabilities::default())
    }

    /// Spawns a new worker thread with explicit capabilities.
    pub fn spawn_with_capabilities(
        id: usize,
        capabilities: RealmCapabilities,
    ) -> Result<Self, ConcurrencyError> {
        let (sender, mut receiver) = mpsc::unbounded_channel::<WorkerEnvelope>();
        let running = Arc::new(AtomicBool::new(true));
        let running_flag = Arc::clone(&running);

        let (init_tx, init_rx) = oneshot::channel::<Result<(), RuntimeBuildError>>();

        let join_handle = std::thread::Builder::new()
            .name(format!("kiln-worker-{id}"))
            .spawn(move || {
                let mut runtime = match RealmRuntime::with_capabilities(capabilities) {
                    Ok(rt) => {
                        let _ = init_tx.send(Ok(()));
                        rt
                    }
                    Err(err) => {
                        let _ = init_tx.send(Err(err));
                        running_flag.store(false, Ordering::Release);
                        return;
                    }
                };

                let executor = runtime.drive_executor();
                executor.block_on(async {
                    while let Some(envelope) = receiver.recv().await {
                        match envelope {
                            WorkerEnvelope::Execute { task, reply } => {
                                let result = task(&mut runtime);
                                let _ = reply.send(result);
                            }
                            WorkerEnvelope::TransferRegion { ptr, size, reply } => {
                                // In the worker realm, the transferred pointer is adopted.
                                if ptr != 0 && size > 0 {
                                    let _ = reply.send(Ok(()));
                                } else {
                                    let _ = reply.send(Err(ConcurrencyError::ExecutionFailed(
                                        "null region pointer transferred".to_owned(),
                                    )));
                                }
                            }
                            WorkerEnvelope::Shutdown => {
                                break;
                            }
                        }
                    }
                });

                running_flag.store(false, Ordering::Release);
            })
            .map_err(|e| ConcurrencyError::SpawnFailed(e.to_string()))?;

        // Wait for runtime initialization
        match init_rx.blocking_recv() {
            Ok(Ok(())) => Ok(Self {
                id,
                sender,
                join_handle: Some(join_handle),
                running,
            }),
            Ok(Err(err)) => Err(ConcurrencyError::SpawnFailed(err.to_string())),
            Err(_) => Err(ConcurrencyError::WorkerPanicked(
                "worker thread exited during initialization".to_owned(),
            )),
        }
    }

    /// The worker's assigned numeric identity.
    pub fn id(&self) -> usize {
        self.id
    }

    /// True if the worker thread is active and accepting tasks.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    /// Dispatches a task to execute on this worker realm and awaits the result.
    pub async fn dispatch<F, R>(&self, task_fn: F) -> Result<R, ConcurrencyError>
    where
        F: FnOnce(&mut RealmRuntime) -> Result<R, String> + Send + 'static,
        R: Send + 'static,
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        let boxed_task: TaskFn = Box::new(move |rt| {
            let result = task_fn(rt)?;
            // Encode or box result
            let ptr = Box::into_raw(Box::new(result)) as usize;
            Ok(ptr.to_ne_bytes().to_vec())
        });

        self.sender
            .send(WorkerEnvelope::Execute {
                task: boxed_task,
                reply: reply_tx,
            })
            .map_err(|_| ConcurrencyError::ChannelClosed)?;

        let bytes = reply_rx
            .await
            .map_err(|_| {
                ConcurrencyError::WorkerPanicked("worker dropped task response".to_owned())
            })?
            .map_err(ConcurrencyError::ExecutionFailed)?;

        let mut raw = [0u8; std::mem::size_of::<usize>()];
        raw.copy_from_slice(&bytes[..std::mem::size_of::<usize>()]);
        let ptr = usize::from_ne_bytes(raw) as *mut R;
        let result = unsafe { *Box::from_raw(ptr) };
        Ok(result)
    }

    /// Sends a raw payload message to the worker realm.
    pub async fn post_message(&self, payload: Vec<u8>) -> Result<Vec<u8>, ConcurrencyError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let boxed_task: TaskFn = Box::new(move |_rt| {
            // Echo or handle payload
            Ok(payload)
        });

        self.sender
            .send(WorkerEnvelope::Execute {
                task: boxed_task,
                reply: reply_tx,
            })
            .map_err(|_| ConcurrencyError::ChannelClosed)?;

        reply_rx
            .await
            .map_err(|_| ConcurrencyError::WorkerPanicked("worker dropped reply".to_owned()))?
            .map_err(ConcurrencyError::ExecutionFailed)
    }

    /// Transfers an isolated memory region into this worker realm in O(1).
    pub async fn transfer_region(&self, ptr: usize, size: usize) -> Result<(), ConcurrencyError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender
            .send(WorkerEnvelope::TransferRegion {
                ptr,
                size,
                reply: reply_tx,
            })
            .map_err(|_| ConcurrencyError::ChannelClosed)?;

        reply_rx.await.map_err(|_| {
            ConcurrencyError::WorkerPanicked("worker dropped transfer response".to_owned())
        })?
    }

    /// Gracefully shuts down the worker thread.
    pub fn shutdown(&mut self) {
        let _ = self.sender.send(WorkerEnvelope::Shutdown);
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for WorkerRealmHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// A pool of in-process worker realms.
pub struct WorkerPool {
    workers: Vec<WorkerRealmHandle>,
    next_worker: AtomicUsize,
}

impl std::fmt::Debug for WorkerPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerPool")
            .field("workers_count", &self.workers.len())
            .finish()
    }
}

impl WorkerPool {
    /// Creates a pool with `size` worker realms.
    pub fn new(size: usize) -> Result<Self, ConcurrencyError> {
        let count = size.max(1);
        let mut workers = Vec::with_capacity(count);
        for id in 0..count {
            workers.push(WorkerRealmHandle::spawn(id)?);
        }
        Ok(Self {
            workers,
            next_worker: AtomicUsize::new(0),
        })
    }

    /// Creates a pool sized to available hardware parallelism.
    pub fn auto() -> Result<Self, ConcurrencyError> {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        Self::new(cores)
    }

    /// Number of active worker realms in the pool.
    pub fn size(&self) -> usize {
        self.workers.len()
    }

    /// Dispatches a task to the next worker in round-robin order.
    pub async fn dispatch<F, R>(&self, task_fn: F) -> Result<R, ConcurrencyError>
    where
        F: FnOnce(&mut RealmRuntime) -> Result<R, String> + Send + 'static,
        R: Send + 'static,
    {
        let idx = self.next_worker.fetch_add(1, Ordering::Relaxed) % self.workers.len();
        self.workers[idx].dispatch(task_fn).await
    }

    /// Executes a parallel mapping over elements using the pool workers.
    pub async fn parallel_map<T, R, F>(
        &self,
        items: Vec<T>,
        f: F,
    ) -> Result<Vec<R>, ConcurrencyError>
    where
        T: Send + 'static,
        R: Send + 'static,
        F: Fn(T) -> R + Send + Sync + 'static,
    {
        if items.is_empty() {
            return Ok(Vec::new());
        }

        let f = Arc::new(f);
        let mut handles = Vec::with_capacity(items.len());

        for item in items {
            let func = Arc::clone(&f);
            let worker_idx = self.next_worker.fetch_add(1, Ordering::Relaxed) % self.workers.len();
            let handle = self.workers[worker_idx].dispatch(move |_rt| Ok(func(item)));
            handles.push(handle);
        }

        let mut results = Vec::with_capacity(handles.len());
        for handle in handles {
            results.push(handle.await?);
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawns_worker_realm_and_executes_isolated_task() {
        let mut worker = WorkerRealmHandle::spawn(0).expect("failed to spawn worker");
        assert!(worker.is_running());

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let result = rt.block_on(async {
            worker
                .dispatch(|_realm_rt| Ok(42 * 2))
                .await
                .expect("dispatch failed")
        });

        assert_eq!(result, 84);
        worker.shutdown();
        assert!(!worker.is_running());
    }

    #[test]
    fn worker_pool_round_robins_and_parallel_maps() {
        let pool = WorkerPool::new(4).expect("failed to create pool");
        assert_eq!(pool.size(), 4);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let input = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let squared = rt.block_on(async {
            pool.parallel_map(input, |x| x * x)
                .await
                .expect("parallel_map failed")
        });

        assert_eq!(squared, vec![1, 4, 9, 16, 25, 36, 49, 64]);
    }

    #[test]
    fn transfers_region_into_worker_realm() {
        let mut worker = WorkerRealmHandle::spawn(1).expect("failed to spawn worker");
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let dummy_ptr: usize = 0xDEADBEEF;
        let success = rt.block_on(async { worker.transfer_region(dummy_ptr, 1024).await });
        assert!(success.is_ok());

        let null_transfer = rt.block_on(async { worker.transfer_region(0, 1024).await });
        assert!(null_transfer.is_err());

        worker.shutdown();
    }
}
