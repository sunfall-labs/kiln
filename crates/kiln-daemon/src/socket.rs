//! Where a project's daemon socket lives.
//!
//! A client must be able to find the daemon for the project it is standing in with no config
//! file and no discovery handshake, so the path is a pure function of the project root:
//!
//! ```text
//! <temp dir>/<43 Base64URL characters of SHA-256(canonical root)>/s
//! ```
//!
//! Four decisions worth recording:
//!
//! * **Temp dir, not the project.** `sun_path` is 104 bytes on macOS, and a socket inside a deep
//!   checkout or on a network filesystem overflows it or is simply unsupported. Hashing keeps the
//!   path a bounded length whatever the root is, and keeps build artefacts out of the source tree.
//! * **Canonicalised root.** `.`, a trailing slash and a symlinked checkout must all resolve to
//!   the same daemon; otherwise two daemons silently fight over one project.
//! * **Full-width identity.** The socket name is the whole project-selection decision; there is no
//!   root behind it for a client to compare. All 256 SHA-256 bits are retained, with Base64URL and
//!   a short filename buying back the Unix-socket path budget instead of truncating the digest.
//! * **No protocol version in the steady-state path.** It is tempting to make incompatible builds
//!   miss each other, but then a version skew shows up as a mysterious second daemon instead of the
//!   clear `Ping` rejection the protocol is designed to give. The one pre-release migration from
//!   the weak FNV layout requires stopping old daemons; it is not a legacy lookup mode.

use kiln_digest::Digest;
use std::io;
use std::path::{Path, PathBuf};

/// What distinguishes *this build* of the executable from the one before it.
///
/// Deliberately not a version constant. `PROTOCOL_VERSION` is bumped by hand when the wire shape
/// changes, which is exactly the skew that is easy to notice; the skew that costs an afternoon is
/// a rebuild that changes an *answer* while every message stays well formed. The executable's
/// length and modification time move on every link and cost a `stat`, where hashing the binary
/// itself would cost milliseconds on every invocation of a CLI that is supposed to feel instant.
///
/// Zero means "unknown" -- no current executable, or no metadata for it -- and compares equal to
/// itself, so a platform that cannot answer degrades to the old behaviour instead of refusing to
/// run.
#[must_use]
pub fn build_identity() -> u64 {
    // Captured once, and that is the whole point rather than an optimisation. A build system that
    // replaces the executable *in place* -- which the one this repository uses does -- leaves a
    // running daemon's `current_exe()` pointing at the new file, so a daemon that recomputed this
    // per handshake would read the client's identity and agree with it every time. What has to be
    // compared is the build each side was *started* from.
    static IDENTITY: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    fn identity() -> Option<u64> {
        let metadata = std::fs::metadata(std::env::current_exe().ok()?).ok()?;
        let modified = metadata
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?;
        let mut bytes = Vec::with_capacity(24);
        bytes.extend_from_slice(&metadata.len().to_le_bytes());
        bytes.extend_from_slice(&modified.as_nanos().to_le_bytes());
        let digest = Digest::hash(&bytes).to_base64_url();
        let mut identity = [0u8; 8];
        identity.copy_from_slice(digest.as_bytes().get(..8)?);
        Some(u64::from_le_bytes(identity))
    }
    *IDENTITY.get_or_init(|| identity().unwrap_or(0))
}

/// Conservative bound on a `sockaddr_un` path: 104 bytes on macOS including the NUL, 108 on
/// Linux. Exceeding it makes `bind` fail with a much less obvious error than this one.
const MAX_SOCKET_PATH: usize = 100;

/// The socket path for the project rooted at `project_root`.
///
/// The root must exist, since it is canonicalised. The parent directory is *not* created here;
/// [`crate::Daemon::bind`] does that, because only the server needs it to exist.
pub fn socket_path(project_root: &Path) -> io::Result<PathBuf> {
    let canonical = project_root.canonicalize()?;
    socket_path_in(&canonical, &std::env::temp_dir())
}

fn socket_path_in(canonical_root: &Path, temp_dir: &Path) -> io::Result<PathBuf> {
    let digest = Digest::hash(canonical_root.as_os_str().as_encoded_bytes()).to_base64_url();
    let path = temp_dir.join(digest).join("s");

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_identity_is_one_answer_for_the_life_of_the_process() {
        // The daemon compares the build it started from against the client's. If this moved while
        // a daemon was running -- which it would if it were read from the executable on every
        // call, on a build system that replaces that file in place -- a stale daemon would agree
        // with every client and the comparison would be decorative.
        assert_eq!(build_identity(), build_identity());
        assert_ne!(
            build_identity(),
            0,
            "a test binary has an executable with metadata, so this is a real identity"
        );
    }

    #[test]
    fn same_root_always_yields_the_same_socket() {
        let root = std::env::temp_dir();
        let a = socket_path(&root).expect("temp dir exists");
        let b = socket_path(&root.join(".")).expect("temp dir exists");
        assert_eq!(a, b, "`.` must not fork a second daemon for one project");
    }

    #[test]
    fn a_symlinked_checkout_finds_the_same_daemon() {
        let base =
            std::env::temp_dir().join(format!("kiln-socket-symlink-test-{}", std::process::id()));
        let root = base.join("root");
        let alias = base.join("alias");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&root).expect("create project root");
        std::os::unix::fs::symlink(&root, &alias).expect("create root alias");

        let direct = socket_path(&root).expect("direct socket");
        let indirect = socket_path(&alias).expect("symlink socket");
        let _ = std::fs::remove_dir_all(&base);

        assert_eq!(direct, indirect);
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
    fn socket_identity_keeps_every_sha256_bit_in_a_compact_path() {
        let root = std::env::temp_dir()
            .canonicalize()
            .expect("temp dir exists");
        let path = socket_path(&root).expect("socket path");
        let expected = Digest::hash(root.as_os_str().as_encoded_bytes()).to_base64_url();

        assert_eq!(path.file_name(), Some(std::ffi::OsStr::new("s")));
        assert_eq!(
            path.parent().and_then(Path::file_name),
            Some(expected.as_ref())
        );
        assert_eq!(
            expected.len(),
            43,
            "the full 256-bit digest must be encoded"
        );
    }

    #[test]
    fn compact_layout_uses_the_entire_portable_path_budget() {
        let synthetic_temp = PathBuf::from(format!("/{}", "t".repeat(53)));
        let path = socket_path_in(Path::new("/canonical/project"), &synthetic_temp)
            .expect("a 54-byte temporary directory still fits");
        assert_eq!(path.as_os_str().len(), MAX_SOCKET_PATH);

        let too_long = PathBuf::from(format!("/{}", "t".repeat(54)));
        let error = socket_path_in(Path::new("/canonical/project"), &too_long)
            .expect_err("one more byte must be rejected before bind");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn missing_root_is_an_error_rather_than_a_guess() {
        let missing = std::env::temp_dir().join("kiln-definitely-not-here-93f1");
        assert!(socket_path(&missing).is_err());
    }
}
