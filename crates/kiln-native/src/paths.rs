use super::*;
use kiln_native_toolchain::{CompilerKind, LinkOptions};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) struct WorkDirectory {
    pub(crate) path: PathBuf,
}

impl WorkDirectory {
    pub(crate) fn create() -> Result<Self, NativeError> {
        let counter = WORK_COUNTER.fetch_add(1, Ordering::Relaxed);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "kiln-native-{}-{counter}-{timestamp}",
            std::process::id()
        ));
        fs::create_dir(&path).map_err(|source| NativeError::Io {
            operation: "create native work directory",
            path: path.clone(),
            source,
        })?;
        // Canonicalized, because the compiler emits the canonical form and `-ffile-prefix-map`
        // only rewrites a path it can match literally.
        //
        // `std::env::temp_dir()` reads `TMPDIR`, and on macOS that is `/var/folders/...`, which
        // is a symlink into `/private/var/folders/...`. The toolchain builds its prefix map out
        // of this path and the compiler writes the resolved one, so the map missed and every
        // object embedded the directory it happened to be built in -- a directory whose name
        // carries a pid and a nanosecond timestamp, so no two builds ever agreed.
        //
        // That made the artifact cache refuse its own entries: one key is a claim that exactly
        // one payload exists, the payloads differed in those bytes alone, and the second writer
        // reported `same cache key already contains a different payload`. Cold cache and parallel
        // tests were what it took to see, so it read as a flake rather than as the compiler being
        // told the wrong prefix. Twelve bytes, one string, no other difference in two megabytes.
        //
        // Failing soft: a path that cannot be resolved is still a usable work directory, and the
        // determinism it costs is what we already had.
        let path = fs::canonicalize(&path).unwrap_or(path);
        Ok(Self { path })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

/// Set to keep a native build's scratch directory instead of deleting it.
///
/// The generated C is the only artifact that can explain a native performance result, and until
/// now nothing could read it: it is written into a temporary directory this `Drop` removes, and
/// the copy in the artifact cache is stored compressed. Measuring the emitted shape therefore
/// meant reconstructing it by hand from `kiln-backend-c`'s snapshots — which is how ADR 0016's
/// own table was produced, and why that table describes an emission *shape* rather than the
/// pipeline that ships. Keeping the directory is the difference between the two.
///
/// An environment variable rather than a flag because this crate is below the CLI and the
/// protocol, and a diagnostic that has to be plumbed through both is a diagnostic nobody reaches
/// for. The path is announced on stderr because a directory kept silently is a directory left
/// behind: this one is not cleaned up, by design, and the operator has to know where it went.
const KEEP_WORK_ENV: &str = "KILN_NATIVE_KEEP_WORK";

impl Drop for WorkDirectory {
    fn drop(&mut self) {
        if std::env::var_os(KEEP_WORK_ENV).is_some() {
            eprintln!(
                "kiln: kept native build directory {} ({KEEP_WORK_ENV} is set)",
                self.path.display()
            );
            return;
        }
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub(crate) struct BuildPaths {
    pub(crate) module_c: PathBuf,
    pub(crate) module_o: PathBuf,
    pub(crate) prelude_c: PathBuf,
    pub(crate) prelude_o: PathBuf,
    pub(crate) runtime_c: PathBuf,
    pub(crate) runtime_o: PathBuf,
    pub(crate) launcher_c: PathBuf,
    pub(crate) launcher_o: PathBuf,
    pub(crate) number_provider_a: PathBuf,
    pub(crate) executable: PathBuf,
}

pub(crate) struct SharedLibraryPaths {
    pub(crate) module_c: PathBuf,
    pub(crate) module_o: PathBuf,
    pub(crate) prelude_c: PathBuf,
    pub(crate) prelude_o: PathBuf,
    pub(crate) number_provider_a: PathBuf,
    pub(crate) library: PathBuf,
}

pub(crate) struct SharedGraphLibraryPaths {
    pub(crate) module_c: Vec<PathBuf>,
    pub(crate) module_o: Vec<PathBuf>,
    pub(crate) prelude_c: PathBuf,
    pub(crate) prelude_o: PathBuf,
    pub(crate) number_provider_a: PathBuf,
    pub(crate) wrapper_c: PathBuf,
    pub(crate) wrapper_o: PathBuf,
    pub(crate) library: PathBuf,
}

pub(crate) struct GraphBuildPaths {
    pub(crate) module_c: Vec<PathBuf>,
    pub(crate) module_o: Vec<PathBuf>,
    pub(crate) prelude_c: PathBuf,
    pub(crate) prelude_o: PathBuf,
    pub(crate) runtime_c: PathBuf,
    pub(crate) runtime_o: PathBuf,
    pub(crate) launcher_c: PathBuf,
    pub(crate) launcher_o: PathBuf,
    pub(crate) number_provider_a: PathBuf,
    pub(crate) executable: PathBuf,
}

impl GraphBuildPaths {
    pub(crate) fn new(root: &Path, symbols: &[impl AsRef<str>]) -> Self {
        let mut module_c = Vec::with_capacity(symbols.len());
        let mut module_o = Vec::with_capacity(symbols.len());
        for symbol in symbols {
            let symbol = symbol.as_ref();
            module_c.push(root.join(format!("{symbol}.c")));
            module_o.push(root.join(format!("{symbol}.o")));
        }
        Self {
            module_c,
            module_o,
            prelude_c: root.join("prelude.c"),
            prelude_o: root.join("prelude.o"),
            runtime_c: root.join("runtime.c"),
            runtime_o: root.join("runtime.o"),
            launcher_c: root.join("launcher.c"),
            launcher_o: root.join("launcher.o"),
            number_provider_a: root.join("libkiln_native_number_provider.a"),
            executable: root.join(if cfg!(windows) {
                "program.exe"
            } else {
                "program"
            }),
        }
    }
}

impl BuildPaths {
    pub(crate) fn new(root: &Path) -> Self {
        Self {
            module_c: root.join("module.c"),
            module_o: root.join("module.o"),
            prelude_c: root.join("prelude.c"),
            prelude_o: root.join("prelude.o"),
            runtime_c: root.join("runtime.c"),
            runtime_o: root.join("runtime.o"),
            launcher_c: root.join("launcher.c"),
            launcher_o: root.join("launcher.o"),
            number_provider_a: root.join("libkiln_native_number_provider.a"),
            executable: root.join(if cfg!(windows) {
                "program.exe"
            } else {
                "program"
            }),
        }
    }
}

impl SharedLibraryPaths {
    pub(crate) fn new(root: &Path) -> Self {
        Self {
            module_c: root.join("module.c"),
            module_o: root.join("module.o"),
            prelude_c: root.join("prelude.c"),
            prelude_o: root.join("prelude.o"),
            number_provider_a: root.join("libkiln_native_number_provider.a"),
            library: root.join(shared_library_name()),
        }
    }
}

impl SharedGraphLibraryPaths {
    pub(crate) fn new(root: &Path, symbols: &[impl AsRef<str>]) -> Self {
        let mut module_c = Vec::with_capacity(symbols.len());
        let mut module_o = Vec::with_capacity(symbols.len());
        for symbol in symbols {
            let symbol = symbol.as_ref();
            module_c.push(root.join(format!("{symbol}.c")));
            module_o.push(root.join(format!("{symbol}.o")));
        }
        Self {
            module_c,
            module_o,
            prelude_c: root.join("prelude.c"),
            prelude_o: root.join("prelude.o"),
            number_provider_a: root.join("libkiln_native_number_provider.a"),
            wrapper_c: root.join("wrapper.c"),
            wrapper_o: root.join("wrapper.o"),
            library: root.join(shared_library_name()),
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) const fn shared_library_name() -> &'static str {
    "program.dylib"
}

#[cfg(target_os = "linux")]
pub(crate) const fn shared_library_name() -> &'static str {
    "program.so"
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(crate) const fn shared_library_name() -> &'static str {
    "program.so"
}

/// Derive the public C header paired with a shared-library output.
///
/// This is intentionally a pure path transform so CLI and daemon preflight checks can protect
/// source files before invoking the native toolchain.
pub fn shared_library_header_path(output: &Path) -> PathBuf {
    output.with_extension("h")
}

pub(crate) fn shared_library_link_identity(output: &Path) -> String {
    let name = output
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new(shared_library_name()))
        .to_string_lossy();
    if cfg!(target_os = "macos") {
        format!("@rpath/{name}")
    } else {
        name.into_owned()
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn shared_library_content_identity(
    compiler: &str,
    backend: Backend,
    embedding_abi: &str,
    target: &str,
    profile: NativeProfile,
    public_symbols: &[&str],
    inputs: impl IntoIterator<Item = Digest>,
) -> Digest {
    let identity_options = Digest::hash(
        format!(
            "shared-loader-content-v2\0{}\0system-math\0{}\0module-api-v{}\0exports={}",
            profile.as_str(),
            shared_library_link_policy(),
            EMBEDDING_MODULE_API_VERSION,
            public_symbols.join(","),
        )
        .as_bytes(),
    );
    key(
        ArtifactKind::LinkedArtifact,
        compiler,
        backend,
        embedding_abi,
        target,
        profile.cache(),
        identity_options,
        inputs,
    )
    .digest()
}

pub(crate) fn shared_library_loader_path(
    output: &Path,
    identity: Digest,
) -> Result<PathBuf, NativeError> {
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| NativeError::Io {
        operation: "create shared output directory",
        path: parent.to_path_buf(),
        source,
    })?;
    let parent = fs::canonicalize(parent).map_err(|source| NativeError::Io {
        operation: "canonicalize shared output directory",
        path: parent.to_path_buf(),
        source,
    })?;
    const LOADER_DIRECTORY: &str = ".kiln-loaders";
    if output
        .file_name()
        .is_some_and(|name| name == LOADER_DIRECTORY)
        || parent
            .components()
            .any(|component| component.as_os_str() == LOADER_DIRECTORY)
    {
        return Err(NativeError::ArtifactPathCollision {
            first: output.to_path_buf(),
            second: parent,
        });
    }
    let extension = Path::new(shared_library_name())
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("so");
    Ok(parent.join(LOADER_DIRECTORY).join(format!(
        "kiln-{}.{}",
        identity.to_base64_url(),
        extension
    )))
}

pub(crate) const fn shared_library_link_policy() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos-content-loader-install-name-rpath-export-list-v3"
    } else if cfg!(target_os = "linux") {
        "linux-content-loader-soname-version-script-v3"
    } else {
        "unsupported-content-loader-shared-link-v2"
    }
}

pub(crate) fn configure_shared_library_link(
    options: &mut LinkOptions,
    output: &Path,
    public_symbols: &[&str],
    workspace: &Path,
    _compiler: CompilerKind,
) -> Result<(), NativeError> {
    let identity = shared_library_link_identity(output);
    #[cfg(target_os = "macos")]
    {
        let exports = workspace.join("exports.txt");
        let source = public_symbols
            .iter()
            .map(|symbol| format!("_{symbol}\n"))
            .collect::<String>();
        materialize(&exports, source.as_bytes(), false)?;
        options
            .extra_args
            .push(format!("-Wl,-install_name,{identity}").into());
        options
            .extra_args
            .push(format!("-Wl,-exported_symbols_list,{}", exports.to_string_lossy()).into());
    }
    #[cfg(target_os = "linux")]
    {
        let exports = workspace.join("exports.map");
        let source = format!(
            "KILN_1 {{\n  global:\n{}  local:\n    *;\n}};\n",
            public_symbols
                .iter()
                .map(|symbol| format!("    {symbol};\n"))
                .collect::<String>()
        );
        materialize(&exports, source.as_bytes(), false)?;
        options
            .extra_args
            .push(format!("-Wl,-soname,{identity}").into());
        options
            .extra_args
            .push(format!("-Wl,--version-script,{}", exports.to_string_lossy()).into());
        // `zig cc` rejects `--exclude-libs` outright -- "unsupported linker arg" -- rather than
        // passing it down, so a shared build under zig cannot use it. Nothing is lost: the version
        // script above ends in `local: *;`, which already hides every symbol not named in it, and
        // `--exclude-libs` was the second lock on the same door.
        if compiler != CompilerKind::Zig {
            options.extra_args.push("-Wl,--exclude-libs,ALL".into());
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let _ = (options, identity, public_symbols, workspace);
    Ok(())
}
