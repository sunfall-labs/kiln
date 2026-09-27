//! One listening socket shared by the realms of a `KILN_WORKERS` thread-mode process.
//!
//! In process mode each worker binds its own socket with `SO_REUSEPORT` and the kernel hashes
//! every new connection's 4-tuple to one of them. That spreads connections evenly only on
//! average: 64 keep-alive connections over 16 hashed sockets leave some realms with eight and
//! some with one or none for the life of the connections, and a realm that is busy keeps
//! receiving its share.
//!
//! In thread mode the realms live in one process, so the balancing moves in process, as node's
//! cluster primary does it: the first realm to listen on an address binds the one socket, a
//! thread of this module accepts on it, and every accepted connection goes to the member realm
//! with the fewest *active* connections -- accepted and not yet closed -- with ties taken
//! round-robin from the member after the last one chosen. A member is a realm's listener
//! resource; the connection's [`ListenTicket`] rides with its resource in the realm and gives
//! the count back when the resource is closed, released by the HTTP engine, or dropped with the
//! realm.
//!
//! Only threads that joined the group ([`join_listen_group`], which the provider's thread-mode
//! launcher calls on each worker realm's thread) listen this way; every other realm, including a
//! `node:worker_threads` worker started from a member, binds as it always has. A realm that
//! listens twice on one address gets a separate socket the second time, so two servers of one
//! realm never share connections with each other. As with node's cluster, the realms listening
//! on port 0 share one ephemeral port.
//!
//! When the last member closes, the address leaves the registry and the accept thread closes
//! the socket. A member that closes while others remain hands its not-yet-accepted connections
//! to them rather than dropping them.

use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::thread::ThreadId;

thread_local! {
    static JOINED: Cell<bool> = const { Cell::new(false) };
}

/// Make every listener a realm on this thread opens a member of its address's shared listener.
/// Called once on each worker realm's thread in `KILN_WORKERS` thread mode, before the realm is
/// made.
pub fn join_listen_group() {
    JOINED.with(|joined| joined.set(true));
}

/// Whether listeners opened on this thread join a shared listener.
pub(crate) fn joined() -> bool {
    JOINED.with(Cell::get)
}

type Key = (String, u16);

static REGISTRY: LazyLock<Mutex<HashMap<Key, Weak<Shared>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// One realm's share of a shared listener: the connections dispatched to it and not yet taken.
struct Inbox {
    owner: ThreadId,
    queue: Mutex<VecDeque<std::net::TcpStream>>,
    ready: tokio::sync::Notify,
    /// Connections dispatched to this member and not yet closed (queued ones included).
    active: AtomicUsize,
}

struct Members {
    list: Vec<Arc<Inbox>>,
    next: usize,
}

struct Shared {
    key: Key,
    local: std::net::SocketAddr,
    listener: OwnedFd,
    /// The accept thread's wake-up: a byte here after `stopping` is set ends it.
    wake_read: OwnedFd,
    wake_write: OwnedFd,
    stopping: AtomicBool,
    members: Mutex<Members>,
}

impl Shared {
    /// Hand one accepted connection to the member with the fewest active connections.
    fn dispatch(&self, stream: std::net::TcpStream) {
        let mut members = lock(&self.members);
        let count = members.list.len();
        if count == 0 {
            return; // Closing: dropping the stream closes it.
        }
        let start = members.next % count;
        let mut chosen = start;
        let mut least = usize::MAX;
        for offset in 0..count {
            let at = (start + offset) % count;
            let active = members.list[at].active.load(Ordering::Relaxed);
            if active < least {
                least = active;
                chosen = at;
            }
        }
        members.next = chosen + 1;
        let inbox = &members.list[chosen];
        inbox.active.fetch_add(1, Ordering::Relaxed);
        lock(&inbox.queue).push_back(stream);
        inbox.ready.notify_one();
    }

    fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
        let byte = 1u8;
        // SAFETY: a valid pipe write end owned by `self`, and a one-byte buffer.
        unsafe {
            libc::write(self.wake_write.as_raw_fd(), (&byte as *const u8).cast(), 1);
        }
    }
}

fn accept_one(listener: &OwnedFd) -> io::Result<std::net::TcpStream> {
    // SAFETY: `accept` on a listening socket this module owns; the address is not asked for.
    #[cfg(any(target_os = "linux", target_os = "android", target_os = "freebsd"))]
    let fd = unsafe {
        libc::accept4(
            listener.as_raw_fd(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
        )
    };
    #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "freebsd")))]
    let fd = unsafe {
        let fd = libc::accept(
            listener.as_raw_fd(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        if fd >= 0 {
            libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK);
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
        fd
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `accept` returned a new descriptor this function owns.
    Ok(unsafe { std::net::TcpStream::from_raw_fd(fd) })
}

/// The accept thread: wait for the listener or the wake pipe, accept everything ready, dispatch.
fn accept_loop(shared: Arc<Shared>) {
    loop {
        let mut fds = [
            libc::pollfd {
                fd: shared.listener.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: shared.wake_read.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: two valid pollfd entries.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
        if shared.stopping.load(Ordering::Acquire) {
            return;
        }
        if ready < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
        loop {
            match accept_one(&shared.listener) {
                Ok(stream) => shared.dispatch(stream),
                Err(error) => match error.raw_os_error() {
                    Some(libc::EINTR) | Some(libc::ECONNABORTED) | Some(libc::EPROTO) => continue,
                    Some(libc::EMFILE) | Some(libc::ENFILE) | Some(libc::ENOBUFS)
                    | Some(libc::ENOMEM) => {
                        // Out of descriptors: leave the connection in the backlog and let the
                        // realms close some, rather than spinning on a ready listener.
                        std::thread::sleep(std::time::Duration::from_millis(10));
                        break;
                    }
                    _ => break,
                },
            }
        }
    }
}

/// A realm's listener on a shared address.
pub(crate) struct GroupMember {
    shared: Arc<Shared>,
    inbox: Arc<Inbox>,
}

/// One accepted connection's place in its member's active count, given back on drop.
pub(crate) struct ListenTicket(Arc<Inbox>);

impl Drop for ListenTicket {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::Relaxed);
    }
}

impl std::fmt::Debug for ListenTicket {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ListenTicket")
    }
}

impl GroupMember {
    pub(crate) fn local_addr(&self) -> io::Result<std::net::SocketAddr> {
        Ok(self.shared.local)
    }

    /// The next connection dispatched to this member.
    pub(crate) fn accept(
        &self,
    ) -> impl std::future::Future<Output = io::Result<(std::net::TcpStream, ListenTicket)>> + 'static
    {
        let inbox = Arc::clone(&self.inbox);
        async move {
            loop {
                if let Some(stream) = lock(&inbox.queue).pop_front() {
                    return Ok((stream, ListenTicket(Arc::clone(&inbox))));
                }
                inbox.ready.notified().await;
            }
        }
    }
}

impl Drop for GroupMember {
    fn drop(&mut self) {
        let mut registry = lock(&REGISTRY);
        let orphans: Vec<std::net::TcpStream> = {
            let mut members = lock(&self.shared.members);
            members
                .list
                .retain(|inbox| !Arc::ptr_eq(inbox, &self.inbox));
            let orphans: Vec<_> = lock(&self.inbox.queue).drain(..).collect();
            self.inbox
                .active
                .fetch_sub(orphans.len(), Ordering::Relaxed);
            if members.list.is_empty() {
                if registry
                    .get(&self.shared.key)
                    .is_some_and(|entry| std::ptr::eq(entry.as_ptr(), Arc::as_ptr(&self.shared)))
                {
                    registry.remove(&self.shared.key);
                }
                self.shared.stop();
                return;
            }
            orphans
        };
        drop(registry);
        for stream in orphans {
            self.shared.dispatch(stream);
        }
    }
}

fn wake_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: room for the two descriptors `pipe` writes.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `pipe` returned two new descriptors this function owns.
    let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    for fd in [&read, &write] {
        // SAFETY: valid descriptors.
        unsafe {
            libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK);
        }
    }
    Ok((read, write))
}

/// What a thread-mode realm's listen gives: a member of the shared listener for `(host, port)`,
/// or `None` when this realm already has one there and must bind a socket of its own.
pub(crate) fn listen(host: &str, port: u16) -> io::Result<Option<GroupMember>> {
    let key = (host.to_owned(), port);
    let owner = std::thread::current().id();
    let mut registry = lock(&REGISTRY);
    if let Some(shared) = registry.get(&key).and_then(Weak::upgrade)
        && !shared.stopping.load(Ordering::Acquire)
    {
        let mut members = lock(&shared.members);
        if members.list.iter().any(|inbox| inbox.owner == owner) {
            return Ok(None);
        }
        let inbox = Arc::new(Inbox {
            owner,
            queue: Mutex::new(VecDeque::new()),
            ready: tokio::sync::Notify::new(),
            active: AtomicUsize::new(0),
        });
        members.list.push(Arc::clone(&inbox));
        drop(members);
        return Ok(Some(GroupMember { shared, inbox }));
    }
    let listener = crate::bind_tcp_listener_std(host, port)?;
    let local = listener.local_addr()?;
    let (wake_read, wake_write) = wake_pipe()?;
    let inbox = Arc::new(Inbox {
        owner,
        queue: Mutex::new(VecDeque::new()),
        ready: tokio::sync::Notify::new(),
        active: AtomicUsize::new(0),
    });
    let shared = Arc::new(Shared {
        key: key.clone(),
        local,
        listener: OwnedFd::from(listener),
        wake_read,
        wake_write,
        stopping: AtomicBool::new(false),
        members: Mutex::new(Members {
            list: vec![Arc::clone(&inbox)],
            next: 0,
        }),
    });
    let thread_shared = Arc::clone(&shared);
    std::thread::Builder::new()
        .name(format!("kiln-accept-{}", local.port()))
        .spawn(move || accept_loop(thread_shared))?;
    registry.insert(key, Arc::downgrade(&shared));
    Ok(Some(GroupMember { shared, inbox }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::time::Duration;

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn accept_within(
        runtime: &tokio::runtime::Runtime,
        member: &GroupMember,
    ) -> Option<(std::net::TcpStream, ListenTicket)> {
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(2), member.accept())
                .await
                .ok()
                .map(Result::unwrap)
        })
    }

    /// A port nothing listens on. Port 0 itself is one shared key, as in node's cluster, so tests
    /// running side by side would join each other's listener through it.
    fn free_port() -> u16 {
        std::net::TcpListener::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    /// Members on other threads: each test thread stands in for a realm's thread.
    fn member_on_thread(port: u16) -> GroupMember {
        std::thread::spawn(move || listen("127.0.0.1", port).unwrap().unwrap())
            .join()
            .unwrap()
    }

    #[test]
    fn connections_go_to_the_member_with_the_fewest_active() {
        let port = free_port();
        let first = listen("127.0.0.1", port).unwrap().unwrap();
        let second = member_on_thread(port);
        let third = member_on_thread(port);
        assert_eq!(second.local_addr().unwrap().port(), port);
        let runtime = runtime();

        // Six connections kept open: two each, whatever order they arrive in.
        let clients: Vec<_> = (0..6)
            .map(|_| std::net::TcpStream::connect(("127.0.0.1", port)).unwrap())
            .collect();
        let mut held = Vec::new();
        for member in [&first, &second, &third] {
            for _ in 0..2 {
                held.push(accept_within(&runtime, member).expect("two connections each"));
            }
        }
        assert!(runtime.block_on(async {
            tokio::time::timeout(Duration::from_millis(100), first.accept())
                .await
                .is_err()
        }));

        // Closing the second member's two frees it: the next two connections both go there.
        held.drain(2..4);
        let _more: Vec<_> = (0..2)
            .map(|_| std::net::TcpStream::connect(("127.0.0.1", port)).unwrap())
            .collect();
        assert!(accept_within(&runtime, &second).is_some());
        assert!(accept_within(&runtime, &second).is_some());
        drop(clients);
    }

    #[test]
    fn a_closing_member_hands_its_queue_on_and_the_last_closes_the_port() {
        let port = free_port();
        let first = listen("127.0.0.1", port).unwrap().unwrap();
        let second = member_on_thread(port);
        let runtime = runtime();
        let mut client = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut other = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        // Wait until both are dispatched, one to each member.
        let (mut stream, _ticket) = accept_within(&runtime, &first).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        drop(second);
        let (mut handed, _ticket2) = accept_within(&runtime, &first).expect("handed on");
        for (server, client) in [(&mut stream, &mut client), (&mut handed, &mut other)] {
            server.set_nonblocking(false).unwrap();
            server.write_all(b"x").unwrap();
            let mut byte = [0u8; 1];
            client.read_exact(&mut byte).unwrap();
        }
        drop(first);
        std::thread::sleep(Duration::from_millis(100));
        assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err());
    }

    #[test]
    fn a_realm_listening_twice_binds_its_own_socket() {
        let port = free_port();
        let _first = listen("127.0.0.1", port).unwrap().unwrap();
        assert!(listen("127.0.0.1", port).unwrap().is_none());
    }
}
