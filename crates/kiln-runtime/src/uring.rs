//! A Linux io_uring transport for connections the HTTP engines own (ADR 0046).
//!
//! The host hands Hyper an owned byte transport through `take_transport`. On Linux, when the
//! io_uring backend is selected, that transport is a [`UringStream`] over the same socket
//! instead of a Tokio `TcpStream`. Nothing above the seam changes: Hyper still sees
//! `AsyncRead + AsyncWrite`, the realm still runs on its current-thread Tokio runtime and
//! `LocalSet`, and `ScriptedHost`/`ScriptedTransport` never reach this module.
//!
//! # Ownership
//!
//! One [`Ring`] per realm host, created lazily on the realm thread the first time a transport
//! is taken. It is not a second executor: its only task is the *driver*, an ordinary
//! `spawn_local` task on the realm's `LocalSet`. The ring is created with `SINGLE_ISSUER |
//! COOP_TASKRUN | TASKRUN_FLAG`: completion work the kernel owes the ring runs when the realm
//! thread next crosses into the kernel, never by interrupting it, and the shared
//! `IORING_SQ_TASKRUN` flag says when such work is waiting.
//!
//! # Parking
//!
//! The ring's own descriptor is registered with Tokio's reactor through `AsyncFd`, so the
//! realm's existing park (`epoll_wait`, which also covers timers, fs, processes and every
//! other socket) wakes for ring completions too. A socket event while the thread is parked
//! queues task work, which ends the `epoll_wait` (`EINTR`); running it on the way out posts
//! the completions, which makes the ring readable for the next `epoll_wait`, and the driver
//! then reaps them without entering the kernel at all.
//!
//! `DEFER_TASKRUN` was the first design and was measured out: its local task work never wakes
//! a poller of the ring descriptor, and a registered eventfd is signalled for it only through
//! `call_rcu_hurry` when the work is queued from a socket wakeup (Linux 7.2,
//! `io_eventfd_signal(.., defer)`), which cost 6-8 ms on every keep-alive request.
//!
//! # The flush point
//!
//! Transports never enter the kernel. A write appends to the connection's pending buffer and
//! a read that finds nothing buffered just registers its waker; either one *kicks* the driver,
//! which schedules it on the `LocalSet` behind every task already queued. When the driver
//! runs it turns every pending write into one send SQE per connection and makes a single
//! `io_uring_enter` (submit, plus `GETEVENTS` when the kernel has flagged deferred work), then
//! reaps every CQE, wakes the tasks those completions belong to, and reschedules itself behind
//! them. So one enter covers everything the tasks polled in one pass of the run queue queued,
//! and a busy server loops enter → run tasks → enter without touching `epoll_wait` except for
//! Tokio's own periodic reactor poll. Only when an enter produces nothing does the driver
//! wait on the ring's readiness and let the thread park.
//!
//! # Buffers
//!
//! Receives use one multishot `RECV` per connection over a provided-buffer ring
//! (`IORING_REGISTER_PBUF_RING`). Each completion's bytes are copied into the connection's own
//! receive buffer as they are reaped and the provided buffer is recycled at once, so a slow
//! reader can never starve the shared pool; a connection holding more than [`RX_HIGH`]
//! unread bytes cancels its receive and re-arms once the reader drains it (TCP flow control
//! then pushes back on the peer, as it does on the epoll path).
//!
//! Sends are plain `SEND` with `MSG_WAITALL` (the kernel retries short stream sends itself),
//! at most one in flight per connection so bytes can never be reordered, from a buffer the
//! connection owns. Writes past [`TX_HIGH`] queued bytes return `Pending`. `poll_flush` is
//! immediate, as it is for a `TcpStream`: bytes the ring owns are the equivalent of bytes in
//! the kernel's send buffer. `poll_shutdown` waits for every queued byte to be sent before
//! `shutdown(SHUT_WR)`, which is what keeps a response ahead of the FIN.
//!
//! Every buffer and every descriptor lives in the ring's connection table, not in the
//! transport. Dropping a transport cancels its receive and lets any queued send finish; the
//! slot and its socket are released only when the last in-flight operation has completed,
//! so a buffer always outlives the kernel's use of it, and a descriptor number can never be
//! reused under an SQE that still names it. Realm shutdown sends what is queued, cancels
//! everything else and waits for the kernel to finish; if it cannot confirm that within a
//! deadline it leaks the memory rather than free something the kernel may still write.
//!
//! # Listeners
//!
//! With the backend selected, a listening socket belongs to the ring too ([`RingListener`]):
//! one multishot `ACCEPT` stays armed, the connections it accepts are never registered with
//! Tokio's reactor, and the HTTP engine takes them straight onto the ring. A raw
//! `node:net` socket still gets Tokio halves, on its first discrete read, write or shutdown.
//!
//! # Selection
//!
//! `KILN_IO=uring|epoll|auto` (default `auto`, which currently means epoll; see
//! [`AUTO_SELECTS_URING`]). Selecting uring probes the kernel: a failed `io_uring_setup`
//! (ENOSYS, EPERM from seccomp or `kernel.io_uring_disabled`), a kernel without
//! `SINGLE_ISSUER` (before 6.0, which multishot receive also needs), a missing opcode, or a
//! failed buffer-ring registration all fall back to epoll for that realm.

use std::cell::RefCell;
use std::io;
use std::mem::ManuallyDrop;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU16, Ordering};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use io_uring::{IoUring, Probe, cqueue, opcode, squeue, types};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Whether `KILN_IO` unset (or `auto`) selects io_uring. Off: the ring saves syscalls under
/// load but costs latency on an idle connection (a parked realm is woken through task work,
/// `EINTR` and a second `epoll_wait`), and it is the newer path.
pub const AUTO_SELECTS_URING: bool = false;

const SQ_ENTRIES: u32 = 256;
const CQ_ENTRIES: u32 = 4096;
/// Provided receive buffers: count (a power of two) and size.
const BUF_COUNT: u16 = 256;
const BUF_SIZE: usize = 4096;
const BUF_GROUP: u16 = 0;
/// Unread bytes past which a connection stops receiving until its reader catches up.
pub const RX_HIGH: usize = 256 * 1024;
/// Queued, unsent bytes past which a write returns `Pending`.
pub const TX_HIGH: usize = 256 * 1024;
/// How long realm shutdown lets queued sends finish, and then how long it waits for the
/// kernel to confirm every cancellation.
const SHUTDOWN_SEND_GRACE: Duration = Duration::from_millis(250);
const SHUTDOWN_CANCEL_DEADLINE: Duration = Duration::from_secs(1);

const KIND_RECV: u64 = 1;
const KIND_SEND: u64 = 2;
const KIND_CANCEL: u64 = 3;
const KIND_ACCEPT: u64 = 4;
/// Accepted connections the ring holds for a listener before it stops accepting and leaves the
/// rest in the kernel's backlog, and the level at which it starts again.
const ACCEPT_HIGH: usize = 256;
const ACCEPT_LOW: usize = 64;

/// `kind` in the top byte, the slot's generation in the next 24 bits, the slot below. The
/// generation keeps a cancellation aimed at one connection's receive from ever matching the
/// receive of a later connection that reuses the slot.
fn user_data(kind: u64, generation: u32, index: u32) -> u64 {
    (kind << 56) | (u64::from(generation & 0x00ff_ffff) << 32) | u64::from(index)
}

/// What the program asked for through `KILN_IO`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoPreference {
    Auto,
    Epoll,
    Uring,
}

impl IoPreference {
    pub fn from_env() -> Self {
        static FROM_ENV: OnceLock<IoPreference> = OnceLock::new();
        *FROM_ENV.get_or_init(|| match std::env::var("KILN_IO") {
            Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
                "uring" | "io_uring" | "io-uring" => IoPreference::Uring,
                "epoll" => IoPreference::Epoll,
                "" | "auto" => IoPreference::Auto,
                other => {
                    eprintln!("kiln: ignoring KILN_IO={other:?}; expected uring, epoll or auto");
                    IoPreference::Auto
                }
            },
            Err(_) => IoPreference::Auto,
        })
    }

    fn wants_uring(self) -> bool {
        match self {
            IoPreference::Uring => true,
            IoPreference::Epoll => false,
            IoPreference::Auto => AUTO_SELECTS_URING,
        }
    }
}

/// The host's lazily created ring, or the decision not to have one.
pub(crate) struct HostRing {
    preference: IoPreference,
    state: HostRingState,
}

enum HostRingState {
    Untried,
    Ready(Rc<Ring>),
    Unavailable,
}

impl HostRing {
    pub(crate) fn new() -> Self {
        Self {
            preference: IoPreference::from_env(),
            state: HostRingState::Untried,
        }
    }

    /// Override `KILN_IO` for this host. Only meaningful before the first transport.
    pub(crate) fn set_preference(&mut self, preference: IoPreference) {
        self.preference = preference;
        if matches!(self.state, HostRingState::Unavailable) {
            self.state = HostRingState::Untried;
        }
    }

    /// The ring to put a taken transport on, creating it on first use, or `None` for epoll.
    pub(crate) fn ring(&mut self) -> Option<Rc<Ring>> {
        match &self.state {
            HostRingState::Ready(ring) if !ring.is_closed() => return Some(Rc::clone(ring)),
            HostRingState::Ready(_) | HostRingState::Unavailable => return None,
            HostRingState::Untried => {}
        }
        if !self.preference.wants_uring() {
            self.state = HostRingState::Unavailable;
            return None;
        }
        match Ring::new() {
            Ok(ring) => {
                crate::http_trace_line(format_args!("io backend: io_uring"));
                self.state = HostRingState::Ready(Rc::clone(&ring));
                Some(ring)
            }
            Err(error) => {
                if self.preference == IoPreference::Uring {
                    static WARNED: OnceLock<()> = OnceLock::new();
                    WARNED.get_or_init(|| {
                        eprintln!(
                            "kiln: KILN_IO=uring requested but io_uring is unavailable ({error}); using epoll"
                        );
                    });
                }
                crate::http_trace_line(format_args!("io backend: epoll ({error})"));
                self.state = HostRingState::Unavailable;
                None
            }
        }
    }

    pub(crate) fn shutdown(&mut self) {
        if let HostRingState::Ready(ring) = &self.state {
            ring.shutdown();
        }
    }
}

/// The provided-buffer ring: a page-aligned array of `io_uring_buf` entries the kernel pops
/// from, and the memory those entries point into.
struct BufRing {
    entries: *mut types::BufRingEntry,
    entries_len: usize,
    data: *mut u8,
    data_len: usize,
    tail: u16,
}

impl BufRing {
    fn new() -> io::Result<Self> {
        let entries_len = usize::from(BUF_COUNT) * std::mem::size_of::<types::BufRingEntry>();
        let data_len = usize::from(BUF_COUNT) * BUF_SIZE;
        let entries = anonymous_map(entries_len)?;
        let data = match anonymous_map(data_len) {
            Ok(data) => data,
            Err(error) => {
                // SAFETY: `entries` was mapped just above with this length.
                unsafe { libc::munmap(entries.cast(), entries_len) };
                return Err(error);
            }
        };
        let mut ring = Self {
            entries: entries.cast(),
            entries_len,
            data,
            data_len,
            tail: 0,
        };
        for bid in 0..BUF_COUNT {
            ring.push(bid);
        }
        ring.publish();
        Ok(ring)
    }

    fn push(&mut self, bid: u16) {
        let slot = usize::from(self.tail & (BUF_COUNT - 1));
        // SAFETY: `slot < BUF_COUNT` and the mapping holds `BUF_COUNT` entries. The kernel only
        // reads entries between its head and our published tail, and this slot is past it.
        unsafe {
            let entry = &mut *self.entries.add(slot);
            entry.set_addr(self.data.add(usize::from(bid) * BUF_SIZE) as u64);
            entry.set_len(BUF_SIZE as u32);
            entry.set_bid(bid);
        }
        self.tail = self.tail.wrapping_add(1);
    }

    fn publish(&self) {
        // SAFETY: the tail lives in the first entry's reserved field and is only ever written
        // by us; the kernel reads it with acquire semantics.
        unsafe {
            let tail = types::BufRingEntry::tail(self.entries) as *const AtomicU16;
            (*tail).store(self.tail, Ordering::Release);
        }
    }

    fn bytes(&self, bid: u16, len: usize) -> &[u8] {
        assert!(bid < BUF_COUNT && len <= BUF_SIZE);
        // SAFETY: bounds checked above; the kernel wrote `len` bytes into this buffer before
        // posting the completion that names it, and does not touch it again until we push it.
        unsafe { std::slice::from_raw_parts(self.data.add(usize::from(bid) * BUF_SIZE), len) }
    }
}

impl Drop for BufRing {
    fn drop(&mut self) {
        // SAFETY: both regions were mapped by `new` with these lengths. The caller only drops a
        // `BufRing` once no receive can still select from it (see `Ring::drop`).
        unsafe {
            libc::munmap(self.entries.cast(), self.entries_len);
            libc::munmap(self.data.cast(), self.data_len);
        }
    }
}

fn anonymous_map(len: usize) -> io::Result<*mut u8> {
    // SAFETY: a fresh private anonymous mapping aliases nothing.
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        Err(io::Error::last_os_error())
    } else {
        Ok(ptr.cast())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecvState {
    Idle,
    Armed,
    Cancelling,
}

/// Everything about one socket, owned by the ring so it outlives the kernel's use of it.
struct Conn {
    fd: Option<OwnedFd>,
    generation: u32,
    rx: Vec<u8>,
    rx_pos: usize,
    rx_eof: bool,
    rx_err: Option<i32>,
    recv: RecvState,
    read_waker: Option<Waker>,
    tx_pending: Vec<u8>,
    tx_inflight: Vec<u8>,
    tx_off: usize,
    sending: bool,
    tx_err: Option<i32>,
    write_waker: Option<Waker>,
    wr_shut: bool,
    /// In-flight kernel operations naming this slot: the armed receive and the send.
    ops: u32,
    dropped: bool,
    dirty: bool,
}

impl Conn {
    fn new(fd: OwnedFd, generation: u32) -> Self {
        Self {
            fd: Some(fd),
            generation,
            rx: Vec::new(),
            rx_pos: 0,
            rx_eof: false,
            rx_err: None,
            recv: RecvState::Idle,
            read_waker: None,
            tx_pending: Vec::new(),
            tx_inflight: Vec::new(),
            tx_off: 0,
            sending: false,
            tx_err: None,
            write_waker: None,
            wr_shut: false,
            ops: 0,
            dropped: false,
            dirty: false,
        }
    }

    fn buffered(&self) -> usize {
        self.rx.len() - self.rx_pos
    }

    fn queued(&self) -> usize {
        self.tx_pending.len() + (self.tx_inflight.len() - self.tx_off)
    }

    fn raw_fd(&self) -> RawFd {
        self.fd.as_ref().map_or(-1, AsRawFd::as_raw_fd)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Work {
    None,
    /// Only the kernel's IORING_SQ_TASKRUN flag, which can be stale (see `poll_drive`).
    TaskrunOnly,
    Other,
}

/// A listening socket with a multishot accept on the ring, and the connections it has accepted
/// that nobody has asked for yet.
struct Listen {
    fd: Option<OwnedFd>,
    generation: u32,
    accept: RecvState,
    queue: std::collections::VecDeque<OwnedFd>,
    error: Option<i32>,
    waiters: Vec<Waker>,
    /// The host closed the listener; the slot goes once the accept has completed.
    closed: bool,
}

#[derive(Default)]
struct DriverState {
    spawned: bool,
    scheduled: bool,
    waker: Option<Waker>,
}

struct Inner {
    uring: IoUring,
    /// Unmapped by `Ring::drop` only once the kernel is provably done with it.
    bufs: ManuallyDrop<BufRing>,
    conns: Vec<Option<Conn>>,
    listeners: Vec<Option<Listen>>,
    free: Vec<u32>,
    free_listeners: Vec<u32>,
    dirty: Vec<u32>,
    wake: Vec<Waker>,
    cqes: Vec<cqueue::Entry>,
    cancels: u32,
    generation: u32,
    taskrun_stale: bool,
    closed: bool,
    failure: Option<String>,
    driver: DriverState,
    /// Shutdown could not confirm the kernel was done with our memory; never free it.
    leak: bool,
    stats: RingStats,
}

/// Counters for tests and `KILN_HTTP_TRACE`.
#[derive(Debug, Default, Clone, Copy)]
pub struct RingStats {
    pub enters: u64,
    pub cqes: u64,
    pub sends: u64,
    pub recv_arms: u64,
    pub parks: u64,
    pub stale_taskrun: u64,
}

/// One io_uring instance per realm host. See the module documentation.
pub(crate) struct Ring {
    inner: RefCell<Inner>,
}

impl Ring {
    pub(crate) fn new() -> io::Result<Rc<Self>> {
        let uring = IoUring::builder()
            .setup_single_issuer()
            .setup_coop_taskrun()
            .setup_taskrun_flag()
            .setup_cqsize(CQ_ENTRIES)
            .build(SQ_ENTRIES)?;
        let submitter = uring.submitter();
        let mut probe = Probe::new();
        submitter.register_probe(&mut probe)?;
        for (code, name) in [
            (opcode::RecvMulti::CODE, "RECV"),
            (opcode::Send::CODE, "SEND"),
            (opcode::AsyncCancel::CODE, "ASYNC_CANCEL"),
            (opcode::AcceptMulti::CODE, "ACCEPT"),
        ] {
            if !probe.is_supported(code) {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!("io_uring opcode {name} is not supported"),
                ));
            }
        }
        let bufs = BufRing::new()?;
        // SAFETY: the entries stay mapped until `Ring::drop` unregisters the group (or leaks it).
        unsafe {
            submitter.register_buf_ring_with_flags(bufs.entries as u64, BUF_COUNT, BUF_GROUP, 0)?;
        }
        Ok(Rc::new(Self {
            inner: RefCell::new(Inner {
                uring,
                bufs: ManuallyDrop::new(bufs),
                conns: Vec::new(),
                listeners: Vec::new(),
                free: Vec::new(),
                free_listeners: Vec::new(),
                dirty: Vec::new(),
                wake: Vec::new(),
                cqes: Vec::with_capacity(256),
                cancels: 0,
                generation: 0,
                taskrun_stale: false,
                closed: false,
                failure: None,
                driver: DriverState::default(),
                leak: false,
                stats: RingStats::default(),
            }),
        }))
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.inner.borrow().closed
    }

    #[cfg(test)]
    pub(crate) fn stats(&self) -> RingStats {
        self.inner.borrow().stats
    }

    /// Put `fd` (a connected, non-blocking stream socket) on the ring. The receive is queued
    /// now and submitted at the next flush, which the first poll of the stream triggers.
    pub(crate) fn register(self: &Rc<Self>, fd: OwnedFd) -> Result<UringStream, OwnedFd> {
        let mut inner = self.inner.borrow_mut();
        if inner.closed {
            return Err(fd);
        }
        inner.generation = inner.generation.wrapping_add(1);
        let conn = Conn::new(fd, inner.generation);
        let index = match inner.free.pop() {
            Some(index) => {
                inner.conns[index as usize] = Some(conn);
                index
            }
            None => {
                inner.conns.push(Some(conn));
                (inner.conns.len() - 1) as u32
            }
        };
        inner.arm_recv(index);
        Ok(UringStream {
            ring: Rc::clone(self),
            index,
        })
    }

    /// Make sure the driver will run behind the tasks already queued. Must be called from a
    /// task on the realm's `LocalSet` the first time, since that is where the driver is spawned.
    fn kick(self: &Rc<Self>) {
        let waker = {
            let mut inner = self.inner.borrow_mut();
            if inner.closed {
                return;
            }
            if !inner.driver.spawned {
                inner.driver.spawned = true;
                inner.driver.scheduled = true;
                drop(inner);
                tokio::task::spawn_local(Driver {
                    ring_fd: None,
                    ring: Rc::clone(self),
                });
                return;
            }
            if inner.driver.scheduled {
                return;
            }
            inner.driver.scheduled = true;
            inner.driver.waker.clone()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    /// Wake an already spawned driver; used where no task context is guaranteed (drop).
    fn kick_existing(&self) {
        let waker = {
            let Ok(mut inner) = self.inner.try_borrow_mut() else {
                return;
            };
            if inner.closed || !inner.driver.spawned || inner.driver.scheduled {
                return;
            }
            inner.driver.scheduled = true;
            inner.driver.waker.clone()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    fn poll_drive(&self, cx: &mut Context<'_>, ring_fd: &mut Option<AsyncFd<RingFd>>) -> Poll<()> {
        if ring_fd.is_none() {
            let fd = RingFd(self.inner.borrow().uring.as_raw_fd());
            match AsyncFd::with_interest(fd, tokio::io::Interest::READABLE) {
                Ok(fd) => *ring_fd = Some(fd),
                Err(error) => {
                    self.fail(format!(
                        "could not register the ring with the reactor: {error}"
                    ));
                    return Poll::Ready(());
                }
            }
        }
        let ring_fd = ring_fd.as_ref().expect("ring registered above");
        loop {
            let (reaped, wakers) = {
                let mut inner = self.inner.borrow_mut();
                if inner.closed {
                    return Poll::Ready(());
                }
                inner.driver.scheduled = false;
                match &mut inner.driver.waker {
                    Some(waker) => waker.clone_from(cx.waker()),
                    slot @ None => *slot = Some(cx.waker().clone()),
                }
                let reaped = inner.turn();
                (reaped, std::mem::take(&mut inner.wake))
            };
            let failed = self.inner.borrow().closed;
            for waker in wakers {
                waker.wake();
            }
            if failed {
                return Poll::Ready(());
            }
            if reaped > 0 {
                // Let the tasks those completions woke run, then flush what they queued.
                self.inner.borrow_mut().driver.scheduled = true;
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            {
                let mut inner = self.inner.borrow_mut();
                let work = inner.has_work();
                // A flag that the enter just made proved stale is not work (see `turn`). Parking
                // on it is safe without DEFER_TASKRUN: real pending task work makes the park's
                // own epoll_wait return, and running it posts the completions that wake us.
                let stale = work == Work::TaskrunOnly && inner.taskrun_stale;
                if work != Work::None && !stale {
                    // More to do, but never loop inside one poll: the work may be waiting on
                    // a task that only runs once the driver yields.
                    inner.driver.scheduled = true;
                    drop(inner);
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }
                if stale {
                    inner.stats.stale_taskrun += 1;
                }
                inner.stats.parks += 1;
            }
            match ring_fd.poll_read_ready(cx) {
                Poll::Ready(Ok(mut guard)) => guard.clear_ready(),
                Poll::Ready(Err(error)) => {
                    self.fail(format!("ring readiness failed: {error}"));
                    return Poll::Ready(());
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }

    /// A ring-level failure: every transport sees an error from now on.
    fn fail(&self, message: String) {
        crate::http_trace_line(format_args!("io_uring failure: {message}"));
        let wakers = {
            let mut inner = self.inner.borrow_mut();
            inner.failure.get_or_insert(message);
            inner.closed = true;
            inner.collect_all_wakers()
        };
        for waker in wakers {
            waker.wake();
        }
    }

    /// Realm teardown: send what is queued, cancel everything else, and wait for the kernel
    /// to finish with every buffer. Idempotent.
    pub(crate) fn shutdown(&self) {
        let wakers = {
            let Ok(mut inner) = self.inner.try_borrow_mut() else {
                return;
            };
            if inner.closed && inner.failure.is_none() && inner.inflight() == 0 {
                return;
            }
            inner.drain_for_shutdown();
            inner.collect_all_wakers()
        };
        for waker in wakers {
            waker.wake();
        }
    }
}

impl Drop for Ring {
    fn drop(&mut self) {
        let inner = self.inner.get_mut();
        if inner.inflight() > 0 && !inner.leak {
            inner.drain_for_shutdown();
        }
        if inner.leak {
            // The kernel may still own some of this memory; losing it beats a use-after-free.
            // The buffer ring stays mapped (it is never dropped) and so do the send buffers.
            std::mem::forget(std::mem::take(&mut inner.conns));
            std::mem::forget(std::mem::take(&mut inner.listeners));
        } else {
            // Nothing is in flight, so no receive can select a buffer again. Unregister the
            // group before unmapping it rather than rely on the ring closing afterwards.
            let _ = inner.uring.submitter().unregister_buf_ring(BUF_GROUP);
            // SAFETY: unregistered above and nothing is in flight; dropped exactly once.
            unsafe { ManuallyDrop::drop(&mut inner.bufs) };
        }
    }
}

impl Inner {
    fn conn(&mut self, index: u32) -> &mut Conn {
        self.conns[index as usize]
            .as_mut()
            .expect("io_uring connection slot in use")
    }

    fn inflight(&self) -> u32 {
        self.cancels
            + self
                .conns
                .iter()
                .flatten()
                .map(|conn| conn.ops)
                .sum::<u32>()
            + self
                .listeners
                .iter()
                .flatten()
                .map(|listen| u32::from(listen.accept != RecvState::Idle))
                .sum::<u32>()
    }

    fn push(&mut self, entry: squeue::Entry) {
        loop {
            // SAFETY: every entry built here points at memory the connection table owns until
            // the matching completion is reaped.
            if unsafe { self.uring.submission().push(&entry) }.is_ok() {
                return;
            }
            // The submission queue is full: hand what is there to the kernel and retry.
            if let Err(error) = self.enter() {
                self.closed = true;
                self.failure
                    .get_or_insert(format!("io_uring submit failed: {error}"));
                return;
            }
        }
    }

    fn enter(&mut self) -> io::Result<usize> {
        loop {
            self.stats.enters += 1;
            match self.uring.submit() {
                Ok(count) => return Ok(count),
                Err(error) if error.raw_os_error() == Some(libc::EINTR) => continue,
                // Completions are backed up; reaping makes room, the caller retries.
                Err(error) if matches!(error.raw_os_error(), Some(libc::EBUSY | libc::EAGAIN)) => {
                    self.reap();
                    return Ok(0);
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn arm_recv(&mut self, index: u32) {
        let conn = self.conn(index);
        let fd = conn.raw_fd();
        let generation = conn.generation;
        conn.recv = RecvState::Armed;
        conn.ops += 1;
        self.stats.recv_arms += 1;
        let entry = opcode::RecvMulti::new(types::Fd(fd), BUF_GROUP)
            .build()
            .user_data(user_data(KIND_RECV, generation, index));
        self.push(entry);
    }

    fn should_arm(&self, conn: &Conn) -> bool {
        !self.closed
            && !conn.dropped
            && !conn.rx_eof
            && conn.rx_err.is_none()
            && conn.recv == RecvState::Idle
            && conn.fd.is_some()
            && conn.buffered() < RX_HIGH / 2
    }

    fn cancel_recv(&mut self, index: u32) {
        let conn = self.conn(index);
        if conn.recv != RecvState::Armed {
            return;
        }
        conn.recv = RecvState::Cancelling;
        let generation = conn.generation;
        self.cancels += 1;
        let entry = opcode::AsyncCancel::new(user_data(KIND_RECV, generation, index))
            .build()
            .user_data(user_data(KIND_CANCEL, generation, index));
        self.push(entry);
    }

    fn mark_dirty(&mut self, index: u32) {
        let conn = self.conn(index);
        if !conn.dirty {
            conn.dirty = true;
            self.dirty.push(index);
        }
    }

    /// Turn every connection's pending bytes into a send, one in flight per connection.
    fn flush_sends(&mut self) {
        let mut dirty = std::mem::take(&mut self.dirty);
        for &index in &dirty {
            let closed = self.closed;
            let Some(conn) = self.conns[index as usize].as_mut() else {
                continue;
            };
            conn.dirty = false;
            if closed || conn.sending || conn.tx_pending.is_empty() || conn.tx_err.is_some() {
                continue;
            }
            std::mem::swap(&mut conn.tx_pending, &mut conn.tx_inflight);
            conn.tx_pending.clear();
            conn.tx_off = 0;
            self.submit_send(index);
        }
        dirty.clear();
        if self.dirty.is_empty() {
            self.dirty = dirty;
        }
    }

    fn submit_send(&mut self, index: u32) {
        let conn = self.conn(index);
        let fd = conn.raw_fd();
        let generation = conn.generation;
        let remaining = &conn.tx_inflight[conn.tx_off..];
        let (ptr, len) = (
            remaining.as_ptr(),
            remaining.len().min(u32::MAX as usize) as u32,
        );
        conn.sending = true;
        conn.ops += 1;
        self.stats.sends += 1;
        let entry = opcode::Send::new(types::Fd(fd), ptr, len)
            .flags(libc::MSG_NOSIGNAL | libc::MSG_WAITALL)
            .build()
            .user_data(user_data(KIND_SEND, generation, index));
        self.push(entry);
    }

    fn has_work(&mut self) -> Work {
        if !self.dirty.is_empty() {
            return Work::Other;
        }
        let sq = self.uring.submission();
        if !sq.is_empty() || sq.cq_overflow() {
            return Work::Other;
        }
        let taskrun = sq.taskrun();
        drop(sq);
        if !self.uring.completion().is_empty() {
            Work::Other
        } else if taskrun {
            Work::TaskrunOnly
        } else {
            Work::None
        }
    }

    fn enter_and_reap(&mut self) -> usize {
        if let Err(error) = self.enter() {
            self.closed = true;
            self.failure
                .get_or_insert(format!("io_uring submit failed: {error}"));
            let mut wakers = self.collect_all_wakers();
            self.wake.append(&mut wakers);
            return 0;
        }
        self.reap()
    }

    /// One flush: queue sends, enter the kernel if there is anything to submit or deferred
    /// work to run, and reap. Returns the number of completions handled.
    fn turn(&mut self) -> usize {
        self.flush_sends();
        let (submit, taskrun) = {
            let sq = self.uring.submission();
            (!sq.is_empty() || sq.cq_overflow(), sq.taskrun())
        };
        let reaped = if submit || taskrun {
            self.enter_and_reap()
        } else {
            self.reap()
        };
        // `submit` enters with GETEVENTS too whenever the flag is set, so either kind of
        // enter that finds nothing leaves the flag proven stale.
        self.taskrun_stale = taskrun && reaped == 0;
        reaped
    }

    fn reap(&mut self) -> usize {
        let mut total = 0;
        loop {
            let mut cqes = std::mem::take(&mut self.cqes);
            cqes.extend(self.uring.completion());
            if cqes.is_empty() {
                self.cqes = cqes;
                break;
            }
            total += cqes.len();
            self.stats.cqes += cqes.len() as u64;
            for cqe in cqes.drain(..) {
                self.complete(cqe);
            }
            self.cqes = cqes;
            self.bufs.publish();
        }
        total
    }

    fn complete(&mut self, cqe: cqueue::Entry) {
        let data = cqe.user_data();
        let index = data as u32;
        match data >> 56 {
            KIND_CANCEL => self.cancels -= 1,
            KIND_RECV => self.complete_recv(index, cqe.result(), cqe.flags()),
            KIND_SEND => self.complete_send(index, cqe.result()),
            KIND_ACCEPT => self.complete_accept(index, cqe.result(), cqe.flags()),
            kind => debug_assert!(false, "unknown io_uring completion kind {kind}"),
        }
    }

    fn complete_recv(&mut self, index: u32, result: i32, flags: u32) {
        let more = cqueue::more(flags);
        let bid = cqueue::buffer_select(flags);
        let closed = self.closed;
        let conn = self.conns[index as usize]
            .as_mut()
            .expect("receive completion for a live slot");
        if let Some(bid) = bid {
            if result > 0 && !conn.dropped && conn.rx_err.is_none() && !closed {
                if conn.rx_pos == conn.rx.len() {
                    conn.rx.clear();
                    conn.rx_pos = 0;
                }
                conn.rx
                    .extend_from_slice(self.bufs.bytes(bid, result as usize));
            }
            self.bufs.push(bid);
        }
        let mut wake = result > 0;
        if !more {
            conn.recv = RecvState::Idle;
            conn.ops -= 1;
            match result {
                0 => {
                    conn.rx_eof = true;
                    wake = true;
                }
                error
                    if error < 0
                        && error != -libc::ENOBUFS
                        && error != -libc::ECANCELED
                        && !conn.dropped =>
                {
                    conn.rx_err = Some(-error);
                    wake = true;
                }
                _ => {}
            }
        }
        if wake && let Some(waker) = conn.read_waker.take() {
            self.wake.push(waker);
        }
        let conn = self.conns[index as usize].as_ref().expect("live slot");
        if conn.recv == RecvState::Armed && conn.buffered() >= RX_HIGH {
            self.cancel_recv(index);
        } else if self.should_arm(conn) {
            self.arm_recv(index);
        }
        self.maybe_release(index);
    }

    fn complete_send(&mut self, index: u32, result: i32) {
        let conn = self.conn(index);
        conn.ops -= 1;
        conn.sending = false;
        let mut resend = false;
        if result < 0 {
            conn.tx_err = Some(-result);
            conn.tx_inflight.clear();
            conn.tx_pending.clear();
            conn.tx_off = 0;
        } else {
            conn.tx_off += result as usize;
            if result > 0 && conn.tx_off < conn.tx_inflight.len() {
                resend = true;
            } else {
                if conn.tx_off < conn.tx_inflight.len() {
                    // A zero-length completion for a non-empty send: the peer is gone.
                    conn.tx_err = Some(libc::EPIPE);
                    conn.tx_pending.clear();
                }
                conn.tx_inflight.clear();
                conn.tx_off = 0;
            }
        }
        let pending = !conn.tx_pending.is_empty();
        if let Some(waker) = conn.write_waker.take() {
            self.wake.push(waker);
        }
        if resend && !self.closed {
            self.submit_send(index);
        } else if pending {
            self.mark_dirty(index);
        }
        self.maybe_release(index);
    }

    /// Close a dropped connection's socket once the kernel is done with every buffer in it.
    fn maybe_release(&mut self, index: u32) {
        let closed = self.closed;
        let Some(conn) = self.conns[index as usize].as_mut() else {
            return;
        };
        if !conn.dropped || conn.ops > 0 || conn.sending {
            return;
        }
        if !conn.tx_pending.is_empty() && conn.tx_err.is_none() && !closed {
            return;
        }
        if conn.buffered() > 0
            && let Some(fd) = &conn.fd
        {
            // The epoll path closes a socket with unread input, which resets the connection.
            // The ring has already taken those bytes out of the kernel, so ask for the reset.
            set_linger_zero(fd.as_raw_fd());
        }
        self.conns[index as usize] = None;
        if conn_dirty_listed(&self.dirty, index) {
            self.dirty.retain(|&dirty| dirty != index);
        }
        self.free.push(index);
    }

    fn collect_all_wakers(&mut self) -> Vec<Waker> {
        let mut wakers = std::mem::take(&mut self.wake);
        for conn in self.conns.iter_mut().flatten() {
            wakers.extend(conn.read_waker.take());
            wakers.extend(conn.write_waker.take());
        }
        for listen in self.listeners.iter_mut().flatten() {
            wakers.append(&mut listen.waiters);
        }
        wakers
    }

    fn listen(&mut self, index: u32) -> &mut Listen {
        self.listeners[index as usize]
            .as_mut()
            .expect("io_uring listener slot in use")
    }

    fn arm_accept(&mut self, index: u32) {
        let listen = self.listen(index);
        let Some(fd) = listen.fd.as_ref().map(AsRawFd::as_raw_fd) else {
            return;
        };
        let generation = listen.generation;
        listen.accept = RecvState::Armed;
        let entry = opcode::AcceptMulti::new(types::Fd(fd))
            .flags(libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC)
            .build()
            .user_data(user_data(KIND_ACCEPT, generation, index));
        self.push(entry);
    }

    fn cancel_accept(&mut self, index: u32) {
        let listen = self.listen(index);
        if listen.accept != RecvState::Armed {
            return;
        }
        listen.accept = RecvState::Cancelling;
        let generation = listen.generation;
        self.cancels += 1;
        let entry = opcode::AsyncCancel::new(user_data(KIND_ACCEPT, generation, index))
            .build()
            .user_data(user_data(KIND_CANCEL, generation, index));
        self.push(entry);
    }

    fn should_arm_accept(&self, listen: &Listen) -> bool {
        !self.closed
            && !listen.closed
            && listen.accept == RecvState::Idle
            && listen.error.is_none()
            && listen.fd.is_some()
            && listen.queue.len() < ACCEPT_LOW
    }

    fn complete_accept(&mut self, index: u32, result: i32, flags: u32) {
        let more = cqueue::more(flags);
        let listen = self.listeners[index as usize]
            .as_mut()
            .expect("accept completion for a live listener");
        if result >= 0 {
            // SAFETY: a successful accept completion carries a fresh descriptor we now own.
            let fd = unsafe { <OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(result) };
            if listen.closed {
                set_linger_zero(fd.as_raw_fd());
            } else {
                listen.queue.push_back(fd);
            }
        }
        if !more {
            listen.accept = RecvState::Idle;
            if result < 0 && result != -libc::ECANCELED && !listen.closed {
                listen.error = Some(-result);
            }
        }
        let wake = result >= 0 || listen.error.is_some();
        if wake {
            self.wake.append(&mut listen.waiters);
        }
        let listen = self.listeners[index as usize]
            .as_ref()
            .expect("live listener");
        if listen.accept == RecvState::Armed && listen.queue.len() >= ACCEPT_HIGH {
            self.cancel_accept(index);
        } else if self.should_arm_accept(listen) {
            self.arm_accept(index);
        }
        self.maybe_release_listener(index);
    }

    fn maybe_release_listener(&mut self, index: u32) {
        let Some(listen) = self.listeners[index as usize].as_mut() else {
            return;
        };
        if !listen.closed || listen.accept != RecvState::Idle {
            return;
        }
        // Connections accepted for nobody: reset them, as closing a listener resets its backlog.
        for fd in listen.queue.drain(..) {
            set_linger_zero(fd.as_raw_fd());
        }
        self.listeners[index as usize] = None;
        self.free_listeners.push(index);
    }

    fn drain_for_shutdown(&mut self) {
        // Hand queued bytes to the kernel first, as closing a socket would on the epoll path.
        self.closed = false;
        self.flush_sends();
        self.closed = true;
        let indexes: Vec<u32> = (0..self.conns.len() as u32)
            .filter(|&index| self.conns[index as usize].is_some())
            .collect();
        for &index in &indexes {
            self.cancel_recv(index);
        }
        for index in 0..self.listeners.len() as u32 {
            if self.listeners[index as usize].is_some() {
                self.cancel_accept(index);
            }
        }
        self.wait_inflight(SHUTDOWN_SEND_GRACE);
        if self.inflight() > 0 {
            self.cancels += 1;
            let entry = opcode::AsyncCancel2::new(types::CancelBuilder::any())
                .build()
                .user_data(user_data(KIND_CANCEL, 0, u32::MAX));
            self.push(entry);
            self.wait_inflight(SHUTDOWN_CANCEL_DEADLINE);
        }
        if self.inflight() > 0 {
            self.leak = true;
            return;
        }
        // Nothing is in flight: every socket can close now.
        for listen in self.listeners.iter_mut().flatten() {
            listen.fd = None;
            listen.queue.clear();
        }
        for slot in &mut self.conns {
            if let Some(conn) = slot {
                conn.fd = None;
                if conn.dropped {
                    *slot = None;
                }
            }
        }
    }

    fn wait_inflight(&mut self, budget: Duration) {
        let deadline = Instant::now() + budget;
        while self.inflight() > 0 {
            let now = Instant::now();
            if now >= deadline {
                return;
            }
            let left = deadline - now;
            let timespec = types::Timespec::new()
                .sec(left.as_secs())
                .nsec(left.subsec_nanos());
            let args = types::SubmitArgs::new().timespec(&timespec);
            self.stats.enters += 1;
            match self.uring.submitter().submit_with_args(1, &args) {
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.raw_os_error(),
                        Some(libc::ETIME | libc::EINTR | libc::EBUSY | libc::EAGAIN)
                    ) => {}
                Err(_) => return,
            }
            self.reap();
        }
    }
}

fn conn_dirty_listed(dirty: &[u32], index: u32) -> bool {
    dirty.contains(&index)
}

fn set_linger_zero(fd: RawFd) {
    let linger = libc::linger {
        l_onoff: 1,
        l_linger: 0,
    };
    // SAFETY: plain setsockopt on a descriptor we own; failure only loses the reset.
    unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_LINGER,
            (&linger as *const libc::linger).cast(),
            std::mem::size_of::<libc::linger>() as libc::socklen_t,
        );
    }
}

/// The ring's only task. See the module documentation for the loop it runs.
struct Driver {
    // Declared first so it leaves the reactor before the last `Rc<Ring>` can close the ring.
    ring_fd: Option<AsyncFd<RingFd>>,
    ring: Rc<Ring>,
}

/// The ring's descriptor as the reactor sees it; the ring itself owns and closes it.
struct RingFd(RawFd);

impl AsRawFd for RingFd {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

impl std::future::Future for Driver {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        this.ring.poll_drive(cx, &mut this.ring_fd)
    }
}

/// A listening socket whose connections the ring accepts (multishot `ACCEPT`). The accept is
/// armed on the first [`RingListener::accept`] and then stays armed, so a busy listener costs
/// no syscall per connection; accepted sockets are never registered with Tokio's reactor.
/// Dropping the handle cancels the accept, resets connections nobody took, and closes the
/// socket once the kernel has let go of it.
pub struct RingListener {
    ring: Rc<Ring>,
    index: u32,
    generation: u32,
}

impl std::fmt::Debug for RingListener {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RingListener")
            .field("index", &self.index)
            .finish()
    }
}

impl Ring {
    /// Take ownership of a bound, listening, non-blocking socket.
    pub(crate) fn listen(self: &Rc<Self>, fd: OwnedFd) -> Result<RingListener, OwnedFd> {
        let mut inner = self.inner.borrow_mut();
        if inner.closed {
            return Err(fd);
        }
        inner.generation = inner.generation.wrapping_add(1);
        let generation = inner.generation;
        let listen = Listen {
            fd: Some(fd),
            generation,
            accept: RecvState::Idle,
            queue: std::collections::VecDeque::new(),
            error: None,
            waiters: Vec::new(),
            closed: false,
        };
        let index = match inner.free_listeners.pop() {
            Some(index) => {
                inner.listeners[index as usize] = Some(listen);
                index
            }
            None => {
                inner.listeners.push(Some(listen));
                (inner.listeners.len() - 1) as u32
            }
        };
        Ok(RingListener {
            ring: Rc::clone(self),
            index,
            generation,
        })
    }
}

impl RingListener {
    /// The next accepted connection: a connected, non-blocking socket.
    pub fn accept(&self) -> RingAccept {
        RingAccept {
            ring: Rc::clone(&self.ring),
            index: self.index,
            generation: self.generation,
        }
    }

    pub fn local_addr(&self) -> io::Result<std::net::SocketAddr> {
        let inner = self.ring.inner.borrow();
        let fd = inner.listeners[self.index as usize]
            .as_ref()
            .filter(|listen| listen.generation == self.generation)
            .and_then(|listen| listen.fd.as_ref())
            .map(AsRawFd::as_raw_fd)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "listener closed"))?;
        // SAFETY: borrowed for one getsockname; ManuallyDrop keeps the ring's descriptor open.
        let borrowed = ManuallyDrop::new(unsafe {
            <std::net::TcpListener as std::os::fd::FromRawFd>::from_raw_fd(fd)
        });
        borrowed.local_addr()
    }
}

impl Drop for RingListener {
    fn drop(&mut self) {
        let wakers = {
            let Ok(mut inner) = self.ring.inner.try_borrow_mut() else {
                return;
            };
            let Some(listen) = inner.listeners[self.index as usize].as_mut() else {
                return;
            };
            if listen.generation != self.generation {
                return;
            }
            listen.closed = true;
            let wakers = std::mem::take(&mut listen.waiters);
            if !inner.closed {
                inner.cancel_accept(self.index);
            } else {
                let listen = inner.listen(self.index);
                listen.accept = RecvState::Idle;
            }
            inner.maybe_release_listener(self.index);
            wakers
        };
        for waker in wakers {
            waker.wake();
        }
        self.ring.kick_existing();
    }
}

/// One accept from a [`RingListener`].
pub struct RingAccept {
    ring: Rc<Ring>,
    index: u32,
    generation: u32,
}

impl std::future::Future for RingAccept {
    type Output = io::Result<OwnedFd>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<OwnedFd>> {
        let this = self.get_mut();
        let mut inner = this.ring.inner.borrow_mut();
        if inner.closed {
            return Poll::Ready(Err(ring_closed(&inner)));
        }
        let listener_gone = || io::Error::new(io::ErrorKind::NotConnected, "listener closed");
        let Some(listen) = inner.listeners[this.index as usize].as_mut() else {
            return Poll::Ready(Err(listener_gone()));
        };
        if listen.generation != this.generation || listen.closed {
            return Poll::Ready(Err(listener_gone()));
        }
        if let Some(fd) = listen.queue.pop_front() {
            let rearm = inner
                .should_arm_accept(inner.listeners[this.index as usize].as_ref().expect("live"));
            if rearm {
                inner.arm_accept(this.index);
                drop(inner);
                this.ring.kick();
            }
            return Poll::Ready(Ok(fd));
        }
        if let Some(error) = listen.error.take() {
            return Poll::Ready(Err(io::Error::from_raw_os_error(error)));
        }
        if !listen
            .waiters
            .iter()
            .any(|waker| waker.will_wake(cx.waker()))
        {
            listen.waiters.push(cx.waker().clone());
        }
        let arm =
            inner.should_arm_accept(inner.listeners[this.index as usize].as_ref().expect("live"));
        if arm {
            inner.arm_accept(this.index);
        }
        drop(inner);
        this.ring.kick();
        Poll::Pending
    }
}

/// A connection on the ring, handed to Hyper as the host transport.
pub struct UringStream {
    ring: Rc<Ring>,
    index: u32,
}

impl std::fmt::Debug for UringStream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("UringStream")
            .field("index", &self.index)
            .finish()
    }
}

fn ring_closed(inner: &Inner) -> io::Error {
    match &inner.failure {
        Some(message) => io::Error::other(message.clone()),
        None => io::Error::new(
            io::ErrorKind::NotConnected,
            "the realm's io_uring was shut down",
        ),
    }
}

impl UringStream {
    fn write_bufs(
        &self,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let mut inner = self.ring.inner.borrow_mut();
        if inner.closed {
            return Poll::Ready(Err(ring_closed(&inner)));
        }
        let conn = inner.conn(self.index);
        if let Some(error) = conn.tx_err {
            return Poll::Ready(Err(io::Error::from_raw_os_error(error)));
        }
        if conn.wr_shut {
            return Poll::Ready(Err(io::Error::from_raw_os_error(libc::EPIPE)));
        }
        let total: usize = bufs.iter().map(|buf| buf.len()).sum();
        if total == 0 {
            return Poll::Ready(Ok(0));
        }
        let room = TX_HIGH.saturating_sub(conn.queued());
        if room == 0 {
            conn.write_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        // Take everything when it fits; otherwise only what fits, so the caller sees
        // backpressure at the same granularity as a kernel send buffer.
        let mut left = room.min(total);
        let taken = left;
        conn.tx_pending.reserve(left);
        for buf in bufs {
            if left == 0 {
                break;
            }
            let part = buf.len().min(left);
            conn.tx_pending.extend_from_slice(&buf[..part]);
            left -= part;
        }
        let needs_flush = !conn.sending;
        if needs_flush {
            inner.mark_dirty(self.index);
        }
        drop(inner);
        if needs_flush {
            self.ring.kick();
        }
        Poll::Ready(Ok(taken))
    }
}

impl AsyncRead for UringStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let mut inner = this.ring.inner.borrow_mut();
        if inner.closed {
            return Poll::Ready(Err(ring_closed(&inner)));
        }
        let conn = inner.conn(this.index);
        if conn.buffered() > 0 {
            let len = conn.buffered().min(buf.remaining());
            buf.put_slice(&conn.rx[conn.rx_pos..conn.rx_pos + len]);
            conn.rx_pos += len;
            if conn.rx_pos == conn.rx.len() {
                conn.rx.clear();
                conn.rx_pos = 0;
                if conn.rx.capacity() > 4 * RX_HIGH {
                    conn.rx.shrink_to(BUF_SIZE);
                }
            }
            let rearm = inner.should_arm(inner.conns[this.index as usize].as_ref().expect("live"));
            if rearm {
                inner.arm_recv(this.index);
                drop(inner);
                this.ring.kick();
            }
            return Poll::Ready(Ok(()));
        }
        if let Some(error) = conn.rx_err {
            return Poll::Ready(Err(io::Error::from_raw_os_error(error)));
        }
        if conn.rx_eof {
            return Poll::Ready(Ok(()));
        }
        match &mut conn.read_waker {
            Some(waker) => waker.clone_from(cx.waker()),
            slot @ None => *slot = Some(cx.waker().clone()),
        }
        let idle = conn.recv == RecvState::Idle;
        if idle && inner.should_arm(inner.conns[this.index as usize].as_ref().expect("live")) {
            inner.arm_recv(this.index);
        }
        drop(inner);
        // The receive (armed at registration or just now) may not be submitted yet.
        this.ring.kick();
        Poll::Pending
    }
}

impl AsyncWrite for UringStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.write_bufs(cx, &[io::IoSlice::new(buf)])
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.write_bufs(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut inner = self.ring.inner.borrow_mut();
        if inner.closed {
            return Poll::Ready(Err(ring_closed(&inner)));
        }
        match inner.conn(self.index).tx_err {
            Some(error) => Poll::Ready(Err(io::Error::from_raw_os_error(error))),
            None => Poll::Ready(Ok(())),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut inner = self.ring.inner.borrow_mut();
        if inner.closed {
            return Poll::Ready(Err(ring_closed(&inner)));
        }
        let conn = inner.conn(self.index);
        if conn.wr_shut {
            return Poll::Ready(Ok(()));
        }
        if let Some(error) = conn.tx_err {
            return Poll::Ready(Err(io::Error::from_raw_os_error(error)));
        }
        if conn.sending || !conn.tx_pending.is_empty() {
            // The FIN must follow every byte already written.
            conn.write_waker = Some(cx.waker().clone());
            let unsubmitted = !conn.sending;
            if unsubmitted {
                inner.mark_dirty(self.index);
            }
            drop(inner);
            self.ring.kick();
            return Poll::Pending;
        }
        conn.wr_shut = true;
        let fd = conn.raw_fd();
        // SAFETY: plain syscall on a descriptor the ring owns.
        let result = unsafe { libc::shutdown(fd, libc::SHUT_WR) };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ENOTCONN) {
                return Poll::Ready(Err(error));
            }
        }
        Poll::Ready(Ok(()))
    }
}

impl Drop for UringStream {
    fn drop(&mut self) {
        let Ok(mut inner) = self.ring.inner.try_borrow_mut() else {
            // Only reachable if a waker we woke dropped a stream re-entrantly; the slot is
            // then reclaimed at realm shutdown.
            return;
        };
        let Some(conn) = inner.conns[self.index as usize].as_mut() else {
            return;
        };
        conn.dropped = true;
        conn.read_waker = None;
        conn.write_waker = None;
        let unsent = !conn.sending && !conn.tx_pending.is_empty() && conn.tx_err.is_none();
        if inner.closed {
            let conn = inner.conn(self.index);
            if conn.ops == 0 {
                inner.conns[self.index as usize] = None;
                inner.free.push(self.index);
            }
            return;
        }
        inner.cancel_recv(self.index);
        if unsent {
            inner.mark_dirty(self.index);
        }
        inner.maybe_release(self.index);
        drop(inner);
        self.ring.kick_existing();
    }
}

#[cfg(test)]
mod tests;
