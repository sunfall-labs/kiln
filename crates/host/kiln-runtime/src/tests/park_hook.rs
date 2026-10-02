//! The park hook (ADR 0074): a realm parked inside a foreign event loop wakes for its next
//! timer exactly, for completions from any thread, and for the loop's own events, without
//! polling when no I/O needs the driver.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::{DriveWake, ParkHook, wait_for_turn_in, workers};

/// A stand-in for a GUI run loop: `wait` sleeps until its timeout or a `wake`, and reports
/// events when a test arms them. Each test uses its own slot so tests can run in parallel.
struct FakeLoop {
    woken: Mutex<bool>,
    changed: Condvar,
    waits: AtomicU32,
    last_timeout_us: AtomicU64,
    events_pending: AtomicBool,
}

impl FakeLoop {
    const fn new() -> Self {
        Self {
            woken: Mutex::new(false),
            changed: Condvar::new(),
            waits: AtomicU32::new(0),
            last_timeout_us: AtomicU64::new(0),
            events_pending: AtomicBool::new(false),
        }
    }

    fn wait(&self, timeout_ms: f64) -> i32 {
        self.waits.fetch_add(1, Ordering::SeqCst);
        self.last_timeout_us.store(
            if timeout_ms < 0.0 {
                u64::MAX
            } else {
                (timeout_ms * 1000.0) as u64
            },
            Ordering::SeqCst,
        );
        if self.events_pending.swap(false, Ordering::SeqCst) {
            return 1;
        }
        let mut woken = self.woken.lock().unwrap();
        let deadline = (timeout_ms >= 0.0)
            .then(|| Instant::now() + Duration::from_secs_f64(timeout_ms / 1000.0));
        while !*woken {
            match deadline {
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        break;
                    }
                    woken = self.changed.wait_timeout(woken, deadline - now).unwrap().0;
                }
                None => woken = self.changed.wait(woken).unwrap(),
            }
        }
        *woken = false;
        0
    }

    fn wake(&self) {
        *self.woken.lock().unwrap() = true;
        self.changed.notify_all();
    }
}

macro_rules! fake_loop {
    ($name:ident) => {
        mod $name {
            pub static LOOP: super::FakeLoop = super::FakeLoop::new();
            pub extern "C" fn wait(timeout_ms: f64) -> i32 {
                LOOP.wait(timeout_ms)
            }
            pub extern "C" fn wake() {
                LOOP.wake()
            }
        }
    };
}

fake_loop!(deadline_loop);
fake_loop!(completion_loop);
fake_loop!(io_loop);
fake_loop!(event_loop);

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn idle_park_waits_for_the_exact_deadline_in_one_wait() {
    let hook = ParkHook {
        wait: deadline_loop::wait,
        wake: deadline_loop::wake,
        port: 0,
    };
    let signal = Arc::new(tokio::sync::Notify::new());
    let started = Instant::now();
    let wake = runtime().block_on(wait_for_turn_in(
        hook,
        signal,
        Some(Duration::from_millis(60)),
        || false,
    ));
    let elapsed = started.elapsed();
    assert_eq!(wake, DriveWake::Timer);
    assert!(
        elapsed >= Duration::from_millis(60),
        "woke early: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(250),
        "woke late: {elapsed:?}"
    );
    // One wait for the whole interval (a second only if the first returned a hair early):
    // no polling.
    assert!(deadline_loop::LOOP.waits.load(Ordering::SeqCst) <= 2);
    assert!(deadline_loop::LOOP.last_timeout_us.load(Ordering::SeqCst) > 0);
}

#[test]
fn a_completion_from_another_thread_interrupts_the_wait() {
    let hook = ParkHook {
        wait: completion_loop::wait,
        wake: completion_loop::wake,
        port: 0,
    };
    let signal = Arc::new(tokio::sync::Notify::new());
    let remote = Arc::clone(&signal);
    let started = Instant::now();
    let notifier = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(40));
        remote.notify_one();
    });
    // No deadline at all: only the completion can end this park.
    let wake = runtime().block_on(wait_for_turn_in(hook, signal, None, || false));
    notifier.join().unwrap();
    assert_eq!(wake, DriveWake::Completion);
    assert!(started.elapsed() < Duration::from_secs(2));
    // The first wait was unbounded: the park relied on `wake`, not on a poll.
    assert_eq!(
        completion_loop::LOOP.last_timeout_us.load(Ordering::SeqCst),
        u64::MAX
    );
}

#[test]
fn pending_io_keeps_the_driver_turning_in_short_slices() {
    let hook = ParkHook {
        wait: io_loop::wait,
        wake: io_loop::wake,
        port: 0,
    };
    let signal = Arc::new(tokio::sync::Notify::new());
    let wake = runtime().block_on(wait_for_turn_in(
        hook,
        signal,
        Some(Duration::from_millis(30)),
        || true,
    ));
    assert_eq!(wake, DriveWake::Timer);
    assert!(
        io_loop::LOOP.waits.load(Ordering::SeqCst) >= 3,
        "the driver was not turned between slices"
    );
    assert!(io_loop::LOOP.last_timeout_us.load(Ordering::SeqCst) <= 4_000);
}

#[test]
fn loop_events_are_announced_on_the_hook_port() {
    let (port, _peer) = workers::channel();
    let hook = ParkHook {
        wait: event_loop::wait,
        wake: event_loop::wake,
        port,
    };
    let signal = Arc::new(tokio::sync::Notify::new());
    event_loop::LOOP
        .events_pending
        .store(true, Ordering::SeqCst);
    let wake = runtime().block_on(wait_for_turn_in(
        hook,
        signal,
        Some(Duration::from_millis(500)),
        || false,
    ));
    assert!(
        workers::readable(port),
        "events were not posted to the port"
    );
    assert!(workers::take(port).is_some());
    // Posting to an unsubscribed port wakes nobody here, so the park ends at its deadline.
    assert_eq!(wake, DriveWake::Timer);
}
