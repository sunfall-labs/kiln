//! Reader-transactional publication for embedded shared libraries.
//!
//! The requested output and header remain compatibility aliases.  Readers that need a coherent
//! pair use the package rooted beside the output and select one immutable generation through
//! `CURRENT`.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use kiln_artifact_cache::Digest;
use serde::{Deserialize, Serialize};

pub const PACKAGE_FORMAT_TAG: &str = "kiln-shared-library-package-v1";
const PACKAGE_DIRECTORY: &str = ".kiln-packages";
const CURRENT_FILE: &str = "CURRENT";
const GENERATIONS_DIRECTORY: &str = "generations";
const MANIFEST_FILE: &str = "manifest.json";
const HEADER_FILE: &str = "header.h";
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The strict, self-describing manifest stored in every immutable generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedLibraryPackageManifest {
    pub format: String,
    pub generation: String,
    pub header_sha256: String,
    pub library_sha256: String,
    pub loader_leaf: String,
    pub legacy_entry_symbol: String,
    pub v2_api_symbol: String,
}

/// A validated snapshot selected by one `CURRENT` pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedLibraryPackage {
    pub package_dir: PathBuf,
    pub current: PathBuf,
    pub generation_dir: PathBuf,
    pub manifest: SharedLibraryPackageManifest,
}

impl SharedLibraryPackage {
    pub fn package_dir(&self) -> &Path {
        &self.package_dir
    }

    pub fn current_path(&self) -> &Path {
        &self.current
    }

    pub fn generation_dir(&self) -> &Path {
        &self.generation_dir
    }

    pub fn manifest(&self) -> &SharedLibraryPackageManifest {
        &self.manifest
    }

    pub fn header_path(&self) -> PathBuf {
        self.generation_dir.join(HEADER_FILE)
    }

    pub fn loader_path(&self) -> PathBuf {
        self.generation_dir.join(&self.manifest.loader_leaf)
    }

    pub fn read(output: &Path) -> Result<Self, SharedPackageError> {
        read_shared_library_package(output)
    }
}

/// Return the package directory corresponding to a requested output alias.
pub fn shared_library_package_path(output: &Path) -> PathBuf {
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    parent
        .join(PACKAGE_DIRECTORY)
        .join(output.file_name().unwrap_or_default())
}

/// Short alias for callers that already have a shared-library output path.
pub fn package_path(output: &Path) -> PathBuf {
    shared_library_package_path(output)
}

fn validate_output_path(output: &Path) -> Result<(), SharedPackageError> {
    let leaf = output
        .file_name()
        .ok_or_else(|| invalid(output, "output must have an ordinary file name"))?;
    let leaf_text = leaf
        .to_str()
        .ok_or_else(|| invalid(output, "output file name must be UTF-8"))?;
    ensure_component_name_str(leaf_text, output, "output leaf")?;
    if output
        .components()
        .any(|component| component.as_os_str() == PACKAGE_DIRECTORY)
    {
        return Err(invalid(
            output,
            "outputs inside the reserved .kiln-packages namespace are forbidden",
        ));
    }
    Ok(())
}

/// Resolve and validate one complete package generation selected by `CURRENT`.
pub fn read_shared_library_package(
    output: &Path,
) -> Result<SharedLibraryPackage, SharedPackageError> {
    validate_output_path(output)?;
    let package_dir = shared_library_package_path(output);
    require_real_dir(
        package_dir
            .parent()
            .ok_or_else(|| invalid(&package_dir, "package has no parent"))?,
        "package namespace",
    )?;
    require_real_dir(&package_dir, "package directory")?;
    let generations = package_dir.join(GENERATIONS_DIRECTORY);
    require_real_dir(&generations, "generations directory")?;
    let current = package_dir.join(CURRENT_FILE);
    require_regular_file(&current, "CURRENT")?;
    let current_bytes =
        fs::read(&current).map_err(|source| io_error("read CURRENT", &current, source))?;
    let generation = parse_current(&current, &current_bytes)?;
    let generation_dir = generations.join(&generation);
    require_component_name(&generation, &generation_dir, "generation")?;
    require_real_dir(&generation_dir, "generation directory")?;
    let manifest_path = generation_dir.join(MANIFEST_FILE);
    require_regular_file(&manifest_path, "manifest")?;
    let manifest_bytes = fs::read(&manifest_path)
        .map_err(|source| io_error("read package manifest", &manifest_path, source))?;
    let manifest = parse_manifest(&manifest_path, &manifest_bytes)?;
    validate_generation(&generation_dir, &generation, &manifest)?;
    Ok(SharedLibraryPackage {
        package_dir,
        current,
        generation_dir,
        manifest,
    })
}

/// Publish a complete generation. The caller must hold the existing process and output-directory
/// writer locks. No old generation is removed, and `CURRENT` is replaced only after all package
/// files and compatibility aliases have been successfully installed.
pub(crate) fn publish_locked(
    output: &Path,
    loader: &Path,
    header: &[u8],
    library: &[u8],
    legacy_entry_symbol: &str,
    v2_api_symbol: &str,
) -> Result<SharedLibraryPackage, SharedPackageError> {
    validate_output_path(output)?;
    if output == loader {
        return Err(invalid(output, "output and loader paths must differ"));
    }
    let package_dir = shared_library_package_path(output);
    let namespace = package_dir
        .parent()
        .ok_or_else(|| invalid(&package_dir, "package has no namespace"))?;
    ensure_real_dir(namespace, true, "package namespace")?;
    ensure_real_dir(&package_dir, true, "package directory")?;
    let generations = package_dir.join(GENERATIONS_DIRECTORY);
    ensure_real_dir(&generations, true, "generations directory")?;

    // The canonical loader is installed first and is immutable by content address.
    install_immutable(loader, library)?;
    let loader_leaf = loader
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| invalid(loader, "canonical loader must have a UTF-8 leaf"))?;
    ensure_component_name(
        loader.file_name().unwrap_or_default(),
        loader,
        "loader leaf",
    )?;
    reject_reserved_leaf(loader_leaf, loader)?;
    validate_symbol(legacy_entry_symbol, "legacy entry symbol")?;
    validate_symbol(v2_api_symbol, "v2 API symbol")?;
    let header_sha256 = Digest::hash(header).to_hex();
    let library_sha256 = Digest::hash(library).to_hex();
    let generation = generation_identity(
        &header_sha256,
        &library_sha256,
        loader_leaf,
        legacy_entry_symbol,
        v2_api_symbol,
    );
    let manifest = SharedLibraryPackageManifest {
        format: PACKAGE_FORMAT_TAG.to_owned(),
        generation: generation.clone(),
        header_sha256,
        library_sha256,
        loader_leaf: loader_leaf.to_owned(),
        legacy_entry_symbol: legacy_entry_symbol.to_owned(),
        v2_api_symbol: v2_api_symbol.to_owned(),
    };
    let manifest_bytes = serialize_manifest(&manifest)?;
    let generation_dir = generations.join(&generation);
    install_generation(&generation_dir, &manifest, &manifest_bytes, header, library)?;
    // These are intentionally compatibility mirrors, not reader inputs. Their update occurs
    // before CURRENT so a reader selecting the new generation can trust all package files.
    atomic_replace(&output.with_extension("h"), header, false)?;
    atomic_replace(output, library, false)?;
    let current = package_dir.join(CURRENT_FILE);
    atomic_replace(&current, format!("{generation}\n").as_bytes(), false)?;
    read_shared_library_package(output)
}

fn install_generation(
    destination: &Path,
    manifest: &SharedLibraryPackageManifest,
    manifest_bytes: &[u8],
    header: &[u8],
    library: &[u8],
) -> Result<(), SharedPackageError> {
    match fs::symlink_metadata(destination) {
        Ok(metadata) => {
            if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
                return Err(invalid(
                    destination,
                    "generation path is not a real directory",
                ));
            }
            validate_generation(destination, &manifest.generation, manifest)?;
            return Ok(());
        }
        Err(source) if source.kind() != io::ErrorKind::NotFound => {
            return Err(io_error(
                "inspect shared package generation",
                destination,
                source,
            ));
        }
        Err(_) => {}
    }
    let parent = destination
        .parent()
        .ok_or_else(|| invalid(destination, "generation has no parent"))?;
    for attempt in 0..128 {
        let temporary = parent.join(format!(
            ".{}.{}-{}.tmp",
            destination
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
            std::process::id(),
            TEMP_COUNTER
                .fetch_add(1, Ordering::Relaxed)
                .saturating_add(attempt),
        ));
        match fs::create_dir(&temporary) {
            Ok(()) => {
                let result = (|| {
                    write_new_file(&temporary.join(MANIFEST_FILE), manifest_bytes)?;
                    write_new_file(&temporary.join(HEADER_FILE), header)?;
                    write_new_file(&temporary.join(&manifest.loader_leaf), library)?;
                    validate_generation(&temporary, &manifest.generation, manifest)?;
                    match fs::rename(&temporary, destination) {
                        Ok(()) => Ok(()),
                        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
                            validate_generation(destination, &manifest.generation, manifest)
                        }
                        Err(source) => Err(io_error(
                            "install shared package generation",
                            destination,
                            source,
                        )),
                    }
                })();
                if result.is_err() {
                    let _ = fs::remove_dir_all(&temporary);
                }
                return result;
            }
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(io_error(
                    "create temporary shared package generation",
                    &temporary,
                    source,
                ));
            }
        }
    }
    Err(invalid(
        destination,
        "temporary generation name collision limit reached",
    ))
}

fn validate_generation(
    generation_dir: &Path,
    generation: &str,
    expected: &SharedLibraryPackageManifest,
) -> Result<(), SharedPackageError> {
    require_real_dir(generation_dir, "generation directory")?;
    let manifest_path = generation_dir.join(MANIFEST_FILE);
    require_regular_file(&manifest_path, "manifest")?;
    let bytes = fs::read(&manifest_path)
        .map_err(|source| io_error("read package manifest", &manifest_path, source))?;
    let manifest = parse_manifest(&manifest_path, &bytes)?;
    if &manifest != expected || manifest.generation != generation {
        return Err(invalid(
            &manifest_path,
            "immutable generation manifest differs",
        ));
    }
    let header_path = generation_dir.join(HEADER_FILE);
    require_regular_file(&header_path, "generated header")?;
    let loader_path = generation_dir.join(&manifest.loader_leaf);
    require_regular_file(&loader_path, "packaged loader")?;
    let mut entries = fs::read_dir(generation_dir)
        .map_err(|source| io_error("read generation directory", generation_dir, source))?;
    while let Some(entry) = entries
        .next()
        .transpose()
        .map_err(|source| io_error("read generation entry", generation_dir, source))?
    {
        let name = entry.file_name();
        let allowed = name == MANIFEST_FILE
            || name == HEADER_FILE
            || name.to_str() == Some(manifest.loader_leaf.as_str());
        if !allowed {
            return Err(invalid(
                &entry.path(),
                "generation contains an unexpected package path",
            ));
        }
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|source| io_error("inspect generation entry", &entry.path(), source))?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(invalid(
                &entry.path(),
                "generation entries must be regular files",
            ));
        }
    }
    let header = fs::read(&header_path)
        .map_err(|source| io_error("read packaged header", &header_path, source))?;
    let library = fs::read(&loader_path)
        .map_err(|source| io_error("read packaged loader", &loader_path, source))?;
    if Digest::hash(&header).to_hex() != manifest.header_sha256 {
        return Err(invalid(
            &header_path,
            "header digest does not match manifest",
        ));
    }
    if Digest::hash(&library).to_hex() != manifest.library_sha256 {
        return Err(invalid(
            &loader_path,
            "library digest does not match manifest",
        ));
    }
    Ok(())
}

fn generation_identity(
    header_sha256: &str,
    library_sha256: &str,
    loader_leaf: &str,
    legacy_entry_symbol: &str,
    v2_api_symbol: &str,
) -> String {
    let mut preimage = Vec::new();
    for value in [
        PACKAGE_FORMAT_TAG,
        header_sha256,
        library_sha256,
        loader_leaf,
        legacy_entry_symbol,
        v2_api_symbol,
    ] {
        preimage.extend_from_slice(value.len().to_string().as_bytes());
        preimage.push(b':');
        preimage.extend_from_slice(value.as_bytes());
    }
    Digest::hash(&preimage).to_hex()
}

fn serialize_manifest(
    manifest: &SharedLibraryPackageManifest,
) -> Result<Vec<u8>, SharedPackageError> {
    let mut bytes = serde_json::to_vec(manifest)
        .map_err(|error| invalid(Path::new(MANIFEST_FILE), &error.to_string()))?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn parse_manifest(
    path: &Path,
    bytes: &[u8],
) -> Result<SharedLibraryPackageManifest, SharedPackageError> {
    let manifest: SharedLibraryPackageManifest = serde_json::from_slice(bytes)
        .map_err(|error| invalid(path, &format!("strict manifest is invalid: {error}")))?;
    if serialize_manifest(&manifest)? != bytes {
        return Err(invalid(path, "manifest is not in canonical strict form"));
    }
    if manifest.format != PACKAGE_FORMAT_TAG
        || !is_digest(&manifest.header_sha256)
        || !is_digest(&manifest.library_sha256)
        || !is_digest(&manifest.generation)
    {
        return Err(invalid(path, "manifest has invalid format or digest"));
    }
    ensure_component_name(
        Path::new(&manifest.loader_leaf).as_os_str(),
        path,
        "loader leaf",
    )?;
    reject_reserved_leaf(&manifest.loader_leaf, path)?;
    validate_symbol(&manifest.legacy_entry_symbol, "legacy entry symbol")?;
    validate_symbol(&manifest.v2_api_symbol, "v2 API symbol")?;
    let expected = generation_identity(
        &manifest.header_sha256,
        &manifest.library_sha256,
        &manifest.loader_leaf,
        &manifest.legacy_entry_symbol,
        &manifest.v2_api_symbol,
    );
    if manifest.generation != expected {
        return Err(invalid(path, "generation identity does not match manifest"));
    }
    Ok(manifest)
}

fn reject_reserved_leaf(value: &str, path: &Path) -> Result<(), SharedPackageError> {
    if value == MANIFEST_FILE || value == HEADER_FILE || value == CURRENT_FILE {
        return Err(invalid(path, "loader leaf uses a reserved package name"));
    }
    Ok(())
}

fn parse_current(path: &Path, bytes: &[u8]) -> Result<String, SharedPackageError> {
    let text = std::str::from_utf8(bytes).map_err(|_| invalid(path, "CURRENT is not UTF-8"))?;
    let generation = text
        .strip_suffix('\n')
        .ok_or_else(|| invalid(path, "CURRENT must end in one newline"))?;
    if !is_digest(generation) || generation.contains('\r') {
        return Err(invalid(
            path,
            "CURRENT is not one lowercase SHA-256 generation",
        ));
    }
    Ok(generation.to_owned())
}

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn validate_symbol(value: &str, label: &str) -> Result<(), SharedPackageError> {
    if value.is_empty()
        || value
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_whitespace())
    {
        return Err(invalid(
            Path::new(label),
            "symbol is empty or contains whitespace",
        ));
    }
    Ok(())
}

fn ensure_component_name(
    name: &std::ffi::OsStr,
    path: &Path,
    label: &str,
) -> Result<(), SharedPackageError> {
    let value = name
        .to_str()
        .ok_or_else(|| invalid(path, &format!("{label} is not UTF-8")))?;
    ensure_component_name_str(value, path, label)
}

fn require_component_name(value: &str, path: &Path, label: &str) -> Result<(), SharedPackageError> {
    ensure_component_name_str(value, path, label)
}

fn ensure_component_name_str(
    value: &str,
    path: &Path,
    label: &str,
) -> Result<(), SharedPackageError> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
    {
        return Err(invalid(
            path,
            &format!("{label} must be one ordinary path component"),
        ));
    }
    Ok(())
}

fn require_real_dir(path: &Path, label: &str) -> Result<(), SharedPackageError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| io_error("inspect package directory", path, source))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid(path, &format!("{label} must be a real directory")));
    }
    Ok(())
}

fn ensure_real_dir(path: &Path, create: bool, label: &str) -> Result<(), SharedPackageError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
                return Err(invalid(path, &format!("{label} must be a real directory")));
            }
        }
        Err(source) if source.kind() == io::ErrorKind::NotFound && create => {
            fs::create_dir(path)
                .map_err(|source| io_error("create package directory", path, source))?;
        }
        Err(source) => return Err(io_error("inspect package directory", path, source)),
    }
    Ok(())
}

fn require_regular_file(path: &Path, label: &str) -> Result<(), SharedPackageError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| io_error("inspect package file", path, source))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(invalid(path, &format!("{label} must be a regular file")));
    }
    Ok(())
}

fn install_immutable(path: &Path, bytes: &[u8]) -> Result<(), SharedPackageError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    if fs::symlink_metadata(parent).is_err() {
        fs::create_dir_all(parent)
            .map_err(|source| io_error("create canonical loader directory", parent, source))?;
    }
    ensure_real_dir(parent, false, "canonical loader directory")?;
    if let Ok(metadata) = fs::symlink_metadata(path) {
        return validate_immutable(path, bytes, &metadata);
    }
    let temporary = write_package_temporary(path, bytes)?;
    let result = match fs::hard_link(&temporary, path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(path)
                .map_err(|source| io_error("inspect canonical loader winner", path, source))?;
            validate_immutable(path, bytes, &metadata)
        }
        Err(source) => Err(io_error("install canonical loader", path, source)),
    };
    let cleanup = fs::remove_file(&temporary)
        .map_err(|source| io_error("remove canonical loader temporary", &temporary, source));
    result.and(cleanup)
}

fn validate_immutable(
    path: &Path,
    bytes: &[u8],
    metadata: &fs::Metadata,
) -> Result<(), SharedPackageError> {
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(invalid(path, "canonical loader must be a regular file"));
    }
    let existing =
        fs::read(path).map_err(|source| io_error("read canonical loader", path, source))?;
    if existing == bytes {
        return Ok(());
    }
    Err(invalid(
        path,
        "canonical loader is immutable and has different contents",
    ))
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), SharedPackageError> {
    atomic_create(path, bytes, false)
}

fn atomic_create(path: &Path, bytes: &[u8], _executable: bool) -> Result<(), SharedPackageError> {
    let temporary = write_package_temporary(path, bytes)?;
    let result = fs::rename(&temporary, path)
        .map_err(|source| io_error("install package file", path, source));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn write_package_temporary(path: &Path, bytes: &[u8]) -> Result<PathBuf, SharedPackageError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    ensure_real_dir(parent, false, "artifact parent directory")?;
    for _ in 0..128 {
        let temporary = parent.join(format!(
            ".{}.{}-{}.tmp",
            path.file_name().unwrap_or_default().to_string_lossy(),
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(source) => return Err(io_error("create package temporary", &temporary, source)),
        };
        let result = (|| {
            file.write_all(bytes)
                .map_err(|source| io_error("write package temporary", &temporary, source))?;
            file.flush()
                .map_err(|source| io_error("flush package temporary", &temporary, source))?;
            file.sync_all()
                .map_err(|source| io_error("sync package temporary", &temporary, source))?;
            drop(file);
            Ok(temporary.clone())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        return result;
    }
    Err(invalid(
        path,
        "temporary package name collision limit reached",
    ))
}

fn atomic_replace(path: &Path, bytes: &[u8], executable: bool) -> Result<(), SharedPackageError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    ensure_real_dir(parent, true, "output parent directory")?;
    if let Ok(metadata) = fs::symlink_metadata(path)
        && (!metadata.file_type().is_file() || metadata.file_type().is_symlink())
    {
        return Err(invalid(path, "replace target must be a regular file"));
    }
    atomic_create(path, bytes, executable)
}

fn invalid(path: &Path, message: &str) -> SharedPackageError {
    SharedPackageError::Invalid {
        path: path.to_path_buf(),
        message: message.to_owned(),
    }
}

fn io_error(operation: &'static str, path: &Path, source: io::Error) -> SharedPackageError {
    SharedPackageError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    }
}

#[derive(Debug)]
pub enum SharedPackageError {
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Invalid {
        path: PathBuf,
        message: String,
    },
}

impl fmt::Display for SharedPackageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "could not {operation} {}: {source}",
                path.display()
            ),
            Self::Invalid { path, message } => write!(
                formatter,
                "invalid shared-library package at {}: {message}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for SharedPackageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Invalid { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("kiln-shared-package-{label}-{nonce}"));
        fs::create_dir_all(&path).expect("create test root");
        path
    }

    fn publish(
        output: &Path,
        loader: &Path,
        header: &[u8],
        library: &[u8],
    ) -> SharedLibraryPackage {
        publish_locked(
            output,
            loader,
            header,
            library,
            "entry_symbol",
            "entry_api_v2",
        )
        .expect("publish package")
    }

    #[test]
    fn scalar_and_graph_shaped_exports_have_reader_coherent_packages() {
        let path = root("scalar-graph");
        let scalar_output = path.join("scalar.so");
        let scalar = publish(
            &scalar_output,
            &path.join(".kiln-loaders/scalar.so"),
            b"scalar header",
            b"scalar library",
        );
        assert_eq!(scalar.manifest().legacy_entry_symbol, "entry_symbol");
        assert_eq!(fs::read(scalar.header_path()).unwrap(), b"scalar header");
        assert_eq!(fs::read(scalar.loader_path()).unwrap(), b"scalar library");

        let graph_output = path.join("graph.so");
        let graph = publish(
            &graph_output,
            &path.join(".kiln-loaders/graph.so"),
            b"graph header",
            b"graph library",
        );
        assert_ne!(scalar.generation_dir(), graph.generation_dir());
        assert_eq!(read_shared_library_package(&graph_output).unwrap(), graph);
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn reader_snapshot_survives_atomic_current_replacement() {
        let path = root("pointer");
        let output = path.join("program.so");
        let first = publish(
            &output,
            &path.join(".kiln-loaders/first.so"),
            b"header one",
            b"library one",
        );
        let second = publish(
            &output,
            &path.join(".kiln-loaders/second.so"),
            b"header two",
            b"library two",
        );
        assert_ne!(first.generation_dir(), second.generation_dir());
        assert_eq!(fs::read(first.header_path()).unwrap(), b"header one");
        assert_eq!(fs::read(first.loader_path()).unwrap(), b"library one");
        assert_eq!(read_shared_library_package(&output).unwrap(), second);
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn malformed_symlink_and_mirror_failure_are_rejected_without_pointer_change() {
        let path = root("failure");
        let output = path.join("program.so");
        let first = publish(
            &output,
            &path.join(".kiln-loaders/first.so"),
            b"header one",
            b"library one",
        );
        let current = first.current_path().to_path_buf();
        fs::write(&current, b"not-a-generation\n").unwrap();
        assert!(read_shared_library_package(&output).is_err());
        fs::write(&current, format!("{}\n", first.manifest().generation)).unwrap();
        let header = first.header_path();
        #[cfg(unix)]
        {
            let target = path.join("outside-header");
            fs::write(&target, b"header one").unwrap();
            fs::remove_file(&header).unwrap();
            std::os::unix::fs::symlink(&target, &header).unwrap();
            assert!(read_shared_library_package(&output).is_err());
            fs::remove_file(&header).unwrap();
            fs::write(&header, b"header one").unwrap();
        }
        let second_output = output.clone();
        fs::remove_file(&second_output).unwrap();
        fs::create_dir(&second_output).unwrap();
        assert!(
            publish_locked(
                &second_output,
                &path.join(".kiln-loaders/second.so"),
                b"header two",
                b"library two",
                "entry_symbol",
                "entry_api_v2",
            )
            .is_err()
        );
        fs::remove_dir(&second_output).unwrap();
        assert_eq!(read_shared_library_package(&output).unwrap(), first);
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn output_leaf_and_reserved_namespace_are_rejected() {
        let path = root("output-validation");
        let loader = path.join(".kiln-loaders/loader.so");
        assert!(read_shared_library_package(Path::new(".")).is_err());
        assert!(
            publish_locked(
                Path::new("."),
                &loader,
                b"header",
                b"library",
                "entry_symbol",
                "entry_api_v2",
            )
            .is_err()
        );

        let reserved = path.join(".kiln-packages/nested/program.so");
        assert!(read_shared_library_package(&reserved).is_err());
        assert!(
            publish_locked(
                &reserved,
                &loader,
                b"header",
                b"library",
                "entry_symbol",
                "entry_api_v2",
            )
            .is_err()
        );
        assert!(!path.join(".kiln-packages").exists());
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn immutable_loader_install_validates_a_concurrent_winner() {
        use std::sync::{Arc, Barrier};

        let path = root("loader-race");
        let destination = path.join("loader.so");
        let barrier = Arc::new(Barrier::new(3));
        let writers = [b"first loader".as_slice(), b"second loader".as_slice()]
            .into_iter()
            .map(|bytes| {
                let destination = destination.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    install_immutable(&destination, bytes)
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let results = writers
            .into_iter()
            .map(|writer| writer.join().expect("loader writer exits"))
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
        let installed = fs::read(&destination).expect("read concurrent loader winner");
        assert!(installed == b"first loader" || installed == b"second loader");
        let _ = fs::remove_dir_all(path);
    }
}
