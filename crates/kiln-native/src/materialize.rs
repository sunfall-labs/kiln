use super::*;
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

pub(crate) fn materialize(path: &Path, bytes: &[u8], executable: bool) -> Result<(), NativeError> {
    let temporary = write_materialized_temporary(path, bytes, executable)?;
    let result = install_materialized(&temporary, path);
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(crate) fn write_materialized_temporary(
    path: &Path,
    bytes: &[u8],
    executable: bool,
) -> Result<PathBuf, NativeError> {
    write_materialized_temporary_with(path, bytes, executable, || {
        MATERIALIZE_COUNTER.fetch_add(1, Ordering::Relaxed)
    })
}

pub(crate) fn write_materialized_temporary_with(
    path: &Path,
    bytes: &[u8],
    executable: bool,
    mut next_counter: impl FnMut() -> u64,
) -> Result<PathBuf, NativeError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| NativeError::Io {
        operation: "create output directory",
        path: parent.to_path_buf(),
        source,
    })?;
    for _ in 0..128 {
        let temporary = materialized_temporary_path(path, next_counter());
        let mut file = match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(NativeError::Io {
                    operation: "create temporary artifact",
                    path: temporary,
                    source,
                });
            }
        };
        let result = (|| {
            file.write_all(bytes).map_err(|source| NativeError::Io {
                operation: "write temporary artifact",
                path: temporary.clone(),
                source,
            })?;
            file.flush().map_err(|source| NativeError::Io {
                operation: "flush temporary artifact",
                path: temporary.clone(),
                source,
            })?;
            if executable {
                set_executable(&temporary)?;
            }
            drop(file);
            Ok(temporary.clone())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        return result;
    }
    Err(NativeError::Io {
        operation: "create unique temporary artifact",
        path: path.to_path_buf(),
        source: io::Error::new(
            io::ErrorKind::AlreadyExists,
            "temporary artifact name collided 128 times",
        ),
    })
}

pub(crate) fn materialized_temporary_path(path: &Path, counter: u64) -> PathBuf {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    parent.join(format!(
        ".{}.{}-{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("kiln-artifact"),
        std::process::id(),
        counter,
    ))
}

#[cfg(test)]
pub(crate) fn publish_shared_artifacts(
    output: &Path,
    loader: &Path,
    header_source: &[u8],
    library: &[u8],
) -> Result<(), NativeError> {
    if output == loader {
        return Err(NativeError::ArtifactPathCollision {
            first: output.to_path_buf(),
            second: loader.to_path_buf(),
        });
    }
    let _process_lock = SHARED_PUBLICATION_LOCK.lock();
    let _directory_lock = acquire_shared_publication_lock(output)?;
    materialize_immutable(loader, library)?;
    // Keep the old helper's compatibility-alias ordering for focused materialization tests. The
    // actual shared builders use the transactional package publisher below.
    materialize(&shared_library_header_path(output), header_source, false)?;
    materialize(output, library, false)
}

pub(crate) fn publish_shared_artifacts_with_symbols(
    output: &Path,
    loader: &Path,
    header_source: &[u8],
    library: &[u8],
    legacy_entry_symbol: &str,
    v2_api_symbol: &str,
) -> Result<shared_publication::SharedLibraryPackage, NativeError> {
    if output == loader {
        return Err(NativeError::ArtifactPathCollision {
            first: output.to_path_buf(),
            second: loader.to_path_buf(),
        });
    }
    // `File` locking semantics differ for separately opened handles in one process, so keep an
    // explicit process lock as well as the cross-process directory lock.
    let _process_lock = SHARED_PUBLICATION_LOCK.lock();
    let _directory_lock = acquire_shared_publication_lock(output)?;
    shared_publication::publish_locked(
        output,
        loader,
        header_source,
        library,
        legacy_entry_symbol,
        v2_api_symbol,
    )
    .map_err(NativeError::from)
}

#[cfg(test)]
pub(crate) fn materialize_immutable(path: &Path, bytes: &[u8]) -> Result<(), NativeError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| NativeError::Io {
        operation: "create canonical shared-library directory",
        path: parent.to_path_buf(),
        source,
    })?;
    let parent_metadata = fs::symlink_metadata(parent).map_err(|source| NativeError::Io {
        operation: "inspect canonical shared-library directory",
        path: parent.to_path_buf(),
        source,
    })?;
    if !parent_metadata.file_type().is_dir() {
        return Err(NativeError::ImmutableArtifactCollision {
            path: parent.to_path_buf(),
        });
    }
    if immutable_contents_match(path, bytes)? {
        return Ok(());
    }
    let temporary = write_materialized_temporary(path, bytes, false)?;
    let result = match fs::hard_link(&temporary, path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
            immutable_contents_match(path, bytes).and_then(|matches| {
                matches
                    .then_some(())
                    .ok_or_else(|| NativeError::ImmutableArtifactCollision {
                        path: path.to_path_buf(),
                    })
            })
        }
        Err(source) => Err(NativeError::Io {
            operation: "install canonical shared library",
            path: path.to_path_buf(),
            source,
        }),
    };
    let cleanup = fs::remove_file(&temporary).map_err(|source| NativeError::Io {
        operation: "remove canonical shared-library temporary",
        path: temporary,
        source,
    });
    result.and(cleanup)
}

#[cfg(test)]
pub(crate) fn immutable_contents_match(path: &Path, bytes: &[u8]) -> Result<bool, NativeError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(NativeError::Io {
                operation: "inspect canonical shared library",
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if !metadata.file_type().is_file() {
        return Err(NativeError::ImmutableArtifactCollision {
            path: path.to_path_buf(),
        });
    }
    let existing = fs::read(path).map_err(|source| NativeError::Io {
        operation: "read canonical shared library",
        path: path.to_path_buf(),
        source,
    })?;
    if existing == bytes {
        Ok(true)
    } else {
        Err(NativeError::ImmutableArtifactCollision {
            path: path.to_path_buf(),
        })
    }
}

pub(crate) fn acquire_shared_publication_lock(output: &Path) -> Result<fs::File, NativeError> {
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| NativeError::Io {
        operation: "create shared publication directory",
        path: parent.to_path_buf(),
        source,
    })?;
    // Directory scope is intentional: `foo.so` and `foo.dylib` both derive `foo.h`, so locking
    // only the library pathname would still permit two writers to interleave the shared header.
    let lock = fs::File::open(parent).map_err(|source| NativeError::Io {
        operation: "open shared publication lock",
        path: parent.to_path_buf(),
        source,
    })?;
    lock.lock().map_err(|source| NativeError::Io {
        operation: "lock shared publication",
        path: parent.to_path_buf(),
        source,
    })?;
    Ok(lock)
}

pub(crate) fn install_materialized(
    temporary: &Path,
    destination: &Path,
) -> Result<(), NativeError> {
    match fs::rename(temporary, destination) {
        Ok(()) => Ok(()),
        #[cfg(windows)]
        Err(_first_error) if destination.is_file() => {
            // `std::fs::rename` cannot replace a file on Windows. This is an explicitly requested
            // output path, so remove that one exact file and immediately install the completed
            // temporary artifact. Native runs launch their private copy before this publication.
            fs::remove_file(destination).map_err(|source| NativeError::Io {
                operation: "replace existing artifact",
                path: destination.to_path_buf(),
                source,
            })?;
            fs::rename(temporary, destination).map_err(|source| NativeError::Io {
                operation: "install artifact",
                path: destination.to_path_buf(),
                source,
            })
        }
        Err(source) => Err(NativeError::Io {
            operation: "install artifact",
            path: destination.to_path_buf(),
            source,
        }),
    }
}

#[cfg(unix)]
pub(crate) fn set_executable(path: &Path) -> Result<(), NativeError> {
    use std::os::unix::fs::PermissionsExt as _;
    let mut permissions = fs::metadata(path)
        .map_err(|source| NativeError::Io {
            operation: "read artifact permissions",
            path: path.to_path_buf(),
            source,
        })?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).map_err(|source| NativeError::Io {
        operation: "make artifact executable",
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(unix))]
pub(crate) fn set_executable(_path: &Path) -> Result<(), NativeError> {
    Ok(())
}
