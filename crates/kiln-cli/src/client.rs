//! Talking to a daemon.
//!
//! The wire format lives in `kiln-daemon` beside the server, so this is a thin adapter over
//! [`kiln_daemon::Client`] rather than a second implementation of the framing. What is the
//! CLI's own business is process lifecycle: deciding that no daemon is running, starting one,
//! and waiting for it to accept connections.

use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use kiln_proto::{Request, Response};

use crate::backend::{Backend, interpret};
use crate::error::{Error, Result};

/// How long to wait for a freshly spawned daemon to start accepting connections. Generous:
/// the cost of being wrong is a spurious failure on a loaded machine, and the wait ends as
/// soon as the socket answers.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const STARTUP_POLL: Duration = Duration::from_millis(10);
/// Lines of daemon output quoted back when a spawned daemon dies before it can listen.
const LOG_TAIL_LINES: usize = 10;
/// The daemon's socket directory is 0700 (see `kiln_daemon::Daemon::bind_with`); creating it
/// here to hold the log file must not widen that.
const PRIVATE_DIR: u32 = 0o700;
/// The bound `kiln-daemon` applies to the paths it derives: `sockaddr_un` holds 104 bytes on
/// macOS, 108 on Linux. Repeated here because `--socket` and `$KILN_SOCKET` skip that
/// derivation, and the raw failure ("path must be shorter than SUN_LEN") says nothing about
/// which knob to turn.
const MAX_SOCKET_PATH: usize = 100;

pub struct DaemonClient {
    inner: kiln_daemon::Client,
    socket: PathBuf,
}

impl DaemonClient {
    /// Connect to a daemon that is already listening.
    pub fn connect(socket: &Path) -> Result<DaemonClient> {
        match kiln_daemon::Client::connect(socket) {
            Ok(inner) => Ok(DaemonClient {
                inner,
                socket: socket.to_owned(),
            }),
            // Both kinds mean "nobody is serving here": no socket file at all, or a file left
            // behind by a daemon that died.
            Err(source)
                if matches!(
                    source.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                Err(Error::NotRunning)
            }
            Err(source) => Err(Error::io(
                format!("cannot connect to {}", socket.display()),
                source,
            )),
        }
    }

    /// Connect, starting a daemon first if nothing is listening.
    ///
    /// `kiln check` is the command a developer runs all day, so needing a separate
    /// `kiln daemon start` would be a papercut on the one path that has to feel instant.
    pub fn connect_or_start(socket: &Path) -> Result<DaemonClient> {
        match DaemonClient::connect(socket) {
            Err(Error::NotRunning) => {
                let child = spawn_daemon(socket)?;
                wait_for_daemon(socket, child)
            }
            other => other,
        }
    }

    /// Exchange versions and learn the daemon's pid.
    ///
    /// Worth doing on every connection: a daemon outlives the binary that started it, so the
    /// usual skew is a freshly built CLI talking to yesterday's daemon.
    pub fn handshake(&mut self) -> Result<u32> {
        self.inner
            .handshake()
            .map_err(|source| match source.kind() {
                // The daemon reports a protocol mismatch as `Response::Error`, and its message
                // already says what to do about it.
                std::io::ErrorKind::InvalidData => Error::Protocol(source.to_string()),
                _ => Error::io("cannot greet the daemon", source),
            })
    }

    /// Ask the daemon to exit. It unlinks its own socket on the way out.
    pub fn shutdown(mut self) -> Result<()> {
        match self.request(Request::Shutdown)? {
            Response::Ok => Ok(()),
            other => Err(Error::Protocol(format!(
                "Shutdown was answered with {other:?}"
            ))),
        }
    }
}

impl Backend for DaemonClient {
    fn peer(&self) -> Result<Box<dyn Backend>> {
        let mut peer = DaemonClient::connect(&self.socket)?;
        peer.handshake()?;
        Ok(Box::new(peer))
    }

    fn request(&mut self, request: Request) -> Result<Response> {
        let response = self
            .inner
            .request(&request)
            .map_err(|source| Error::io("cannot reach the daemon", source))?;
        interpret(response)
    }
}

/// Where this project's daemon listens.
///
/// The derivation belongs to `kiln-daemon` so that the client and the server cannot disagree;
/// all the CLI adds is the overrides. `$KILN_SOCKET` and `--socket` exist so that a test, or
/// two experiments side by side, can have a daemon of their own.
pub fn socket_path(explicit: Option<PathBuf>, project_root: &Path) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return within_sun_path(path, "--socket");
    }
    if let Some(path) = std::env::var_os("KILN_SOCKET") {
        return within_sun_path(PathBuf::from(path), "$KILN_SOCKET");
    }
    kiln_daemon::socket_path(project_root)
        .map_err(|source| Error::io("cannot derive the daemon socket path", source))
}

fn within_sun_path(path: PathBuf, source: &str) -> Result<PathBuf> {
    let length = path.as_os_str().len();
    if length > MAX_SOCKET_PATH {
        return Err(Error::usage(format!(
            "{source} is {length} bytes; a unix socket path must be at most {MAX_SOCKET_PATH}"
        )));
    }
    Ok(path)
}

/// Where a spawned daemon's output goes, so a daemon that dies at startup is diagnosable.
pub fn log_path(socket: &Path) -> PathBuf {
    socket.with_extension("log")
}

fn spawn_daemon(socket: &Path) -> Result<Child> {
    let log = log_path(socket);
    if let Some(parent) = log.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(PRIVATE_DIR)
            .create(parent)
            .map_err(|source| Error::io(format!("cannot create {}", parent.display()), source))?;
    }
    let out = std::fs::File::create(&log)
        .map_err(|source| Error::io(format!("cannot create {}", log.display()), source))?;
    let err = out
        .try_clone()
        .map_err(|source| Error::io("cannot duplicate the daemon log handle", source))?;

    let exe = std::env::current_exe()
        .map_err(|source| Error::io("cannot locate the kiln executable", source))?;

    // The daemon inherits this process group, so a Ctrl-C aimed at an interactive shell
    // reaches it too. Detaching properly needs setsid(2), which is a libc call this crate
    // deliberately does not have; until then, a killed daemon simply restarts on the next
    // command, which is the behaviour autostart already provides.
    Command::new(exe)
        .arg("daemon")
        .arg("serve")
        .arg("--socket")
        .arg(socket)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err))
        .spawn()
        .map_err(|source| Error::io("cannot start the daemon", source))
}

fn wait_for_daemon(socket: &Path, mut child: Child) -> Result<DaemonClient> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        match DaemonClient::connect(socket) {
            Err(Error::NotRunning) => {}
            other => return other,
        }
        if let Ok(Some(status)) = child.try_wait() {
            // Losing a startup race with another `kiln` is a normal outcome, and the daemon
            // refuses to double-bind rather than fighting over the socket. The winner is
            // listening, so try once more before calling this a failure.
            if let Ok(client) = DaemonClient::connect(socket) {
                return Ok(client);
            }
            return Err(Error::Daemon(format!(
                "the daemon exited immediately ({status})\n{}",
                log_tail(&log_path(socket))
            )));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            return Err(Error::Daemon(format!(
                "the daemon did not start listening on {} within {}s\n{}",
                socket.display(),
                STARTUP_TIMEOUT.as_secs(),
                log_tail(&log_path(socket))
            )));
        }
        std::thread::sleep(STARTUP_POLL);
    }
}

fn log_tail(log: &Path) -> String {
    let Ok(text) = std::fs::read_to_string(log) else {
        return format!("(no output in {})", log.display());
    };
    let lines: Vec<&str> = text.lines().collect();
    let tail = &lines[lines.len().saturating_sub(LOG_TAIL_LINES)..];
    if tail.is_empty() {
        format!("(no output in {})", log.display())
    } else {
        format!(
            "last output from {}:\n  {}",
            log.display(),
            tail.join("\n  ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands;

    #[test]
    fn an_explicit_socket_beats_the_derived_one() {
        let chosen = socket_path(Some(PathBuf::from("/tmp/explicit.sock")), Path::new("."))
            .expect("an explicit path needs no derivation");
        assert_eq!(chosen, PathBuf::from("/tmp/explicit.sock"));
    }

    #[test]
    fn an_over_long_socket_path_is_rejected_with_advice() {
        let too_long = PathBuf::from(format!("/tmp/{}.sock", "x".repeat(MAX_SOCKET_PATH)));
        let error = socket_path(Some(too_long), Path::new(".")).expect_err("should be rejected");
        assert!(error.to_string().contains("--socket is"), "{error}");
    }

    #[test]
    fn the_log_sits_beside_the_socket() {
        assert_eq!(
            log_path(Path::new("/tmp/kiln-abc/daemon.sock")),
            PathBuf::from("/tmp/kiln-abc/daemon.log")
        );
    }

    #[test]
    fn connecting_to_nothing_reports_that_nothing_is_running() {
        let nowhere = std::env::temp_dir().join("kiln-cli-no-daemon-here.sock");
        let _ = std::fs::remove_file(&nowhere);
        assert!(matches!(
            DaemonClient::connect(&nowhere),
            Err(Error::NotRunning)
        ));
    }

    /// The whole client stack against a real server: framing, handshake, request order and
    /// the rendered report. Everything except spawning a process, which is what the daemon's
    /// own tests already cover.
    #[test]
    fn checks_a_file_end_to_end_against_a_real_daemon() {
        let dir = std::env::temp_dir().join(format!("kiln-cli-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create fixture");
        let source = dir.join("hot.js");
        std::fs::write(&source, "export function greet() { return 1; }\n").expect("write source");

        let socket = dir.join("daemon.sock");
        let daemon = kiln_daemon::Daemon::bind(&socket).expect("bind the test daemon");
        let stop = daemon.shutdown_handle();
        let serving = std::thread::spawn(move || daemon.serve());

        let mut client = DaemonClient::connect(&socket).expect("connect");
        assert_eq!(
            client.handshake().expect("handshake"),
            std::process::id(),
            "the in-process daemon should report this process"
        );

        let first = commands::check(&mut client, &source)
            .expect("first check")
            .text;
        let second = commands::check(&mut client, &source)
            .expect("second check")
            .text;

        // A file that did not change must produce the same answer, and the report must show
        // that the second answer cost less work than the first.
        let identity = |text: &str| {
            text.lines()
                .filter(|line| {
                    line.starts_with("  interface") || line.starts_with("  implementation")
                })
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            identity(&first),
            identity(&second),
            "{first}\n---\n{second}"
        );
        assert_eq!(identity(&first).len(), 2, "{first}");
        assert!(first.contains("hot.js  r"), "{first}");
        assert!(
            second.contains("reused"),
            "an unchanged file must reuse something:\n{second}"
        );

        let missing = commands::check(&mut client, Path::new("/definitely/not/here.js"));
        assert!(matches!(missing, Err(Error::MissingFile(_))), "{missing:?}");

        stop.shutdown();
        serving.join().expect("the accept loop does not panic");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
