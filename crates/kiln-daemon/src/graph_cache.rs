//! The daemon's bounded cache of immutable, entry-rooted module graph snapshots.
//!
//! A graph is prepared from one authoritative source view and then consumed after the daemon
//! state lock has been released.  Keeping this cache separate from the query database is
//! intentional: a query can memoize one module's IR, while a graph additionally captures the
//! entry, resolution policy, traversal order, and link-time export origins.

use kiln_digest::Digest;
use kiln_module_graph::ModuleGraph;
use kiln_proto::ResolutionMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Bump when the in-memory graph snapshot contract changes.  The cache is process-local, but the
/// version keeps key construction explicit and prevents accidental reuse if this ever changes to
/// a longer-lived backing store.
pub(crate) const GRAPH_CACHE_FORMAT: u16 = 1;

/// The exact content and linking context that produced one graph.
///
/// `SourceView` deliberately does not participate. It is a compare-and-swap authority token
/// validated while the daemon lock is held, not content identity; a byte-identical authoritative
/// sync advances the token without changing the graph represented here.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct GraphCacheKey {
    pub(crate) project_root: PathBuf,
    pub(crate) entry: PathBuf,
    pub(crate) revision: u64,
    /// `None` means no pinned resolution snapshot was supplied.  It is deliberately distinct from
    /// the identity of an empty [`ResolutionMap`].
    pub(crate) resolution_identity: Option<Digest>,
    pub(crate) format: u16,
}

impl GraphCacheKey {
    pub(crate) fn new(
        project_root: PathBuf,
        entry: PathBuf,
        revision: u64,
        resolutions: Option<&ResolutionMap>,
    ) -> Self {
        Self {
            project_root,
            entry,
            revision,
            resolution_identity: resolutions.map(ResolutionMap::identity),
            format: GRAPH_CACHE_FORMAT,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct GraphCacheStats {
    pub(crate) hits: u64,
    pub(crate) misses: u64,
    pub(crate) evictions: u64,
    pub(crate) invalidations: u64,
}

#[derive(Debug)]
struct Entry {
    key: GraphCacheKey,
    graph: Arc<ModuleGraph>,
}

/// A deliberately one-entry cache.  The daemon normally serves one project and one active entry;
/// a small LRU can be added after measuring alternating-entry workloads without making this first
/// correctness slice retain several arbitrarily large graphs.
#[derive(Debug, Default)]
pub(crate) struct GraphCache {
    entry: Option<Entry>,
    stats: GraphCacheStats,
}

impl GraphCache {
    pub(crate) fn get_or_build<E>(
        &mut self,
        key: GraphCacheKey,
        build: impl FnOnce() -> Result<ModuleGraph, E>,
    ) -> Result<Arc<ModuleGraph>, E> {
        if let Some(entry) = self.entry.as_ref()
            && entry.key == key
        {
            self.stats.hits += 1;
            return Ok(Arc::clone(&entry.graph));
        }

        self.stats.misses += 1;
        let graph = Arc::new(build()?);
        if self
            .entry
            .replace(Entry {
                key,
                graph: Arc::clone(&graph),
            })
            .is_some()
        {
            self.stats.evictions += 1;
        }
        Ok(graph)
    }

    /// Drop a retained graph after a content mutation or a failed mutation whose effects are
    /// uncertain. Key checks remain the safety net for any future mutation path that forgets to
    /// call this method.
    pub(crate) fn invalidate(&mut self) {
        self.stats.invalidations += 1;
        self.entry = None;
    }

    pub(crate) fn reset_stats(&mut self) {
        self.stats = GraphCacheStats::default();
    }

    pub(crate) fn stats(&self) -> GraphCacheStats {
        self.stats
    }

    pub(crate) fn entries(&self) -> u64 {
        u64::from(self.entry.is_some())
    }

    pub(crate) fn cached_modules(&self) -> u64 {
        self.entry
            .as_ref()
            .map_or(0, |entry| entry.graph.module_count() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiln_proto::{PackageResolution, source_digest};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn graph_fixture() -> (PathBuf, PathBuf, PathBuf, ModuleGraph) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "kiln-graph-cache-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).expect("create graph cache fixture");
        let first = root.join("first.js");
        let second = root.join("second.js");
        std::fs::write(&first, "export const first = 1;\n").expect("write first module");
        std::fs::write(&second, "export const second = 2;\n").expect("write second module");
        let root = std::fs::canonicalize(root).expect("canonical fixture root");
        let first = std::fs::canonicalize(first).expect("canonical first module");
        let second = std::fs::canonicalize(second).expect("canonical second module");
        let graph = ModuleGraph::build(
            &first,
            &root,
            |path: &std::path::Path| std::fs::read_to_string(path),
            frontend_oxc::parse,
        )
        .expect("build fixture graph");
        (root, first, second, graph)
    }

    fn key(
        root: &std::path::Path,
        entry: &std::path::Path,
        revision: u64,
        resolutions: Option<&ResolutionMap>,
    ) -> GraphCacheKey {
        GraphCacheKey::new(
            root.to_path_buf(),
            entry.to_path_buf(),
            revision,
            resolutions,
        )
    }

    #[test]
    fn every_content_dimension_is_part_of_the_cache_key() {
        let (root, first, second, graph) = graph_fixture();
        let package = ResolutionMap::new(
            root.to_string_lossy(),
            vec![PackageResolution {
                importer: first.to_string_lossy().into_owned(),
                specifier: "pkg".into(),
                target: second.to_string_lossy().into_owned(),
                package_json: root.join("package.json").to_string_lossy().into_owned(),
                package_json_hash: source_digest("manifest"),
            }],
        )
        .expect("valid resolution map");
        let base = key(&root, &first, 1, None);
        let mut cache = GraphCache::default();

        assert_eq!(
            cache
                .get_or_build(base.clone(), || Ok::<_, ()>(graph.clone()))
                .unwrap()
                .entry(),
            first
        );
        assert_eq!(
            cache
                .get_or_build(base, || Ok::<_, ()>(graph.clone()))
                .unwrap()
                .entry(),
            first
        );
        assert_eq!(cache.stats().hits, 1);

        for changed in [
            key(&root, &first, 2, None),
            key(&root, &first, 2, Some(&package)),
            key(&root, &second, 2, Some(&package)),
        ] {
            cache
                .get_or_build(changed, || Ok::<_, ()>(graph.clone()))
                .expect("changed authority builds a graph");
        }
        assert_eq!(cache.stats().misses, 4);
        assert_eq!(cache.stats().evictions, 3);
        assert_eq!(cache.entries(), 1);
    }

    #[test]
    fn failed_builds_are_not_cached() {
        let (root, first, _second, graph) = graph_fixture();
        let cache_key = key(&root, &first, 1, None);
        let mut cache = GraphCache::default();
        assert_eq!(
            cache.get_or_build(cache_key.clone(), || Err::<ModuleGraph, _>(
                "compile failure"
            )),
            Err("compile failure")
        );
        cache
            .get_or_build(cache_key, || Ok::<_, &str>(graph))
            .expect("a failed build must not poison the key");
        assert_eq!(cache.stats().misses, 2);
        assert_eq!(cache.stats().evictions, 0);
        assert_eq!(cache.entries(), 1);
    }

    #[test]
    fn one_entry_eviction_does_not_invalidate_an_in_flight_snapshot() {
        let (root, first, second, graph) = graph_fixture();
        let first_key = key(&root, &first, 1, None);
        let second_key = key(&root, &second, 1, None);
        let mut cache = GraphCache::default();
        let in_flight = cache
            .get_or_build(first_key, || Ok::<_, ()>(graph.clone()))
            .expect("first graph");
        cache
            .get_or_build(second_key, || Ok::<_, ()>(graph))
            .expect("second graph");
        assert_eq!(in_flight.entry(), first);
        assert_eq!(cache.stats().evictions, 1);
        assert_eq!(cache.entries(), 1);
    }

    #[test]
    fn reset_stats_retains_the_snapshot() {
        let (root, first, _second, graph) = graph_fixture();
        let cache_key = key(&root, &first, 1, None);
        let mut cache = GraphCache::default();
        cache
            .get_or_build(cache_key, || Ok::<_, ()>(graph))
            .expect("graph");
        cache.reset_stats();
        assert_eq!(cache.stats(), GraphCacheStats::default());
        assert_eq!(cache.entries(), 1);
        assert_eq!(cache.cached_modules(), 1);
    }
}
