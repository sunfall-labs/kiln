//! `kiln daemon start | stop | status | serve`.
//!
//! Process lifecycle, kept apart from the query commands because it is the one part of the
//! CLI that is about the daemon rather than about the compiler. Everyday use should never
//! need any of it - `kiln check` starts a daemon on its own - so these exist for the cases
//! where a developer wants to be explicit: pinning a daemon before a benchmark, restarting
//! one after a rebuild, or checking whether that stale process is still around.

use std::path::Path;

use kiln_daemon::Daemon;
use kiln_proto::PROTOCOL_VERSION;

use crate::client::{DaemonClient, log_path};
use crate::commands::Output;
use crate::error::{Error, Result};

pub fn start(socket: &Path) -> Result<Output> {
    match DaemonClient::connect(socket) {
        Ok(mut client) => {
            let (pid, _) = client.handshake()?;
            Ok(Output::new(format!(
                "already running  pid {pid}  {}\n",
                socket.display()
            )))
        }
        Err(Error::NotRunning) => {
            let mut client = DaemonClient::connect_or_start(socket)?;
            let (pid, _) = client.handshake()?;
            Ok(Output::new(format!(
                "started  pid {pid}  {}\n",
                socket.display()
            )))
        }
        Err(other) => Err(other),
    }
}

pub fn stop(socket: &Path) -> Result<Output> {
    match DaemonClient::connect(socket) {
        Ok(mut client) => match client.handshake() {
            Ok((pid, _)) => {
                client.shutdown()?;
                Ok(Output::new(format!("stopped  pid {pid}\n")))
            }
            // A daemon speaking a protocol this binary does not is the one most in need of
            // stopping, and it is exactly the one the handshake refuses to talk to. Refusing to
            // stop it leaves `kill` as the only way out of an ordinary version bump, which is
            // the opposite of what this command is for.
            //
            // The daemon hangs up on a mismatched greeting, so this needs a fresh connection and
            // no greeting on it. That is safe for `Shutdown` specifically and for nothing else:
            // it carries no payload, so there is no shape to disagree about, and the daemon
            // answers it without requiring a prior `Ping`. The pid is not reported because
            // learning it *is* the greeting that just failed.
            Err(Error::Protocol(mismatch)) => {
                DaemonClient::connect(socket)?.shutdown()?;
                Ok(Output::new(format!(
                    "stopped  {}\n  the daemon spoke another protocol: {mismatch}\n",
                    socket.display()
                )))
            }
            Err(other) => Err(other),
        },
        // Stopping something that is not running is the outcome the caller wanted, so it is
        // not a failure - the same reasoning as `rm -f`.
        Err(Error::NotRunning) => Ok(Output::new(format!("not running  {}\n", socket.display()))),
        Err(other) => Err(other),
    }
}

/// Returns the report and the exit code: non-zero when no daemon is running, so that shell
/// scripts can branch on it without parsing the output.
pub fn status(socket: &Path) -> Result<(Output, i32)> {
    match DaemonClient::connect(socket) {
        Ok(mut client) => {
            let (pid, _) = client.handshake()?;
            let text = format!(
                "running  pid {pid}  protocol {PROTOCOL_VERSION}  {}\n",
                socket.display()
            );
            Ok((Output::new(text), 0))
        }
        Err(Error::NotRunning) => Ok((
            Output::new(format!("not running  {}\n", socket.display())),
            1,
        )),
        Err(other) => Err(other),
    }
}

/// Run the accept loop in the foreground. This is what `start` and autostart spawn, and what
/// a developer runs by hand when they want to watch the daemon's own output.
pub fn serve(socket: &Path) -> Result<()> {
    let daemon = Daemon::bind(socket)
        .map_err(|source| Error::Daemon(source.to_string()))?
        .isolate_hosted_programs();
    // The spawned copy has this redirected into the log beside the socket, which is where
    // `wait_for_daemon` looks when a daemon dies before it can listen.
    eprintln!(
        "kiln daemon: serving {} (protocol {PROTOCOL_VERSION}, pid {})",
        socket.display(),
        std::process::id()
    );
    daemon.serve();
    // Serving ended, so nothing will read the log again; leaving it would make the next
    // startup failure ambiguous.
    let _ = std::fs::remove_file(log_path(socket));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        dir: std::path::PathBuf,
        socket: std::path::PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Fixture {
            let dir =
                std::env::temp_dir().join(format!("kiln-cli-life-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create fixture");
            let socket = dir.join("daemon.sock");
            Fixture { dir, socket }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn status_reports_a_live_daemon_and_exits_zero() {
        let fixture = Fixture::new("status");
        let daemon = Daemon::bind(&fixture.socket).expect("bind");
        let stop_handle = daemon.shutdown_handle();
        let serving = std::thread::Builder::new()
            .stack_size(kiln_daemon::SERVE_STACK_BYTES)
            .spawn(move || daemon.serve())
            .expect("spawn the in-process daemon");

        let (output, code) = status(&fixture.socket).expect("status succeeds");
        assert_eq!(code, 0);
        assert!(output.text.starts_with("running  pid "), "{}", output.text);
        assert!(
            output.text.contains(&format!("pid {}", std::process::id())),
            "{}",
            output.text
        );

        stop_handle.shutdown();
        serving.join().expect("the accept loop does not panic");
    }

    #[test]
    fn status_exits_non_zero_when_nothing_is_running() {
        let fixture = Fixture::new("nostatus");
        let (output, code) = status(&fixture.socket).expect("status succeeds");
        assert_eq!(code, 1, "scripts branch on this");
        assert!(output.text.starts_with("not running  "), "{}", output.text);
    }

    #[test]
    fn stop_shuts_a_daemon_down_and_is_harmless_when_repeated() {
        let fixture = Fixture::new("stop");
        let daemon = Daemon::bind(&fixture.socket).expect("bind");
        let serving = std::thread::Builder::new()
            .stack_size(kiln_daemon::SERVE_STACK_BYTES)
            .spawn(move || daemon.serve())
            .expect("spawn the in-process daemon");

        let output = stop(&fixture.socket).expect("stop succeeds");
        assert!(output.text.starts_with("stopped  pid "), "{}", output.text);
        serving.join().expect("the accept loop ends after Shutdown");

        let again = stop(&fixture.socket).expect("stopping twice is not an error");
        assert!(again.text.starts_with("not running  "), "{}", again.text);
    }
}
