use std::env;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use kiln_artifact_cache::Digest;
use kiln_native_number::{NUMBER_FORMAT_VERSION, NUMBER_PARSE_VERSION};
use kiln_native_toolchain::ToolIdentity;
use serde::Deserialize;

pub(crate) const PROVIDER_SCHEMA_VERSION: u16 = 1;
pub(crate) const PROVIDER_ABI_VERSION: u32 = 1;
const PROVIDER_NAME: &str = "kiln-native-number-ffi";
/// `debuginfo=0` because the workspace's release profile keeps line tables and this archive is a
/// link input to *every* artifact: with the regexp engine in it, the difference is 11.7 MiB versus
/// 8.4 MiB, multiplied by every cached build. Nobody debugs through a private ABI adapter.
const PROVIDER_RUSTFLAGS: &str =
    "-C panic=abort -C relocation-model=pic -C target-cpu=generic -C debuginfo=0";
const MANIFEST_ENV: &str = "KILN_NATIVE_NUMBER_PROVIDER_MANIFEST";
const MANIFEST_FILE: &str = "kiln-native-number-provider.json";
/// What a packaged archive has to define.
///
/// The regexp five joined the number two when the provider grew a second semantic core (ADR
/// 0025). The list is part of the identity below, so an archive built before them is rejected
/// rather than linked and found wanting.
const REQUIRED_EXPORTS: [&str; 16] = [
    "kiln_number_to_js_string",
    "kiln_string_to_number",
    "kiln_regexp_compile",
    "kiln_regexp_free",
    "kiln_regexp_search",
    "kiln_regexp_group_count",
    "kiln_regexp_group_name",
    "kiln_bigint_parse",
    "kiln_bigint_from_double",
    "kiln_bigint_free",
    "kiln_bigint_to_double",
    "kiln_bigint_to_string",
    "kiln_bigint_binary",
    "kiln_bigint_negate",
    "kiln_bigint_compare",
    "kiln_bigint_is_zero",
];

#[derive(Debug, Clone)]
pub(crate) struct NumberProvider {
    pub(crate) archive: Arc<[u8]>,
    pub(crate) archive_digest: Digest,
    pub(crate) identity: String,
    pub(crate) rust_target: String,
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
}

pub(crate) fn discover(identity: &ToolIdentity) -> Result<NumberProvider, NumberProviderError> {
    if let Some(path) = env::var_os(MANIFEST_ENV) {
        return load_manifest(&PathBuf::from(path), identity);
    }

    for manifest in installed_manifest_candidates() {
        if manifest.is_file() {
            return load_manifest(&manifest, identity);
        }
    }

    build_from_source(identity)
}

fn installed_manifest_candidates() -> Vec<PathBuf> {
    let Ok(executable) = env::current_exe() else {
        return Vec::new();
    };
    installed_manifest_candidates_for(&executable)
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
) -> Result<NumberProvider, NumberProviderError> {
    require_regular_package_file(manifest_path, manifest_path, "manifest")?;
    let bytes = read(manifest_path, "read number-provider manifest")?;
    let manifest: ArchiveManifest =
        serde_json::from_slice(&bytes).map_err(|source| NumberProviderError::InvalidManifest {
            path: manifest_path.to_path_buf(),
            message: source.to_string(),
        })?;
    validate_manifest(manifest_path, &manifest, identity)?;

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
    } else if manifest.profile != "release" {
        Some("the provider archive must use the `release` profile".to_owned())
    } else if manifest.exports
        != REQUIRED_EXPORTS
            .map(str::to_owned)
            .into_iter()
            .collect::<Vec<_>>()
    {
        Some("the provider export list does not exactly match its private ABI".to_owned())
    } else if !is_sha256_identity(&manifest.rustc_identity) {
        Some("`rustc_identity` must be `sha256:` followed by 64 lowercase hex digits".to_owned())
    } else if Digest::from_hex(&manifest.source_sha256).is_err() {
        Some("`source_sha256` is not a SHA-256 digest".to_owned())
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

fn build_from_source(identity: &ToolIdentity) -> Result<NumberProvider, NumberProviderError> {
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
    let build_root = env::temp_dir()
        .join("kiln-native-number-provider")
        .join(source_digest.to_hex());
    fs::create_dir_all(&build_root).map_err(|source| NumberProviderError::Read {
        operation: "create number-provider build directory",
        path: build_root.clone(),
        source,
    })?;

    let arguments = [
        OsString::from("rustc"),
        OsString::from("--locked"),
        OsString::from("-p"),
        OsString::from(PROVIDER_NAME),
        // The semantic cores are off by default so a workspace build does not unify their `std`
        // features into a `no_std` crate; the packaged archive is the one place that wants them,
        // and this is that place.
        OsString::from("--features"),
        OsString::from("regexp,bigint"),
        OsString::from("--release"),
        OsString::from("--target"),
        OsString::from(&rust_target),
        OsString::from("--crate-type"),
        OsString::from("staticlib"),
    ];
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
    let archive_path = build_root
        .join(&rust_target)
        .join("release")
        .join("libkiln_native_number_ffi.a");
    let archive = read(&archive_path, "read source-built number-provider archive")?;
    let archive_digest = Digest::hash(&archive);
    let identity = format!(
        "schema={PROVIDER_SCHEMA_VERSION};provider={PROVIDER_NAME};abi={PROVIDER_ABI_VERSION};format={NUMBER_FORMAT_VERSION};parse={NUMBER_PARSE_VERSION};target={rust_target};rustc={};source={source_digest};archive={archive_digest};linkage=static;pic=true;profile=release;cpu=generic;deployment={};libc={};exports={}",
        rustc_digest_identity,
        env::var("MACOSX_DEPLOYMENT_TARGET").unwrap_or_default(),
        env::var("KILN_LIBC_BASELINE").unwrap_or_default(),
        REQUIRED_EXPORTS.join(","),
    );
    Ok(NumberProvider {
        archive: archive.into(),
        archive_digest,
        identity,
        rust_target,
    })
}

fn source_digest(root: &Path) -> Result<Digest, NumberProviderError> {
    let paths = [
        "Cargo.lock",
        "crates/kiln-number-core/Cargo.toml",
        "crates/kiln-number-core/src/lib.rs",
        "crates/kiln-native-number-ffi/Cargo.toml",
        "crates/kiln-native-number-ffi/src/lib.rs",
    ];
    let mut identity = Vec::new();
    for relative in paths {
        let bytes = read(&root.join(relative), "read number-provider source")?;
        identity.extend_from_slice(relative.as_bytes());
        identity.push(0);
        identity.extend_from_slice(bytes.len().to_string().as_bytes());
        identity.push(0);
        identity.extend_from_slice(&bytes);
    }
    Ok(Digest::hash(&identity))
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
        "schema={};provider={};abi={};format={};parse={};target={};rustc={};source={};archive={};linkage={};pic={};profile={};cpu={};deployment={};libc={};exports={}",
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
        }
    }

    #[test]
    fn source_identity_uses_the_packager_canonical_framing() {
        let root = TestDirectory::new();
        for (relative, bytes) in [
            ("Cargo.lock", b"lock\n".as_slice()),
            ("crates/kiln-number-core/Cargo.toml", b"[package]\n"),
            (
                "crates/kiln-number-core/src/lib.rs",
                b"core\0bytes\n".as_slice(),
            ),
            ("crates/kiln-native-number-ffi/Cargo.toml", b"[lib]\n"),
            (
                "crates/kiln-native-number-ffi/src/lib.rs",
                b"ffi\n".as_slice(),
            ),
        ] {
            let path = root.0.join(relative);
            fs::create_dir_all(path.parent().expect("fixture path has parent"))
                .expect("create source identity fixture directory");
            fs::write(path, bytes).expect("write source identity fixture");
        }

        assert_eq!(
            source_digest(&root.0).expect("hash source identity fixture"),
            Digest::from_hex("c2e718c392784bcd2cb366769fc9ec4312141a5cebaeade4116f4910b385d238")
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
            "profile": "release",
            "cpu_features": "generic",
            "deployment": "",
            "libc_baseline": "",
            "exports": REQUIRED_EXPORTS,
        });
        let manifest_path = root.0.join(MANIFEST_FILE);
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).expect("serialize manifest fixture"),
        )
        .expect("write manifest fixture");

        let provider = load_manifest(&manifest_path, &identity("arm64-apple-darwin25.6.0"))
            .expect("valid manifest loads");
        assert_eq!(&*provider.archive, archive);
        assert_eq!(provider.archive_digest, Digest::hash(archive));

        fs::write(root.0.join("provider.a"), b"corrupt").expect("corrupt archive fixture");
        let error = load_manifest(&manifest_path, &identity("arm64-apple-darwin25.6.0"))
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
            let error = load_manifest(&manifest_path, &identity("arm64-apple-darwin25.6.0"))
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
            "profile": "release",
            "cpu_features": "generic",
            "deployment": "",
            "libc_baseline": "",
            "exports": REQUIRED_EXPORTS,
        });
        let manifest_path = root.0.join(MANIFEST_FILE);
        fs::write(
            &manifest_path,
            serde_json::to_vec(&manifest).expect("serialize traversal fixture"),
        )
        .expect("write traversal fixture");
        assert!(
            load_manifest(&manifest_path, &identity("aarch64-apple-darwin"))
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
            load_manifest(&manifest_path, &identity("aarch64-apple-darwin"))
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
            load_manifest(&manifest_path, &identity("aarch64-apple-darwin"))
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
            load_manifest(&manifest_path, &identity("aarch64-apple-darwin"))
                .expect_err("unknown manifest fields are rejected")
                .to_string()
                .contains("unknown field")
        );
    }
}
