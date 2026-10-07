//! The incremental query database (design.md §2, §3).
//!
//! Compilation is modelled as memoized queries over versioned inputs. The engine lives behind
//! the [`QueryEngine`] trait so the compiler is never tied to one implementation - see
//! docs/adr/0001 for why that boundary exists and docs/adr/0002 for what we expect of a
//! candidate backend such as salsa.

mod engine;
mod hash;
mod summary;

pub use engine::MemoEngine;
pub use hash::content_hash;
pub use kiln_digest::Digest;
pub use kiln_hir::Module as HirModule;
pub use kiln_ir::{CompileDiagnostic, ModuleProgram, Program};
pub use summary::{FunctionSummary, TypeGuess, summaries_digest};

/// Memoized result of lowering owned HIR to parser-independent bytecode.
pub type Compiled = Result<Program, Vec<CompileDiagnostic>>;
/// Memoized linkage-aware bytecode for graph execution.
pub type CompiledModule = Result<ModuleProgram, Vec<CompileDiagnostic>>;
/// Backend-owned query used to register function summaries with the memo engine.
pub use kiln_hir::source_types::CheckResult as SourceCheck;
/// Source checker injected at the composition root; checker internals stay out of the database.
pub type SourceCheckQuery = fn(
    &str,
    &[(String, std::sync::Arc<HirModule>)],
    &[(String, std::sync::Arc<[u8]>)],
) -> SourceCheck;
pub type SummaryQuery = fn(&Program) -> Vec<FunctionSummary>;

/// A parser/lowerer injected into the memo engine.
///
/// Keeping this as a plain function pointer makes the engine `Send`, cheap to construct, and
/// independent of Oxc. Production injects `kiln_frontend_oxc::parse`; database unit tests inject a
/// tiny deterministic fixture frontend.
pub type Frontend = fn(path: &str, source: &str) -> HirModule;

/// A source file interned by the database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId(pub u32);

/// Monotonic clock. Every source addition, replacement, or removal bumps it; nothing else does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Revision(pub u64);

/// The owned result of parsing and lowering one module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    /// Everything about the module, including things no dependent can observe.
    pub implementation_hash: u64,
    /// Convenience projection used by the current interface query. The complete owned module
    /// HIR is retained below for subsequent lowering and interpreter queries.
    pub exports: Vec<String>,
    pub lines: usize,
    pub module: std::sync::Arc<HirModule>,
}

impl Parsed {
    pub fn new(source: &str, module: HirModule) -> Self {
        Self {
            implementation_hash: content_hash(source),
            exports: module.runtime_export_names(),
            lines: source.lines().count(),
            module: std::sync::Arc::new(module),
        }
    }
}

/// The kind of runtime export edge that can change how a module is linked without changing its
/// declared export names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LinkageKind {
    ReExport,
    Star,
    NamespaceReExport,
}

/// One normalized runtime re-export edge in a module.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LinkageDescriptor {
    pub kind: LinkageKind,
    pub exported: Option<String>,
    pub source: String,
    pub imported: Option<String>,
}

/// Runtime export wiring included in a module's interface and project identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Linkage {
    pub descriptors: Vec<LinkageDescriptor>,
    pub digest: Digest,
}

impl Linkage {
    /// The canonical empty runtime wiring value used for interfaces built from names alone.
    pub fn empty() -> Self {
        Self::from_descriptors(Vec::new())
    }

    /// Derive normalized runtime re-export wiring from parsed HIR.
    pub fn from_module(module: &HirModule) -> Self {
        let mut descriptors = module
            .exports
            .iter()
            .filter(|export| !export.type_only)
            .filter_map(|export| {
                let exported = export
                    .exported
                    .as_ref()
                    .map(|name| name.as_str().to_owned());
                match &export.origin {
                    kiln_hir::ExportOrigin::ReExport { source, imported } => {
                        Some(LinkageDescriptor {
                            kind: LinkageKind::ReExport,
                            exported,
                            source: source.clone(),
                            imported: Some(import_name(imported)),
                        })
                    }
                    kiln_hir::ExportOrigin::Star { source } => Some(LinkageDescriptor {
                        kind: LinkageKind::Star,
                        exported,
                        source: source.clone(),
                        imported: None,
                    }),
                    kiln_hir::ExportOrigin::NamespaceReExport { source } => {
                        Some(LinkageDescriptor {
                            kind: LinkageKind::NamespaceReExport,
                            exported,
                            source: source.clone(),
                            imported: None,
                        })
                    }
                    kiln_hir::ExportOrigin::Local { .. }
                    | kiln_hir::ExportOrigin::DefaultExpression
                    | kiln_hir::ExportOrigin::Unsupported { .. } => None,
                }
            })
            .collect::<Vec<_>>();
        descriptors.sort();
        descriptors.dedup();

        Self::from_descriptors(descriptors)
    }

    fn from_descriptors(descriptors: Vec<LinkageDescriptor>) -> Self {
        let digest = linkage_digest(&descriptors);
        Self {
            descriptors,
            digest,
        }
    }
}

fn import_name(name: &kiln_hir::ImportName) -> String {
    match name {
        kiln_hir::ImportName::Name(name) => name.clone(),
        kiln_hir::ImportName::Default => "default".to_owned(),
        kiln_hir::ImportName::Namespace => "*".to_owned(),
    }
}

fn linkage_digest(descriptors: &[LinkageDescriptor]) -> Digest {
    fn frame(output: &mut Vec<u8>, bytes: &[u8]) {
        output.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        output.extend_from_slice(bytes);
    }

    let mut encoded = Vec::new();
    encoded.extend_from_slice(b"kiln-module-linkage-v1\0");
    encoded.extend_from_slice(&(descriptors.len() as u64).to_le_bytes());
    for descriptor in descriptors {
        encoded.push(match descriptor.kind {
            LinkageKind::ReExport => 1,
            LinkageKind::Star => 2,
            LinkageKind::NamespaceReExport => 3,
        });
        match &descriptor.exported {
            Some(name) => {
                encoded.push(1);
                frame(&mut encoded, name.as_bytes());
            }
            None => encoded.push(0),
        }
        frame(&mut encoded, descriptor.source.as_bytes());
        match &descriptor.imported {
            Some(name) => {
                encoded.push(1);
                frame(&mut encoded, name.as_bytes());
            }
            None => encoded.push(0),
        }
    }
    Digest::hash(&encoded)
}

/// The externally visible surface of a module, including runtime export wiring and function summaries.
///
/// This is the early-cutoff point. A dependent is recomputed only when this value *changes*,
/// which is how design.md §3's "interface hash" is realised without maintaining a second hash
/// by hand - see docs/adr/0003.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Interface {
    pub interface_hash: u64,
    pub exports: Vec<String>,
    pub linkage: Linkage,
    pub summaries_digest: Digest,
    pub summaries: Vec<FunctionSummary>,
}

impl Interface {
    /// Build the canonical set-like interface value used for early cutoff.
    pub fn from_exports(exports: Vec<String>) -> Self {
        Self::from_exports_and_linkage(exports, Linkage::empty())
    }

    /// Build an interface from declared runtime exports and normalized runtime wiring.
    pub fn from_exports_and_linkage(exports: Vec<String>, linkage: Linkage) -> Self {
        Self::from_exports_linkage_and_summaries(exports, linkage, Vec::new())
    }

    /// Build an interface from declared runtime exports, normalized runtime wiring, and function summaries.
    pub fn from_exports_linkage_and_summaries(
        mut exports: Vec<String>,
        linkage: Linkage,
        summaries: Vec<FunctionSummary>,
    ) -> Self {
        exports.sort();
        exports.dedup();
        let summaries_digest = summaries_digest(&summaries);
        let names_hash = hash::hash_names(&exports);
        let interface_hash =
            if linkage.descriptors.is_empty() && summaries_digest == Digest::default() {
                // Preserve the long-standing wire spelling for modules without re-export wiring or summaries.
                names_hash
            } else {
                let mut summary = hash::Fnv1a::new();
                summary.write(b"kiln-interface-with-linkage-and-summaries-v1\0");
                summary.write_u64(names_hash);
                summary.write(linkage.digest.as_bytes());
                summary.write(summaries_digest.as_bytes());
                summary.finish()
            };
        Self {
            interface_hash,
            exports,
            linkage,
            summaries_digest,
            summaries,
        }
    }

    /// Build the complete interface value retained by the database from parsed HIR.
    pub fn from_module(module: &HirModule) -> Self {
        Self::from_exports_and_linkage(module.runtime_export_names(), Linkage::from_module(module))
    }

    /// Build the complete interface value from parsed HIR and extracted function summaries.
    pub fn from_module_and_summaries(module: &HirModule, summaries: Vec<FunctionSummary>) -> Self {
        Self::from_exports_linkage_and_summaries(
            module.runtime_export_names(),
            Linkage::from_module(module),
            summaries,
        )
    }
}

/// Whole-project rollup. Its identity includes every active module's logical path, interface, and
/// runtime export wiring, so a body-only edit that changes none of those does not recompute it.
/// The interface hash is an opaque deterministic summary for diagnostics and transport, not an
/// artifact-cache authority.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Project {
    pub modules: usize,
    pub exports: usize,
    /// Collision-resistant equality identity over full logical paths and normalized exports.
    pub identity_digest: Digest,
    /// Compact readable summary retained for diagnostics and protocol compatibility.
    pub interface_hash: u64,
}

impl Project {
    /// Build a deterministic project value from `(logical path, interface)` pairs.
    ///
    /// Input order is deliberately irrelevant. Logical paths must already use the caller's
    /// chosen normalization policy; their exact bytes are module identity. [`Interface`] values
    /// are canonical because external implementations construct them with
    /// [`Interface::from_exports`], [`Interface::from_exports_and_linkage`], or
    /// [`Interface::from_module`].
    pub fn from_interfaces<'a>(
        interfaces: impl IntoIterator<Item = (&'a str, &'a Interface)>,
    ) -> Self {
        Self::from_interfaces_impl(interfaces)
    }

    fn from_interfaces_impl<'a>(
        interfaces: impl IntoIterator<Item = (&'a str, &'a Interface)>,
    ) -> Self {
        const DOMAIN: &[u8] = b"kiln-project-interface-v1\0";

        fn write_len(output: &mut Vec<u8>, len: usize) {
            output.extend_from_slice(&(len as u64).to_le_bytes());
        }

        fn write_framed(output: &mut Vec<u8>, value: &[u8]) {
            write_len(output, value.len());
            output.extend_from_slice(value);
        }

        let mut interfaces: Vec<_> = interfaces.into_iter().collect();
        interfaces.sort_by(|(left_path, left), (right_path, right)| {
            left_path
                .cmp(right_path)
                .then_with(|| left.exports.cmp(&right.exports))
                .then_with(|| left.interface_hash.cmp(&right.interface_hash))
                .then_with(|| left.linkage.cmp(&right.linkage))
                .then_with(|| left.summaries_digest.cmp(&right.summaries_digest))
        });

        let mut summary = hash::Fnv1a::new();
        let mut identity = Vec::new();
        identity.extend_from_slice(DOMAIN);
        write_len(&mut identity, interfaces.len());

        let mut exports = 0;
        for (path, interface) in &interfaces {
            summary.write_u64(path.len() as u64);
            summary.write(path.as_bytes());
            summary.write_u64(interface.interface_hash);
            summary.write(interface.linkage.digest.as_bytes());
            summary.write(interface.summaries_digest.as_bytes());

            write_framed(&mut identity, path.as_bytes());
            write_len(&mut identity, interface.exports.len());
            for export in &interface.exports {
                write_framed(&mut identity, export.as_bytes());
            }
            identity.extend_from_slice(interface.linkage.digest.as_bytes());
            identity.extend_from_slice(interface.summaries_digest.as_bytes());
            write_len(&mut identity, interface.linkage.descriptors.len());
            for descriptor in &interface.linkage.descriptors {
                identity.push(match descriptor.kind {
                    LinkageKind::ReExport => 1,
                    LinkageKind::Star => 2,
                    LinkageKind::NamespaceReExport => 3,
                });
                match &descriptor.exported {
                    Some(name) => {
                        identity.push(1);
                        write_framed(&mut identity, name.as_bytes());
                    }
                    None => identity.push(0),
                }
                write_framed(&mut identity, descriptor.source.as_bytes());
                match &descriptor.imported {
                    Some(name) => {
                        identity.push(1);
                        write_framed(&mut identity, name.as_bytes());
                    }
                    None => identity.push(0),
                }
            }
            exports += interface.exports.len();
        }

        Self {
            modules: interfaces.len(),
            exports,
            identity_digest: Digest::hash(&identity),
            interface_hash: summary.finish(),
        }
    }
}

/// Recompute accounting. Not debug output: the tests and the timing harness assert on it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub revision: u64,
    pub parse_computed: u64,
    pub ir_computed: u64,
    pub interface_computed: u64,
    pub project_computed: u64,
    pub validated: u64,
    pub early_cutoff: u64,
}

/// The compiler's view of incremental state.
///
/// Implementations must guarantee:
/// 1. A query's value is recomputed only if a transitive dependency's value *changed*.
/// 2. Recomputing a query that yields an equal value does not invalidate its dependents.
/// 3. Reads are pure: the same query at the same revision returns an equal value.
pub trait QueryEngine {
    /// Intern a path, creating a [`FileId`] if new.
    fn intern(&mut self, path: &str) -> FileId;

    /// Replace an input.
    ///
    /// Writing byte-identical text is not an edit: the revision does not move, nothing is
    /// invalidated, and no dependent recomputes. A caller may therefore re-send state it is
    /// not certain the engine already holds without paying for it, which is what makes a
    /// stateless client safe to point at a warm daemon.
    fn set_source(&mut self, file: FileId, text: String);

    /// Return the interned files that currently have source text, together with their paths.
    ///
    /// Interning a path does not make it an active source: callers can use this snapshot to
    /// reconcile an authoritative source set and call [`QueryEngine::remove_source`] for paths
    /// that disappeared.
    fn source_files(&self) -> Vec<(FileId, String)>;

    /// Remove a source while retaining its interned [`FileId`] for a later restore.
    ///
    /// Removing an already-absent source is a no-op. Implementations must invalidate queries
    /// that read either this source or the active file set.
    fn remove_source(&mut self, file: FileId);

    fn source(&self, file: FileId) -> Option<&str>;
    /// Revision at which this active source's bytes last changed.
    ///
    /// Returns `None` for an unknown or inactive file. This lets source adapters reconcile two
    /// logical spellings without guessing which editor upload is newer.
    fn source_revision(&self, file: FileId) -> Option<Revision>;

    fn parse(&mut self, file: FileId) -> Parsed;
    fn source_check(&mut self, _file: FileId) -> SourceCheck {
        SourceCheck::default()
    }
    fn ir(&mut self, file: FileId) -> Compiled;
    /// Linkage-aware lowering. Engines may override this to memoize it independently; the
    /// default preserves compatibility for small test/reference engines.
    fn module_ir(&mut self, file: FileId) -> CompiledModule {
        kiln_ir::compile_module(&self.parse(file).module)
    }
    /// Compile module IR using a stable logical identity for private names. The default is
    /// intentionally uncached; memoizing engines should key this query by both file and identity.
    fn module_ir_with_identity(&mut self, file: FileId, identity: &str) -> CompiledModule {
        let mut module = self.parse(file).module.as_ref().clone();
        module.module_id = identity.into();
        kiln_ir::compile_module(&module)
    }
    /// Per-function summaries supplied by the engine's registered analysis query.
    ///
    /// The default reports no analysis, not that the module declares no functions.
    fn function_summaries(&mut self, _file: FileId) -> Vec<FunctionSummary> {
        Vec::new()
    }
    fn interface(&mut self, file: FileId) -> Interface;
    fn project(&mut self) -> Project;

    fn revision(&self) -> Revision;
    fn stats(&self) -> Stats;
    fn reset_stats(&mut self);
}
