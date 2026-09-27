//! Process signals delivered to realms: the runtime half of node's `process.on(signal)` (realms
//! issue 23).
//!
//! Node listens for a signal only while `process` has a listener for it, and a signal it listens
//! for no longer does what the signal would otherwise do: that is the whole of graceful shutdown
//! (`process.on('SIGTERM', () => server.close())`). Here a realm *subscribes* to a signal
//! ([`listen`]) and gets an endpoint of the cross-realm ports (`crate::workers`); every arrival of
//! the signal queues one [`crate::workers::kind::SIGNAL`] message on every subscribed endpoint,
//! and the realm's TypeScript (`kiln-builtins/src/signals.ts`) waits on it with the port wait
//! every engine already has and emits the event from node's poll phase. Everything about
//! listeners and events is that TypeScript's; what is here is process-wide: the disposition of
//! each signal and who is subscribed to it.
//!
//! # Dispositions
//!
//! The first subscription to a signal installs this module's handler and keeps the action it
//! replaced; the last one to go puts that action back. The handler is async-signal-safe: it
//! writes the signal's number to a self-pipe and calls a replaced handler that was a function
//! (the native prelude's SIGUSR1 introspection dump keeps working under a listener). One thread
//! reads the pipe and queues the messages ([`pump`]). `SIGKILL` and `SIGSTOP` cannot be caught,
//! and [`listen`] answers `EINVAL` for them as `uv_signal_start` does.
//!
//! # A listener removed
//!
//! Node stops listening when the last listener goes; the realm here learns it at the next
//! arrival instead, finds nothing to emit to, and calls [`unhandled`], which unsubscribes it and
//! -- when no realm in the process listens any more -- restores the disposition and sends the
//! signal to the process again, so it does what it would have done. The difference is only
//! when: a realm stuck in synchronous code does not die of a signal it stopped listening for
//! until it reaches its event loop.
//!
//! # Several realms
//!
//! In `KILN_WORKERS` thread mode the launcher blocks the stop signals in every thread and takes
//! them with `sigwait` on one; it hands each to [`deliver`], and ends the process by the signal
//! only when no realm took it. A realm's subscriptions end with its thread
//! ([`release_thread`]), so a realm that has finished never swallows a signal.
//!
//! # A program that is not the process (realms issue 26)
//!
//! `kiln run` without `--one-shot` runs the program inside the resident compiler daemon, and a
//! program must observe exactly what it would observe run alone -- in particular it must never
//! be able to signal the daemon. So the launcher of an interpreted program wraps the run in
//! [`host_program`], and while that guard lives the realm's signals are *the program's*:
//!
//! * **Signals it sends itself are delivered in the realm, never with `kill(2)`.** "Itself" is
//!   its own `process.pid`; for a daemon-hosted program also the daemon's real pid, `0`, `-1`
//!   and the daemon's process group, since a program alone would have been in all of those. A
//!   listener gets the event exactly as issue 23 delivers it; with no listener the signal's
//!   default action happens to the program: a terminating signal ends the realm on the spot
//!   (the engine unwinds as `process.exit(128 + n)` would, and [`ProgramGuard::terminated_by`]
//!   tells the launcher, which reports a death by that signal -- the client re-raises it, so its
//!   own parent sees node's `WIFSIGNALED` status and 128 + n in a shell); an ignoring one
//!   (`SIGCHLD`, `SIGURG`, `SIGWINCH`, `SIGCONT`, `SIGINFO`) does nothing; a stopping one
//!   (`SIGSTOP`, `SIGTSTP`, `SIGTTIN`, `SIGTTOU`) does nothing in the daemon, which cannot be
//!   stopped for one program, and really stops a `--one-shot` process.
//! * **A daemon-hosted program has a virtual pid** ([`pid`]): one per run, above every real pid
//!   the kernels kiln runs on hand out (Linux's `pid_max` is at most 2^22, macOS's 99,999), so it
//!   can never name another process. A program that logs it, compares it with itself, or
//!   signals it behaves as alone; one that looks it up outside itself (`/proc/<pid>`, `ps`, a
//!   child's `process.ppid`) finds nothing or the daemon, and `kill(pid, 0)` from *another*
//!   process answers `ESRCH`. Its listeners take nothing from the process: the daemon's own
//!   dispositions are never touched, so a real `SIGTERM` to the daemon stops the daemon.
//! * **Signals to any other pid stay real `kill(2)`.**
//!
//! A `--one-shot` run is the process, so it keeps the real pid and real dispositions (a server
//! run that way still drains on an external `SIGTERM`); only its self-directed signals take the
//! in-realm path, which is what lets the launcher print what the program wrote before the
//! signal ended it. `node:worker_threads` workers inherit their program.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread::ThreadId;

use crate::workers::{self, Message, Token};

/// Signal names in the order the TypeScript and the engines number them: an index into this
/// table crosses the boundary instead of a platform's signal number, which differs between
/// Linux and macOS for a third of them. Append only.
pub const NAMES: &[&str] = &[
    "SIGHUP",
    "SIGINT",
    "SIGQUIT",
    "SIGILL",
    "SIGTRAP",
    "SIGABRT",
    "SIGIOT",
    "SIGBUS",
    "SIGFPE",
    "SIGKILL",
    "SIGUSR1",
    "SIGSEGV",
    "SIGUSR2",
    "SIGPIPE",
    "SIGALRM",
    "SIGTERM",
    "SIGCHLD",
    "SIGSTKFLT",
    "SIGCONT",
    "SIGSTOP",
    "SIGTSTP",
    "SIGTTIN",
    "SIGTTOU",
    "SIGURG",
    "SIGXCPU",
    "SIGXFSZ",
    "SIGVTALRM",
    "SIGPROF",
    "SIGWINCH",
    "SIGIO",
    "SIGPOLL",
    "SIGPWR",
    "SIGSYS",
    "SIGINFO",
    "SIGEMT",
    "SIGBREAK",
];

/// The platform's number for [`NAMES`]`[index]`, 0 where the platform has no such signal.
pub fn number(index: u32) -> i32 {
    let Some(name) = NAMES.get(index as usize) else {
        return 0;
    };
    platform::number(name)
}

/// Whether `signal` could be a signal number here at all.
fn valid(signal: i32) -> bool {
    (1..platform::SIGNAL_LIMIT).contains(&signal)
}

/// Whether this process hosts programs that are not it (the resident daemon): see the module
/// documentation. Set once, by the daemon, before it serves anything.
static ISOLATED: AtomicBool = AtomicBool::new(false);

/// The next virtual pid; see [`pid`].
static NEXT_VIRTUAL_PID: AtomicI32 = AtomicI32::new(VIRTUAL_PID_BASE);

/// Above every pid Linux (`PID_MAX_LIMIT`, 2^22) and macOS (99,999) hand out.
pub const VIRTUAL_PID_BASE: i32 = 1 << 22;

/// Make every program run under [`host_program`] from now on a virtual process: its own pid,
/// its signals delivered in its realm and never to this process. The resident daemon calls this
/// once at startup; a `--one-shot` run does not, and its program is the process.
pub fn isolate_hosted_programs() {
    ISOLATED.store(true, Ordering::Release);
}

/// One program's signal state, shared by its main realm and the workers it starts.
pub struct Program {
    pid: i32,
    isolated: bool,
    state: Mutex<ProgramState>,
}

#[derive(Default)]
struct ProgramState {
    /// This program's subscriptions, per signal.
    listeners: HashMap<i32, Vec<u64>>,
    /// The signal whose default action ended the program.
    terminated_by: Option<i32>,
}

impl Program {
    fn state(&self) -> MutexGuard<'_, ProgramState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Whether `kill(pid, _)` from this program names the program itself.
    fn targets_self(&self, pid: i32) -> bool {
        if pid == self.pid {
            return true;
        }
        if !self.isolated {
            return false;
        }
        #[cfg(unix)]
        // SAFETY: `getpid` and `getpgrp` cannot fail.
        let (real, group) = unsafe { (libc::getpid(), libc::getpgrp()) };
        #[cfg(not(unix))]
        let (real, group) = (std::process::id() as i32, 0);
        pid == real || pid == 0 || pid == -1 || (group > 0 && pid == -group)
    }

    /// `signal` arrived for this program; answers what [`kill`] does.
    fn signal_self(&self, signal: i32) -> i32 {
        if signal == 0 {
            return 0;
        }
        let mut state = self.state();
        if let Some(listeners) = state.listeners.get(&signal).filter(|l| !l.is_empty()) {
            for endpoint in listeners {
                let message = Message::new(
                    workers::kind::SIGNAL,
                    vec![Token::Number(f64::from(signal))],
                );
                workers::enqueue(*endpoint, message);
            }
            return 0;
        }
        self.default_action(&mut state, signal)
    }

    /// What `signal` does to a program that does not listen for it.
    fn default_action(&self, state: &mut ProgramState, signal: i32) -> i32 {
        match platform::default_action(signal) {
            Action::Ignore => 0,
            Action::Stop if self.isolated => 0,
            Action::Stop => {
                #[cfg(unix)]
                // SAFETY: `kill` on this process with a valid signal number.
                unsafe {
                    libc::kill(libc::getpid(), signal);
                }
                0
            }
            Action::Terminate => {
                state.terminated_by.get_or_insert(signal);
                0
            }
        }
    }

    fn release(&self) {
        let listeners: Vec<u64> = std::mem::take(&mut self.state().listeners)
            .into_values()
            .flatten()
            .collect();
        if !self.isolated {
            let mut registry = registry();
            for endpoint in &listeners {
                remove(&mut registry, *endpoint);
            }
        }
        for endpoint in listeners {
            workers::close(endpoint);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    Terminate,
    Ignore,
    Stop,
}

thread_local! {
    static PROGRAM: RefCell<Option<Arc<Program>>> = const { RefCell::new(None) };
}

/// The program the realm on this thread belongs to, if its launcher said.
pub fn current_program() -> Option<Arc<Program>> {
    PROGRAM.with(|program| program.borrow().clone())
}

/// Make this thread's realm part of `program` (a worker, on its own thread); `None` clears it.
pub fn adopt_program(program: Option<Arc<Program>>) {
    PROGRAM.with(|slot| *slot.borrow_mut() = program);
}

/// Run the program on this thread as a program of its own until the guard drops: see the module
/// documentation. A daemon that called [`isolate_hosted_programs`] gives it a virtual pid.
pub fn host_program() -> ProgramGuard {
    let isolated = ISOLATED.load(Ordering::Acquire);
    let pid = if isolated {
        NEXT_VIRTUAL_PID.fetch_add(1, Ordering::Relaxed)
    } else {
        real_pid()
    };
    let program = Arc::new(Program {
        pid,
        isolated,
        state: Mutex::new(ProgramState::default()),
    });
    let previous = PROGRAM.with(|slot| slot.borrow_mut().replace(Arc::clone(&program)));
    ProgramGuard { program, previous }
}

/// The program [`host_program`] started; it ends when this drops.
pub struct ProgramGuard {
    program: Arc<Program>,
    previous: Option<Arc<Program>>,
}

impl ProgramGuard {
    /// The program's `process.pid`.
    pub fn pid(&self) -> i32 {
        self.program.pid
    }

    /// The signal whose default action ended the program, if one did: the launcher reports a
    /// death by that signal, not the `128 + n` the realm unwound with.
    pub fn terminated_by(&self) -> Option<i32> {
        self.program.state().terminated_by
    }
}

impl Drop for ProgramGuard {
    fn drop(&mut self) {
        self.program.release();
        let previous = self.previous.take();
        PROGRAM.with(|slot| *slot.borrow_mut() = previous);
    }
}

fn real_pid() -> i32 {
    #[cfg(unix)]
    // SAFETY: `getpid` cannot fail.
    unsafe {
        libc::getpid()
    }
    #[cfg(not(unix))]
    {
        std::process::id() as i32
    }
}

/// `process.pid` for the realm on this thread: its program's, else this process's.
pub fn pid() -> i32 {
    PROGRAM
        .with(|program| program.borrow().as_ref().map(|program| program.pid))
        .unwrap_or_else(real_pid)
}

/// The signal that has ended this thread's program, if one has: an engine checks this after
/// `kill` and after [`unhandled`] and unwinds its realm as `process.exit(128 + n)` would.
pub fn termination() -> Option<i32> {
    PROGRAM.with(|program| {
        program
            .borrow()
            .as_ref()
            .and_then(|program| program.state().terminated_by)
    })
}

#[derive(Default)]
struct Registry {
    /// Subscribed endpoints per signal, in subscription order.
    subscribers: HashMap<i32, Vec<u64>>,
    /// Each subscription: its signal and the thread whose realm made it.
    endpoints: HashMap<u64, (i32, ThreadId)>,
    /// The action each installed handler replaced, restored when its last subscriber goes.
    #[cfg(unix)]
    replaced: HashMap<i32, libc::sigaction>,
}

fn registry() -> MutexGuard<'static, Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY
        .get_or_init(|| Mutex::new(Registry::default()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Subscribe this thread's realm to `signal`: the endpoint its arrivals are queued on, or the
/// errno that says why not (`EINVAL` for `SIGKILL`, `SIGSTOP` and anything not a signal).
pub fn listen(signal: i32) -> Result<u64, i32> {
    if !valid(signal) || platform::uncatchable(signal) {
        return Err(platform::EINVAL);
    }
    let program = current_program();
    let endpoint = match &program {
        // A hosted program's listener takes nothing from the process (see the module docs).
        Some(program) if program.isolated => workers::endpoint(),
        _ => listen_process(signal)?,
    };
    if let Some(program) = program {
        program
            .state()
            .listeners
            .entry(signal)
            .or_default()
            .push(endpoint);
    }
    Ok(endpoint)
}

/// Subscribe to `signal` arriving at the process.
fn listen_process(signal: i32) -> Result<u64, i32> {
    let mut registry = registry();
    let first = registry
        .subscribers
        .get(&signal)
        .is_none_or(|subscribers| subscribers.is_empty());
    if first {
        #[cfg(unix)]
        {
            let replaced = platform::install(signal)?;
            registry.replaced.insert(signal, replaced);
        }
        #[cfg(not(unix))]
        return Err(platform::ENOSYS);
    }
    let endpoint = workers::endpoint();
    registry
        .endpoints
        .insert(endpoint, (signal, std::thread::current().id()));
    registry
        .subscribers
        .entry(signal)
        .or_default()
        .push(endpoint);
    Ok(endpoint)
}

/// Remove one subscription; true when it was the signal's last.
fn remove(registry: &mut Registry, endpoint: u64) -> Option<(i32, bool)> {
    let (signal, _) = registry.endpoints.remove(&endpoint)?;
    let last = match registry.subscribers.get_mut(&signal) {
        Some(subscribers) => {
            subscribers.retain(|subscribed| *subscribed != endpoint);
            subscribers.is_empty()
        }
        None => true,
    };
    if last {
        registry.subscribers.remove(&signal);
        #[cfg(unix)]
        if let Some(replaced) = registry.replaced.remove(&signal) {
            platform::restore(signal, &replaced);
        }
    }
    Some((signal, last))
}

/// A signal arrived at a realm that no longer listens for it: end the subscription and, if no
/// realm in the process listens any more, restore the signal's disposition and send it to the
/// process again, so that it does what it would have done without a listener.
pub fn unhandled(endpoint: u64) {
    if let Some(program) = current_program() {
        let mut state = program.state();
        let owned = state.listeners.iter_mut().find_map(|(signal, listeners)| {
            let before = listeners.len();
            listeners.retain(|listener| *listener != endpoint);
            (listeners.len() != before).then_some(*signal)
        });
        if let Some(signal) = owned {
            if !program.isolated {
                // The process's disposition goes back when this was its last listener; the
                // signal itself is the program's to act on, below, not re-sent.
                remove(&mut registry(), endpoint);
            }
            workers::close(endpoint);
            if state.listeners.get(&signal).is_none_or(Vec::is_empty) {
                state.listeners.remove(&signal);
                program.default_action(&mut state, signal);
            }
            return;
        }
    }
    let removed = remove(&mut registry(), endpoint);
    workers::close(endpoint);
    if let Some((signal, true)) = removed {
        flush_stdio();
        #[cfg(unix)]
        // SAFETY: `kill` on this process with a valid signal number.
        unsafe {
            libc::kill(libc::getpid(), signal);
        }
        #[cfg(not(unix))]
        let _ = signal;
    }
}

/// End every subscription the realm on this thread made: its realm has finished, and a signal
/// must not wait on a realm that will never read it.
pub fn release_thread() {
    let thread = std::thread::current().id();
    let mut registry = registry();
    let mine: Vec<u64> = registry
        .endpoints
        .iter()
        .filter(|(_, (_, owner))| *owner == thread)
        .map(|(endpoint, _)| *endpoint)
        .collect();
    for endpoint in &mine {
        remove(&mut registry, *endpoint);
    }
    drop(registry);
    for endpoint in mine {
        workers::close(endpoint);
    }
}

/// Arrivals handed to [`deliver`] so far, per signal: what a realm that sent itself a signal
/// waits on.
static DELIVERED: [AtomicU64; SIGNAL_SLOTS] = [const { AtomicU64::new(0) }; SIGNAL_SLOTS];
const SIGNAL_SLOTS: usize = 65;

/// `signal` arrived: queue it for every realm listening. Answers how many there were.
pub fn deliver(signal: i32) -> usize {
    if let Some(count) = usize::try_from(signal)
        .ok()
        .and_then(|slot| DELIVERED.get(slot))
    {
        count.fetch_add(1, Ordering::AcqRel);
    }
    let registry = registry();
    let Some(subscribers) = registry.subscribers.get(&signal) else {
        return 0;
    };
    let mut delivered = 0;
    for endpoint in subscribers {
        let message = Message::new(
            workers::kind::SIGNAL,
            vec![Token::Number(f64::from(signal))],
        );
        if workers::enqueue(*endpoint, message) {
            delivered += 1;
        }
    }
    delivered
}

/// `kill(pid, signal)`: 0, or the errno.
///
/// A signal this process sends itself while a realm listens for it is queued before this
/// returns: the kernel runs the handler on whichever thread it picks (Linux: the main one, not
/// the sender), so the sender waits -- briefly; it is the handler's `write` and one pipe read --
/// for the arrival to reach [`deliver`]. That is what makes node's order hold for
/// `process.kill(process.pid, ...)`: the event in the poll phase of the very next loop turn.
pub fn kill(pid: i32, signal: i32) -> i32 {
    if signal != 0 && !valid(signal) {
        return platform::EINVAL;
    }
    if let Some(program) = current_program()
        && program.targets_self(pid)
    {
        return program.signal_self(signal);
    }
    #[cfg(unix)]
    {
        let slot = usize::try_from(signal)
            .ok()
            .filter(|slot| *slot < SIGNAL_SLOTS);
        // SAFETY: `getpid` cannot fail.
        let to_self = pid == unsafe { libc::getpid() }
            && signal != 0
            && registry().subscribers.contains_key(&signal);
        let before = slot.map_or(0, |slot| DELIVERED[slot].load(Ordering::Acquire));
        // A signal to this process that nothing listens for may end it here: what the program
        // wrote before goes out first, as node's synchronous standard streams have it.
        // SAFETY: `getpid` cannot fail.
        if !to_self && signal != 0 && (pid == unsafe { libc::getpid() } || pid <= 0) {
            flush_stdio();
        }
        // SAFETY: `kill` takes any pid and a checked signal number.
        if unsafe { libc::kill(pid, signal) } != 0 {
            return std::io::Error::last_os_error()
                .raw_os_error()
                .unwrap_or(platform::EINVAL);
        }
        if let (true, Some(slot)) = (to_self, slot) {
            let started = std::time::Instant::now();
            loop {
                pump();
                if DELIVERED[slot].load(Ordering::Acquire) != before
                    || started.elapsed() > std::time::Duration::from_millis(500)
                {
                    break;
                }
                std::thread::yield_now();
            }
        }
        0
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        platform::ENOSYS
    }
}

/// Write out what either engine's standard streams hold -- Rust's and C's -- before a signal
/// that may end the process.
fn flush_stdio() {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    #[cfg(unix)]
    // SAFETY: `fflush(NULL)` flushes every C stream.
    unsafe {
        libc::fflush(std::ptr::null_mut());
    }
}

/// Queue every signal the handler has recorded and nothing has queued yet.
pub fn pump() {
    #[cfg(unix)]
    platform::pump();
}

#[cfg(unix)]
mod platform {
    use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
    use std::sync::{Mutex, Once};

    pub const EINVAL: i32 = libc::EINVAL;
    /// One past the largest signal number: 65 covers Linux's real-time signals, and macOS's
    /// 32 are below it.
    pub const SIGNAL_LIMIT: i32 = 65;

    pub fn number(name: &str) -> i32 {
        match name {
            "SIGHUP" => libc::SIGHUP,
            "SIGINT" => libc::SIGINT,
            "SIGQUIT" => libc::SIGQUIT,
            "SIGILL" => libc::SIGILL,
            "SIGTRAP" => libc::SIGTRAP,
            "SIGABRT" | "SIGIOT" => libc::SIGABRT,
            "SIGBUS" => libc::SIGBUS,
            "SIGFPE" => libc::SIGFPE,
            "SIGKILL" => libc::SIGKILL,
            "SIGUSR1" => libc::SIGUSR1,
            "SIGSEGV" => libc::SIGSEGV,
            "SIGUSR2" => libc::SIGUSR2,
            "SIGPIPE" => libc::SIGPIPE,
            "SIGALRM" => libc::SIGALRM,
            "SIGTERM" => libc::SIGTERM,
            "SIGCHLD" => libc::SIGCHLD,
            #[cfg(any(target_os = "linux", target_os = "android"))]
            "SIGSTKFLT" => libc::SIGSTKFLT,
            "SIGCONT" => libc::SIGCONT,
            "SIGSTOP" => libc::SIGSTOP,
            "SIGTSTP" => libc::SIGTSTP,
            "SIGTTIN" => libc::SIGTTIN,
            "SIGTTOU" => libc::SIGTTOU,
            "SIGURG" => libc::SIGURG,
            "SIGXCPU" => libc::SIGXCPU,
            "SIGXFSZ" => libc::SIGXFSZ,
            "SIGVTALRM" => libc::SIGVTALRM,
            "SIGPROF" => libc::SIGPROF,
            "SIGWINCH" => libc::SIGWINCH,
            "SIGIO" => libc::SIGIO,
            #[cfg(any(target_os = "linux", target_os = "android"))]
            "SIGPOLL" => libc::SIGPOLL,
            #[cfg(any(target_os = "linux", target_os = "android"))]
            "SIGPWR" => libc::SIGPWR,
            "SIGSYS" => libc::SIGSYS,
            #[cfg(target_vendor = "apple")]
            "SIGINFO" => libc::SIGINFO,
            #[cfg(target_vendor = "apple")]
            "SIGEMT" => libc::SIGEMT,
            _ => 0,
        }
    }

    pub fn uncatchable(signal: i32) -> bool {
        signal == libc::SIGKILL || signal == libc::SIGSTOP
    }

    /// What `signal` does by default (POSIX's table; a core dump counts as terminating).
    pub fn default_action(signal: i32) -> super::Action {
        use super::Action;
        match signal {
            libc::SIGCHLD | libc::SIGURG | libc::SIGWINCH | libc::SIGCONT => Action::Ignore,
            #[cfg(target_vendor = "apple")]
            libc::SIGINFO => Action::Ignore,
            libc::SIGSTOP | libc::SIGTSTP | libc::SIGTTIN | libc::SIGTTOU => Action::Stop,
            _ => Action::Terminate,
        }
    }

    /// The self-pipe: read end, write end; -1 until the first handler is installed.
    static PIPE: [AtomicI32; 2] = [AtomicI32::new(-1), AtomicI32::new(-1)];
    /// Per signal, the handler this module replaced when it was a function, to call after
    /// recording (0: none), and whether it takes `siginfo_t`.
    static CHAINED: [AtomicUsize; SIGNAL_LIMIT as usize] =
        [const { AtomicUsize::new(0) }; SIGNAL_LIMIT as usize];
    static CHAINED_INFO: [AtomicUsize; SIGNAL_LIMIT as usize] =
        [const { AtomicUsize::new(0) }; SIGNAL_LIMIT as usize];
    /// Held across a read of the pipe and the delivery of what it read, so that a caller of
    /// `pump` never returns while another thread holds a signal it has read but not queued.
    static PUMPING: Mutex<()> = Mutex::new(());

    #[cfg(any(target_os = "linux", target_os = "android"))]
    unsafe fn errno_location() -> *mut libc::c_int {
        unsafe { libc::__errno_location() }
    }
    #[cfg(target_vendor = "apple")]
    unsafe fn errno_location() -> *mut libc::c_int {
        unsafe { libc::__error() }
    }
    #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
    unsafe fn errno_location() -> *mut libc::c_int {
        unsafe { libc::__errno() }
    }

    /// Async-signal-safe: a `write` to a non-blocking pipe and a call through an atomic.
    extern "C" fn on_signal(
        signal: libc::c_int,
        info: *mut libc::siginfo_t,
        context: *mut libc::c_void,
    ) {
        // SAFETY: errno is this thread's; it is put back before returning.
        let saved = unsafe { *errno_location() };
        let fd = PIPE[1].load(Ordering::Relaxed);
        if fd >= 0 {
            let byte = signal as u8;
            // SAFETY: one byte from the stack to a descriptor this module owns.
            unsafe {
                libc::write(fd, (&byte as *const u8).cast(), 1);
            }
        }
        if let Some(slot) = usize::try_from(signal)
            .ok()
            .filter(|slot| *slot < CHAINED.len())
        {
            let chained = CHAINED[slot].load(Ordering::Acquire);
            if chained != 0 {
                if CHAINED_INFO[slot].load(Ordering::Acquire) != 0 {
                    // SAFETY: the replaced handler was installed with SA_SIGINFO.
                    let handler: extern "C" fn(
                        libc::c_int,
                        *mut libc::siginfo_t,
                        *mut libc::c_void,
                    ) = unsafe { std::mem::transmute(chained) };
                    handler(signal, info, context);
                } else {
                    // SAFETY: the replaced handler was a plain one.
                    let handler: extern "C" fn(libc::c_int) =
                        unsafe { std::mem::transmute(chained) };
                    handler(signal);
                }
            }
        }
        // SAFETY: as above.
        unsafe { *errno_location() = saved };
    }

    fn start_pipe() -> Result<(), i32> {
        static STARTED: Once = Once::new();
        let mut failure = None;
        STARTED.call_once(|| {
            let mut fds = [-1; 2];
            // SAFETY: `fds` has room for two descriptors.
            if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
                failure = std::io::Error::last_os_error().raw_os_error();
                return;
            }
            for fd in fds {
                // SAFETY: descriptors this call just made.
                unsafe {
                    let flags = libc::fcntl(fd, libc::F_GETFL);
                    libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
                    let flags = libc::fcntl(fd, libc::F_GETFD);
                    libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
                }
            }
            PIPE[0].store(fds[0], Ordering::Release);
            PIPE[1].store(fds[1], Ordering::Release);
            let reader = fds[0];
            let spawned = std::thread::Builder::new()
                .name("kiln-signals".to_owned())
                .spawn(move || {
                    loop {
                        let mut wake = libc::pollfd {
                            fd: reader,
                            events: libc::POLLIN,
                            revents: 0,
                        };
                        // SAFETY: one pollfd on the stack.
                        unsafe { libc::poll(&mut wake, 1, -1) };
                        pump();
                    }
                });
            if spawned.is_err() {
                failure = Some(libc::EAGAIN);
            }
        });
        match failure {
            Some(errno) => Err(errno),
            None if PIPE[1].load(Ordering::Acquire) < 0 => Err(libc::EAGAIN),
            None => Ok(()),
        }
    }

    /// Put this module's handler on `signal`; the action it replaced.
    pub fn install(signal: i32) -> Result<libc::sigaction, i32> {
        start_pipe()?;
        // SAFETY: zeroed `sigaction`s are valid empty actions; `signal` was checked.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            let mut replaced: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_signal as *const () as usize;
            action.sa_flags = libc::SA_SIGINFO | libc::SA_RESTART | libc::SA_ONSTACK;
            libc::sigemptyset(&mut action.sa_mask);
            if libc::sigaction(signal, std::ptr::null(), &mut replaced) != 0 {
                return Err(std::io::Error::last_os_error()
                    .raw_os_error()
                    .unwrap_or(EINVAL));
            }
            let slot = signal as usize;
            let chained = replaced.sa_sigaction;
            if chained != libc::SIG_DFL
                && chained != libc::SIG_IGN
                && chained != on_signal as *const () as usize
            {
                CHAINED_INFO[slot].store(
                    usize::from(replaced.sa_flags & libc::SA_SIGINFO != 0),
                    Ordering::Release,
                );
                CHAINED[slot].store(chained, Ordering::Release);
            }
            if libc::sigaction(signal, &action, std::ptr::null_mut()) != 0 {
                CHAINED[slot].store(0, Ordering::Release);
                return Err(std::io::Error::last_os_error()
                    .raw_os_error()
                    .unwrap_or(EINVAL));
            }
            Ok(replaced)
        }
    }

    /// Put back the action [`install`] replaced.
    pub fn restore(signal: i32, replaced: &libc::sigaction) {
        // SAFETY: an action `sigaction` itself reported for this signal.
        unsafe {
            libc::sigaction(signal, replaced, std::ptr::null_mut());
        }
        CHAINED[signal as usize].store(0, Ordering::Release);
    }

    pub fn pump() {
        let reader = PIPE[0].load(Ordering::Acquire);
        if reader < 0 {
            return;
        }
        let _pumping = PUMPING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut bytes = [0_u8; 64];
        loop {
            // SAFETY: reading into a stack buffer of its own length.
            let read = unsafe { libc::read(reader, bytes.as_mut_ptr().cast(), bytes.len()) };
            let Ok(read) = usize::try_from(read) else {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return;
            };
            if read == 0 {
                return;
            }
            for byte in &bytes[..read] {
                super::deliver(i32::from(*byte));
            }
        }
    }
}

#[cfg(not(unix))]
mod platform {
    pub const EINVAL: i32 = 22;
    pub const ENOSYS: i32 = 38;
    pub const SIGNAL_LIMIT: i32 = 65;

    pub fn number(name: &str) -> i32 {
        match name {
            "SIGINT" => 2,
            "SIGILL" => 4,
            "SIGABRT" | "SIGIOT" => 22,
            "SIGFPE" => 8,
            "SIGKILL" => 9,
            "SIGSEGV" => 11,
            "SIGTERM" => 15,
            "SIGBREAK" => 21,
            _ => 0,
        }
    }

    pub fn uncatchable(signal: i32) -> bool {
        signal == 9
    }

    pub fn default_action(_signal: i32) -> super::Action {
        super::Action::Terminate
    }
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// The one test that raises a real signal in the test process: a subscription turns it into
    /// a message, `kill` has queued it by the time it returns, and giving the subscription up
    /// restores the disposition it replaced (ignored here, so the re-sent signal is harmless).
    #[test]
    fn a_subscription_queues_each_arrival_and_restores_the_disposition() {
        let signal = libc::SIGURG;
        assert_eq!(
            number(NAMES.iter().position(|name| *name == "SIGURG").unwrap() as u32),
            signal
        );
        assert_eq!(listen(libc::SIGKILL), Err(libc::EINVAL));
        assert_eq!(listen(libc::SIGSTOP), Err(libc::EINVAL));
        assert_eq!(listen(0), Err(libc::EINVAL));

        let endpoint = listen(signal).expect("SIGURG can be caught");
        // SAFETY: getpid cannot fail.
        let me = unsafe { libc::getpid() };
        assert_eq!(kill(me, signal), 0);
        assert_eq!(kill(me, signal), 0);
        for _ in 0..2 {
            let message =
                workers::take(endpoint).expect("each arrival is queued before kill returns");
            assert_eq!(message.kind, workers::kind::SIGNAL);
            assert!(
                matches!(message.tokens.as_slice(), [Token::Number(n)] if *n == f64::from(signal))
            );
        }
        assert!(workers::take(endpoint).is_none());

        unhandled(endpoint);
        // SAFETY: reading the current disposition only.
        let current = unsafe {
            let mut current: libc::sigaction = std::mem::zeroed();
            libc::sigaction(signal, std::ptr::null(), &mut current);
            current.sa_sigaction
        };
        assert_eq!(current, libc::SIG_DFL, "SIGURG's default (ignore) is back");
        assert_eq!(kill(me, 1234), libc::EINVAL);
        assert_eq!(kill(me, 0), 0);
    }

    /// A daemon-hosted program (realms issue 26): a virtual pid, and every signal it sends
    /// itself -- by that pid or by the daemon's -- stays in it. None of this may reach the test
    /// process: `SIGUSR2`'s default would end it, which is the property under test.
    #[test]
    fn a_hosted_program_signals_only_itself() {
        isolate_hosted_programs();
        let signal = libc::SIGUSR2;
        // SAFETY: getpid cannot fail.
        let me = unsafe { libc::getpid() };
        let hosted = host_program();
        assert!(hosted.pid() >= VIRTUAL_PID_BASE);
        assert_eq!(pid(), hosted.pid());

        let endpoint = listen(signal).expect("SIGUSR2 can be caught");
        // SAFETY: reading the current disposition only.
        let disposition = unsafe {
            let mut current: libc::sigaction = std::mem::zeroed();
            libc::sigaction(signal, std::ptr::null(), &mut current);
            current.sa_sigaction
        };
        assert_eq!(
            disposition,
            libc::SIG_DFL,
            "the process's disposition is untouched"
        );

        for target in [hosted.pid(), me, 0] {
            assert_eq!(kill(target, signal), 0);
            let message = workers::take(endpoint).expect("delivered in the realm");
            assert_eq!(message.kind, workers::kind::SIGNAL);
        }
        assert_eq!(kill(hosted.pid(), 0), 0);
        assert_eq!(kill(hosted.pid(), libc::SIGCHLD), 0, "ignored by default");
        assert_eq!(termination(), None);

        // The last listener gone, the signal's default action is the program's.
        unhandled(endpoint);
        assert_eq!(termination(), Some(signal));
        assert_eq!(hosted.terminated_by(), Some(signal));
        drop(hosted);
        assert_eq!(pid(), me);
        assert_eq!(termination(), None);
    }
}
