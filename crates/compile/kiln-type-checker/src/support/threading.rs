//! Safe synchronization primitives used by the checker port.

use std::ops::{Deref, DerefMut};

/// Mutex protected value with a poisoning-free lock interface.
pub struct Guarded<T>(std::sync::Mutex<T>);

impl<T> Guarded<T> {
    pub const fn new(value: T) -> Self {
        Self(std::sync::Mutex::new(value))
    }
    pub fn lock(&self) -> GuardedLock<'_, T> {
        GuardedLock(Some(
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        ))
    }
    pub fn get_mut(&mut self) -> &mut T {
        self.0
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl<T: Default> Default for Guarded<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

pub struct GuardedLock<'a, T>(Option<std::sync::MutexGuard<'a, T>>);
impl<T> Deref for GuardedLock<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.0.as_deref().expect("guard temporarily released")
    }
}
impl<T> DerefMut for GuardedLock<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.0.as_deref_mut().expect("guard temporarily released")
    }
}

#[derive(Default)]
pub struct Condvar(std::sync::Condvar);
impl Condvar {
    pub const fn new() -> Self {
        Self(std::sync::Condvar::new())
    }
    pub fn notify_one(&self) {
        self.0.notify_one();
    }
    pub fn notify_all(&self) {
        self.0.notify_all();
    }
    pub fn wait_guarded<T>(&self, guard: &mut GuardedLock<'_, T>) {
        let held = guard.0.take().expect("guard already released");
        let held = self
            .0
            .wait(held)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.0 = Some(held);
    }
}

pub const DEFAULT_THREAD_STACK_SIZE: u32 = 2 * 1024 * 1024;

#[derive(Clone, Copy)]
pub struct Config {
    pub stack_size: u32,
    pub max_threads: u32,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            stack_size: DEFAULT_THREAD_STACK_SIZE,
            max_threads: 0,
        }
    }
}

/// A scoped worker pool facade. Each call starts bounded scoped workers and
/// joins every worker before returning; no submitted callback can be lost.
pub struct ThreadPool {
    max_threads: usize,
    stack_size: usize,
}
impl ThreadPool {
    pub fn init(config: Config) -> Self {
        Self {
            max_threads: (config.max_threads as usize).max(1),
            stack_size: (config.stack_size as usize).max(1),
        }
    }
    pub fn max_threads(&self) -> usize {
        self.max_threads
    }

    pub fn each_while<Ctx, V, F>(
        &self,
        ctx: Ctx,
        run_fn: F,
        values: &mut [V],
        on_calling_thread: impl FnOnce(),
    ) where
        F: Fn(&Ctx, V, usize) + Sync,
        Ctx: Sync,
        V: Copy + Sync + Send,
    {
        let workers = self.max_threads.min(values.len());
        if workers == 0 {
            on_calling_thread();
            return;
        }
        let values: &[V] = values;
        std::thread::scope(|scope| {
            let mut joins = Vec::with_capacity(workers);
            for worker in 0..workers {
                let start = worker * values.len() / workers;
                let end = (worker + 1) * values.len() / workers;
                let run_fn = &run_fn;
                let ctx = &ctx;
                let stack_size = self.stack_size;
                let builder = std::thread::Builder::new().stack_size(stack_size);
                joins.push(
                    builder
                        .spawn_scoped(scope, move || {
                            for index in start..end {
                                run_fn(ctx, values[index], worker);
                            }
                        })
                        .expect("checker worker spawn failed"),
                );
            }
            on_calling_thread();
            for join in joins {
                join.join().expect("checker worker panicked");
            }
        });
    }
}
