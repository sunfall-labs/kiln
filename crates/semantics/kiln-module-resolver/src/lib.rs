//! Deterministic filesystem resolution for Kiln's first ESM graph slice.
//!
//! The resolver intentionally has a small policy surface. [`ModuleResolver`] accepts only
//! explicit relative and absolute filesystem specifiers, never guesses an extension or an
//! `index` file, and verifies the canonical target remains inside the canonical project root.
//! [`NodePackageResolver`] is a separate package-policy adapter behind [`ResolutionPolicy`].

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Source extensions understood by the initial filesystem resolver.
pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "js", "mjs", "cjs", "jsx", "ts", "mts", "cts", "tsx", "json", "json5", "jsonc", "yaml", "yml",
    "toml", "wasm", "scm",
];

/// Binary or opaque file extensions accepted by `import(..., { with: { type: "file" } })`.
/// These are resolved as path assets and never passed through a UTF-8 source loader.
pub const ASSET_EXTENSIONS: &[&str] = &["wasm", "scm"];

pub fn is_asset_extension(extension: &str) -> bool {
    ASSET_EXTENSIONS.contains(&extension)
}

/// Whether a resolved request was written as an explicit relative or absolute specifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResolutionKind {
    Relative,
    Absolute,
    Package,
    Builtin,
}

impl ResolutionKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Relative => "relative",
            Self::Absolute => "absolute",
            Self::Package => "package",
            Self::Builtin => "builtin",
        }
    }
}

impl fmt::Display for ResolutionKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A successfully resolved source module.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedModule {
    /// The symlink-normalized, absolute source path, or a stable virtual builtin identity.
    pub path: PathBuf,
    pub kind: ResolutionKind,
}

/// Builtins supported by Kiln's Node compatibility adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NodeBuiltin {
    Process,
    Shared,
    Actors,
    ActorsCore,
    ActorsWorker,
    Gui,
    Assert,
    Buffer,
    AsyncHooks,
    Console,
    DiagnosticsChannel,
    Crypto,
    Dns,
    Events,
    Ffi,
    FsPromises,
    Fs,
    Net,
    Http,
    Module,
    Path,
    WorkerThreads,
    Http2,
    Https,
    PerfHooks,
    Os,
    Tty,
    Zlib,
    Stream,
    StringDecoder,
    QueryString,
    Url,
    Util,
    ChildProcess,
    Timers,
    TimersPromises,
    Readline,
    ReadlinePromises,
    StreamPromises,
    Sqlite,
    UtilTypes,
    StreamWeb,
    Constants,
    V8,
    Vm,
}

impl NodeBuiltin {
    pub const ALL: &[Self] = &[
        Self::Process,
        Self::Shared,
        Self::Actors,
        Self::ActorsCore,
        Self::ActorsWorker,
        Self::Gui,
        Self::Assert,
        Self::Buffer,
        Self::AsyncHooks,
        Self::Console,
        Self::DiagnosticsChannel,
        Self::Crypto,
        Self::Dns,
        Self::Events,
        Self::Ffi,
        Self::FsPromises,
        Self::Fs,
        Self::Net,
        Self::Http,
        Self::Module,
        Self::Path,
        Self::WorkerThreads,
        Self::Http2,
        Self::Https,
        Self::PerfHooks,
        Self::Os,
        Self::Tty,
        Self::Zlib,
        Self::Stream,
        Self::StringDecoder,
        Self::QueryString,
        Self::Url,
        Self::Util,
        Self::ChildProcess,
        Self::Timers,
        Self::TimersPromises,
        Self::Readline,
        Self::ReadlinePromises,
        Self::StreamPromises,
        Self::Sqlite,
        Self::UtilTypes,
        Self::StreamWeb,
        Self::Constants,
        Self::V8,
        Self::Vm,
    ];

    pub fn parse(specifier: &str) -> Option<Self> {
        if specifier.starts_with("node:kiln:") {
            return None;
        }
        Some(match specifier.strip_prefix("node:").unwrap_or(specifier) {
            "process" | "kiln:process" => Self::Process,
            "kiln:shared" => Self::Shared,
            "kiln:actors" => Self::Actors,
            "kiln:actors/core" => Self::ActorsCore,
            "kiln:actors/worker" => Self::ActorsWorker,
            "kiln:gui" => Self::Gui,
            "assert" | "assert/strict" => Self::Assert,
            "buffer" => Self::Buffer,
            "async_hooks" => Self::AsyncHooks,
            "console" => Self::Console,
            "diagnostics_channel" => Self::DiagnosticsChannel,
            "crypto" => Self::Crypto,
            "dns" => Self::Dns,
            "events" => Self::Events,
            "ffi" => Self::Ffi,
            "fs/promises" => Self::FsPromises,
            "fs" => Self::Fs,
            "net" => Self::Net,
            "http" => Self::Http,
            "module" => Self::Module,
            "path" => Self::Path,
            "worker_threads" => Self::WorkerThreads,
            "http2" => Self::Http2,
            "https" => Self::Https,
            "perf_hooks" => Self::PerfHooks,
            "os" => Self::Os,
            "tty" => Self::Tty,
            "zlib" => Self::Zlib,
            "stream" => Self::Stream,
            "stream/promises" => Self::StreamPromises,
            "string_decoder" => Self::StringDecoder,
            "querystring" => Self::QueryString,
            "url" => Self::Url,
            "util" => Self::Util,
            "child_process" => Self::ChildProcess,
            "timers" => Self::Timers,
            "timers/promises" => Self::TimersPromises,
            "readline" => Self::Readline,
            "readline/promises" => Self::ReadlinePromises,
            "sqlite" => Self::Sqlite,
            "util/types" => Self::UtilTypes,
            "stream/web" => Self::StreamWeb,
            "constants" => Self::Constants,
            "v8" => Self::V8,
            "vm" => Self::Vm,
            _ => return None,
        })
    }

    pub const fn specifier(self) -> &'static str {
        match self {
            Self::Process => "kiln:process",
            Self::Shared => "kiln:shared",
            Self::Actors => "kiln:actors",
            Self::ActorsCore => "kiln:actors/core",
            Self::ActorsWorker => "kiln:actors/worker",
            Self::Gui => "kiln:gui",
            Self::Assert => "node:assert",
            Self::Buffer => "node:buffer",
            Self::AsyncHooks => "node:async_hooks",
            Self::Console => "node:console",
            Self::DiagnosticsChannel => "node:diagnostics_channel",
            Self::Crypto => "node:crypto",
            Self::Dns => "node:dns",
            Self::Events => "node:events",
            Self::Ffi => "node:ffi",
            Self::FsPromises => "node:fs/promises",
            Self::Fs => "node:fs",
            Self::Net => "node:net",
            Self::Http => "node:http",
            Self::Module => "node:module",
            Self::Path => "node:path",
            Self::WorkerThreads => "node:worker_threads",
            Self::Http2 => "node:http2",
            Self::Https => "node:https",
            Self::PerfHooks => "node:perf_hooks",
            Self::Os => "node:os",
            Self::Tty => "node:tty",
            Self::Zlib => "node:zlib",
            Self::Stream => "node:stream",
            Self::StreamPromises => "node:stream/promises",
            Self::StringDecoder => "node:string_decoder",
            Self::QueryString => "node:querystring",
            Self::Url => "node:url",
            Self::Util => "node:util",
            Self::ChildProcess => "node:child_process",
            Self::Timers => "node:timers",
            Self::TimersPromises => "node:timers/promises",
            Self::Readline => "node:readline",
            Self::ReadlinePromises => "node:readline/promises",
            Self::Sqlite => "node:sqlite",
            Self::UtilTypes => "node:util/types",
            Self::StreamWeb => "node:stream/web",
            Self::Constants => "node:constants",
            Self::V8 => "node:v8",
            Self::Vm => "node:vm",
        }
    }

    pub fn virtual_path(self, project_root: &Path) -> PathBuf {
        project_root
            .join(".kiln")
            .join("builtins")
            .join(
                self.specifier()
                    .trim_start_matches("node:")
                    .trim_start_matches("kiln:"),
            )
            .with_extension("builtin")
    }
}

// Engine-owned builtins may depend on other builtins without publishing Node's module surface
// to user modules in a non-Node project. A real source file cannot claim this virtual path.
fn engine_builtin_dependency(
    specifier: &str,
    importer: &Path,
    project_root: &Path,
) -> Option<NodeBuiltin> {
    let dependency = NodeBuiltin::parse(specifier)?;
    (!importer.exists()
        && NodeBuiltin::ALL
            .iter()
            .any(|builtin| importer == builtin.virtual_path(project_root)))
    .then_some(dependency)
}

impl ResolvedModule {
    pub fn builtin(&self) -> Option<NodeBuiltin> {
        if self.kind != ResolutionKind::Builtin {
            return None;
        }
        let mut components = self.path.components().rev();
        let file = components.next()?.as_os_str().to_str()?;
        let stem = file.strip_suffix(".builtin")?;
        let parent = components.next()?.as_os_str().to_str()?;
        let name = if parent == "actors" && matches!(stem, "core" | "worker") {
            format!("kiln:actors/{stem}")
        } else if parent == "builtins" {
            // Kiln namespaces use an unprefixed virtual path. Preserve their identity here
            // without making the same bare names or `node:` names aliases.
            let candidate = format!("kiln:{stem}");
            if NodeBuiltin::parse(&candidate).is_some() {
                candidate
            } else {
                stem.to_owned()
            }
        } else {
            format!("{parent}/{stem}")
        };
        NodeBuiltin::parse(&name)
    }
}

/// A resolved request plus the package manifest that selected it, when package policy applied.
///
/// Clients use this metadata to pin Oxc's decision into an authoritative source snapshot. Graph
/// consumers use only [`module`](Self::module), so Oxc-specific types never cross the resolver
/// seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRequest {
    pub module: ResolvedModule,
    pub package_json: Option<PathBuf>,
}

/// Node's execution format for a resolved source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeModuleFormat {
    EsModule,
    CommonJs,
}

/// The Node package condition selected by the syntax that made a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NodeRequestKind {
    Import,
    Require,
}

/// Classify from explicit extension and the nearest package manifest selected by resolution.
/// `.mjs`/`.mts` are ESM and `.cjs`/`.cts` are CommonJS, definitively; `.js`/`.jsx`/`.ts`/`.tsx`
/// follow `type`, defaulting to CommonJS as Node does.
pub fn node_module_format(
    path: &Path,
    package_json: Option<&Path>,
) -> Result<NodeModuleFormat, ResolveError> {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("mjs" | "mts") => return Ok(NodeModuleFormat::EsModule),
        Some("cjs" | "cts") => return Ok(NodeModuleFormat::CommonJs),
        _ => {}
    }
    let Some(manifest) = package_json else {
        return Ok(NodeModuleFormat::CommonJs);
    };
    let bytes = fs::read(manifest).map_err(|error| ResolveError::Io {
        role: PathRole::Target,
        path: manifest.to_path_buf(),
        kind: error.kind(),
    })?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| ResolveError::InvalidPackageManifest {
            path: manifest.to_path_buf(),
        })?;
    Ok(
        if value.get("type").and_then(serde_json::Value::as_str) == Some("module") {
            NodeModuleFormat::EsModule
        } else {
            NodeModuleFormat::CommonJs
        },
    )
}

impl ResolvedModule {
    pub fn new(path: PathBuf, kind: ResolutionKind) -> Self {
        Self { path, kind }
    }
}

/// Which input path an error concerns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PathRole {
    ProjectRoot,
    Importer,
    Target,
}

impl PathRole {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ProjectRoot => "project root",
            Self::Importer => "importer",
            Self::Target => "target",
        }
    }
}

/// Why a request was rejected before touching the filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InvalidSpecifierReason {
    Empty,
    QueryOrFragment,
    UrlScheme,
    BarePackage,
    EmbeddedNul,
}

impl InvalidSpecifierReason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty specifier",
            Self::QueryOrFragment => "query or fragment is not supported",
            Self::UrlScheme => "URL schemes are not supported",
            Self::BarePackage => "bare package specifiers are not supported",
            Self::EmbeddedNul => "embedded NUL is not supported",
        }
    }
}

impl fmt::Display for InvalidSpecifierReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Structured, deterministic failures from filesystem module resolution.
///
/// IO failures retain only [`io::ErrorKind`], rather than platform-specific OS error text. This
/// makes diagnostics stable across hosts while preserving enough information for callers to
/// decide whether a request should be retried or reported to a user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    InvalidSpecifier {
        specifier: String,
        reason: InvalidSpecifierReason,
    },
    NonAbsolutePath {
        role: PathRole,
        path: PathBuf,
    },
    NonUtf8Path {
        role: PathRole,
        path: PathBuf,
    },
    Io {
        role: PathRole,
        path: PathBuf,
        kind: io::ErrorKind,
    },
    NotDirectory {
        role: PathRole,
        path: PathBuf,
    },
    NotRegularFile {
        role: PathRole,
        path: PathBuf,
    },
    OutsideProject {
        path: PathBuf,
        project_root: PathBuf,
    },
    UnsupportedExtension {
        path: PathBuf,
    },
    NodeFormatNotPinned {
        path: PathBuf,
    },
    PackageNotFound {
        package: String,
        importer: PathBuf,
    },
    InvalidPackageManifest {
        path: PathBuf,
    },
    PackageSubpathNotExported {
        package: String,
        subpath: String,
    },
    InvalidPackageTarget {
        package: String,
        target: String,
    },
    PackageResolution {
        specifier: String,
        importer: PathBuf,
        message: String,
    },
}

impl fmt::Display for ResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSpecifier { specifier, reason } => {
                write!(
                    formatter,
                    "invalid module specifier `{specifier}`: {reason}"
                )
            }
            Self::NonAbsolutePath { role, path } => {
                write!(
                    formatter,
                    "{} must be absolute: {}",
                    role.as_str(),
                    display_path(path)
                )
            }
            Self::NonUtf8Path { role, path } => write!(
                formatter,
                "{} contains a non-UTF-8 path: {}",
                role.as_str(),
                display_path(path)
            ),
            Self::Io { role, path, kind } => write!(
                formatter,
                "could not inspect {} {} ({kind:?})",
                role.as_str(),
                display_path(path)
            ),
            Self::NotDirectory { role, path } => write!(
                formatter,
                "{} is not a directory: {}",
                role.as_str(),
                display_path(path)
            ),
            Self::NotRegularFile { role, path } => write!(
                formatter,
                "{} is not a regular file: {}",
                role.as_str(),
                display_path(path)
            ),
            Self::OutsideProject { path, project_root } => write!(
                formatter,
                "resolved module escapes project root: {} (root {})",
                display_path(path),
                display_path(project_root)
            ),
            Self::UnsupportedExtension { path } => write!(
                formatter,
                "unsupported module extension: {}",
                display_path(path)
            ),
            Self::NodeFormatNotPinned { path } => write!(
                formatter,
                "Node module format is not pinned for {}",
                display_path(path)
            ),
            Self::PackageNotFound { package, importer } => write!(
                formatter,
                "could not find package `{package}` from {}",
                display_path(importer)
            ),
            Self::InvalidPackageManifest { path } => write!(
                formatter,
                "invalid package manifest: {}",
                display_path(path)
            ),
            Self::PackageSubpathNotExported { package, subpath } => {
                write!(formatter, "package `{package}` does not export `{subpath}`")
            }
            Self::InvalidPackageTarget { package, target } => write!(
                formatter,
                "package `{package}` has invalid export target `{target}`"
            ),
            Self::PackageResolution {
                specifier,
                importer,
                message,
            } => {
                let request_kind = if specifier == "."
                    || specifier == ".."
                    || specifier.starts_with("./")
                    || specifier.starts_with("../")
                    || specifier.starts_with('/')
                {
                    "module"
                } else {
                    "package"
                };
                write!(
                    formatter,
                    "could not resolve {request_kind} request `{specifier}` from {}: {message}",
                    display_path(importer)
                )
            }
        }
    }
}

impl std::error::Error for ResolveError {}

/// A resolver with a canonical project-root policy fixed at construction time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleResolver {
    project_root: PathBuf,
}

impl ModuleResolver {
    /// Construct a resolver after canonicalizing and validating `project_root`.
    pub fn new(project_root: impl AsRef<Path>) -> Result<Self, ResolveError> {
        let project_root = canonical_path(project_root.as_ref(), PathRole::ProjectRoot)?;
        if !project_root.is_absolute() {
            return Err(ResolveError::NonAbsolutePath {
                role: PathRole::ProjectRoot,
                path: project_root,
            });
        }
        if !metadata_is_dir(&project_root, PathRole::ProjectRoot)? {
            return Err(ResolveError::NotDirectory {
                role: PathRole::ProjectRoot,
                path: project_root,
            });
        }
        ensure_utf8(&project_root, PathRole::ProjectRoot)?;
        Ok(Self { project_root })
    }

    /// The canonical project root used for containment checks.
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    /// Resolve one explicit specifier from an importer path.
    pub fn resolve(
        &self,
        specifier: &str,
        importer: impl AsRef<Path>,
    ) -> Result<ResolvedModule, ResolveError> {
        // Engine-owned builtin dependencies remain available in the graph even when a program
        // has not enabled the Node module adapter.
        if let Some(builtin) =
            engine_builtin_dependency(specifier, importer.as_ref(), self.project_root())
        {
            return Ok(ResolvedModule::new(
                builtin.virtual_path(self.project_root()),
                ResolutionKind::Builtin,
            ));
        }
        if matches!(
            specifier,
            "kiln:shared" | "kiln:actors" | "kiln:actors/core" | "kiln:actors/worker"
        ) {
            let builtin = NodeBuiltin::parse(specifier).expect("Kiln builtin");
            return Ok(ResolvedModule::new(
                builtin.virtual_path(self.project_root()),
                ResolutionKind::Builtin,
            ));
        }
        resolve_in_root(specifier, importer.as_ref(), &self.project_root)
    }
}

/// The module-resolution seam consumed by graph construction.
///
/// Implementations select one canonical source path for a specifier and importer. Keeping the
/// policy behind this interface lets explicit filesystem imports and Node package imports remain
/// separate adapters without making the graph know about package metadata.
pub trait ResolutionPolicy {
    fn project_root(&self) -> &Path;

    fn resolve(&self, specifier: &str, importer: &Path) -> Result<ResolvedModule, ResolveError>;

    /// Resolve a request with the Node package condition implied by its syntax.
    ///
    /// Exact filesystem policies ignore this context; Node adapters select `import` or
    /// `require` export conditions behind the same seam.
    fn resolve_for_request(
        &self,
        specifier: &str,
        importer: &Path,
        _request: NodeRequestKind,
    ) -> Result<ResolvedModule, ResolveError> {
        self.resolve(specifier, importer)
    }

    /// Node execution format when this policy has authoritative package metadata.
    fn node_format(
        &self,
        _module: &ResolvedModule,
    ) -> Result<Option<NodeModuleFormat>, ResolveError> {
        Ok(None)
    }
}

impl ResolutionPolicy for ModuleResolver {
    fn project_root(&self) -> &Path {
        self.project_root()
    }

    fn resolve(&self, specifier: &str, importer: &Path) -> Result<ResolvedModule, ResolveError> {
        ModuleResolver::resolve(self, specifier, importer)
    }
}

/// Node-style package resolution layered over the exact filesystem resolver.
///
/// Oxc owns the Node resolution state machine: nearest `node_modules`, conditional and wildcard
/// exports, package imports, self-references and `main` fallback. Kiln retains only its project
/// containment and supported-source checks.
#[derive(Debug)]
pub struct NodePackageResolver {
    explicit: ModuleResolver,
    import_packages: oxc_resolver::Resolver,
    require_packages: oxc_resolver::Resolver,
}

impl NodePackageResolver {
    pub fn new(project_root: impl AsRef<Path>) -> Result<Self, ResolveError> {
        let explicit = ModuleResolver::new(project_root)?;
        let tsconfig_path = explicit.project_root().join("tsconfig.json");
        let jsconfig_path = explicit.project_root().join("jsconfig.json");
        let tsconfig_file = if tsconfig_path.is_file() {
            Some(tsconfig_path)
        } else if jsconfig_path.is_file() {
            Some(jsconfig_path)
        } else {
            None
        };
        let tsconfig = tsconfig_file.map(|config_file| {
            oxc_resolver::TsconfigDiscovery::Manual(oxc_resolver::TsconfigOptions {
                config_file,
                references: oxc_resolver::TsconfigReferences::Auto,
            })
        });
        Ok(Self {
            explicit,
            import_packages: Self::package_resolver("import", tsconfig.clone()),
            require_packages: Self::package_resolver("require", tsconfig),
        })
    }

    fn package_resolver(
        condition: &str,
        tsconfig: Option<oxc_resolver::TsconfigDiscovery>,
    ) -> oxc_resolver::Resolver {
        let options = oxc_resolver::ResolveOptions {
            tsconfig,
            condition_names: vec!["node".to_owned(), condition.to_owned()],
            extensions: SUPPORTED_EXTENSIONS
                .iter()
                .map(|extension| format!(".{extension}"))
                .collect(),
            extension_alias: vec![
                (
                    ".js".to_owned(),
                    vec![
                        ".ts".to_owned(),
                        ".tsx".to_owned(),
                        ".js".to_owned(),
                        ".jsx".to_owned(),
                    ],
                ),
                (
                    ".mjs".to_owned(),
                    vec![".mts".to_owned(), ".mjs".to_owned()],
                ),
                (
                    ".cjs".to_owned(),
                    vec![".cts".to_owned(), ".cjs".to_owned()],
                ),
                (
                    ".jsx".to_owned(),
                    vec![".tsx".to_owned(), ".jsx".to_owned()],
                ),
            ],
            // Node's package and directory fallback when no `exports` or `main` target wins.
            main_files: vec!["index".to_owned()],
            node_path: false,
            builtin_modules: true,
            ..oxc_resolver::ResolveOptions::default()
        };
        oxc_resolver::Resolver::new(options)
    }

    pub fn project_root(&self) -> &Path {
        self.explicit.project_root()
    }

    pub fn resolve(
        &self,
        specifier: &str,
        importer: impl AsRef<Path>,
    ) -> Result<ResolvedModule, ResolveError> {
        self.resolve_with_metadata(specifier, importer)
            .map(|resolved| resolved.module)
    }

    /// Resolve one request under the selected Node package condition.
    pub fn resolve_for_request(
        &self,
        specifier: &str,
        importer: impl AsRef<Path>,
        request: NodeRequestKind,
    ) -> Result<ResolvedModule, ResolveError> {
        self.resolve_with_metadata_for_request(specifier, importer, request)
            .map(|resolved| resolved.module)
    }

    /// Resolve one request and retain the exact package manifest Oxc consulted.
    pub fn resolve_with_metadata(
        &self,
        specifier: &str,
        importer: impl AsRef<Path>,
    ) -> Result<ResolvedRequest, ResolveError> {
        self.resolve_with_metadata_for_request(specifier, importer, NodeRequestKind::Import)
    }

    /// Resolve one request and retain metadata while selecting import or require conditions.
    pub fn resolve_with_metadata_for_request(
        &self,
        specifier: &str,
        importer: impl AsRef<Path>,
        request: NodeRequestKind,
    ) -> Result<ResolvedRequest, ResolveError> {
        let importer = importer.as_ref();
        let packages = match request {
            NodeRequestKind::Import => &self.import_packages,
            NodeRequestKind::Require => &self.require_packages,
        };
        if !matches!(
            classify_specifier(specifier),
            Err(ResolveError::InvalidSpecifier {
                reason: InvalidSpecifierReason::BarePackage,
                ..
            })
        ) {
            if matches!(classify_specifier(specifier), Ok(ResolutionKind::Relative)) {
                let canonical_importer = canonical_importer(importer, self.project_root())?;
                let importer_directory = canonical_importer.parent().unwrap_or(self.project_root());
                let resolution = packages
                    .resolve(importer_directory, specifier)
                    .map_err(|error| map_package_error(specifier, &canonical_importer, error))?;
                let package_json = resolution
                    .package_json()
                    .map(|manifest| manifest.path().to_path_buf());
                let canonical_target = resolution.into_path_buf();
                return validate_node_target(
                    canonical_target,
                    ResolutionKind::Relative,
                    package_json,
                    self.project_root(),
                );
            }
            return self
                .explicit
                .resolve(specifier, importer)
                .map(|module| ResolvedRequest {
                    module,
                    package_json: None,
                });
        }

        let canonical_importer = canonical_importer(importer, self.project_root())?;
        let importer_directory = canonical_importer.parent().unwrap_or(self.project_root());
        let resolution = packages
            .resolve(importer_directory, specifier)
            .map_err(|error| map_package_error(specifier, &canonical_importer, error))?;
        let package_json = resolution
            .package_json()
            .map(|manifest| manifest.path().to_path_buf());
        let canonical_target = resolution.into_path_buf();
        validate_node_target(
            canonical_target,
            ResolutionKind::Package,
            package_json,
            self.project_root(),
        )
    }
}

fn validate_node_target(
    canonical_target: PathBuf,
    kind: ResolutionKind,
    package_json: Option<PathBuf>,
    project_root: &Path,
) -> Result<ResolvedRequest, ResolveError> {
    ensure_utf8(&canonical_target, PathRole::Target)?;
    if !canonical_target.starts_with(project_root) {
        return Err(ResolveError::OutsideProject {
            path: canonical_target,
            project_root: project_root.to_path_buf(),
        });
    }
    if !metadata_is_regular_file(&canonical_target, PathRole::Target)? {
        return Err(ResolveError::NotRegularFile {
            role: PathRole::Target,
            path: canonical_target,
        });
    }
    if !has_supported_extension(&canonical_target) {
        return Err(ResolveError::UnsupportedExtension {
            path: canonical_target,
        });
    }
    Ok(ResolvedRequest {
        module: ResolvedModule::new(canonical_target, kind),
        package_json,
    })
}

impl ResolutionPolicy for NodePackageResolver {
    fn project_root(&self) -> &Path {
        self.project_root()
    }

    fn resolve(&self, specifier: &str, importer: &Path) -> Result<ResolvedModule, ResolveError> {
        NodePackageResolver::resolve(self, specifier, importer)
    }

    fn resolve_for_request(
        &self,
        specifier: &str,
        importer: &Path,
        request: NodeRequestKind,
    ) -> Result<ResolvedModule, ResolveError> {
        NodePackageResolver::resolve_for_request(self, specifier, importer, request)
    }

    fn node_format(
        &self,
        module: &ResolvedModule,
    ) -> Result<Option<NodeModuleFormat>, ResolveError> {
        Ok(Some(node_module_format_for_path(&module.path)?))
    }
}

/// Node package policy plus explicit builtin-module identities.
///
/// Builtins never touch the filesystem or package resolver. Unknown `node:` names remain errors,
/// while unprefixed names are admitted only for the exact compatibility surface above.
#[derive(Debug)]
pub struct NodeRuntimeResolver {
    packages: NodePackageResolver,
}

impl NodeRuntimeResolver {
    pub fn new(project_root: impl AsRef<Path>) -> Result<Self, ResolveError> {
        Ok(Self {
            packages: NodePackageResolver::new(project_root)?,
        })
    }

    pub fn project_root(&self) -> &Path {
        self.packages.project_root()
    }

    pub fn resolve(
        &self,
        specifier: &str,
        importer: impl AsRef<Path>,
    ) -> Result<ResolvedModule, ResolveError> {
        if let Some(builtin) = NodeBuiltin::parse(specifier) {
            return Ok(ResolvedModule::new(
                builtin.virtual_path(self.project_root()),
                ResolutionKind::Builtin,
            ));
        }
        if specifier.starts_with("node:") {
            return Err(ResolveError::InvalidSpecifier {
                specifier: specifier.to_owned(),
                reason: InvalidSpecifierReason::UrlScheme,
            });
        }
        self.packages.resolve(specifier, importer)
    }
}

impl ResolutionPolicy for NodeRuntimeResolver {
    fn project_root(&self) -> &Path {
        self.project_root()
    }

    fn resolve(&self, specifier: &str, importer: &Path) -> Result<ResolvedModule, ResolveError> {
        NodeRuntimeResolver::resolve(self, specifier, importer)
    }

    fn resolve_for_request(
        &self,
        specifier: &str,
        importer: &Path,
        request: NodeRequestKind,
    ) -> Result<ResolvedModule, ResolveError> {
        if let Some(builtin) = NodeBuiltin::parse(specifier) {
            return Ok(ResolvedModule::new(
                builtin.virtual_path(self.project_root()),
                ResolutionKind::Builtin,
            ));
        }
        if specifier.starts_with("node:") {
            return Err(ResolveError::InvalidSpecifier {
                specifier: specifier.to_owned(),
                reason: InvalidSpecifierReason::UrlScheme,
            });
        }
        self.packages
            .resolve_for_request(specifier, importer, request)
    }

    fn node_format(
        &self,
        module: &ResolvedModule,
    ) -> Result<Option<NodeModuleFormat>, ResolveError> {
        if module.kind == ResolutionKind::Builtin {
            Ok(None)
        } else {
            Ok(Some(node_module_format_for_path(&module.path)?))
        }
    }
}

fn node_module_format_for_path(path: &Path) -> Result<NodeModuleFormat, ResolveError> {
    if matches!(
        path.extension().and_then(|value| value.to_str()),
        Some("mjs" | "mts" | "cjs" | "cts")
    ) {
        return node_module_format(path, None);
    }
    let mut directory = path.parent();
    while let Some(current) = directory {
        let manifest = current.join("package.json");
        if manifest.is_file() {
            return node_module_format(path, Some(&manifest));
        }
        directory = current.parent();
    }
    node_module_format(path, None)
}

/// Whether Node leaves this path's module format to syntax detection.
///
/// Definitive extensions (`.mjs`/`.mts` ESM, `.cjs`/`.cts` CommonJS) and manifests with an
/// explicit `"type"` field decide the format up front. Anything else is ambiguous, and Node
/// 22+ runs ESM syntax found in it as ESM (`--experimental-detect-module`, on by default)
/// rather than failing it as CommonJS. A manifest without a `"type"` field is ambiguous too.
pub fn node_format_is_ambiguous(path: &Path) -> bool {
    if matches!(
        path.extension().and_then(|value| value.to_str()),
        Some("mjs" | "mts" | "cjs" | "cts")
    ) {
        return false;
    }
    let mut directory = path.parent();
    while let Some(current) = directory {
        let manifest = current.join("package.json");
        if manifest.is_file() {
            let explicit = fs::read(&manifest)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .and_then(|value| {
                    value
                        .get("type")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                });
            return explicit.is_none();
        }
        directory = current.parent();
    }
    true
}

/// Replays package decisions captured by a client-side [`NodePackageResolver`].
///
/// Every captured request is a keyed lookup. This includes Node's extension/directory-resolved
/// relative requests: graph construction never reruns those filesystem heuristics after sync.
/// Requests absent from the map retain [`ModuleResolver`]'s exact explicit-filesystem policy.
#[derive(Debug, Clone)]
pub struct PinnedPackageResolver {
    explicit: ModuleResolver,
    packages: HashMap<(PathBuf, String, NodeRequestKind), PathBuf>,
    formats: HashMap<PathBuf, NodeModuleFormat>,
    node_builtins: bool,
}

impl PinnedPackageResolver {
    pub fn new<I>(project_root: impl AsRef<Path>, entries: I) -> Result<Self, ResolveError>
    where
        I: IntoIterator<Item = (PathBuf, String, PathBuf)>,
    {
        Self::new_with_requests_and_formats(
            project_root,
            entries.into_iter().map(|(importer, specifier, target)| {
                (importer, specifier, NodeRequestKind::Import, target)
            }),
            std::iter::empty(),
        )
    }

    /// Construct a pinned resolver with the request condition selected for each package edge.
    pub fn new_with_requests<I>(
        project_root: impl AsRef<Path>,
        entries: I,
    ) -> Result<Self, ResolveError>
    where
        I: IntoIterator<Item = (PathBuf, String, NodeRequestKind, PathBuf)>,
    {
        Self::new_with_requests_and_formats(project_root, entries, std::iter::empty())
    }

    /// Construct a pinned resolver with effective Node formats captured for canonical sources.
    ///
    /// The format table is authoritative: replay never searches for or reads a package manifest.
    pub fn new_with_formats<I, F>(
        project_root: impl AsRef<Path>,
        entries: I,
        formats: F,
    ) -> Result<Self, ResolveError>
    where
        I: IntoIterator<Item = (PathBuf, String, PathBuf)>,
        F: IntoIterator<Item = (PathBuf, NodeModuleFormat)>,
    {
        Self::new_with_requests_and_formats(
            project_root,
            entries.into_iter().map(|(importer, specifier, target)| {
                (importer, specifier, NodeRequestKind::Import, target)
            }),
            formats,
        )
    }

    /// Construct a pinned resolver with request conditions and effective Node source formats.
    pub fn new_with_requests_and_formats<I, F>(
        project_root: impl AsRef<Path>,
        entries: I,
        formats: F,
    ) -> Result<Self, ResolveError>
    where
        I: IntoIterator<Item = (PathBuf, String, NodeRequestKind, PathBuf)>,
        F: IntoIterator<Item = (PathBuf, NodeModuleFormat)>,
    {
        Ok(Self {
            explicit: ModuleResolver::new(project_root)?,
            packages: entries
                .into_iter()
                .map(|(importer, specifier, request, target)| {
                    ((importer, specifier, request), target)
                })
                .collect(),
            formats: formats.into_iter().collect(),
            node_builtins: false,
        })
    }

    /// Construct a pinned resolver with the explicit Node compatibility builtin surface enabled.
    /// Package decisions remain pinned exactly as they are for the default resolver.
    pub fn new_with_node_builtins<I>(
        project_root: impl AsRef<Path>,
        entries: I,
    ) -> Result<Self, ResolveError>
    where
        I: IntoIterator<Item = (PathBuf, String, PathBuf)>,
    {
        let mut resolver = Self::new(project_root, entries)?;
        resolver.node_builtins = true;
        Ok(resolver)
    }

    /// Construct a pinned resolver with Node builtins and effective Node source formats enabled.
    pub fn new_with_node_builtins_and_formats<I, F>(
        project_root: impl AsRef<Path>,
        entries: I,
        formats: F,
    ) -> Result<Self, ResolveError>
    where
        I: IntoIterator<Item = (PathBuf, String, PathBuf)>,
        F: IntoIterator<Item = (PathBuf, NodeModuleFormat)>,
    {
        let mut resolver = Self::new_with_formats(project_root, entries, formats)?;
        resolver.node_builtins = true;
        Ok(resolver)
    }

    /// Construct a pinned resolver with Node builtins, request conditions, and formats.
    pub fn new_with_node_builtins_requests_and_formats<I, F>(
        project_root: impl AsRef<Path>,
        entries: I,
        formats: F,
    ) -> Result<Self, ResolveError>
    where
        I: IntoIterator<Item = (PathBuf, String, NodeRequestKind, PathBuf)>,
        F: IntoIterator<Item = (PathBuf, NodeModuleFormat)>,
    {
        let mut resolver = Self::new_with_requests_and_formats(project_root, entries, formats)?;
        resolver.node_builtins = true;
        Ok(resolver)
    }

    pub fn project_root(&self) -> &Path {
        self.explicit.project_root()
    }

    pub fn resolve(
        &self,
        specifier: &str,
        importer: impl AsRef<Path>,
    ) -> Result<ResolvedModule, ResolveError> {
        self.resolve_with_request(specifier, importer, NodeRequestKind::Import)
    }

    pub fn resolve_with_request(
        &self,
        specifier: &str,
        importer: impl AsRef<Path>,
        request: NodeRequestKind,
    ) -> Result<ResolvedModule, ResolveError> {
        let importer = importer.as_ref();
        if let Some(builtin) = engine_builtin_dependency(specifier, importer, self.project_root()) {
            return Ok(ResolvedModule::new(
                builtin.virtual_path(self.project_root()),
                ResolutionKind::Builtin,
            ));
        }
        if (self.node_builtins || specifier.starts_with("kiln:"))
            && let Some(builtin) = NodeBuiltin::parse(specifier)
        {
            return Ok(ResolvedModule::new(
                builtin.virtual_path(self.project_root()),
                ResolutionKind::Builtin,
            ));
        }
        let canonical_importer = canonical_importer(importer, self.project_root())?;
        let Some(target) = self
            .packages
            .get(&(canonical_importer.clone(), specifier.to_owned(), request))
            .cloned()
        else {
            if !matches!(
                classify_specifier(specifier),
                Err(ResolveError::InvalidSpecifier {
                    reason: InvalidSpecifierReason::BarePackage,
                    ..
                })
            ) {
                return self.explicit.resolve(specifier, importer);
            }
            return Err(ResolveError::PackageResolution {
                specifier: specifier.to_owned(),
                importer: canonical_importer,
                message: "request is absent from the synchronized resolution map".to_owned(),
            });
        };
        let canonical_target = canonical_path(&target, PathRole::Target)?;
        ensure_utf8(&canonical_target, PathRole::Target)?;
        if !canonical_target.starts_with(self.project_root()) {
            return Err(ResolveError::OutsideProject {
                path: canonical_target,
                project_root: self.project_root().to_path_buf(),
            });
        }
        if !metadata_is_regular_file(&canonical_target, PathRole::Target)? {
            return Err(ResolveError::NotRegularFile {
                role: PathRole::Target,
                path: canonical_target,
            });
        }
        if !has_supported_extension(&canonical_target) {
            return Err(ResolveError::UnsupportedExtension {
                path: canonical_target,
            });
        }
        let kind = classify_specifier(specifier).unwrap_or(ResolutionKind::Package);
        Ok(ResolvedModule::new(canonical_target, kind))
    }
}

impl ResolutionPolicy for PinnedPackageResolver {
    fn project_root(&self) -> &Path {
        self.project_root()
    }

    fn resolve(&self, specifier: &str, importer: &Path) -> Result<ResolvedModule, ResolveError> {
        PinnedPackageResolver::resolve(self, specifier, importer)
    }

    fn resolve_for_request(
        &self,
        specifier: &str,
        importer: &Path,
        request: NodeRequestKind,
    ) -> Result<ResolvedModule, ResolveError> {
        PinnedPackageResolver::resolve_with_request(self, specifier, importer, request)
    }

    fn node_format(
        &self,
        module: &ResolvedModule,
    ) -> Result<Option<NodeModuleFormat>, ResolveError> {
        if module.kind == ResolutionKind::Builtin {
            Ok(None)
        } else if matches!(
            module
                .path
                .extension()
                .and_then(|extension| extension.to_str()),
            Some("mjs" | "cjs")
        ) {
            Ok(Some(node_module_format(&module.path, None)?))
        } else {
            self.formats
                .get(&module.path)
                .copied()
                .map(Some)
                .ok_or_else(|| ResolveError::NodeFormatNotPinned {
                    path: module.path.clone(),
                })
        }
    }
}

fn map_package_error(
    specifier: &str,
    importer: &Path,
    error: oxc_resolver::ResolveError,
) -> ResolveError {
    let is_explicit = specifier == "."
        || specifier == ".."
        || specifier.starts_with("./")
        || specifier.starts_with("../")
        || specifier.starts_with('/');
    if is_explicit {
        return ResolveError::PackageResolution {
            specifier: specifier.to_owned(),
            importer: importer.to_path_buf(),
            message: match &error {
                oxc_resolver::ResolveError::NotFound(_) => "target not found".to_owned(),
                _ => error.to_string(),
            },
        };
    }
    let package = package_name(specifier);
    match error {
        oxc_resolver::ResolveError::NotFound(_) => ResolveError::PackageNotFound {
            package,
            importer: importer.to_path_buf(),
        },
        oxc_resolver::ResolveError::PackagePathNotExported { subpath, .. } => {
            ResolveError::PackageSubpathNotExported { package, subpath }
        }
        oxc_resolver::ResolveError::InvalidPackageTarget(target, _, _) => {
            ResolveError::InvalidPackageTarget { package, target }
        }
        oxc_resolver::ResolveError::Json(error) => {
            ResolveError::InvalidPackageManifest { path: error.path }
        }
        oxc_resolver::ResolveError::InvalidPackageConfig(path)
        | oxc_resolver::ResolveError::InvalidPackageConfigDefault(path)
        | oxc_resolver::ResolveError::InvalidPackageConfigDirectory(path) => {
            ResolveError::InvalidPackageManifest { path }
        }
        error => ResolveError::PackageResolution {
            specifier: specifier.to_owned(),
            importer: importer.to_path_buf(),
            message: error.to_string(),
        },
    }
}

fn package_name(specifier: &str) -> String {
    let mut parts = specifier.split('/');
    let first = parts.next().unwrap_or_default();
    if first.starts_with('@') {
        parts
            .next()
            .map_or_else(|| first.to_owned(), |name| format!("{first}/{name}"))
    } else {
        first.to_owned()
    }
}

/// Resolve one explicit module specifier against a project root.
///
/// `importer` and `project_root` may be absolute paths that have not yet been canonicalized;
/// both are canonicalized before the candidate is checked. The returned path is always canonical.
pub fn resolve(
    specifier: &str,
    importer: impl AsRef<Path>,
    project_root: impl AsRef<Path>,
) -> Result<ResolvedModule, ResolveError> {
    let resolver = ModuleResolver::new(project_root)?;
    resolver.resolve(specifier, importer)
}

fn resolve_in_root(
    specifier: &str,
    importer: &Path,
    project_root: &Path,
) -> Result<ResolvedModule, ResolveError> {
    let kind = classify_specifier(specifier)?;

    ensure_utf8(project_root, PathRole::ProjectRoot)?;
    let canonical_importer = canonical_importer(importer, project_root)?;

    let candidate = match kind {
        ResolutionKind::Relative => canonical_importer
            .parent()
            .unwrap_or(project_root)
            .join(specifier),
        ResolutionKind::Absolute => PathBuf::from(specifier),
        ResolutionKind::Package => unreachable!("explicit resolver cannot classify packages"),
        ResolutionKind::Builtin => unreachable!("builtins bypass filesystem resolution"),
    };
    let canonical_target = canonical_path(&candidate, PathRole::Target)?;
    ensure_utf8(&canonical_target, PathRole::Target)?;

    if !canonical_target.starts_with(project_root) {
        return Err(ResolveError::OutsideProject {
            path: canonical_target,
            project_root: project_root.to_path_buf(),
        });
    }
    if !metadata_is_regular_file(&canonical_target, PathRole::Target)? {
        return Err(ResolveError::NotRegularFile {
            role: PathRole::Target,
            path: canonical_target,
        });
    }
    if !has_supported_extension(&canonical_target) {
        return Err(ResolveError::UnsupportedExtension {
            path: canonical_target,
        });
    }

    Ok(ResolvedModule::new(canonical_target, kind))
}

fn classify_specifier(specifier: &str) -> Result<ResolutionKind, ResolveError> {
    let reason = if specifier.is_empty() {
        Some(InvalidSpecifierReason::Empty)
    } else if specifier.starts_with('#') {
        if specifier.len() == 1 {
            Some(InvalidSpecifierReason::Empty)
        } else if specifier[1..].contains(['?', '#']) {
            Some(InvalidSpecifierReason::QueryOrFragment)
        } else {
            None
        }
    } else if specifier.contains(['?', '#']) {
        Some(InvalidSpecifierReason::QueryOrFragment)
    } else if specifier.as_bytes().contains(&0) {
        Some(InvalidSpecifierReason::EmbeddedNul)
    } else {
        None
    };
    if let Some(reason) = reason {
        return Err(ResolveError::InvalidSpecifier {
            specifier: specifier.to_owned(),
            reason,
        });
    }

    let path = Path::new(specifier);
    if path.is_absolute() {
        return Ok(ResolutionKind::Absolute);
    }
    if specifier == "."
        || specifier == ".."
        || specifier.starts_with("./")
        || specifier.starts_with("../")
    {
        return Ok(ResolutionKind::Relative);
    }
    if has_url_scheme(specifier) {
        return Err(ResolveError::InvalidSpecifier {
            specifier: specifier.to_owned(),
            reason: InvalidSpecifierReason::UrlScheme,
        });
    }
    Err(ResolveError::InvalidSpecifier {
        specifier: specifier.to_owned(),
        reason: InvalidSpecifierReason::BarePackage,
    })
}

fn has_url_scheme(specifier: &str) -> bool {
    let Some(colon) = specifier.find(':') else {
        return false;
    };
    let scheme = &specifier[..colon];
    !scheme.is_empty()
        && scheme.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphabetic()
                || (index > 0 && (byte.is_ascii_digit() || matches!(byte, b'+' | b'-' | b'.')))
        })
}

fn canonical_importer(importer: &Path, project_root: &Path) -> Result<PathBuf, ResolveError> {
    if !importer.is_absolute() {
        return Err(ResolveError::NonAbsolutePath {
            role: PathRole::Importer,
            path: importer.to_path_buf(),
        });
    }
    ensure_utf8(importer, PathRole::Importer)?;
    let canonical_importer = canonical_path(importer, PathRole::Importer)?;
    ensure_utf8(&canonical_importer, PathRole::Importer)?;
    if !canonical_importer.starts_with(project_root) {
        return Err(ResolveError::OutsideProject {
            path: canonical_importer,
            project_root: project_root.to_path_buf(),
        });
    }
    if !metadata_is_regular_file(&canonical_importer, PathRole::Importer)? {
        return Err(ResolveError::NotRegularFile {
            role: PathRole::Importer,
            path: canonical_importer,
        });
    }
    Ok(canonical_importer)
}

fn canonical_path(path: &Path, role: PathRole) -> Result<PathBuf, ResolveError> {
    if !path.is_absolute() {
        return Err(ResolveError::NonAbsolutePath {
            role,
            path: path.to_path_buf(),
        });
    }
    ensure_utf8(path, role)?;
    fs::canonicalize(path).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => ResolveError::Io {
            role,
            path: path.to_path_buf(),
            kind: io::ErrorKind::NotFound,
        },
        kind => ResolveError::Io {
            role,
            path: path.to_path_buf(),
            kind,
        },
    })
}

fn ensure_utf8(path: &Path, role: PathRole) -> Result<(), ResolveError> {
    if path.to_str().is_some() {
        Ok(())
    } else {
        Err(ResolveError::NonUtf8Path {
            role,
            path: path.to_path_buf(),
        })
    }
}

fn metadata_is_dir(path: &Path, role: PathRole) -> Result<bool, ResolveError> {
    fs::metadata(path)
        .map(|metadata| metadata.is_dir())
        .map_err(|error| ResolveError::Io {
            role,
            path: path.to_path_buf(),
            kind: error.kind(),
        })
}

fn metadata_is_regular_file(path: &Path, role: PathRole) -> Result<bool, ResolveError> {
    fs::metadata(path)
        .map(|metadata| metadata.is_file())
        .map_err(|error| ResolveError::Io {
            role,
            path: path.to_path_buf(),
            kind: error.kind(),
        })
}

fn has_supported_extension(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| SUPPORTED_EXTENSIONS.contains(&extension))
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempProject {
        root: PathBuf,
        outside: PathBuf,
    }

    impl TempProject {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock before epoch")
                .as_nanos();
            let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let name = format!(
                "kiln-module-resolver-{nonce}-{}-{counter}",
                std::process::id()
            );
            let root = std::env::temp_dir().join(&name);
            let outside = std::env::temp_dir().join(format!("{name}-outside"));
            fs::create_dir_all(&root).expect("create test project");
            fs::create_dir_all(&outside).expect("create outside project");
            Self { root, outside }
        }

        fn write(&self, relative: &str, contents: &str) -> PathBuf {
            let path = self.root.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create source parent");
            }
            let mut file = File::create(&path).expect("create source");
            file.write_all(contents.as_bytes()).expect("write source");
            path
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
            let _ = fs::remove_dir_all(&self.outside);
        }
    }

    fn resolver(project: &TempProject) -> ModuleResolver {
        ModuleResolver::new(&project.root).expect("valid project root")
    }

    #[test]
    fn resolves_relative_source_without_guessing() {
        let project = TempProject::new();
        let importer = project.write("src/main.ts", "import './dep.js';");
        let target = project.write("src/dep.js", "export const answer = 42;");
        let resolved = resolver(&project)
            .resolve("./dep.js", &importer)
            .expect("resolve explicit relative source");
        assert_eq!(resolved.kind, ResolutionKind::Relative);
        assert_eq!(resolved.path, fs::canonicalize(target).unwrap());
        assert!(resolved.path.is_absolute());
    }

    #[test]
    fn resolves_absolute_source() {
        let project = TempProject::new();
        let importer = project.write("main.mjs", "import x from '/unused';");
        let target = project.write("dep.mts", "export default 1;");
        let target = fs::canonicalize(target).unwrap();
        let resolved = resolver(&project)
            .resolve(target.to_str().unwrap(), &importer)
            .expect("resolve explicit absolute source");
        assert_eq!(resolved.kind, ResolutionKind::Absolute);
        assert_eq!(resolved.path, target);
    }

    #[test]
    fn supports_every_explicit_source_extension() {
        let project = TempProject::new();
        let importer = project.write("main.js", "");
        for extension in SUPPORTED_EXTENSIONS {
            let name = format!("dep.{extension}");
            project.write(&name, "");
            assert!(
                resolver(&project)
                    .resolve(&format!("./{name}"), &importer)
                    .is_ok()
            );
        }
    }

    #[test]
    fn does_not_guess_extensions_or_index_files() {
        let project = TempProject::new();
        let importer = project.write("main.js", "");
        project.write("dep.ts", "");
        project.write("dir/index.ts", "");
        assert!(matches!(
            resolver(&project).resolve("./dep", &importer),
            Err(ResolveError::Io {
                role: PathRole::Target,
                kind: io::ErrorKind::NotFound,
                ..
            })
        ));
        assert!(matches!(
            resolver(&project).resolve("./dir", &importer),
            Err(ResolveError::NotRegularFile {
                role: PathRole::Target,
                ..
            })
        ));
    }

    #[test]
    fn rejects_bare_packages_and_url_schemes() {
        let project = TempProject::new();
        let importer = project.write("main.js", "");
        for (specifier, reason) in [
            ("react", InvalidSpecifierReason::BarePackage),
            ("@scope/pkg", InvalidSpecifierReason::BarePackage),
            (
                "https://example.test/dep.js",
                InvalidSpecifierReason::UrlScheme,
            ),
            ("node:fs", InvalidSpecifierReason::UrlScheme),
            ("data:text/javascript,1", InvalidSpecifierReason::UrlScheme),
            ("npm+ssh:pkg", InvalidSpecifierReason::UrlScheme),
        ] {
            assert_eq!(
                resolver(&project).resolve(specifier, &importer),
                Err(ResolveError::InvalidSpecifier {
                    specifier: specifier.to_owned(),
                    reason,
                })
            );
        }
    }

    fn write_package(project: &TempProject, package: &str, manifest: &str) {
        project.write(&format!("node_modules/{package}/package.json"), manifest);
    }

    #[test]
    fn resolves_nearest_scoped_and_unscoped_packages_with_exact_exports() {
        let project = TempProject::new();
        let importer = project.write("src/nested/main.ts", "");
        let root_entry = project.write("node_modules/pkg/dist/root.js", "");
        let feature_entry = project.write("node_modules/pkg/dist/feature.ts", "");
        write_package(
            &project,
            "pkg",
            r#"{"exports":{".":"./dist/root.js","./feature":"./dist/feature.ts","./blocked":null}}"#,
        );
        let scoped_entry = project.write("node_modules/@scope/name/index.mjs", "");
        write_package(
            &project,
            "@scope/name",
            r#"{"exports":{".":"./index.mjs"}}"#,
        );

        let resolver = NodePackageResolver::new(&project.root).expect("valid project root");
        let resolved = resolver
            .resolve("pkg", &importer)
            .expect("resolve package root");
        assert_eq!(resolved.kind, ResolutionKind::Package);
        assert_eq!(resolved.path, fs::canonicalize(root_entry).unwrap());
        let resolved = resolver
            .resolve("pkg/feature", &importer)
            .expect("resolve exact package subpath");
        assert_eq!(resolved.path, fs::canonicalize(feature_entry).unwrap());
        let resolved = resolver
            .resolve("@scope/name", &importer)
            .expect("resolve scoped package root");
        assert_eq!(resolved.path, fs::canonicalize(scoped_entry).unwrap());
    }

    #[test]
    fn resolves_typescript_extension_aliases_under_node_policy() {
        let project = TempProject::new();
        let importer = project.write("src/main.ts", "import './dep.js'; import './ui.js';");
        let target_ts = project.write("src/dep.ts", "export const answer = 42;");
        let target_tsx = project.write("src/ui.tsx", "export const Widget = () => null;");
        let resolver = NodePackageResolver::new(&project.root).expect("valid project root");

        let resolved_ts = resolver
            .resolve("./dep.js", &importer)
            .expect("resolve .js to .ts");
        assert_eq!(resolved_ts.kind, ResolutionKind::Relative);
        assert_eq!(resolved_ts.path, fs::canonicalize(target_ts).unwrap());

        let resolved_tsx = resolver
            .resolve("./ui.js", &importer)
            .expect("resolve .js to .tsx");
        assert_eq!(resolved_tsx.kind, ResolutionKind::Relative);
        assert_eq!(resolved_tsx.path, fs::canonicalize(target_tsx).unwrap());

        // A missing relative module reports as module resolution failure, not package not found
        let err = resolver
            .resolve("./nonexistent.js", &importer)
            .expect_err("nonexistent module fails");
        let msg = err.to_string();
        assert!(
            msg.contains("could not resolve module request `./nonexistent.js`"),
            "expected module error, got: {msg}"
        );
        assert!(
            !msg.contains("package `.`"),
            "must not format as package `.`, got: {msg}"
        );
    }

    #[test]
    fn nearest_package_wins_and_explicit_resolution_is_preserved() {
        let project = TempProject::new();
        let importer = project.write("src/deep/main.js", "");
        let nearest = project.write("src/node_modules/pkg/main.ts", "");
        project.write("node_modules/pkg/main.ts", "");
        write_package(&project, "pkg", r#"{"main":"main.ts"}"#);
        let nested_manifest = project.root.join("src/node_modules/pkg/package.json");
        fs::write(&nested_manifest, r#"{"main":"main.ts"}"#).expect("write nested manifest");
        project.write("src/explicit.js", "");

        let resolver = NodePackageResolver::new(&project.root).expect("valid project root");
        assert_eq!(
            resolver.resolve("pkg", &importer).unwrap().path,
            fs::canonicalize(nearest).unwrap()
        );
        assert_eq!(
            resolver.resolve("../explicit.js", &importer).unwrap().kind,
            ResolutionKind::Relative
        );
    }

    #[test]
    fn package_exports_reject_unlisted_and_null_subpaths() {
        let project = TempProject::new();
        let importer = project.write("main.js", "");
        project.write("node_modules/pkg/main.js", "");
        write_package(
            &project,
            "pkg",
            r#"{"exports":{".":"./main.js","./private":null}}"#,
        );
        let resolver = NodePackageResolver::new(&project.root).expect("valid project root");
        assert!(matches!(
            resolver.resolve("pkg/private", &importer),
            Err(ResolveError::PackageSubpathNotExported { .. })
        ));
        assert!(matches!(
            resolver.resolve("pkg/missing", &importer),
            Err(ResolveError::PackageSubpathNotExported { .. })
        ));
    }

    #[test]
    fn package_policy_inherits_conditions_and_wildcards_from_oxc() {
        let project = TempProject::new();
        let importer = project.write("main.js", "");
        let imported = project.write("node_modules/pkg/esm.js", "");
        let feature = project.write("node_modules/pkg/features/one.js", "");
        project.write("node_modules/pkg/fallback.js", "");
        write_package(
            &project,
            "pkg",
            r#"{"exports":{".":{"import":"./esm.js","default":"./fallback.js"},"./features/*":"./features/*.js"}}"#,
        );

        let resolver = NodePackageResolver::new(&project.root).expect("valid project root");
        assert_eq!(
            resolver.resolve("pkg", &importer).unwrap().path,
            fs::canonicalize(imported).unwrap()
        );
        assert_eq!(
            resolver
                .resolve("pkg/features/one", &importer)
                .unwrap()
                .path,
            fs::canonicalize(feature).unwrap()
        );
    }

    #[test]
    fn package_decisions_can_be_pinned_and_replayed_without_the_manifest() {
        let project = TempProject::new();
        let importer = project.write("main.js", "");
        let target = project.write("node_modules/pkg/index.js", "");
        write_package(&project, "pkg", r#"{"exports":"./index.js"}"#);

        let live = NodePackageResolver::new(&project.root)
            .expect("valid project root")
            .resolve_with_metadata("pkg", &importer)
            .expect("resolve package with metadata");
        assert_eq!(
            live.package_json,
            Some(
                fs::canonicalize(project.root.join("node_modules/pkg/package.json"))
                    .expect("canonical manifest"),
            )
        );
        fs::remove_file(project.root.join("node_modules/pkg/package.json"))
            .expect("remove manifest after pinning");

        let pinned = PinnedPackageResolver::new(
            &project.root,
            [(
                fs::canonicalize(&importer).unwrap(),
                "pkg".to_owned(),
                fs::canonicalize(&target).unwrap(),
            )],
        )
        .expect("construct pinned resolver");
        assert_eq!(pinned.resolve("pkg", &importer).unwrap(), live.module);
        assert!(matches!(
            pinned.resolve("other", &importer),
            Err(ResolveError::PackageResolution { .. })
        ));
    }

    #[test]
    fn pinned_node_format_is_replayed_without_the_package_manifest() {
        let project = TempProject::new();
        let importer = project.write("main.js", "");
        let target = project.write("node_modules/pkg/index.js", "");
        write_package(
            &project,
            "pkg",
            r#"{"exports":"./index.js","type":"module"}"#,
        );

        let resolver = NodePackageResolver::new(&project.root).expect("valid project root");
        let live = resolver
            .resolve_with_metadata("pkg", &importer)
            .expect("resolve package with metadata");
        assert_eq!(
            resolver.node_format(&live.module).unwrap(),
            Some(NodeModuleFormat::EsModule)
        );
        let importer = fs::canonicalize(importer).expect("canonical importer");
        let target = fs::canonicalize(target).expect("canonical target");
        let pinned = PinnedPackageResolver::new_with_node_builtins_and_formats(
            &project.root,
            [(importer.clone(), "pkg".to_owned(), target.clone())],
            [(target.clone(), NodeModuleFormat::EsModule)],
        )
        .expect("construct pinned resolver");

        let resolved = pinned
            .resolve("pkg", &importer)
            .expect("replay package target");
        fs::write(
            project.root.join("node_modules/pkg/package.json"),
            r#"{"exports":"./index.js","type":"commonjs"}"#,
        )
        .expect("modify manifest after capture");
        assert_eq!(
            pinned.node_format(&resolved).unwrap(),
            Some(NodeModuleFormat::EsModule)
        );
        fs::remove_file(project.root.join("node_modules/pkg/package.json"))
            .expect("remove manifest after capture");
        assert_eq!(
            pinned.node_format(&resolved).unwrap(),
            Some(NodeModuleFormat::EsModule)
        );
    }

    #[test]
    fn pinned_source_format_is_available_without_node_builtins() {
        let project = TempProject::new();
        let importer = fs::canonicalize(project.write("main.js", "")).expect("canonical importer");
        let target = fs::canonicalize(project.write("pkg/index.js", "")).expect("canonical target");
        let pinned = PinnedPackageResolver::new_with_formats(
            &project.root,
            std::iter::empty::<(PathBuf, String, PathBuf)>(),
            [(target.clone(), NodeModuleFormat::CommonJs)],
        )
        .expect("construct ordinary pinned resolver");
        let resolved = pinned
            .resolve("./pkg/index.js", &importer)
            .expect("resolve explicit module");

        assert_eq!(
            pinned.node_format(&resolved).unwrap(),
            Some(NodeModuleFormat::CommonJs)
        );
        assert!(matches!(
            pinned.resolve("node:http", &importer),
            Err(ResolveError::InvalidSpecifier {
                reason: InvalidSpecifierReason::UrlScheme,
                ..
            })
        ));
    }

    #[test]
    fn package_conditions_follow_import_and_require_requests_and_can_be_pinned() {
        let project = TempProject::new();
        let importer = project.write("main.mjs", "");
        let esm = project.write("node_modules/pkg/esm.mjs", "");
        let cjs = project.write("node_modules/pkg/cjs.cjs", "");
        write_package(
            &project,
            "pkg",
            r#"{"exports":{"import":"./esm.mjs","require":"./cjs.cjs"}}"#,
        );
        let resolver = NodePackageResolver::new(&project.root).expect("valid project root");
        let import_target = resolver
            .resolve_with_metadata_for_request("pkg", &importer, NodeRequestKind::Import)
            .expect("resolve import condition");
        let require_target = resolver
            .resolve_with_metadata_for_request("pkg", &importer, NodeRequestKind::Require)
            .expect("resolve require condition");
        let importer = fs::canonicalize(importer).expect("canonical importer");
        let esm = fs::canonicalize(esm).expect("canonical ESM target");
        let cjs = fs::canonicalize(cjs).expect("canonical CommonJS target");
        assert_eq!(import_target.module.path, esm);
        assert_eq!(require_target.module.path, cjs);

        let pinned = PinnedPackageResolver::new_with_node_builtins_requests_and_formats(
            &project.root,
            [
                (
                    importer.clone(),
                    "pkg".to_owned(),
                    NodeRequestKind::Import,
                    esm.clone(),
                ),
                (
                    importer.clone(),
                    "pkg".to_owned(),
                    NodeRequestKind::Require,
                    cjs.clone(),
                ),
            ],
            [
                (esm.clone(), NodeModuleFormat::EsModule),
                (cjs.clone(), NodeModuleFormat::CommonJs),
            ],
        )
        .expect("construct pinned resolver");
        fs::remove_file(project.root.join("node_modules/pkg/package.json"))
            .expect("remove manifest after capture");
        assert_eq!(
            pinned
                .resolve_with_request("pkg", &importer, NodeRequestKind::Import)
                .unwrap()
                .path,
            esm
        );
        assert_eq!(
            pinned
                .resolve_with_request("pkg", &importer, NodeRequestKind::Require)
                .unwrap()
                .path,
            cjs
        );
    }

    #[test]
    fn package_target_cannot_escape_into_another_in_project_directory() {
        let project = TempProject::new();
        let importer = project.write("main.js", "");
        project.write("node_modules/shared.js", "");
        write_package(&project, "pkg", r#"{"exports":{".":"./../shared.js"}}"#);
        let resolver = NodePackageResolver::new(&project.root).expect("valid project root");

        assert!(matches!(
            resolver.resolve("pkg", &importer),
            Err(ResolveError::InvalidPackageTarget { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn package_targets_keep_containment_and_extension_guards() {
        use std::os::unix::fs::symlink;

        let project = TempProject::new();
        let importer = project.write("main.js", "");
        let outside = project.outside.join("escape.js");
        fs::write(&outside, "").expect("write outside target");
        project.write("node_modules/escape/package.json", "{}");
        symlink(&outside, project.root.join("node_modules/escape/escape.js"))
            .expect("write escaping package target");
        write_package(&project, "escape", r#"{"exports":{".":"./escape.js"}}"#);
        project.write("node_modules/data/payload.txt", "");
        write_package(&project, "data", r#"{"exports":{".":"./payload.txt"}}"#);
        let resolver = NodePackageResolver::new(&project.root).expect("valid project root");
        assert!(matches!(
            resolver.resolve("escape", &importer),
            Err(ResolveError::OutsideProject { .. })
        ));
        assert!(matches!(
            resolver.resolve("data", &importer),
            Err(ResolveError::UnsupportedExtension { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn pinned_package_targets_are_canonical_and_cannot_escape_through_symlinks() {
        use std::os::unix::fs::symlink;

        let project = TempProject::new();
        let importer = project.write("main.js", "");
        let canonical_importer = fs::canonicalize(&importer).expect("canonical importer");
        let target = project.write("node_modules/inside/target.js", "");
        let inside_link = project.root.join("node_modules/inside/link.js");
        symlink(&target, &inside_link).expect("write in-project package symlink");

        let outside = project.outside.join("escape.js");
        fs::write(&outside, "").expect("write outside target");
        let escaping_link = project.root.join("node_modules/escape.js");
        symlink(&outside, &escaping_link).expect("write escaping package symlink");

        let pinned = PinnedPackageResolver::new(
            &project.root,
            [
                (canonical_importer.clone(), "inside".to_owned(), inside_link),
                (canonical_importer, "escape".to_owned(), escaping_link),
            ],
        )
        .expect("construct pinned resolver");

        assert_eq!(
            pinned.resolve("inside", &importer).unwrap().path,
            fs::canonicalize(target).expect("canonical target")
        );
        assert!(matches!(
            pinned.resolve("escape", &importer),
            Err(ResolveError::OutsideProject { .. })
        ));
    }

    #[test]
    fn rejects_query_fragment_and_empty_specifiers() {
        let project = TempProject::new();
        let importer = project.write("main.js", "");
        for specifier in ["", "./dep.js?raw", "./dep.js#section"] {
            assert!(matches!(
                resolver(&project).resolve(specifier, &importer),
                Err(ResolveError::InvalidSpecifier { .. })
            ));
        }
    }

    #[test]
    fn rejects_unsupported_extension_and_directories() {
        let project = TempProject::new();
        let importer = project.write("main.js", "");
        project.write("data.txt", "{}");
        fs::create_dir(project.root.join("folder.ts")).expect("create directory with extension");
        assert!(matches!(
            resolver(&project).resolve("./data.txt", &importer),
            Err(ResolveError::UnsupportedExtension { .. })
        ));
        assert!(matches!(
            resolver(&project).resolve("./folder.ts", &importer),
            Err(ResolveError::NotRegularFile { .. })
        ));
    }

    #[test]
    fn rejects_relative_importer() {
        let project = TempProject::new();
        project.write("main.js", "");
        assert_eq!(
            resolver(&project).resolve("./main.js", "main.js"),
            Err(ResolveError::NonAbsolutePath {
                role: PathRole::Importer,
                path: PathBuf::from("main.js"),
            })
        );
    }

    #[test]
    fn rejects_missing_target() {
        let project = TempProject::new();
        let importer = project.write("main.js", "");
        assert!(matches!(
            resolver(&project).resolve("./missing.ts", &importer),
            Err(ResolveError::Io {
                role: PathRole::Target,
                kind: io::ErrorKind::NotFound,
                ..
            })
        ));
    }

    #[test]
    fn rejects_traversal_outside_project_after_canonicalization() {
        let project = TempProject::new();
        let importer = project.write("nested/main.js", "");
        project.write("inside.ts", "");
        let escaped = project.outside.join("escaped.ts");
        fs::write(&escaped, "").expect("write outside source");

        let outside = fs::canonicalize(&project.outside).unwrap();
        let importer_parent = fs::canonicalize(importer.parent().unwrap()).unwrap();
        let relative = pathdiff(&outside, &importer_parent).join("escaped.ts");
        assert!(matches!(
            resolver(&project).resolve(relative.to_str().unwrap(), &importer),
            Err(ResolveError::OutsideProject { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape_after_canonicalization() {
        use std::os::unix::fs::symlink;

        let project = TempProject::new();
        let importer = project.write("main.js", "");
        let outside_target = project.outside.join("escaped.ts");
        fs::write(&outside_target, "").expect("write outside source");
        symlink(&outside_target, project.root.join("escape.ts")).expect("create symlink");
        assert!(matches!(
            resolver(&project).resolve("./escape.ts", &importer),
            Err(ResolveError::OutsideProject { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn normalizes_in_project_symlink() {
        use std::os::unix::fs::symlink;

        let project = TempProject::new();
        let importer = project.write("main.js", "");
        let target = project.write("real.ts", "");
        symlink(&target, project.root.join("alias.ts")).expect("create symlink");
        let resolved = resolver(&project)
            .resolve("./alias.ts", &importer)
            .expect("resolve in-project symlink");
        assert_eq!(resolved.path, fs::canonicalize(target).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_non_utf8_path() {
        use std::os::unix::ffi::OsStringExt;

        let project = TempProject::new();
        let invalid = project.root.join(std::ffi::OsString::from_vec(vec![
            b'x', 0xff, b'.', b't', b's',
        ]));
        let error = resolve("./x.ts", invalid, &project.root)
            .expect_err("non-UTF8 importer should be rejected");
        assert!(matches!(
            error,
            ResolveError::NonUtf8Path {
                role: PathRole::Importer,
                ..
            }
        ));
    }

    #[test]
    fn errors_have_stable_display() {
        let error = ResolveError::InvalidSpecifier {
            specifier: "lodash".to_owned(),
            reason: InvalidSpecifierReason::BarePackage,
        };
        assert_eq!(
            error.to_string(),
            "invalid module specifier `lodash`: bare package specifiers are not supported"
        );
        let error = ResolveError::OutsideProject {
            path: PathBuf::from("/tmp/outside.ts"),
            project_root: PathBuf::from("/tmp/project"),
        };
        assert_eq!(
            error.to_string(),
            "resolved module escapes project root: /tmp/outside.ts (root /tmp/project)"
        );
    }

    #[test]
    fn node_runtime_policy_resolves_only_its_explicit_builtin_surface() {
        let project = TempProject::new();
        let importer = project.write("main.js", "");
        let resolver = NodeRuntimeResolver::new(&project.root).expect("valid project root");
        for (specifier, builtin) in [
            ("node:events", NodeBuiltin::Events),
            ("node:fs/promises", NodeBuiltin::FsPromises),
            ("node:buffer", NodeBuiltin::Buffer),
            ("node:fs", NodeBuiltin::Fs),
            ("node:net", NodeBuiltin::Net),
            ("node:http", NodeBuiltin::Http),
            ("node:module", NodeBuiltin::Module),
            ("node:path", NodeBuiltin::Path),
            ("node:tty", NodeBuiltin::Tty),
            ("node:zlib", NodeBuiltin::Zlib),
            ("node:string_decoder", NodeBuiltin::StringDecoder),
            ("node:worker_threads", NodeBuiltin::WorkerThreads),
        ] {
            let resolved = resolver
                .resolve(specifier, &importer)
                .expect("builtin resolves");
            assert_eq!(resolved.kind, ResolutionKind::Builtin);
            assert_eq!(resolved.builtin(), Some(builtin));
            assert_eq!(resolved.path, builtin.virtual_path(resolver.project_root()));
            assert!(
                !resolved.path.exists(),
                "builtin identity is not a source file"
            );
        }
        assert!(matches!(
            resolver.resolve("node:tls", &importer),
            Err(ResolveError::InvalidSpecifier {
                reason: InvalidSpecifierReason::UrlScheme,
                ..
            })
        ));
    }

    #[test]
    fn unprefixed_node_builtin_aliases_share_one_identity() {
        let project = TempProject::new();
        let importer = project.write("main.js", "");
        let resolver = NodeRuntimeResolver::new(&project.root).expect("valid project root");
        for name in [
            "events",
            "fs/promises",
            "buffer",
            "fs",
            "net",
            "http",
            "module",
            "path",
            "tty",
            "zlib",
            "string_decoder",
            "worker_threads",
        ] {
            assert_eq!(
                resolver.resolve(name, &importer).unwrap(),
                resolver
                    .resolve(&format!("node:{name}"), &importer)
                    .unwrap()
            );
        }
    }

    #[test]
    fn bounded_node_builtin_parse_and_specifier_round_trip() {
        assert_eq!(NodeBuiltin::parse("kiln:shared"), Some(NodeBuiltin::Shared));
        assert_eq!(NodeBuiltin::parse("kiln:gui"), Some(NodeBuiltin::Gui));
        assert_eq!(NodeBuiltin::Gui.specifier(), "kiln:gui");
        assert_eq!(NodeBuiltin::parse("shared"), None);
        assert_eq!(NodeBuiltin::parse("node:shared"), None);
        assert_eq!(NodeBuiltin::parse("node:kiln:shared"), None);
        for builtin in [
            NodeBuiltin::Buffer,
            NodeBuiltin::Console,
            NodeBuiltin::Fs,
            NodeBuiltin::Module,
            NodeBuiltin::Path,
            NodeBuiltin::Tty,
            NodeBuiltin::Zlib,
            NodeBuiltin::StringDecoder,
            NodeBuiltin::WorkerThreads,
        ] {
            assert_eq!(NodeBuiltin::parse(builtin.specifier()), Some(builtin));
            if builtin == NodeBuiltin::Process {
                assert_eq!(NodeBuiltin::parse("kiln:process"), Some(builtin));
            } else {
                assert_eq!(
                    NodeBuiltin::parse(builtin.specifier().trim_start_matches("node:")),
                    Some(builtin)
                );
            }
        }
        let project_root = Path::new("/test_project");
        for builtin in NodeBuiltin::ALL {
            let path = builtin.virtual_path(project_root);
            let resolved = ResolvedModule {
                path: path.clone(),
                kind: ResolutionKind::Builtin,
            };
            assert_eq!(
                resolved.builtin(),
                Some(*builtin),
                "Builtin {:?} (specifier '{}', virtual_path '{}') must round-trip through ResolvedModule::builtin()",
                builtin,
                builtin.specifier(),
                path.display()
            );
        }
    }

    #[test]
    fn engine_builtin_dependencies_do_not_expose_node_modules_to_ordinary_sources() {
        let project = TempProject::new();
        let source = project.write("main.ts", "");
        let explicit = ModuleResolver::new(&project.root).expect("explicit resolver");
        let pinned = PinnedPackageResolver::new(
            &project.root,
            std::iter::empty::<(PathBuf, String, PathBuf)>(),
        )
        .expect("ordinary pinned resolver");
        let actor = NodeBuiltin::Actors.virtual_path(&project.root);
        let worker = NodeBuiltin::WorkerThreads.virtual_path(&project.root);
        for resolved in [
            explicit.resolve("node:worker_threads", &actor),
            pinned.resolve("node:worker_threads", &actor),
        ] {
            assert_eq!(
                resolved.expect("actor transport").builtin(),
                Some(NodeBuiltin::WorkerThreads)
            );
        }
        for resolved in [
            explicit.resolve("node:events", &worker),
            pinned.resolve("node:events", &worker),
        ] {
            assert_eq!(
                resolved.expect("builtin dependency").builtin(),
                Some(NodeBuiltin::Events)
            );
        }
        assert!(explicit.resolve("node:worker_threads", &source).is_err());
        assert!(pinned.resolve("node:worker_threads", &source).is_err());
        assert_eq!(
            pinned
                .resolve("kiln:actors/core", &actor)
                .expect("Kiln namespace")
                .builtin(),
            Some(NodeBuiltin::ActorsCore)
        );
        let fake = project.write(".kiln/builtins/actors.builtin", "");
        assert!(pinned.resolve("node:worker_threads", &fake).is_err());
    }

    #[test]
    fn pinned_node_policy_is_opt_in_and_keeps_unknown_builtins_refused() {
        let project = TempProject::new();
        let importer = project.write("main.js", "");
        let ordinary = PinnedPackageResolver::new(
            &project.root,
            std::iter::empty::<(PathBuf, String, PathBuf)>(),
        )
        .expect("ordinary pinned resolver");
        assert!(matches!(
            ordinary.resolve("node:http", &importer),
            Err(ResolveError::InvalidSpecifier {
                reason: InvalidSpecifierReason::UrlScheme,
                ..
            })
        ));

        let node = PinnedPackageResolver::new_with_node_builtins(
            &project.root,
            std::iter::empty::<(PathBuf, String, PathBuf)>(),
        )
        .expect("Node pinned resolver");
        assert_eq!(
            node.resolve("node:http", &importer)
                .expect("recognized builtin resolves")
                .builtin(),
            Some(NodeBuiltin::Http)
        );
        assert!(matches!(
            node.resolve("node:tls", &importer),
            Err(ResolveError::InvalidSpecifier {
                reason: InvalidSpecifierReason::UrlScheme,
                ..
            })
        ));
    }

    #[test]
    fn node_format_ambiguity_marks_only_undetectable_paths() {
        let project = TempProject::new();
        // No manifest anywhere above: ambiguous whatever the (non-definitive) extension.
        assert!(node_format_is_ambiguous(Path::new("script.ts")));
        assert!(node_format_is_ambiguous(Path::new("script.js")));
        // Definitive extensions are never ambiguous, manifest or not.
        for name in ["a.mjs", "a.mts", "a.cjs", "a.cts"] {
            assert!(!node_format_is_ambiguous(Path::new(name)));
        }
        // A manifest without a "type" field leaves every ordinary file ambiguous.
        let typeless = project.write("plain/package.json", r#"{"name":"x"}"#);
        let typeless_js = typeless.with_file_name("app.js");
        assert!(node_format_is_ambiguous(&typeless_js));
        // An explicit "type" field decides the format: nothing to detect.
        let common = project.write("cjs/package.json", r#"{"type":"commonjs"}"#);
        assert!(!node_format_is_ambiguous(&common.with_file_name("app.ts")));
        let module = project.write("esm/package.json", r#"{"type":"module"}"#);
        assert!(!node_format_is_ambiguous(&module.with_file_name("app.ts")));
    }

    #[test]
    fn node_format_uses_definitive_extensions_and_package_type() {
        let project = TempProject::new();
        let common_manifest = project.write("cjs/package.json", r#"{"type":"commonjs"}"#);
        let module_manifest = project.write("esm/package.json", r#"{"type":"module"}"#);
        assert_eq!(
            node_module_format(Path::new("x.cjs"), Some(&module_manifest)).unwrap(),
            NodeModuleFormat::CommonJs
        );
        assert_eq!(
            node_module_format(Path::new("x.mjs"), Some(&common_manifest)).unwrap(),
            NodeModuleFormat::EsModule
        );
        // TypeScript twin extensions are definitive like their JavaScript sides: Node
        // type-stripping always runs `.mts` as ESM and `.cts` as CommonJS.
        assert_eq!(
            node_module_format(Path::new("x.cts"), Some(&module_manifest)).unwrap(),
            NodeModuleFormat::CommonJs
        );
        assert_eq!(
            node_module_format(Path::new("x.mts"), Some(&common_manifest)).unwrap(),
            NodeModuleFormat::EsModule
        );
        assert_eq!(
            node_module_format(Path::new("x.js"), Some(&common_manifest)).unwrap(),
            NodeModuleFormat::CommonJs
        );
        assert_eq!(
            node_module_format(Path::new("x.js"), Some(&module_manifest)).unwrap(),
            NodeModuleFormat::EsModule
        );
        assert_eq!(
            node_module_format(Path::new("x.js"), None).unwrap(),
            NodeModuleFormat::CommonJs
        );
    }

    #[test]
    fn resolves_tsconfig_paths_and_base_url() {
        let project = TempProject::new();
        let importer = project.write(
            "src/nested/app.ts",
            "import '@/utils/math'; import '@components/button';",
        );
        let target_math = project.write("src/utils/math.ts", "export const add = 1;");
        let target_button = project.write("src/components/button.tsx", "export const Button = 2;");
        project.write(
            "tsconfig.json",
            r#"{
                "compilerOptions": {
                    "baseUrl": ".",
                    "paths": {
                        "@/*": ["src/*"],
                        "@components/*": ["src/components/*"]
                    }
                }
            }"#,
        );
        let resolver = NodePackageResolver::new(&project.root).expect("valid project root");

        let resolved = resolver
            .resolve("@/utils/math", &importer)
            .expect("resolve tsconfig path alias");
        assert_eq!(resolved.path, fs::canonicalize(target_math).unwrap());

        let resolved = resolver
            .resolve("@components/button", &importer)
            .expect("resolve components alias");
        assert_eq!(resolved.path, fs::canonicalize(target_button).unwrap());
    }

    #[test]
    fn resolves_tsconfig_with_extends() {
        let project = TempProject::new();
        let importer = project.write("src/app.ts", "import '@/services/api';");
        let target_api = project.write("src/services/api.ts", "export const api = 1;");
        project.write(
            "tsconfig.base.json",
            r#"{
                "compilerOptions": {
                    "baseUrl": ".",
                    "paths": {
                        "@/*": ["src/*"]
                    }
                }
            }"#,
        );
        project.write(
            "tsconfig.json",
            r#"{
                "extends": "./tsconfig.base.json"
            }"#,
        );
        let resolver = NodePackageResolver::new(&project.root).expect("valid project root");

        let resolved = resolver
            .resolve("@/services/api", &importer)
            .expect("resolve tsconfig path alias via extends");
        assert_eq!(resolved.path, fs::canonicalize(target_api).unwrap());
    }

    #[test]
    fn resolves_jsconfig_paths_fallback() {
        let project = TempProject::new();
        let importer = project.write("src/app.js", "import '@/helpers/format';");
        let target = project.write("src/helpers/format.js", "export const format = () => '';");
        project.write(
            "jsconfig.json",
            r#"{
                "compilerOptions": {
                    "baseUrl": ".",
                    "paths": {
                        "@/*": ["src/*"]
                    }
                }
            }"#,
        );
        let resolver = NodePackageResolver::new(&project.root).expect("valid project root");

        let resolved = resolver
            .resolve("@/helpers/format", &importer)
            .expect("resolve jsconfig path alias");
        assert_eq!(resolved.path, fs::canonicalize(target).unwrap());
    }

    // Small lexical path-difference helper kept local to tests so the crate stays dependency-free.
    fn pathdiff(path: &Path, base: &Path) -> PathBuf {
        let mut path_components = path.components();
        let mut base_components = base.components();
        while path_components.clone().next() == base_components.clone().next()
            && path_components.clone().next().is_some()
        {
            path_components.next();
            base_components.next();
        }
        let mut result = PathBuf::new();
        for _ in base_components {
            result.push("..");
        }
        for component in path_components {
            result.push(component.as_os_str());
        }
        result
    }
}
