use std::env;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use kiln_artifact_cache::Digest;
use kiln_native_number::{NUMBER_FORMAT_VERSION, NUMBER_PARSE_VERSION};
pub(crate) use kiln_native_number_abi::PROVIDER_ABI_VERSION;
use kiln_native_number_abi::PROVIDER_EXPORTS;
use kiln_native_provider_contract as provider_contract;
use kiln_native_toolchain::ToolIdentity;
use serde::Deserialize;

pub(crate) const PROVIDER_SCHEMA_VERSION: u16 = provider_contract::SCHEMA_VERSION;
const PROVIDER_NAME: &str = provider_contract::PROVIDER_NAME;
/// Release plus whole-archive LTO; see the profile's own note in `Cargo.toml`.
const PROVIDER_PROFILE: &str = provider_contract::PROFILE;
/// `debuginfo=0` because the workspace's release profile keeps line tables and this archive is a
/// link input to *every* artifact: with the regexp engine in it, the difference is 11.7 MiB versus
/// 8.4 MiB, multiplied by every cached build. Nobody debugs through a private ABI adapter.
const PROVIDER_RUSTFLAGS: &str = provider_contract::RUSTFLAGS;
const MANIFEST_ENV: &str = "KILN_NATIVE_NUMBER_PROVIDER_MANIFEST";
const MANIFEST_FILE: &str = provider_contract::FULL_MANIFEST;
/// Manifest for the slim provider archive: same schema, provider ABI, and export list as
/// the full manifest, but the archive was built without the Hyper-backed HTTP engines.
const MANIFEST_SLIM_FILE: &str = provider_contract::SLIM_MANIFEST;
// `kiln-native-number-abi` owns the ordered export registry and ABI version. Keep the imported
// contract private to this resolver module so manifest validation and source-build identities
// cannot drift from the provider's compile-time signature assertions.

#[derive(Debug, Clone)]
pub(crate) struct NumberProvider {
    pub(crate) archive: Arc<[u8]>,
    pub(crate) archive_digest: Digest,
    pub(crate) identity: String,
    pub(crate) rust_target: String,
    /// Whether the archive carries the Hyper-backed HTTP engines. The slim archive omits
    /// them (linkable stubs keep the export list identical); only an HTTP-free program may
    /// link it.
    pub(crate) http: bool,
}

#[derive(Debug)]
pub(crate) enum NumberProviderError {
    Read {
        operation: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    InvalidManifest {
        path: PathBuf,
        message: String,
    },
    UnsupportedTarget {
        compiler_target: String,
        rust_target: String,
    },
    ToolUnavailable {
        tool: &'static str,
    },
    CommandFailed {
        stage: &'static str,
        program: PathBuf,
        status: Option<i32>,
        stderr: String,
    },
    SourceUnavailable {
        root: PathBuf,
    },
}

impl fmt::Display for NumberProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "could not {operation} {}: {source}",
                path.display()
            ),
            Self::InvalidManifest { path, message } => {
                write!(
                    formatter,
                    "invalid number-provider manifest {}: {message}",
                    path.display()
                )
            }
            Self::UnsupportedTarget {
                compiler_target,
                rust_target,
            } => write!(
                formatter,
                "Rust number provider target `{rust_target}` is incompatible with C compiler target `{compiler_target}`"
            ),
            Self::ToolUnavailable { tool } => {
                write!(
                    formatter,
                    "number-provider source fallback requires `{tool}`"
                )
            }
            Self::CommandFailed {
                stage,
                program,
                status,
                stderr,
            } => write!(
                formatter,
                "number-provider {stage} command {} failed with status {}: {}",
                program.display(),
                status.map_or_else(|| "signal".to_owned(), |status| status.to_string()),
                stderr.trim()
            ),
            Self::SourceUnavailable { root } => write!(
                formatter,
                "no shipped number provider was found and source fallback is unavailable at {}",
                root.display()
            ),
        }
    }
}

impl std::error::Error for NumberProviderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::InvalidManifest { .. }
            | Self::UnsupportedTarget { .. }
            | Self::ToolUnavailable { .. }
            | Self::CommandFailed { .. }
            | Self::SourceUnavailable { .. } => None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchiveManifest {
    schema: u16,
    provider: String,
    provider_abi: u32,
    format_version: u32,
    parse_version: u32,
    rust_target: String,
    rustc_identity: String,
    source_sha256: String,
    archive_sha256: String,
    archive: String,
    linkage: String,
    pic: bool,
    profile: String,
    cpu_features: String,
    deployment: String,
    libc_baseline: String,
    exports: Vec<String>,
    /// Which provider variant this manifest describes. Missing (all manifests shipped before
    /// the slim variant) means the full archive: the default is full everywhere, never slim.
    #[serde(default = "default_manifest_http")]
    http: bool,
}

fn default_manifest_http() -> bool {
    true
}

pub(crate) fn discover(identity: &ToolIdentity) -> Result<NumberProvider, NumberProviderError> {
    discover_with_http(identity, true)
}

/// Resolve the provider archive for a program that needs the HTTP engines only when
/// `want_http` is set. A full archive satisfies either request; a slim archive only
/// satisfies an HTTP-free one, so slim manifests are consulted (and accepted) only then.
pub(crate) fn discover_with_http(
    identity: &ToolIdentity,
    want_http: bool,
) -> Result<NumberProvider, NumberProviderError> {
    if let Some(path) = env::var_os(MANIFEST_ENV) {
        return load_manifest(&PathBuf::from(path), identity, want_http);
    }

    if !want_http {
        for manifest in installed_slim_manifest_candidates() {
            if manifest.is_file() {
                return load_manifest(&manifest, identity, want_http);
            }
        }
    }
    for manifest in installed_manifest_candidates() {
        if manifest.is_file() {
            return load_manifest(&manifest, identity, want_http);
        }
    }

    build_from_source(identity, want_http)
}

fn installed_manifest_candidates() -> Vec<PathBuf> {
    let Ok(executable) = env::current_exe() else {
        return Vec::new();
    };
    installed_manifest_candidates_for(&executable)
}

fn installed_slim_manifest_candidates() -> Vec<PathBuf> {
    let Ok(executable) = env::current_exe() else {
        return Vec::new();
    };
    installed_manifest_candidates_for(&executable)
        .into_iter()
        .map(|path| path.with_file_name(MANIFEST_SLIM_FILE))
        .collect()
}

fn installed_manifest_candidates_for(executable: &Path) -> Vec<PathBuf> {
    if !executable.is_absolute() {
        return Vec::new();
    }
    let Some(bin) = executable.parent() else {
        return Vec::new();
    };
    vec![
        bin.join(MANIFEST_FILE),
        bin.join("..").join("lib").join("kiln").join(MANIFEST_FILE),
    ]
}

fn load_manifest(
    manifest_path: &Path,
    identity: &ToolIdentity,
    want_http: bool,
) -> Result<NumberProvider, NumberProviderError> {
    require_regular_package_file(manifest_path, manifest_path, "manifest")?;
    let bytes = read(manifest_path, "read number-provider manifest")?;
    let manifest: ArchiveManifest =
        serde_json::from_slice(&bytes).map_err(|source| NumberProviderError::InvalidManifest {
            path: manifest_path.to_path_buf(),
            message: source.to_string(),
        })?;
    validate_manifest(manifest_path, &manifest, identity, want_http)?;

    let archive_name = Path::new(&manifest.archive);
    if archive_name.components().count() != 1
        || !matches!(archive_name.components().next(), Some(Component::Normal(_)))
    {
        return Err(NumberProviderError::InvalidManifest {
            path: manifest_path.to_path_buf(),
            message: "`archive` must be one file name without path components".to_owned(),
        });
    }
    let archive_path = manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(archive_name);
    require_regular_package_file(&archive_path, manifest_path, "archive")?;
    let archive = read(&archive_path, "read number-provider archive")?;
    let actual_digest = Digest::hash(&archive);
    let expected_digest = Digest::from_hex(&manifest.archive_sha256).map_err(|source| {
        NumberProviderError::InvalidManifest {
            path: manifest_path.to_path_buf(),
            message: format!("invalid `archive_sha256`: {source}"),
        }
    })?;
    if actual_digest != expected_digest {
        return Err(NumberProviderError::InvalidManifest {
            path: manifest_path.to_path_buf(),
            message: format!(
                "archive digest mismatch: manifest has {expected_digest}, payload has {actual_digest}"
            ),
        });
    }

    Ok(NumberProvider {
        archive: archive.into(),
        archive_digest: actual_digest,
        identity: manifest_identity(&manifest),
        rust_target: manifest.rust_target,
        http: manifest.http,
    })
}

fn require_regular_package_file(
    path: &Path,
    manifest_path: &Path,
    label: &str,
) -> Result<(), NumberProviderError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| NumberProviderError::Read {
        operation: "inspect number-provider package file",
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_file() {
        Ok(())
    } else {
        Err(NumberProviderError::InvalidManifest {
            path: manifest_path.to_path_buf(),
            message: format!("number-provider {label} must be a regular package file"),
        })
    }
}

fn validate_manifest(
    path: &Path,
    manifest: &ArchiveManifest,
    identity: &ToolIdentity,
    want_http: bool,
) -> Result<(), NumberProviderError> {
    let invalid = if manifest.schema != PROVIDER_SCHEMA_VERSION {
        Some(format!(
            "schema {} does not match {PROVIDER_SCHEMA_VERSION}",
            manifest.schema
        ))
    } else if manifest.provider != PROVIDER_NAME {
        Some(format!(
            "provider `{}` is not `{PROVIDER_NAME}`",
            manifest.provider
        ))
    } else if manifest.provider_abi != PROVIDER_ABI_VERSION {
        Some(format!(
            "provider ABI {} does not match {PROVIDER_ABI_VERSION}",
            manifest.provider_abi
        ))
    } else if manifest.format_version != NUMBER_FORMAT_VERSION
        || manifest.parse_version != NUMBER_PARSE_VERSION
    {
        Some("number semantic versions do not match this compiler".to_owned())
    } else if manifest.linkage != "static" {
        Some("the first provider ABI requires `linkage` to be `static`".to_owned())
    } else if !manifest.pic {
        Some("the provider archive must be position-independent".to_owned())
    } else if manifest.cpu_features != "generic" {
        Some("the first provider package must use the `generic` CPU policy".to_owned())
    } else if manifest.profile != PROVIDER_PROFILE {
        Some(format!(
            "the provider archive must use the `{PROVIDER_PROFILE}` profile"
        ))
    } else if manifest.exports
        != PROVIDER_EXPORTS
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>()
    {
        Some("the provider export list does not exactly match its private ABI".to_owned())
    } else if !is_sha256_identity(&manifest.rustc_identity) {
        Some("`rustc_identity` must be `sha256:` followed by 64 lowercase hex digits".to_owned())
    } else if Digest::from_hex(&manifest.source_sha256).is_err() {
        Some("`source_sha256` is not a SHA-256 digest".to_owned())
    } else if want_http && !manifest.http {
        Some(
            "the provider archive was built without HTTP support (`http` is false) \
             but this build needs the HTTP engines"
                .to_owned(),
        )
    } else {
        None
    };
    if let Some(message) = invalid {
        return Err(NumberProviderError::InvalidManifest {
            path: path.to_path_buf(),
            message,
        });
    }
    require_compatible_target(&identity.target, &manifest.rust_target)
}

fn build_from_source(
    identity: &ToolIdentity,
    want_http: bool,
) -> Result<NumberProvider, NumberProviderError> {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_default();
    if !source_root
        .join("crates/kiln-native-number-ffi/src/lib.rs")
        .is_file()
    {
        return Err(NumberProviderError::SourceUnavailable { root: source_root });
    }

    let cargo = cargo_program().ok_or(NumberProviderError::ToolUnavailable { tool: "cargo" })?;
    let rustc = rustc_program().ok_or(NumberProviderError::ToolUnavailable { tool: "rustc" })?;
    let rustc_output = command_output(&rustc, &[OsString::from("-vV")], "rustc probe", None)?;
    let rustc_digest_identity = sha256_identity(&rustc_output.stdout);
    let rustc_identity = String::from_utf8_lossy(&rustc_output.stdout).into_owned();
    let rust_target = rustc_identity
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(str::to_owned)
        .ok_or_else(|| NumberProviderError::CommandFailed {
            stage: "rustc probe",
            program: rustc.clone(),
            status: rustc_output.status.code(),
            stderr: "`rustc -vV` did not report a host target".to_owned(),
        })?;
    require_compatible_target(&identity.target, &rust_target)?;

    let source_digest = source_digest(&source_root)?;
    // The two variants compile identical sources with different feature sets, so the digest
    // alone cannot name the build directory: a shared directory would publish one variant's
    // archive under the other's finished name.
    let build_root = env::temp_dir()
        .join("kiln-native-number-provider")
        .join(source_digest.to_hex())
        .join(if want_http { "http" } else { "slim" });
    fs::create_dir_all(&build_root).map_err(|source| NumberProviderError::Read {
        operation: "create number-provider build directory",
        path: build_root.clone(),
        source,
    })?;

    // Where cargo writes, and where a *finished* archive is published.
    //
    // These have to be different paths. The readiness check below used to look at cargo's own
    // output, and `is_file` is true from the moment cargo starts writing it -- so a second process
    // arriving mid-build skipped the build and linked a half-written archive. Its program then
    // printed something else, which the native corpus reported as a divergence: sixteen of them on
    // a cold cache, none on a warm one, because warm means somebody already finished.
    //
    // Publishing through a rename fixes it without a lock. A reader sees the complete archive or
    // no archive, never a partial one, because rename is atomic within a filesystem -- the same
    // reasoning `ArtifactCache::write_atomic` gives for installing a record by hard link. Two
    // finishers racing to publish overwrite identical bytes, so the loser costs nothing.
    let built_path = build_root
        .join(&rust_target)
        .join(PROVIDER_PROFILE)
        .join("libkiln_native_number_ffi.a");
    let archive_path = build_root.join("libkiln_native_number_ffi.a");

    if !archive_path.is_file() {
        // The semantic cores remain feature-gated; the default `realm` feature adds the Tokio
        // runtime that now owns native entry as well as interpreted entry. The slim variant
        // additionally disables default features and enables `realm` without `realm-http`, so
        // the Hyper-backed HTTP engines (and only they) drop out of the archive.
        let mut arguments = vec![
            OsString::from("rustc"),
            OsString::from("--locked"),
            OsString::from("-p"),
            OsString::from(PROVIDER_NAME),
        ];
        if !want_http {
            arguments.push(OsString::from("--no-default-features"));
        }
        arguments.extend([
            OsString::from("--features"),
            OsString::from(if want_http {
                provider_contract::FULL_FEATURES
            } else {
                provider_contract::SLIM_FEATURES
            }),
            OsString::from("--profile"),
            OsString::from(PROVIDER_PROFILE),
            OsString::from("--target"),
            OsString::from(&rust_target),
            OsString::from("--crate-type"),
            OsString::from("staticlib"),
        ]);
        let mut command = Command::new(&cargo);
        command
            .args(arguments)
            .current_dir(&source_root)
            .env("CARGO_TARGET_DIR", &build_root)
            // The manifest promises a portable generic CPU. Setting RUSTFLAGS also outranks Cargo
            // config rustflags; removing its higher-precedence encoded form prevents the caller from
            // silently injecting host-only instructions into an artifact advertised as generic.
            // Empty wrapper variables explicitly override wrappers from either the environment or
            // Cargo config, so the rustc identity names the compiler that actually builds the archive.
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .env_remove("CARGO_BUILD_RUSTFLAGS")
            .env("RUSTC_WRAPPER", "")
            .env("RUSTC_WORKSPACE_WRAPPER", "")
            .env("CARGO_BUILD_RUSTC_WRAPPER", "")
            .env("CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER", "")
            .env("RUSTC", &rustc)
            .env("CARGO_BUILD_RUSTC", &rustc)
            .env("RUSTFLAGS", PROVIDER_RUSTFLAGS);
        let output = command
            .output()
            .map_err(|source| NumberProviderError::Read {
                operation: "launch number-provider source build",
                path: cargo.clone(),
                source,
            })?;
        if !output.status.success() {
            return Err(NumberProviderError::CommandFailed {
                stage: "source build",
                program: cargo,
                status: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }

        // Named for this process, so two publishers cannot share a staging file, and created in
        // the destination's own directory so the rename stays within one filesystem.
        let staging = build_root.join(format!(
            ".libkiln_native_number_ffi.a.{}",
            std::process::id()
        ));
        fs::copy(&built_path, &staging).map_err(|source| NumberProviderError::Read {
            operation: "stage source-built number-provider archive",
            path: built_path.clone(),
            source,
        })?;
        if let Err(source) = fs::rename(&staging, &archive_path) {
            let _ = fs::remove_file(&staging);
            return Err(NumberProviderError::Read {
                operation: "publish source-built number-provider archive",
                path: archive_path.clone(),
                source,
            });
        }
    }
    let archive = read(&archive_path, "read source-built number-provider archive")?;
    let archive_digest = Digest::hash(&archive);
    let identity = format!(
        "schema={PROVIDER_SCHEMA_VERSION};provider={PROVIDER_NAME};abi={PROVIDER_ABI_VERSION};format={NUMBER_FORMAT_VERSION};parse={NUMBER_PARSE_VERSION};target={rust_target};rustc={};source={source_digest};archive={archive_digest};linkage=static;pic=true;profile=release;cpu=generic;http={want_http};deployment={};libc={};exports={}",
        rustc_digest_identity,
        env::var("MACOSX_DEPLOYMENT_TARGET").unwrap_or_default(),
        env::var("KILN_LIBC_BASELINE").unwrap_or_default(),
        PROVIDER_EXPORTS.join(","),
    );
    Ok(NumberProvider {
        archive: archive.into(),
        archive_digest,
        identity,
        rust_target,
        http: want_http,
    })
}

fn source_digest(root: &Path) -> Result<Digest, NumberProviderError> {
    provider_contract::source_digest(root).map_err(|source| NumberProviderError::Read {
        operation: "read number-provider source",
        path: root.to_path_buf(),
        source,
    })
}

fn sha256_identity(bytes: &[u8]) -> String {
    format!("sha256:{}", Digest::hash(bytes))
}

fn is_sha256_identity(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
}

fn manifest_identity(manifest: &ArchiveManifest) -> String {
    format!(
        "schema={};provider={};abi={};format={};parse={};target={};rustc={};source={};archive={};linkage={};pic={};profile={};cpu={};http={};deployment={};libc={};exports={}",
        manifest.schema,
        manifest.provider,
        manifest.provider_abi,
        manifest.format_version,
        manifest.parse_version,
        manifest.rust_target,
        manifest.rustc_identity,
        manifest.source_sha256,
        manifest.archive_sha256,
        manifest.linkage,
        manifest.pic,
        manifest.profile,
        manifest.cpu_features,
        manifest.http,
        manifest.deployment,
        manifest.libc_baseline,
        manifest.exports.join(","),
    )
}

fn require_compatible_target(
    compiler_target: &str,
    rust_target: &str,
) -> Result<(), NumberProviderError> {
    let compiler = canonical_rust_target(compiler_target);
    let rust = canonical_rust_target(rust_target);
    if compiler.is_some() && compiler == rust {
        Ok(())
    } else {
        Err(NumberProviderError::UnsupportedTarget {
            compiler_target: compiler_target.to_owned(),
            rust_target: rust_target.to_owned(),
        })
    }
}

fn canonical_rust_target(target: &str) -> Option<String> {
    let lower = target.to_ascii_lowercase();
    let components: Vec<&str> = lower.split('-').collect();
    if components.len() < 2 || components.iter().any(|component| component.is_empty()) {
        return None;
    }

    let arch = match components[0] {
        "aarch64" | "arm64" => "aarch64",
        "x86_64" | "amd64" => "x86_64",
        _ => return None,
    };

    let suffix = match &components[1..] {
        ["macos"] => "apple-darwin",
        [vendor, darwin]
            if matches!(*vendor, "apple" | "unknown") && is_darwin_component(darwin) =>
        {
            "apple-darwin"
        }
        ["linux", "gnu"] | ["unknown" | "pc" | "redhat" | "suse", "linux", "gnu"] => {
            "unknown-linux-gnu"
        }
        ["linux", "musl"] | ["unknown" | "pc" | "redhat" | "suse", "linux", "musl"] => {
            "unknown-linux-musl"
        }
        ["w64", "mingw32"] if arch == "x86_64" => "pc-windows-gnu",
        ["windows", "gnu"] | ["unknown" | "pc" | "w64", "windows", "gnu"] if arch == "x86_64" => {
            "pc-windows-gnu"
        }
        ["windows", "msvc"] | ["unknown" | "pc" | "w64", "windows", "msvc"] => "pc-windows-msvc",
        _ => return None,
    };
    Some(format!("{arch}-{suffix}"))
}

fn is_darwin_component(component: &str) -> bool {
    let Some(version) = component.strip_prefix("darwin") else {
        return false;
    };
    version.is_empty()
        || version
            .split('.')
            .all(|component| !component.is_empty() && component.chars().all(|c| c.is_ascii_digit()))
}

fn cargo_program() -> Option<PathBuf> {
    env::var_os("CARGO")
        .map(PathBuf::from)
        .or_else(|| find_program("cargo"))
}

fn rustc_program() -> Option<PathBuf> {
    env::var_os("RUSTC")
        .map(PathBuf::from)
        .or_else(|| find_program("rustc"))
}

fn find_program(name: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|path| {
        env::split_paths(&path)
            .map(|directory| directory.join(name))
            .find(|candidate| candidate.is_file())
    })
}

fn command_output(
    program: &Path,
    arguments: &[OsString],
    stage: &'static str,
    current_dir: Option<&Path>,
) -> Result<std::process::Output, NumberProviderError> {
    let mut command = Command::new(program);
    command.args(arguments);
    if let Some(current_dir) = current_dir {
        command.current_dir(current_dir);
    }
    let output = command
        .output()
        .map_err(|source| NumberProviderError::Read {
            operation: "launch number-provider command",
            path: program.to_path_buf(),
            source,
        })?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(NumberProviderError::CommandFailed {
            stage,
            program: program.to_path_buf(),
            status: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

fn read(path: &Path, operation: &'static str) -> Result<Vec<u8>, NumberProviderError> {
    fs::read(path).map_err(|source| NumberProviderError::Read {
        operation,
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiln_native_toolchain::CompilerKind;
    use std::sync::atomic::{AtomicU64, Ordering};

    static DIRECTORY_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = env::temp_dir().join(format!(
                "kiln-number-provider-manifest-{}-{}",
                std::process::id(),
                DIRECTORY_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("create manifest test directory");
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn identity(target: &str) -> ToolIdentity {
        ToolIdentity {
            path: PathBuf::from("cc"),
            canonical_path: PathBuf::from("cc"),
            kind: CompilerKind::Clang,
            version: "test".to_owned(),
            target: target.to_owned(),
            linker: kiln_native_toolchain::LinkerKind::Default,
        }
    }

    #[test]
    fn source_identity_uses_the_packager_canonical_framing() {
        let root = TestDirectory::new();
        for (relative, bytes) in [
            ("Cargo.lock", b"lock\n".as_slice()),
            ("crates/kiln-artifact-cache/Cargo.toml", b"[package]\n"),
            ("crates/kiln-artifact-cache/src/eviction.rs", b"eviction\n"),
            ("crates/kiln-artifact-cache/src/lib.rs", b"cache\n"),
            ("crates/kiln-digest/Cargo.toml", b"[package]\n"),
            ("crates/kiln-digest/src/lib.rs", b"digest\n"),
            ("crates/kiln-host-abi/Cargo.toml", b"[package]\n"),
            ("crates/kiln-host-abi/src/lib.rs", b"host\n"),
            ("crates/kiln-number-core/Cargo.toml", b"[package]\n"),
            (
                "crates/kiln-number-core/src/lib.rs",
                b"core\0bytes\n".as_slice(),
            ),
            ("crates/kiln-native-number-ffi/Cargo.toml", b"[lib]\n"),
            (
                "crates/kiln-native-number-ffi/src/ffi.rs",
                b"dynamic ffi\n".as_slice(),
            ),
            (
                "crates/kiln-native-number-ffi/src/lib.rs",
                b"ffi\n".as_slice(),
            ),
            ("crates/kiln-native-number-abi/Cargo.toml", b"[package]\n"),
            (
                "crates/kiln-native-number-abi/src/lib.rs",
                b"abi\n".as_slice(),
            ),
            ("crates/kiln-node-path/Cargo.toml", b"[package]\n"),
            ("crates/kiln-node-path/src/lib.rs", b"node-path\n"),
            ("crates/kiln-temporal/Cargo.toml", b"[package]\n"),
            ("crates/kiln-temporal/src/lib.rs", b"temporal\n"),
            ("crates/kiln-intl/Cargo.toml", b"[package]\n"),
            ("crates/kiln-intl/src/collator.rs", b"collator\n"),
            ("crates/kiln-intl/src/datetime.rs", b"datetime\n"),
            ("crates/kiln-intl/src/displaynames.rs", b"displaynames\n"),
            ("crates/kiln-intl/src/duration.rs", b"duration\n"),
            ("crates/kiln-intl/src/lib.rs", b"intl\n"),
            ("crates/kiln-intl/src/list.rs", b"list\n"),
            ("crates/kiln-intl/src/locale.rs", b"locale\n"),
            ("crates/kiln-intl/src/number.rs", b"number\n"),
            ("crates/kiln-intl/src/parts.rs", b"parts\n"),
            ("crates/kiln-intl/src/plurals.rs", b"plurals\n"),
            ("crates/kiln-intl/src/relative.rs", b"relative\n"),
            ("crates/kiln-intl/src/segmenter.rs", b"segmenter\n"),
            ("crates/kiln-json/Cargo.toml", b"[package]\n"),
            ("crates/kiln-json/src/lib.rs", b"json\n"),
            ("crates/kiln-json/src/scalar.rs", b"scalar\n"),
            ("crates/kiln-runtime/Cargo.toml", b"[package]\n"),
            ("crates/kiln-runtime/src/http_body.rs", b"body\n"),
            ("crates/kiln-runtime/src/http_client.rs", b"client\n"),
            ("crates/kiln-runtime/src/http_engine.rs", b"engine\n"),
            ("crates/kiln-runtime/src/lib.rs", b"runtime\n"),
            ("crates/kiln-runtime/src/process_wire.rs", b"wire\n"),
            ("crates/kiln-runtime/src/node_http.rs", b"http\n"),
            ("crates/kiln-runtime/src/node_http_agent.rs", b"agent\n"),
            ("crates/kiln-runtime/src/node_net.rs", b"net\n"),
            ("crates/kiln-runtime/src/node_stream.rs", b"stream\n"),
            (
                "crates/kiln-runtime/src/scripted_transport.rs",
                b"scripted\n",
            ),
        ] {
            let path = root.0.join(relative);
            fs::create_dir_all(path.parent().expect("fixture path has parent"))
                .expect("create source identity fixture directory");
            fs::write(path, bytes).expect("write source identity fixture");
        }
        let source_list = root.0.join("crates/kiln-native/provider-sources.txt");
        fs::create_dir_all(source_list.parent().expect("source list has parent"))
            .expect("create source list directory");
        fs::write(source_list, include_str!("../provider-sources.txt"))
            .expect("write source identity manifest");

        assert_eq!(
            source_digest(&root.0).expect("hash source identity fixture"),
            Digest::from_hex("eb5e22a45e4217c32809a61899fb5160b534adb5c58bcef3829704a0ccfa6ecd")
                .expect("fixture digest")
        );
    }

    #[test]
    fn rustc_identity_uses_the_packager_sha256_spelling() {
        let rustc_probe = b"rustc 1.97.1 (000000000 2026-01-01)\nhost: aarch64-apple-darwin\n";
        assert_eq!(
            sha256_identity(rustc_probe),
            "sha256:018d6710ad546f6f891b0b19382992394bd029bc8b4991c06a3d18dc583a2c98"
        );
        assert!(is_sha256_identity(&sha256_identity(rustc_probe)));
        assert!(!is_sha256_identity(
            "018d6710ad546f6f891b0b19382992394bd029bc8b4991c06a3d18dc583a2c98"
        ));
        assert!(!is_sha256_identity(
            "sha256:018D6710AD546F6F891B0B19382992394BD029BC8B4991C06A3D18DC583A2C98"
        ));
    }

    #[test]
    fn target_compatibility_normalizes_compiler_spelling_and_version() {
        require_compatible_target("arm64-apple-darwin25.6.0", "aarch64-apple-darwin")
            .expect("compatible Apple targets");
        assert!(
            require_compatible_target("x86_64-linux-gnu", "aarch64-unknown-linux-gnu").is_err()
        );
        assert!(
            require_compatible_target("x86_64-linux-gnu", "x86_64-unknown-linux-musl").is_err()
        );
        assert!(
            require_compatible_target("unknown", "aarch64-apple-darwin").is_err(),
            "an unavailable C target probe must not silently become the build host"
        );
        assert_eq!(
            identity("aarch64-apple-darwin").target,
            "aarch64-apple-darwin"
        );
    }

    #[test]
    fn target_compatibility_accepts_supported_compiler_target_spellings() {
        for (compiler_target, rust_target) in [
            ("aarch64-apple-darwin", "aarch64-apple-darwin"),
            ("aarch64-unknown-darwin25.6.0", "aarch64-apple-darwin"),
            ("aarch64-macos", "aarch64-apple-darwin"),
            ("arm64-apple-darwin25.6.0", "aarch64-apple-darwin"),
            ("x86_64-apple-darwin24.0.0", "x86_64-apple-darwin"),
            ("amd64-unknown-linux-gnu", "x86_64-unknown-linux-gnu"),
            ("x86_64-redhat-linux-gnu", "x86_64-unknown-linux-gnu"),
            ("aarch64-linux-musl", "aarch64-unknown-linux-musl"),
            ("x86_64-w64-mingw32", "x86_64-pc-windows-gnu"),
            ("x86_64-unknown-windows-gnu", "x86_64-pc-windows-gnu"),
            ("x86_64-pc-windows-gnu", "x86_64-pc-windows-gnu"),
            ("aarch64-unknown-windows-msvc", "aarch64-pc-windows-msvc"),
            ("amd64-w64-windows-msvc", "x86_64-pc-windows-msvc"),
        ] {
            require_compatible_target(compiler_target, rust_target)
                .unwrap_or_else(|error| panic!("{compiler_target} should be compatible: {error}"));
        }
    }

    #[test]
    fn target_compatibility_rejects_unknown_abi_near_misses() {
        for compiler_target in [
            "x86_64-unknown-linux-gnux32",
            "x86_64-unknown-linux-uclibc",
            "x86_64-unknown-linux-gnullvm",
            "aarch64-unknown-linux-gnueabihf",
            "x86_64-unknown-linux-musleabihf",
            "x86_64-apple-darwinfoo",
            "x86_64-evil-apple-darwin25.0.0",
            "x86_64-apple-darwin25..0",
            "x86_64-pc-windows-mingw32",
            "x86_64-pc-windows-msvc-debug",
            "aarch64-unknown-windows-gnu",
            "x86_64-foo-bar-linux-gnu",
            "aarch64-none-linux-gnu",
            "aarch64-ohos-linux-gnu",
        ] {
            assert!(
                canonical_rust_target(compiler_target).is_none(),
                "unknown target ABI should be rejected: {compiler_target}"
            );
        }
    }

    #[test]
    fn installed_layout_searches_beside_bin_then_under_lib_kiln() {
        let executable = Path::new("/opt/kiln/bin/kiln");
        assert_eq!(
            installed_manifest_candidates_for(executable),
            vec![
                PathBuf::from("/opt/kiln/bin").join(MANIFEST_FILE),
                PathBuf::from("/opt/kiln/bin/../lib/kiln").join(MANIFEST_FILE),
            ]
        );
        assert!(installed_manifest_candidates_for(Path::new("kiln")).is_empty());
    }

    #[test]
    fn manifest_loader_validates_contract_target_and_archive_digest() {
        let root = TestDirectory::new();
        let archive = b"valid archive fixture";
        fs::write(root.0.join("provider.a"), archive).expect("write provider fixture");
        let manifest = serde_json::json!({
            "schema": PROVIDER_SCHEMA_VERSION,
            "provider": PROVIDER_NAME,
            "provider_abi": PROVIDER_ABI_VERSION,
            "format_version": NUMBER_FORMAT_VERSION,
            "parse_version": NUMBER_PARSE_VERSION,
            "rust_target": "aarch64-apple-darwin",
            "rustc_identity": sha256_identity(b"rustc fixture"),
            "source_sha256": Digest::hash(b"source fixture").to_hex(),
            "archive_sha256": Digest::hash(archive).to_hex(),
            "archive": "provider.a",
            "linkage": "static",
            "pic": true,
            "profile": "provider",
            "cpu_features": "generic",
            "deployment": "",
            "libc_baseline": "",
            "exports": PROVIDER_EXPORTS,
        });
        let manifest_path = root.0.join(MANIFEST_FILE);
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).expect("serialize manifest fixture"),
        )
        .expect("write manifest fixture");

        let provider = load_manifest(&manifest_path, &identity("arm64-apple-darwin25.6.0"), true)
            .expect("valid manifest loads");
        assert_eq!(&*provider.archive, archive);
        assert_eq!(provider.archive_digest, Digest::hash(archive));
        // A manifest predating the slim variant names no variant: it is the full archive.
        assert!(provider.http);
        assert!(provider.identity.contains(";http=true;"));

        fs::write(root.0.join("provider.a"), b"corrupt").expect("corrupt archive fixture");
        let error = load_manifest(&manifest_path, &identity("arm64-apple-darwin25.6.0"), true)
            .expect_err("digest mismatch is rejected");
        assert!(error.to_string().contains("digest mismatch"));

        #[cfg(unix)]
        {
            let outside = TestDirectory::new();
            let outside_archive = outside.0.join("outside.a");
            fs::write(&outside_archive, archive).expect("write outside archive fixture");
            fs::remove_file(root.0.join("provider.a")).expect("remove corrupt archive fixture");
            std::os::unix::fs::symlink(outside_archive, root.0.join("provider.a"))
                .expect("create escaping archive symlink");
            let error = load_manifest(&manifest_path, &identity("arm64-apple-darwin25.6.0"), true)
                .expect_err("archive symlink is rejected even when its bytes match");
            assert!(
                error.to_string().contains("regular package file"),
                "{error}"
            );
        }
    }

    #[test]
    fn manifest_rejects_nonportable_cpu_unknown_fields_and_archive_path_traversal() {
        let root = TestDirectory::new();
        let archive = b"archive";
        fs::write(root.0.join("provider.a"), archive).expect("write provider fixture");
        let mut manifest = serde_json::json!({
            "schema": PROVIDER_SCHEMA_VERSION,
            "provider": PROVIDER_NAME,
            "provider_abi": PROVIDER_ABI_VERSION,
            "format_version": NUMBER_FORMAT_VERSION,
            "parse_version": NUMBER_PARSE_VERSION,
            "rust_target": "aarch64-apple-darwin",
            "rustc_identity": sha256_identity(b"rustc fixture"),
            "source_sha256": Digest::hash(b"source fixture").to_hex(),
            "archive_sha256": Digest::hash(archive).to_hex(),
            "archive": "../provider.a",
            "linkage": "static",
            "pic": true,
            "profile": "provider",
            "cpu_features": "generic",
            "deployment": "",
            "libc_baseline": "",
            "exports": PROVIDER_EXPORTS,
        });
        let manifest_path = root.0.join(MANIFEST_FILE);
        fs::write(
            &manifest_path,
            serde_json::to_vec(&manifest).expect("serialize traversal fixture"),
        )
        .expect("write traversal fixture");
        assert!(
            load_manifest(&manifest_path, &identity("aarch64-apple-darwin"), true)
                .expect_err("archive traversal is rejected")
                .to_string()
                .contains("one file name")
        );

        manifest["archive"] = serde_json::Value::String("provider.a".to_owned());
        manifest["cpu_features"] = serde_json::Value::String("native".to_owned());
        fs::write(
            &manifest_path,
            serde_json::to_vec(&manifest).expect("serialize native CPU fixture"),
        )
        .expect("write native CPU fixture");
        assert!(
            load_manifest(&manifest_path, &identity("aarch64-apple-darwin"), true)
                .expect_err("host-specific provider CPU policy is rejected")
                .to_string()
                .contains("`generic` CPU policy")
        );

        manifest["cpu_features"] = serde_json::Value::String("generic".to_owned());
        manifest["rustc_identity"] = serde_json::Value::String("rustc fixture".to_owned());
        fs::write(
            &manifest_path,
            serde_json::to_vec(&manifest).expect("serialize noncanonical rustc fixture"),
        )
        .expect("write noncanonical rustc fixture");
        assert!(
            load_manifest(&manifest_path, &identity("aarch64-apple-darwin"), true)
                .expect_err("noncanonical rustc identity is rejected")
                .to_string()
                .contains("`rustc_identity`")
        );

        manifest["rustc_identity"] = serde_json::Value::String(sha256_identity(b"rustc fixture"));
        manifest["unexpected"] = serde_json::Value::Bool(true);
        fs::write(
            &manifest_path,
            serde_json::to_vec(&manifest).expect("serialize unknown field fixture"),
        )
        .expect("write unknown field fixture");
        assert!(
            load_manifest(&manifest_path, &identity("aarch64-apple-darwin"), true)
                .expect_err("unknown manifest fields are rejected")
                .to_string()
                .contains("unknown field")
        );
    }

    #[test]
    fn slim_manifest_loads_only_for_http_free_builds() {
        let root = TestDirectory::new();
        let archive = b"slim archive fixture";
        fs::write(root.0.join("provider-slim.a"), archive).expect("write slim fixture");
        let manifest = serde_json::json!({
            "schema": PROVIDER_SCHEMA_VERSION,
            "provider": PROVIDER_NAME,
            "provider_abi": PROVIDER_ABI_VERSION,
            "format_version": NUMBER_FORMAT_VERSION,
            "parse_version": NUMBER_PARSE_VERSION,
            "rust_target": "aarch64-apple-darwin",
            "rustc_identity": sha256_identity(b"rustc fixture"),
            "source_sha256": Digest::hash(b"source fixture").to_hex(),
            "archive_sha256": Digest::hash(archive).to_hex(),
            "archive": "provider-slim.a",
            "linkage": "static",
            "pic": true,
            "profile": "provider",
            "cpu_features": "generic",
            "deployment": "",
            "libc_baseline": "",
            "exports": PROVIDER_EXPORTS,
            "http": false,
        });
        let manifest_path = root.0.join(MANIFEST_SLIM_FILE);
        fs::write(
            &manifest_path,
            serde_json::to_vec(&manifest).expect("serialize slim fixture"),
        )
        .expect("write slim fixture");

        let provider = load_manifest(&manifest_path, &identity("aarch64-apple-darwin"), false)
            .expect("slim manifest loads for an HTTP-free build");
        assert!(!provider.http);
        assert!(provider.identity.contains(";http=false;"));

        let error = load_manifest(&manifest_path, &identity("aarch64-apple-darwin"), true)
            .expect_err("slim manifest is rejected when HTTP is needed");
        assert!(
            error.to_string().contains("without HTTP support"),
            "{error}"
        );
    }
}
