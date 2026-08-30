//! The kiln compiler daemon (design.md §1).
//!
//! Iteration speed is the constraint the whole project is organised around, and a per-invocation
//! process cannot meet it: the parser, the built-in definitions and the runtime metadata all cost
//! more to load than a typical edit costs to compile. So the compiler is a resident process and
//! the CLI is a thin client over a unix socket.
//!
//! The three pieces a caller needs:
//!
//! * [`socket_path`] - where a project's daemon lives, derived from the project root alone so no
//!   configuration or discovery step is needed.
//! * [`Daemon`] - bind, then [`Daemon::serve`] until a [`ShutdownHandle`] or a
//!   `Request::Shutdown` stops it. Binding is separate from serving so a caller knows the socket
//!   is accepting connections before it does anything else.
//! * [`Client`] - the other end of the same framing.
//!
//! The server is usable in-process, which is what lets the timing harness measure cold start and
//! warm request latency without shelling out:
//!
//! ```no_run
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let socket = kiln_daemon::socket_path(std::path::Path::new("."))?;
//! let daemon = kiln_daemon::Daemon::bind(&socket)?;
//! let stop = daemon.shutdown_handle();
//! let serving = std::thread::spawn(move || daemon.serve());
//!
//! let mut client = kiln_daemon::Client::connect(&socket)?;
//! let pid = client.handshake()?;
//!
//! stop.shutdown();
//! serving.join().expect("the accept loop does not panic");
//! # let _ = pid;
//! # Ok(())
//! # }
//! ```

mod cache_gc;
mod client;
mod dispatch;
mod framing;
mod output;
mod server;
mod socket;
mod testing;

#[cfg(test)]
mod tests;

/// Stack for a thread that answers requests.
///
/// Every stage of the compiler that walks a tree does it by recursion -- lowering an expression,
/// linking a module graph, folding a project rollup -- and the depth is a property of the *input*,
/// not of the compiler. A spawned thread gets 2 MiB by default where the main thread gets 8 MiB,
/// so the work is asked to fit in a quarter of the room it was written in. It does not: the
/// 641-file `medium` workload needs a little over 2 MiB for `Project`, and overflowed here.
///
/// The failure mode is what makes this worth a constant rather than a bigger default. A stack
/// overflow in Rust is not an unwind that becomes a diagnostic; it is `SIGSEGV` and an aborted
/// process, so the daemon dies mid-connection and every other client dies with it. This is a real
/// exposure and not only a test-fixture one: `Daemon::spawn_connection` serves each client on one
/// of these, so a sufficiently nested program submitted by *any* client would take the process
/// down.
///
/// 16 MiB is address space, committed only as it is touched, so the cost of the headroom is
/// nothing until it is used. It is not a fix for unbounded recursion -- that wants a depth limit
/// reported as a diagnostic -- it is room for the bounded kind.
pub const SERVE_STACK_BYTES: usize = 16 * 1024 * 1024;

pub use client::Client;
pub use framing::MAX_MESSAGE_BYTES;
pub use output::{default_native_output, default_native_shared_output};
pub use server::{Daemon, DaemonError, ShutdownHandle};
pub use socket::socket_path;
