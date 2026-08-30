//! Deterministic filesystem resolution for Kiln's first ESM graph slice.
//!
//! The resolver intentionally has a small policy surface. [`ModuleResolver`] accepts only
//! explicit relative and absolute filesystem specifiers, never guesses an extension or an
//! `index` file, and verifies the canonical target remains inside the canonical project root.
//! [`NodePackageResolver`] is a separate package-policy adapter behind [`ResolutionPolicy`].

use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Source extensions understood by the initial filesystem resolver.
pub const SUPPORTED_EXTENSIONS: &[&str] = &["js", "mjs", "cjs", "jsx", "ts", "mts", "cts", "tsx"];

/// Whether a resolved request was written as an explicit relative or absolute specifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResolutionKind {
    Relative,
    Absolute,
    Package,
}

impl ResolutionKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Relative => "relative",
            Self::Absolute => "absolute",
            Self::Package => "package",
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
    /// The symlink-normalized, absolute source path.
    pub path: PathBuf,
    pub kind: ResolutionKind,
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
    PackageNotFound {
        package: String,
        importer: PathBuf,
    },
    PackageManifestMissing {
        path: PathBuf,
    },
    InvalidPackageManifest {
        path: PathBuf,
    },
    PackageSubpathNotExported {
        package: String,
        subpath: String,
    },
    PackageExportBlocked {
        package: String,
        subpath: String,
    },
    PackageEntryMissing {
        package: String,
    },
    InvalidPackageTarget {
        package: String,
        target: String,
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
            Self::PackageNotFound { package, importer } => write!(
                formatter,
                "could not find package `{package}` from {}",
                display_path(importer)
            ),
            Self::PackageManifestMissing { path } => write!(
                formatter,
                "package manifest is missing: {}",
                display_path(path)
            ),
            Self::InvalidPackageManifest { path } => write!(
                formatter,
                "invalid package manifest: {}",
                display_path(path)
            ),
            Self::PackageSubpathNotExported { package, subpath } => {
                write!(formatter, "package `{package}` does not export `{subpath}`")
            }
            Self::PackageExportBlocked { package, subpath } => write!(
                formatter,
                "package `{package}` blocks access to `{subpath}`"
            ),
            Self::PackageEntryMissing { package } => {
                write!(formatter, "package `{package}` has no entry point")
            }
            Self::InvalidPackageTarget { package, target } => write!(
                formatter,
                "package `{package}` has invalid export target `{target}`"
            ),
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
/// This intentionally implements a bounded package policy: nearest `node_modules`, package
/// `exports` exact keys (including explicit `null` blocking), and `main` fallback. It does not
/// evaluate conditional exports or wildcard keys, and it never guesses an extension or index
/// file. Every selected target still goes through canonical containment and supported-extension
/// checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodePackageResolver {
    explicit: ModuleResolver,
}

impl NodePackageResolver {
    pub fn new(project_root: impl AsRef<Path>) -> Result<Self, ResolveError> {
        Ok(Self {
            explicit: ModuleResolver::new(project_root)?,
        })
    }

    pub fn project_root(&self) -> &Path {
        self.explicit.project_root()
    }

    pub fn resolve(
        &self,
        specifier: &str,
        importer: impl AsRef<Path>,
    ) -> Result<ResolvedModule, ResolveError> {
        let importer = importer.as_ref();
        if !matches!(
            classify_specifier(specifier),
            Err(ResolveError::InvalidSpecifier {
                reason: InvalidSpecifierReason::BarePackage,
                ..
            })
        ) {
            return self.explicit.resolve(specifier, importer);
        }

        let canonical_importer = canonical_importer(importer, self.project_root())?;
        let (package, subpath) = parse_package_specifier(specifier)?;
        let package_dir = self.find_package(&package, &canonical_importer)?;
        let manifest_path = package_dir.join("package.json");
        if !manifest_path.is_file() {
            return Err(ResolveError::PackageManifestMissing {
                path: manifest_path,
            });
        }
        let manifest_text = fs::read_to_string(&manifest_path).map_err(|_| {
            ResolveError::InvalidPackageManifest {
                path: manifest_path.clone(),
            }
        })?;
        let manifest: PackageManifest = serde_json::from_str(&manifest_text).map_err(|_| {
            ResolveError::InvalidPackageManifest {
                path: manifest_path.clone(),
            }
        })?;

        let target = match manifest.exports {
            Some(exports) => select_export(&package, &subpath, exports, &manifest_path)?,
            None if subpath.is_empty() => {
                manifest
                    .main
                    .ok_or_else(|| ResolveError::PackageEntryMissing {
                        package: package.clone(),
                    })?
            }
            None => subpath.clone(),
        };
        self.resolve_package_target(&package, &package_dir, &target)
    }

    fn find_package(&self, package: &str, importer: &Path) -> Result<PathBuf, ResolveError> {
        let mut ancestor = importer.parent();
        while let Some(directory) = ancestor {
            let candidate = directory.join("node_modules").join(package);
            match fs::metadata(&candidate) {
                Ok(metadata) if metadata.is_dir() => {
                    let canonical = canonical_path(&candidate, PathRole::Target)?;
                    if !canonical.starts_with(self.project_root()) {
                        return Err(ResolveError::OutsideProject {
                            path: canonical,
                            project_root: self.project_root().to_path_buf(),
                        });
                    }
                    return Ok(canonical);
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(ResolveError::Io {
                        role: PathRole::Target,
                        path: candidate,
                        kind: error.kind(),
                    });
                }
            }
            if directory == self.project_root() {
                break;
            }
            ancestor = directory.parent();
        }
        Err(ResolveError::PackageNotFound {
            package: package.to_owned(),
            importer: importer.to_path_buf(),
        })
    }

    fn resolve_package_target(
        &self,
        package: &str,
        package_dir: &Path,
        target: &str,
    ) -> Result<ResolvedModule, ResolveError> {
        if target.is_empty() || target.contains(['?', '#']) || target.as_bytes().contains(&0) {
            return Err(ResolveError::InvalidPackageTarget {
                package: package.to_owned(),
                target: target.to_owned(),
            });
        }
        let candidate = package_dir.join(target);
        let canonical_target = canonical_path(&candidate, PathRole::Target)?;
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
        Ok(ResolvedModule::new(
            canonical_target,
            ResolutionKind::Package,
        ))
    }
}

impl ResolutionPolicy for NodePackageResolver {
    fn project_root(&self) -> &Path {
        self.project_root()
    }

    fn resolve(&self, specifier: &str, importer: &Path) -> Result<ResolvedModule, ResolveError> {
        NodePackageResolver::resolve(self, specifier, importer)
    }
}

#[derive(Debug, serde::Deserialize)]
struct PackageManifest {
    exports: Option<serde_json::Value>,
    main: Option<String>,
}

fn select_export(
    package: &str,
    subpath: &str,
    exports: serde_json::Value,
    manifest_path: &Path,
) -> Result<String, ResolveError> {
    let key = if subpath.is_empty() {
        ".".to_owned()
    } else {
        format!("./{subpath}")
    };
    let value = match exports {
        serde_json::Value::Null => {
            return Err(ResolveError::PackageExportBlocked {
                package: package.to_owned(),
                subpath: key,
            });
        }
        serde_json::Value::String(target) if key == "." => target,
        serde_json::Value::String(_) => {
            return Err(ResolveError::PackageSubpathNotExported {
                package: package.to_owned(),
                subpath: key,
            });
        }
        serde_json::Value::Object(exports) => match exports.get(&key) {
            None => {
                return Err(ResolveError::PackageSubpathNotExported {
                    package: package.to_owned(),
                    subpath: key,
                });
            }
            Some(serde_json::Value::Null) => {
                return Err(ResolveError::PackageExportBlocked {
                    package: package.to_owned(),
                    subpath: key,
                });
            }
            Some(serde_json::Value::String(target)) => target.clone(),
            Some(_) => {
                return Err(ResolveError::InvalidPackageManifest {
                    path: manifest_path.to_path_buf(),
                });
            }
        },
        _ => {
            return Err(ResolveError::InvalidPackageManifest {
                path: manifest_path.to_path_buf(),
            });
        }
    };
    if !value.starts_with("./") {
        return Err(ResolveError::InvalidPackageTarget {
            package: package.to_owned(),
            target: value,
        });
    }
    Ok(value)
}

fn parse_package_specifier(specifier: &str) -> Result<(String, String), ResolveError> {
    let mut parts = specifier.split('/');
    let package = if specifier.starts_with('@') {
        let scope = parts.next().unwrap_or_default();
        let name = parts.next().unwrap_or_default();
        if scope.len() < 2 || name.is_empty() {
            return Err(ResolveError::InvalidSpecifier {
                specifier: specifier.to_owned(),
                reason: InvalidSpecifierReason::BarePackage,
            });
        }
        format!("{scope}/{name}")
    } else {
        parts.next().unwrap_or_default().to_owned()
    };
    if package.is_empty() || package == "." || package == ".." {
        return Err(ResolveError::InvalidSpecifier {
            specifier: specifier.to_owned(),
            reason: InvalidSpecifierReason::BarePackage,
        });
    }
    let subpath = if specifier.len() == package.len() {
        String::new()
    } else {
        specifier[package.len() + 1..].to_owned()
    };
    if !subpath.is_empty()
        && subpath
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(ResolveError::InvalidSpecifier {
            specifier: specifier.to_owned(),
            reason: InvalidSpecifierReason::BarePackage,
        });
    }
    Ok((package, subpath))
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
    fn package_exports_block_unlisted_and_null_subpaths() {
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
            Err(ResolveError::PackageExportBlocked { .. })
        ));
        assert!(matches!(
            resolver.resolve("pkg/missing", &importer),
            Err(ResolveError::PackageSubpathNotExported { .. })
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
        write_package(&project, "data", r#"{"exports":{".":"./package.json"}}"#);
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
        project.write("data.json", "{}");
        fs::create_dir(project.root.join("folder.ts")).expect("create directory with extension");
        assert!(matches!(
            resolver(&project).resolve("./data.json", &importer),
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
