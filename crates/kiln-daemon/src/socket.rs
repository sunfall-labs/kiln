//! Where a project's daemon socket lives.
//!
//! A client must be able to find the daemon for the project it is standing in with no config
//! file and no discovery handshake, so the path is a pure function of the project root:
//!
//! ```text
//! <temp dir>/kiln-<16 hex digits of the canonical root>/daemon.sock
//! ```
//!
//! Three decisions worth recording:
//!
//! * **Temp dir, not the project.** `sun_path` is 104 bytes on macOS, and a socket inside a deep
//!   checkout or on a network filesystem overflows it or is simply unsupported. Hashing keeps the
//!   path a bounded length whatever the root is, and keeps build artefacts out of the source tree.
//! * **Canonicalised root.** `.`, a trailing slash and a symlinked checkout must all resolve to
//!   the same daemon; otherwise two daemons silently fight over one project.
//! * **No protocol version in the path.** It is tempting to make incompatible builds miss each
//!   other, but then a version skew shows up as a mysterious second daemon instead of the clear
//!   `Ping` rejection the protocol is designed to give.

use std::io;
use std::path::{Path, PathBuf};

/// Conservative bound on a `sockaddr_un` path: 104 bytes on macOS including the NUL, 108 on
/// Linux. Exceeding it makes `bind` fail with a much less obvious error than this one.
const MAX_SOCKET_PATH: usize = 100;

/// The socket path for the project rooted at `project_root`.
///
/// The root must exist, since it is canonicalised. The parent directory is *not* created here;
/// [`crate::Daemon::bind`] does that, because only the server needs it to exist.
pub fn socket_path(project_root: &Path) -> io::Result<PathBuf> {
    let canonical = project_root.canonicalize()?;
    let digest = fnv1a64(canonical.as_os_str().as_encoded_bytes());
    let path = std::env::temp_dir()
        .join(format!("kiln-{digest:016x}"))
        .join("daemon.sock");

    if path.as_os_str().len() > MAX_SOCKET_PATH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "unix socket path {} is {} bytes, over the {MAX_SOCKET_PATH} byte limit; \
                 set TMPDIR to somewhere shorter",
                path.display(),
                path.as_os_str().len()
            ),
        ));
    }
    Ok(path)
}

/// FNV-1a. Not cryptographic, and it does not need to be: the only requirement is that two
/// different roots are overwhelmingly unlikely to name the same socket. Written out rather than
/// pulled from a crate to keep this crate's dependency set at the three it already has.
fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(PRIME)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_root_always_yields_the_same_socket() {
        let root = std::env::temp_dir();
        let a = socket_path(&root).expect("temp dir exists");
        let b = socket_path(&root.join(".")).expect("temp dir exists");
        assert_eq!(a, b, "`.` must not fork a second daemon for one project");
    }

    #[test]
    fn different_roots_yield_different_sockets() {
        let base = std::env::temp_dir().join(format!("kiln-socket-test-{}", std::process::id()));
        let (one, two) = (base.join("one"), base.join("two"));
        std::fs::create_dir_all(&one).expect("create fixture");
        std::fs::create_dir_all(&two).expect("create fixture");

        let result = socket_path(&one).and_then(|a| socket_path(&two).map(|b| (a, b)));
        let _ = std::fs::remove_dir_all(&base);

        let (a, b) = result.expect("both roots exist");
        assert_ne!(a, b);
    }

    #[test]
    fn socket_path_fits_in_sockaddr_un() {
        let path = socket_path(&std::env::temp_dir()).expect("temp dir exists");
        assert!(
            path.as_os_str().len() <= MAX_SOCKET_PATH,
            "{}",
            path.display()
        );
    }

    #[test]
    fn missing_root_is_an_error_rather_than_a_guess() {
        let missing = std::env::temp_dir().join("kiln-definitely-not-here-93f1");
        assert!(socket_path(&missing).is_err());
    }
}
