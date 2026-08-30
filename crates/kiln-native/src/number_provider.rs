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
const MANIFEST_ENV: &str = "KILN_NATIVE_NUMBER_PROVIDER_MANIFEST";
const MANIFEST_FILE: &str = "kiln-native-number-provider.json";
const REQUIRED_EXPORTS: [&str; 2] = ["kiln_number_to_js_string", "kiln_string_to_number"];

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
    } else if manifest.profile != "release" {
        Some("the provider archive must use the `release` profile".to_owned())
    } else if manifest.exports
        != REQUIRED_EXPORTS
            .map(str::to_owned)
            .into_iter()
            .collect::<Vec<_>>()
    {
        Some("the provider export list does not exactly match its private ABI".to_owned())
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

    let mut rustflags = env::var("RUSTFLAGS").unwrap_or_default();
    if !rustflags.is_empty() {
        rustflags.push(' ');
    }
    rustflags.push_str("-C panic=abort -C relocation-model=pic");
    let arguments = [
        OsString::from("rustc"),
        OsString::from("--locked"),
        OsString::from("-p"),
        OsString::from(PROVIDER_NAME),
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
        .env("RUSTFLAGS", rustflags);
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
        Digest::hash(rustc_identity.as_bytes()),
        env::var("MACOSX_DEPLOYMENT_TARGET").unwrap_or_default(),
        "",
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
        identity.extend_from_slice(&(relative.len() as u64).to_le_bytes());
        identity.extend_from_slice(relative.as_bytes());
        identity.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        identity.extend_from_slice(&bytes);
    }
    Ok(Digest::hash(&identity))
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
    let arch = if lower.starts_with("aarch64-") || lower.starts_with("arm64-") {
        "aarch64"
    } else if lower.starts_with("x86_64-") || lower.starts_with("amd64-") {
        "x86_64"
    } else {
        return None;
    };
    let suffix = if lower.contains("darwin") || lower.contains("apple") {
        "apple-darwin"
    } else if lower.contains("linux") {
        if lower.contains("musl") {
            "unknown-linux-musl"
        } else {
            "unknown-linux-gnu"
        }
    } else if lower.contains("windows") {
        if lower.contains("gnu") {
            "pc-windows-gnu"
        } else {
            "pc-windows-msvc"
        }
    } else {
        return None;
    };
    Some(format!("{arch}-{suffix}"))
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
            "rustc_identity": "rustc fixture",
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
    }

    #[test]
    fn manifest_rejects_unknown_fields_and_archive_path_traversal() {
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
            "rustc_identity": "rustc fixture",
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
