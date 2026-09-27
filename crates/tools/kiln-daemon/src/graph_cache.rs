//! The daemon's bounded cache of immutable, entry-rooted module graph snapshots.
//!
//! A graph is prepared from one authoritative source view and then consumed after the daemon
//! state lock has been released. Each entry retains the revisions of only the sources it reached,
//! so unrelated edits do not destroy a warm graph. Keeping this cache separate from the query
//! database is intentional: a query can memoize one module's IR, while a graph additionally
//! captures the entry, resolution policy, traversal order, and link-time export origins.

use kiln_db::{FileId, Revision};
use kiln_digest::Digest;
use kiln_module_graph::ModuleGraph;
use kiln_proto::ResolutionMap;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

/// Bump when the in-memory graph snapshot contract changes.  The cache is process-local, but the
/// version keeps key construction explicit and prevents accidental reuse if this ever changes to
/// a longer-lived backing store.
pub(crate) const GRAPH_CACHE_FORMAT: u16 = 3;

/// Keep several editor/watch entries warm without retaining an unbounded number of snapshots.
pub(crate) const DEFAULT_GRAPH_CACHE_CAPACITY: usize = 8;

/// The stable lookup context for one graph. Reachable content identity lives on the cache entry as
/// exact source revisions, because it differs for every graph under the same global engine state.
///
/// `SourceView` deliberately does not participate. It is a compare-and-swap authority token
/// validated while the daemon lock is held, not content identity; a byte-identical authoritative
/// sync advances the token without changing the graph represented here.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct GraphCacheKey {
    pub(crate) project_root: PathBuf,
    pub(crate) entry: PathBuf,
    /// `None` means no pinned resolution snapshot was supplied.  It is deliberately distinct from
    /// the identity of an empty [`ResolutionMap`].
    pub(crate) resolution_identity: Option<Digest>,
    /// Resolution capability: Node mode admits typed builtin identities; deterministic mode does
    /// not. The same source and package snapshot can therefore never reuse the other mode's graph.
    pub(crate) node: bool,
    pub(crate) format: u16,
}

impl GraphCacheKey {
    pub(crate) fn new(
        project_root: PathBuf,
        entry: PathBuf,
        resolutions: Option<&ResolutionMap>,
        node: bool,
    ) -> Self {
        Self {
            project_root,
            entry,
            resolution_identity: resolutions.map(ResolutionMap::identity),
            node,
            format: GRAPH_CACHE_FORMAT,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct GraphCacheStats {
    pub(crate) hits: u64,
    pub(crate) partial_hits: u64,
    pub(crate) misses: u64,
    pub(crate) evictions: u64,
    pub(crate) invalidations: u64,
}

#[derive(Debug)]
struct Entry {
    key: GraphCacheKey,
    sources: Vec<GraphSourceRevision>,
    graph: Arc<ModuleGraph>,
}

#[derive(Debug)]
pub(crate) enum GraphCacheLookup {
    Hit(Arc<ModuleGraph>),
    Stale {
        graph: Arc<ModuleGraph>,
        changed: Vec<GraphSourceRevision>,
    },
    Miss,
}

/// One reachable source version captured by a graph snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GraphSourceRevision {
    pub(crate) file: FileId,
    pub(crate) revision: Revision,
}

/// A bounded least-recently-used cache. Entries carry the revisions of exactly the sources they
/// reached, so a global engine revision change does not discard an unrelated graph.
#[derive(Debug)]
pub(crate) struct GraphCache {
    capacity: usize,
    entries: VecDeque<Entry>,
    stats: GraphCacheStats,
}

impl Default for GraphCache {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_GRAPH_CACHE_CAPACITY)
    }
}

impl GraphCache {
    fn with_capacity(capacity: usize) -> Self {
        assert!(capacity > 0, "graph cache capacity must be positive");
        Self {
            capacity,
            entries: VecDeque::with_capacity(capacity),
            stats: GraphCacheStats::default(),
        }
    }

    /// Return and promote a graph only when every reachable source still has the captured
    /// revision. A changed reachable source returns the old entry as a patch candidate; the old
    /// entry remains transactional until either a patched or complete replacement builds.
    #[cfg(test)]
    pub(crate) fn get(
        &mut self,
        key: &GraphCacheKey,
        mut current_revision: impl FnMut(FileId) -> Option<Revision>,
    ) -> Option<Arc<ModuleGraph>> {
        match self.lookup(key, &mut current_revision) {
            GraphCacheLookup::Hit(graph) => Some(graph),
            GraphCacheLookup::Stale { .. } => {
                self.stats.misses += 1;
                None
            }
            GraphCacheLookup::Miss => None,
        }
    }

    pub(crate) fn lookup(
        &mut self,
        key: &GraphCacheKey,
        mut current_revision: impl FnMut(FileId) -> Option<Revision>,
    ) -> GraphCacheLookup {
        let Some(index) = self.entries.iter().position(|entry| entry.key == *key) else {
            self.stats.misses += 1;
            return GraphCacheLookup::Miss;
        };
        let entry = &self.entries[index];
        let changed = entry
            .sources
            .iter()
            .filter(|source| current_revision(source.file) != Some(source.revision))
            .copied()
            .collect::<Vec<_>>();
        if changed.is_empty() {
            let entry = self
                .entries
                .remove(index)
                .expect("the located graph cache entry exists");
            let graph = Arc::clone(&entry.graph);
            self.entries.push_back(entry);
            self.stats.hits += 1;
            return GraphCacheLookup::Hit(graph);
        }

        GraphCacheLookup::Stale {
            graph: Arc::clone(&entry.graph),
            changed,
        }
    }

    pub(crate) fn record_partial_hit(&mut self) {
        self.stats.hits += 1;
        self.stats.partial_hits += 1;
    }

    pub(crate) fn record_miss(&mut self) {
        self.stats.misses += 1;
    }

    /// Publish a completely built immutable snapshot and return its shared handle.
    pub(crate) fn insert(
        &mut self,
        key: GraphCacheKey,
        sources: Vec<GraphSourceRevision>,
        graph: ModuleGraph,
    ) -> Arc<ModuleGraph> {
        if let Some(index) = self.entries.iter().position(|entry| entry.key == key) {
            self.entries.remove(index);
            self.stats.evictions += 1;
        }
        while self.entries.len() >= self.capacity {
            self.entries.pop_front();
            self.stats.evictions += 1;
        }
        let graph = Arc::new(graph);
        self.entries.push_back(Entry {
            key,
            sources,
            graph: Arc::clone(&graph),
        });
        graph
    }

    /// Drop every retained graph after a failed mutation whose effects are uncertain. Ordinary
    /// source edits are validated selectively through each entry's reachable-source revisions.
    pub(crate) fn invalidate(&mut self) {
        self.stats.invalidations += 1;
        self.entries.clear();
    }

    pub(crate) fn reset_stats(&mut self) {
        self.stats = GraphCacheStats::default();
    }

    pub(crate) fn stats(&self) -> GraphCacheStats {
        self.stats
    }

    pub(crate) fn entries(&self) -> u64 {
        self.entries.len() as u64
    }

    pub(crate) fn cached_modules(&self) -> u64 {
        self.entries
            .iter()
            .map(|entry| entry.graph.module_count() as u64)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiln_proto::{NodeRequestKind, PackageResolution, source_digest};
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
            kiln_frontend_oxc::parse,
        )
        .expect("build fixture graph");
        (root, first, second, graph)
    }

    fn key(
        root: &std::path::Path,
        entry: &std::path::Path,
        resolutions: Option<&ResolutionMap>,
    ) -> GraphCacheKey {
        GraphCacheKey::new(root.to_path_buf(), entry.to_path_buf(), resolutions, false)
    }

    fn sources() -> Vec<GraphSourceRevision> {
        vec![GraphSourceRevision {
            file: FileId(0),
            revision: Revision(1),
        }]
    }

    fn get_or_insert(
        cache: &mut GraphCache,
        key: GraphCacheKey,
        graph: ModuleGraph,
    ) -> Arc<ModuleGraph> {
        cache
            .get(&key, |_| Some(Revision(1)))
            .unwrap_or_else(|| cache.insert(key, sources(), graph))
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
                condition: NodeRequestKind::Import,
                package_json: Some(root.join("package.json").to_string_lossy().into_owned()),
                package_json_hash: Some(source_digest("manifest")),
            }],
        )
        .expect("valid resolution map");
        let base = key(&root, &first, None);
        let mut cache = GraphCache::default();

        assert_eq!(
            get_or_insert(&mut cache, base.clone(), graph.clone()).entry(),
            first
        );
        assert_eq!(
            get_or_insert(&mut cache, base, graph.clone()).entry(),
            first
        );
        assert_eq!(cache.stats().hits, 1);

        for changed in [
            key(&root, &first, Some(&package)),
            key(&root, &second, Some(&package)),
        ] {
            get_or_insert(&mut cache, changed, graph.clone());
        }
        assert_eq!(cache.stats().misses, 3);
        assert_eq!(cache.stats().evictions, 0);
        assert_eq!(cache.entries(), 3);
    }

    #[test]
    fn node_mode_is_a_separate_graph_cache_key() {
        let (root, first, _second, _graph) = graph_fixture();
        let deterministic = GraphCacheKey::new(root.clone(), first.clone(), None, false);
        let node = GraphCacheKey::new(root, first, None, true);
        assert_ne!(deterministic, node);
    }

    #[test]
    fn a_miss_does_not_publish_an_entry_without_insert() {
        let (root, first, _second, graph) = graph_fixture();
        let cache_key = key(&root, &first, None);
        let mut cache = GraphCache::default();
        assert!(cache.get(&cache_key, |_| Some(Revision(1))).is_none());
        assert_eq!(cache.entries(), 0);
        assert!(cache.get(&cache_key, |_| Some(Revision(1))).is_none());
        cache.insert(cache_key, sources(), graph);
        assert_eq!(cache.stats().misses, 2);
        assert_eq!(cache.stats().evictions, 0);
        assert_eq!(cache.entries(), 1);
    }

    #[test]
    fn bounded_lru_eviction_does_not_invalidate_an_in_flight_snapshot() {
        let (root, first, second, graph) = graph_fixture();
        let first_key = key(&root, &first, None);
        let second_key = key(&root, &second, None);
        let mut cache = GraphCache::with_capacity(1);
        let in_flight = cache.insert(first_key, sources(), graph.clone());
        cache.insert(second_key, sources(), graph);
        assert_eq!(in_flight.entry(), first);
        assert_eq!(cache.stats().evictions, 1);
        assert_eq!(cache.entries(), 1);
    }

    #[test]
    fn a_hit_promotes_the_entry_before_capacity_eviction() {
        let (root, first, second, graph) = graph_fixture();
        let package = ResolutionMap::new(root.to_string_lossy(), Vec::new())
            .expect("valid empty resolution map");
        let first_key = key(&root, &first, None);
        let second_key = key(&root, &second, None);
        let third_key = key(&root, &first, Some(&package));
        let mut cache = GraphCache::with_capacity(2);
        cache.insert(first_key.clone(), sources(), graph.clone());
        cache.insert(second_key.clone(), sources(), graph.clone());

        assert!(cache.get(&first_key, |_| Some(Revision(1))).is_some());
        cache.insert(third_key, sources(), graph);

        assert!(cache.get(&second_key, |_| Some(Revision(1))).is_none());
        assert!(cache.get(&first_key, |_| Some(Revision(1))).is_some());
        assert_eq!(cache.entries(), 2);
        assert_eq!(cache.stats().evictions, 1);
    }

    #[test]
    fn reachable_source_revisions_validate_without_the_global_revision() {
        let (root, first, _second, graph) = graph_fixture();
        let cache_key = key(&root, &first, None);
        let mut cache = GraphCache::default();
        cache.insert(cache_key.clone(), sources(), graph.clone());

        assert!(cache.get(&cache_key, |_| Some(Revision(1))).is_some());
        assert!(cache.get(&cache_key, |_| None).is_none());
        assert!(cache.get(&cache_key, |_| Some(Revision(2))).is_none());
        cache.insert(
            cache_key.clone(),
            vec![GraphSourceRevision {
                file: FileId(0),
                revision: Revision(2),
            }],
            graph,
        );
        assert!(cache.get(&cache_key, |_| Some(Revision(2))).is_some());
        assert_eq!(cache.stats().hits, 2);
        assert_eq!(cache.stats().misses, 2);
        assert_eq!(cache.stats().evictions, 1);
    }

    #[test]
    fn stale_lookup_returns_old_snapshot_for_transactional_patch() {
        let (root, first, _second, graph) = graph_fixture();
        let cache_key = key(&root, &first, None);
        let mut cache = GraphCache::default();
        cache.insert(cache_key.clone(), sources(), graph.clone());

        let GraphCacheLookup::Stale {
            graph: old,
            changed,
        } = cache.lookup(&cache_key, |_| Some(Revision(2)))
        else {
            panic!("changed source should retain the old snapshot for patching");
        };
        assert_eq!(old.entry(), graph.entry());
        assert_eq!(changed, sources());
        cache.record_partial_hit();
        assert_eq!(cache.stats().partial_hits, 1);
        assert_eq!(
            cache.entries(),
            1,
            "a stale lookup must not evict the old snapshot"
        );
    }

    #[test]
    fn reset_stats_retains_the_snapshot() {
        let (root, first, _second, graph) = graph_fixture();
        let cache_key = key(&root, &first, None);
        let mut cache = GraphCache::default();
        cache.insert(cache_key, sources(), graph);
        cache.reset_stats();
        assert_eq!(cache.stats(), GraphCacheStats::default());
        assert_eq!(cache.entries(), 1);
        assert_eq!(cache.cached_modules(), 1);
    }
}
