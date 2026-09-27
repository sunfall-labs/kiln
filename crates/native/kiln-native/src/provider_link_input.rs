//! Bounded raw staging for the immutable number-provider archive used by native links.
//!
//! ArtifactCache records carry a header, so a linker cannot consume their paths directly. Keep a
//! small raw, content-addressed copy beside the writable cache layer and hard-link it into each
//! fresh work directory. The per-link path remains private to that build, while repeated links do
//! not rewrite a 20+ MiB archive. The pool is an optimization: oversized archives use the normal
//! materialization path, and cache corruption remains an error.

use std::fs::{self, File, FileTimes, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use kiln_artifact_cache::{ArtifactCache, CacheError, Digest};
use parking_lot::RwLock;

use crate::{NativeError, materialize};

const POOL_DIRECTORY: &str = ".raw-link-inputs/v1";
const LOCK_FILE: &str = ".lock";
const ARCHIVE_SUFFIX: &str = ".a";
const TEMP_PREFIX: &str = ".archive-tmp-";
/// Enough for several provider variants without allowing old toolchains/targets to accumulate
/// without bound. A single archive above this limit bypasses the pool.
const MAX_POOL_BYTES: u64 = 96 * 1024 * 1024;
const COPY_BUFFER_BYTES: usize = 64 * 1024;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
// std file locks are advisory and process-scoped differently across supported hosts. This local
// lock makes the shared/exclusive protocol reliable among builders in this process too.
static PROCESS_LOCK: RwLock<()> = RwLock::new(());

/// Materialize a provider archive for a link, reusing its validated raw bytes across link workspaces.
/// Call only on a link-cache miss; a warm executable/shared-library hit never visits this pool.
pub(crate) fn materialize_provider_archive(
    cache: &ArtifactCache,
    destination: &Path,
    bytes: &[u8],
    digest: Digest,
) -> Result<(), NativeError> {
    if bytes.len() as u64 > MAX_POOL_BYTES {
        return materialize(destination, bytes, false);
    }
    let Some(layer) = cache.layers().iter().find(|layer| layer.is_writable()) else {
        return materialize(destination, bytes, false);
    };
    let pool = layer.root().join(POOL_DIRECTORY);
    materialize_from_pool(&pool, destination, bytes, digest, MAX_POOL_BYTES)
}

fn materialize_from_pool(
    pool: &Path,
    destination: &Path,
    bytes: &[u8],
    digest: Digest,
    budget: u64,
) -> Result<(), NativeError> {
    // `bytes` comes from cached_number_provider_for, whose manifest/archive validation binds it
    // to this digest. Reuse checks compare staged bytes directly to this validated slice, avoiding
    // another full-size hash and any archive-sized temporary allocation on every link.
    ensure_pool_directory(pool)?;

    // Warm inputs take shared locks, allowing independent link misses to hard-link different
    // provider variants in parallel. Eviction and publication take the exclusive lock.
    {
        let _process = PROCESS_LOCK.read();
        let lock = open_pool_lock(pool)?;
        lock.lock_shared()
            .map_err(|source| io_error("lock raw provider pool", pool, source))?;
        match verify_and_link_existing(pool, destination, bytes, digest)? {
            Existing::Linked => return Ok(()),
            Existing::NeedsCleanup => {}
            Existing::CopyFallback => {
                drop(lock);
                drop(_process);
                return materialize(destination, bytes, false);
            }
            Existing::Missing => {}
        }
    }

    let _process = PROCESS_LOCK.write();
    let lock = open_pool_lock(pool)?;
    lock.lock()
        .map_err(|source| io_error("lock raw provider pool", pool, source))?;
    remove_stale_temporaries(pool)?;
    match verify_and_link_existing(pool, destination, bytes, digest)? {
        Existing::Linked => return Ok(()),
        Existing::NeedsCleanup => unreachable!("exclusive lock cleaned stale temporaries"),
        Existing::CopyFallback => {
            drop(lock);
            drop(_process);
            return materialize(destination, bytes, false);
        }
        Existing::Missing => {}
    }

    make_room(pool, bytes.len() as u64, budget)?;
    let archive = archive_path(pool, digest);
    let temporary = write_archive_temporary(pool, bytes)?;
    match fs::hard_link(&temporary, &archive) {
        Ok(()) => {}
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
            verify_archive(&archive, bytes)?;
        }
        Err(source) => {
            let _ = fs::remove_file(&temporary);
            return Err(io_error("publish raw provider archive", &archive, source));
        }
    }
    fs::remove_file(&temporary)
        .map_err(|source| io_error("remove raw provider temporary", &temporary, source))?;

    // The exclusive pool lock prevents cleanup from unlinking the staged path before this
    // workspace owns a hard link to it. Any later eviction only removes the pool's name; this
    // build's input inode remains live until its WorkDirectory is removed.
    match hard_link_or_copy_fallback(&archive, destination)? {
        LinkResult::Linked => {
            touch(&archive);
            Ok(())
        }
        LinkResult::CopyFallback => {
            drop(lock);
            drop(_process);
            materialize(destination, bytes, false)
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Existing {
    Missing,
    Linked,
    CopyFallback,
    NeedsCleanup,
}

fn verify_and_link_existing(
    pool: &Path,
    destination: &Path,
    bytes: &[u8],
    digest: Digest,
) -> Result<Existing, NativeError> {
    if has_stale_temporaries(pool)? {
        return Ok(Existing::NeedsCleanup);
    }
    let archive = archive_path(pool, digest);
    match fs::symlink_metadata(&archive) {
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(Existing::Missing),
        Err(source) => Err(io_error("inspect raw provider archive", &archive, source)),
        Ok(metadata) if !metadata.file_type().is_file() => Err(corrupt(
            &archive,
            "content-addressed archive is not a regular file",
        )),
        Ok(_) => {
            verify_archive(&archive, bytes)?;
            match hard_link_or_copy_fallback(&archive, destination)? {
                LinkResult::Linked => {
                    touch(&archive);
                    Ok(Existing::Linked)
                }
                LinkResult::CopyFallback => Ok(Existing::CopyFallback),
            }
        }
    }
}

fn verify_archive(path: &Path, expected: &[u8]) -> Result<(), NativeError> {
    let mut file =
        File::open(path).map_err(|source| io_error("read raw provider archive", path, source))?;
    let metadata = file
        .metadata()
        .map_err(|source| io_error("inspect raw provider archive", path, source))?;
    if !metadata.is_file() || metadata.len() != expected.len() as u64 {
        return Err(corrupt(
            path,
            "same digest address contains a different payload",
        ));
    }
    let mut buffer = [0u8; COPY_BUFFER_BYTES];
    let mut offset = 0;
    while offset < expected.len() {
        let count = (expected.len() - offset).min(buffer.len());
        file.read_exact(&mut buffer[..count])
            .map_err(|source| io_error("read raw provider archive", path, source))?;
        if buffer[..count] != expected[offset..offset + count] {
            return Err(corrupt(
                path,
                "same digest address contains a different payload",
            ));
        }
        offset += count;
    }
    let mut trailing = [0u8; 1];
    if file
        .read(&mut trailing)
        .map_err(|source| io_error("read raw provider archive", path, source))?
        != 0
    {
        return Err(corrupt(
            path,
            "same digest address contains a different payload",
        ));
    }
    Ok(())
}

enum LinkResult {
    Linked,
    CopyFallback,
}

fn hard_link_or_copy_fallback(
    source: &Path,
    destination: &Path,
) -> Result<LinkResult, NativeError> {
    match fs::hard_link(source, destination) {
        Ok(()) => Ok(LinkResult::Linked),
        Err(error) if hard_link_unavailable(&error) => Ok(LinkResult::CopyFallback),
        Err(source) => Err(io_error(
            "link raw provider archive into work directory",
            destination,
            source,
        )),
    }
}

fn hard_link_unavailable(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::Unsupported {
        return true;
    }
    #[cfg(unix)]
    {
        error.raw_os_error().is_some_and(|code| {
            code == libc::EXDEV || code == libc::EPERM || code == libc::EOPNOTSUPP
        })
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn archive_path(pool: &Path, digest: Digest) -> PathBuf {
    pool.join(format!("{}{}", digest.to_hex(), ARCHIVE_SUFFIX))
}

fn write_archive_temporary(pool: &Path, bytes: &[u8]) -> Result<PathBuf, NativeError> {
    for _ in 0..128 {
        let path = pool.join(format!(
            "{TEMP_PREFIX}{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        ));
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(source) => return Err(io_error("create raw provider temporary", &path, source)),
        };
        let write_result = file
            .write_all(bytes)
            .and_then(|()| file.flush())
            .and_then(|()| file.sync_all());
        if let Err(source) = write_result {
            let _ = fs::remove_file(&path);
            return Err(io_error("write raw provider temporary", &path, source));
        }
        return Ok(path);
    }
    Err(io_error(
        "create unique raw provider temporary",
        pool,
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "temporary name collided 128 times",
        ),
    ))
}

fn make_room(pool: &Path, incoming: u64, budget: u64) -> Result<(), NativeError> {
    if incoming > budget {
        return Ok(());
    }
    let mut entries = Vec::new();
    let mut total = 0u64;
    for entry in
        fs::read_dir(pool).map_err(|source| io_error("scan raw provider pool", pool, source))?
    {
        let entry = entry.map_err(|source| io_error("scan raw provider pool", pool, source))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if is_temporary_name(&name) {
            // The exclusive lock proves no writer currently owns a temporary. Removing every
            // abandoned staging file also bounds leftovers after a producer crash.
            fs::remove_file(entry.path()).map_err(|source| {
                io_error("remove stale raw provider temporary", &entry.path(), source)
            })?;
            continue;
        }
        if !is_archive_name(&name) {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|source| io_error("inspect raw provider pool entry", &entry.path(), source))?;
        if !metadata.file_type().is_file() {
            return Err(corrupt(
                &entry.path(),
                "content-addressed archive is not a regular file",
            ));
        }
        total = total.saturating_add(metadata.len());
        entries.push((
            entry.path(),
            metadata.len(),
            metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        ));
    }
    if total.saturating_add(incoming) <= budget {
        return Ok(());
    }
    entries.sort_by_key(|(_, _, modified)| *modified);
    for (path, size, _) in entries {
        fs::remove_file(&path)
            .map_err(|source| io_error("evict raw provider archive", &path, source))?;
        total = total.saturating_sub(size);
        if total.saturating_add(incoming) <= budget {
            return Ok(());
        }
    }
    if total.saturating_add(incoming) > budget {
        return Err(corrupt(
            pool,
            "raw provider pool could not satisfy its fixed byte budget",
        ));
    }
    Ok(())
}

fn remove_stale_temporaries(pool: &Path) -> Result<(), NativeError> {
    // Shared-lock warm hits never create temporary paths; with the exclusive lock held, every
    // temporary found here is abandoned by a process that exited before atomic installation.
    for entry in
        fs::read_dir(pool).map_err(|source| io_error("scan raw provider pool", pool, source))?
    {
        let entry = entry.map_err(|source| io_error("scan raw provider pool", pool, source))?;
        if is_temporary_name(&entry.file_name().to_string_lossy()) {
            fs::remove_file(entry.path()).map_err(|source| {
                io_error("remove stale raw provider temporary", &entry.path(), source)
            })?;
        }
    }
    Ok(())
}

fn has_stale_temporaries(pool: &Path) -> Result<bool, NativeError> {
    for entry in
        fs::read_dir(pool).map_err(|source| io_error("scan raw provider pool", pool, source))?
    {
        let entry = entry.map_err(|source| io_error("scan raw provider pool", pool, source))?;
        if is_temporary_name(&entry.file_name().to_string_lossy()) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn is_temporary_name(name: &str) -> bool {
    name.strip_prefix(TEMP_PREFIX)
        .and_then(|suffix| suffix.split_once('-'))
        .is_some_and(|(pid, sequence)| {
            !pid.is_empty()
                && pid.bytes().all(|byte| byte.is_ascii_digit())
                && !sequence.is_empty()
                && sequence.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn is_archive_name(name: &str) -> bool {
    name.strip_suffix(ARCHIVE_SUFFIX).is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn touch(path: &Path) {
    if let Ok(file) = OpenOptions::new().write(true).open(path) {
        let _ = file.set_times(FileTimes::new().set_modified(SystemTime::now()));
    }
}

fn open_pool_lock(pool: &Path) -> Result<File, NativeError> {
    let path = pool.join(LOCK_FILE);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(corrupt(
                &path,
                "raw provider pool lock is not a regular file",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => return Err(io_error("inspect raw provider pool lock", &path, source)),
    }
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|source| io_error("open raw provider pool lock", &path, source))
}

fn ensure_pool_directory(pool: &Path) -> Result<(), NativeError> {
    fs::create_dir_all(pool)
        .map_err(|source| io_error("create raw provider pool", pool, source))?;
    let metadata = fs::symlink_metadata(pool)
        .map_err(|source| io_error("inspect raw provider pool", pool, source))?;
    if !metadata.file_type().is_dir() {
        return Err(corrupt(pool, "raw provider pool path is not a directory"));
    }
    Ok(())
}

fn io_error(operation: &'static str, path: &Path, source: io::Error) -> NativeError {
    NativeError::Cache(CacheError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    })
}

fn corrupt(path: &Path, reason: &str) -> NativeError {
    NativeError::Cache(CacheError::Corrupt {
        path: path.to_path_buf(),
        reason: reason.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct Directory(PathBuf);

    impl Directory {
        fn new(name: &str) -> Self {
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "kiln-provider-link-input-{name}-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create test directory");
            Self(path)
        }
    }

    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn identical_archives_are_shared_by_hard_link_and_corruption_is_rejected() {
        let root = Directory::new("reuse");
        let pool = root.0.join("pool");
        let bytes = b"validated provider archive bytes";
        let digest = Digest::hash(bytes);
        let first = root.0.join("first.a");
        let second = root.0.join("second.a");

        materialize_from_pool(&pool, &first, bytes, digest, 256).expect("stage first archive");
        materialize_from_pool(&pool, &second, bytes, digest, 256).expect("reuse archive");
        assert_eq!(fs::read(&first).unwrap(), bytes);
        assert_eq!(fs::read(&second).unwrap(), bytes);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(
                fs::metadata(&first).unwrap().ino(),
                fs::metadata(&second).unwrap().ino()
            );
        }

        fs::write(
            archive_path(&pool, digest),
            b"corrupt payload with same address",
        )
        .unwrap();
        let error = materialize_from_pool(&pool, &root.0.join("third.a"), bytes, digest, 256)
            .expect_err("same-address corruption must fail");
        assert!(error.to_string().contains("corrupt cache artifact"));
        assert!(
            error
                .to_string()
                .contains("same digest address contains a different payload")
        );
    }

    #[test]
    fn pool_evicts_to_fixed_budget_without_invalidating_workspace_links() {
        let root = Directory::new("budget");
        let pool = root.0.join("pool");
        let first = b"111111";
        let second = b"222222";
        let third = b"333333";
        let first_path = root.0.join("first.a");
        materialize_from_pool(&pool, &first_path, first, Digest::hash(first), 12).unwrap();
        materialize_from_pool(
            &pool,
            &root.0.join("second.a"),
            second,
            Digest::hash(second),
            12,
        )
        .unwrap();
        materialize_from_pool(
            &pool,
            &root.0.join("third.a"),
            third,
            Digest::hash(third),
            12,
        )
        .unwrap();

        let stored_bytes: u64 = fs::read_dir(&pool)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| is_archive_name(&entry.file_name().to_string_lossy()))
            .map(|entry| fs::metadata(entry.path()).unwrap().len())
            .sum();
        assert!(stored_bytes <= 12, "pool used {stored_bytes} bytes");
        assert_eq!(
            fs::read(first_path).unwrap(),
            first,
            "eviction leaves the workspace link live"
        );
    }

    #[test]
    fn concurrent_builders_publish_one_valid_archive() {
        let root = Arc::new(Directory::new("concurrent"));
        let bytes: &'static [u8] = b"one provider archive shared by concurrent links";
        let digest = Digest::hash(bytes);
        let workers = (0..8)
            .map(|index| {
                let root = Arc::clone(&root);
                std::thread::spawn(move || {
                    let destination = root.0.join(format!("link-{index}.a"));
                    materialize_from_pool(&root.0.join("pool"), &destination, bytes, digest, 1024)
                        .expect("materialize concurrent link");
                    assert_eq!(fs::read(destination).unwrap(), bytes);
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker.join().expect("link worker");
        }
        let archives = fs::read_dir(root.0.join("pool"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| is_archive_name(&entry.file_name().to_string_lossy()))
            .count();
        assert_eq!(archives, 1);
    }
}
