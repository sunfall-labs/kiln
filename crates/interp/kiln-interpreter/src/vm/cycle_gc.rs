//! Conservative cycle collection for weakly seeded Rc graphs.
//!
//! The interpreter uses `Rc` rather than a tracing heap. Weak closure and WeakRef-target censuses
//! shared by the realm's module map let collection discover object graphs across module VMs
//! without making the census itself a root. For each
//! node, the collector subtracts all counted internal edges and its own census handle from the
//! actual strong count. Any remainder is an external owner, so the node and everything it reaches
//! stay live. Only components with no unexplained references have their mutable edges cleared.

use super::*;
use kiln_graph::{build_function_graph, suspended_local_liveness};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Key {
    Object(usize),
    Closure(usize),
    ModuleGeneration(usize),
    ModuleExports(usize),
    Cell(usize),
    Array(usize),
    Promise(usize),
    Generator(usize),
    AsyncDisposal(usize),
    ModuleSuspension(usize),
    AsyncContext(usize),
    Capability(usize),
    Element(usize),
    CombinatorWalk(usize),
    RegExp(usize),
    Date(usize),
    Temporal(usize),
    TypedArray(usize),
    ArrayBuffer(usize),
    DataView(usize),
    Proxy(usize),
    Bound(usize),
    Wrapped(usize),
    CommonJs(usize),
    UniqueSymbol(usize),
    Shared(usize),
    Intl(usize),
    NodeServer(usize),
    NetSocket(usize),
    HttpIncoming(usize),
    HttpClientRequest(usize),
    Readable(usize),
    Writable(usize),
    Zlib(usize),
}

enum Owner {
    Object(Rc<RefCell<ObjectData>>),
    Closure(Rc<Closure>),
    ModuleGeneration(crate::module_map::ModuleGenerationHandle),
    ModuleExports(Rc<HashMap<String, Value>>),
    Cell(Cell),
    Array(Rc<ArrayData>),
    Promise(Rc<PromiseData>),
    Generator(Rc<GeneratorData>),
    AsyncDisposal(Rc<AsyncDisposal>),
    ModuleSuspension(Rc<RefCell<ModuleSuspension>>),
    AsyncContext(AsyncContext),
    Capability(Rc<CapabilityRecord>),
    Element(Rc<ElementFunction>),
    CombinatorWalk(Rc<CombinatorWalk>),
    RegExp(Rc<RegExpData>),
    Date(Rc<DateData>),
    Temporal(Rc<TemporalData>),
    TypedArray(Rc<TypedArrayData>),
    ArrayBuffer(Rc<BufferData>),
    DataView(Rc<DataViewData>),
    Proxy(Rc<ProxyData>),
    Bound(Rc<BoundFunction>),
    Wrapped(Rc<WrappedFunction>),
    CommonJs(Rc<CommonJsRequire>),
    UniqueSymbol(Rc<UniqueSymbol>),
    Shared(Rc<SharedFacade>),
    Intl(Rc<intl::IntlValue>),
    NodeServer(Rc<RefCell<NodeServerState>>),
    NetSocket(Rc<RefCell<NetSocketState>>),
    HttpIncoming(Rc<RefCell<HttpIncomingState>>),
    HttpClientRequest(Rc<RefCell<HttpClientRequestState>>),
    Readable(Rc<RefCell<ReadableState>>),
    Writable(Rc<RefCell<WritableState>>),
    Zlib(Rc<zlib::ZlibState>),
}

impl Owner {
    fn key(&self) -> Key {
        match self {
            Self::Object(value) => Key::Object(Rc::as_ptr(value) as usize),
            Self::Closure(value) => Key::Closure(Rc::as_ptr(value) as usize),
            Self::ModuleGeneration(value) => Key::ModuleGeneration(Rc::as_ptr(value) as usize),
            Self::ModuleExports(value) => Key::ModuleExports(Rc::as_ptr(value) as usize),
            Self::Cell(value) => Key::Cell(Rc::as_ptr(value) as usize),
            Self::Array(value) => Key::Array(Rc::as_ptr(value) as usize),
            Self::Promise(value) => Key::Promise(Rc::as_ptr(value) as usize),
            Self::Generator(value) => Key::Generator(Rc::as_ptr(value) as usize),
            Self::AsyncDisposal(value) => Key::AsyncDisposal(Rc::as_ptr(value) as usize),
            Self::ModuleSuspension(value) => Key::ModuleSuspension(Rc::as_ptr(value) as usize),
            Self::AsyncContext(value) => Key::AsyncContext(Rc::as_ptr(value) as usize),
            Self::Capability(value) => Key::Capability(Rc::as_ptr(value) as usize),
            Self::Element(value) => Key::Element(Rc::as_ptr(value) as usize),
            Self::CombinatorWalk(value) => Key::CombinatorWalk(Rc::as_ptr(value) as usize),
            Self::RegExp(value) => Key::RegExp(Rc::as_ptr(value) as usize),
            Self::Date(value) => Key::Date(Rc::as_ptr(value) as usize),
            Self::Temporal(value) => Key::Temporal(Rc::as_ptr(value) as usize),
            Self::TypedArray(value) => Key::TypedArray(Rc::as_ptr(value) as usize),
            Self::ArrayBuffer(value) => Key::ArrayBuffer(Rc::as_ptr(value) as usize),
            Self::DataView(value) => Key::DataView(Rc::as_ptr(value) as usize),
            Self::Proxy(value) => Key::Proxy(Rc::as_ptr(value) as usize),
            Self::Bound(value) => Key::Bound(Rc::as_ptr(value) as usize),
            Self::Wrapped(value) => Key::Wrapped(Rc::as_ptr(value) as usize),
            Self::CommonJs(value) => Key::CommonJs(Rc::as_ptr(value) as usize),
            Self::UniqueSymbol(value) => Key::UniqueSymbol(Rc::as_ptr(value) as usize),
            Self::Shared(value) => Key::Shared(Rc::as_ptr(value) as usize),
            Self::Intl(value) => Key::Intl(Rc::as_ptr(value) as usize),
            Self::NodeServer(value) => Key::NodeServer(Rc::as_ptr(value) as usize),
            Self::NetSocket(value) => Key::NetSocket(Rc::as_ptr(value) as usize),
            Self::HttpIncoming(value) => Key::HttpIncoming(Rc::as_ptr(value) as usize),
            Self::HttpClientRequest(value) => Key::HttpClientRequest(Rc::as_ptr(value) as usize),
            Self::Readable(value) => Key::Readable(Rc::as_ptr(value) as usize),
            Self::Writable(value) => Key::Writable(Rc::as_ptr(value) as usize),
            Self::Zlib(value) => Key::Zlib(Rc::as_ptr(value) as usize),
        }
    }

    fn strong_count(&self) -> usize {
        match self {
            Self::Object(value) => Rc::strong_count(value),
            Self::Closure(value) => Rc::strong_count(value),
            Self::ModuleGeneration(value) => Rc::strong_count(value),
            Self::ModuleExports(value) => Rc::strong_count(value),
            Self::Cell(value) => Rc::strong_count(value),
            Self::Array(value) => Rc::strong_count(value),
            Self::Promise(value) => Rc::strong_count(value),
            Self::Generator(value) => Rc::strong_count(value),
            Self::AsyncDisposal(value) => Rc::strong_count(value),
            Self::ModuleSuspension(value) => Rc::strong_count(value),
            Self::AsyncContext(value) => Rc::strong_count(value),
            Self::Capability(value) => Rc::strong_count(value),
            Self::Element(value) => Rc::strong_count(value),
            Self::CombinatorWalk(value) => Rc::strong_count(value),
            Self::RegExp(value) => Rc::strong_count(value),
            Self::Date(value) => Rc::strong_count(value),
            Self::Temporal(value) => Rc::strong_count(value),
            Self::TypedArray(value) => Rc::strong_count(value),
            Self::ArrayBuffer(value) => Rc::strong_count(value),
            Self::DataView(value) => Rc::strong_count(value),
            Self::Proxy(value) => Rc::strong_count(value),
            Self::Bound(value) => Rc::strong_count(value),
            Self::Wrapped(value) => Rc::strong_count(value),
            Self::CommonJs(value) => Rc::strong_count(value),
            Self::UniqueSymbol(value) => Rc::strong_count(value),
            Self::Shared(value) => Rc::strong_count(value),
            Self::Intl(value) => Rc::strong_count(value),
            Self::NodeServer(value) => Rc::strong_count(value),
            Self::NetSocket(value) => Rc::strong_count(value),
            Self::HttpIncoming(value) => Rc::strong_count(value),
            Self::HttpClientRequest(value) => Rc::strong_count(value),
            Self::Readable(value) => Rc::strong_count(value),
            Self::Writable(value) => Rc::strong_count(value),
            Self::Zlib(value) => Rc::strong_count(value),
        }
    }

    fn clear_mutable_edges(&self) {
        match self {
            Self::Object(value) => {
                value.replace(ObjectData::default());
            }
            Self::Closure(value) => {
                value.prototype.replace(None);
                value.properties.replace(ObjectData::default());
                value.module_generation_owner.replace(None);
            }
            Self::ModuleGeneration(value) => value.borrow_mut().records.clear(),
            Self::Cell(value) => {
                value.replace(Value::Undefined);
            }
            Self::Array(value) => {
                value.items.borrow_mut().clear();
                value.elements.borrow_mut().clear();
                value.properties.replace(ObjectData::default());
            }
            Self::Promise(value) => {
                let mut state = value.state.borrow_mut();
                state.settled = None;
                state.reactions.clear();
                drop(state);
                value.properties.replace(ObjectData::default());
            }
            Self::Generator(value) => {
                let mut state = value.state.borrow_mut();
                state.locals.clear();
                state.stack.clear();
                state.arguments.clear();
                state.returning = None;
                state.delegate = None;
                state.async_requests = None;
                drop(state);
                value.properties.replace(ObjectData::default());
            }
            Self::AsyncDisposal(value) => {
                value.pending.borrow_mut().clear();
                value.thrown.borrow_mut().take();
            }
            Self::ModuleSuspension(value) => {
                let mut state = value.borrow_mut();
                state.stack.clear();
                state.handlers.clear();
                state.module_generation = None;
            }
            Self::Capability(value) => {
                value.resolve.replace(Value::Undefined);
                value.reject.replace(Value::Undefined);
            }
            Self::RegExp(value) => {
                value.properties.replace(ObjectData::default());
            }
            Self::Date(value) => {
                value.properties.replace(ObjectData::default());
            }
            Self::Temporal(value) => {
                value.properties.replace(ObjectData::default());
            }
            Self::TypedArray(value) => {
                value.properties.replace(ObjectData::default());
            }
            Self::ArrayBuffer(value) => {
                value.properties.replace(ObjectData::default());
            }
            Self::DataView(value) => {
                value.properties.replace(ObjectData::default());
            }
            Self::Proxy(value) => {
                value.private_elements.replace(ObjectData::default());
            }
            Self::Bound(value) => {
                value.properties.replace(ObjectData::default());
            }
            Self::Wrapped(value) => {
                value.properties.replace(ObjectData::default());
            }
            // Contexts and element functions are immutable sidecars. Their parent edges are
            // cleared when the owning reaction/promise or element is cleared.
            Self::AsyncContext(_) | Self::Element(_) | Self::CombinatorWalk(_) => {}
            Self::UniqueSymbol(_)
            | Self::Shared(_)
            | Self::Intl(_)
            | Self::ModuleExports(_)
            | Self::CommonJs(_)
            | Self::HttpIncoming(_)
            | Self::Zlib(_) => {}
            Self::NodeServer(value) => {
                let mut state = value.borrow_mut();
                state.handler = Value::Undefined;
                state.streams.clear();
            }
            Self::NetSocket(value) => {
                let mut state = value.borrow_mut();
                state.corked_callbacks.clear();
                state.queued_write_callbacks.clear();
                state.end_callbacks.clear();
                state.deferred_destroy_callbacks = None;
                state.cancelled_active_write_error = None;
                state.upgrade_write_callbacks.clear();
            }
            Self::HttpClientRequest(value) => {
                let mut state = value.borrow_mut();
                state.socket = None;
                state.object = None;
            }
            Self::Readable(value) => {
                let mut state = value.borrow_mut();
                state.buffer.clear();
                state.pipes.clear();
            }
            Self::Writable(value) => {
                value.borrow_mut().write = None;
            }
        }
    }
}

#[derive(Default)]
struct Node {
    owner: Option<Owner>,
    edges: Vec<usize>,
}

#[derive(Default)]
struct OwnerGraph {
    nodes: Vec<Node>,
    indices: HashMap<Key, usize>,
    work: Vec<usize>,
}

impl OwnerGraph {
    /// Return the node index for this owner, adding it to the discovery queue when new.
    fn ensure(&mut self, owner: Owner) -> usize {
        let key = owner.key();
        match self.indices.entry(key) {
            std::collections::hash_map::Entry::Occupied(entry) => *entry.get(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let index = self.nodes.len();
                self.nodes.push(Node {
                    owner: Some(owner),
                    edges: Vec::new(),
                });
                entry.insert(index);
                self.work.push(index);
                index
            }
        }
    }

    /// Preserve every physical owner edge, including duplicates, while deduplicating graph nodes.
    fn add_edge(&mut self, parent: usize, owner: Owner) -> usize {
        let child = self.ensure(owner);
        self.nodes[parent].edges.push(child);
        child
    }
}

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// The realm session owns the authoritative module map. A VM's local copy can be a stale
    /// per-machine default when it is serving an operation inside a suspended module graph.
    pub(super) fn gc_module_map(&self) -> SharedModuleMap {
        self.module_realm
            .as_ref()
            .map(|realm| Rc::clone(&realm.borrow().module_map))
            .unwrap_or_else(|| Rc::clone(&self.module_map))
    }

    /// Collect unreachable cycles reachable from this VM's realm-wide weak census.
    pub(super) fn collect_closure_cycles(&mut self) {
        let mut graph = OwnerGraph::default();
        // WeakMap values own their Rc payloads, but only conditionally keep them reachable.
        // Store identities here; the value itself remains owned only by its weak-map entry.
        let mut ephemerons = Vec::<(usize, usize, usize)>::new();

        let module_map = self.gc_module_map();
        // These thread-local registries are weak-only. Rc interpreter values cannot cross
        // threads, so one collector pass can discover disconnected cycles from every realm on
        // this thread; another live realm's references remain external owners and stay live.
        let allocated_objects = crate::gc_census::object_candidates();
        let allocated_arrays = crate::gc_census::array_candidates();
        let allocated_proxies = crate::gc_census::proxy_candidates();
        let (
            shared_closures,
            shared_generators,
            shared_weak_objects,
            shared_weak_collections,
            shared_generations,
        ) = {
            let map = module_map.borrow();
            (
                map.gc_closures.clone(),
                map.gc_generators.clone(),
                map.gc_weak_ref_objects.clone(),
                map.gc_weak_collections.clone(),
                map.weak_generation_census(),
            )
        };
        for weak in &shared_closures {
            if let Some(closure) = weak.upgrade() {
                graph.ensure(Owner::Closure(closure));
            }
        }

        for weak in &shared_generators {
            if let Some(generator) = weak.upgrade() {
                graph.ensure(Owner::Generator(generator));
            }
        }

        // WeakRef targets are another non-rooting way for the VM to know an object's identity.
        // This catches object-only cycles that have no function edge at all.
        for weak in &shared_weak_objects {
            if let Some(object) = weak.upgrade() {
                graph.ensure(Owner::Object(object));
            }
        }

        for weak in &allocated_objects {
            if let Some(object) = weak.upgrade() {
                graph.ensure(Owner::Object(object));
            }
        }
        for weak in &allocated_arrays {
            if let Some(array) = weak.upgrade() {
                graph.ensure(Owner::Array(array));
            }
        }
        for weak in &allocated_proxies {
            if let Some(proxy) = weak.upgrade() {
                graph.ensure(Owner::Proxy(proxy));
            }
        }
        for weak in &shared_weak_collections {
            if let Some(object) = weak.upgrade() {
                graph.ensure(Owner::Object(object));
            }
        }
        for weak in &shared_generations {
            if let Some(generation) = weak.upgrade() {
                graph.ensure(Owner::ModuleGeneration(generation));
            }
        }

        while let Some(parent) = graph.work.pop() {
            if let Some(Node {
                owner: Some(Owner::Generator(generator)),
                ..
            }) = graph.nodes.get(parent)
            {
                self.prune_suspended_generator_locals(generator);
            }
            let weak_entries = graph
                .nodes
                .get(parent)
                .and_then(|node| match node.owner.as_ref() {
                    Some(Owner::Object(object)) => {
                        object.borrow().weak_collection_entries.as_deref().cloned()
                    }
                    _ => None,
                });
            for (weak_key, entry_value) in weak_entries.into_iter().flatten() {
                let Some(key_value) = weak_key.upgrade() else {
                    continue;
                };
                let Some(key_owner) = owner_for_value(&key_value) else {
                    // Unknown exotic identities are conservatively kept live until their
                    // representation has an owner-graph node of its own.
                    continue;
                };
                let key = graph.ensure(key_owner);
                if let Some(value_owner) = owner_for_value(&entry_value) {
                    let value = graph.ensure(value_owner);
                    ephemerons.push((parent, key, value));
                }
            }
            let owner = graph
                .nodes
                .get(parent)
                .and_then(|node| node.owner.as_ref())
                .expect("work node exists");
            let children = owner_children(owner);
            for child_owner in owner_sidecar_children(owner) {
                graph.add_edge(parent, child_owner);
            }
            for child in children {
                if let Some(owner) = owner_for_value(&child) {
                    graph.add_edge(parent, owner);
                }
            }
        }

        let mut incoming = vec![0usize; graph.nodes.len()];
        for node in &graph.nodes {
            for edge in &node.edges {
                incoming[*edge] += 1;
            }
        }
        // Count the real Rc owner in every weak-map value even before its ephemeron activates.
        // This is ownership accounting only; it does not make the value reachable.
        for (_, _, value) in &ephemerons {
            incoming[*value] += 1;
        }

        // Nodes whose counts exceed the collector's one owner plus all incoming edges have an
        // owner outside this discovered graph. Unknown or unaccounted owners are always live.
        let mut live = vec![false; graph.nodes.len()];
        let mut mark = Vec::new();
        for (index, node) in graph.nodes.iter().enumerate() {
            let Some(owner) = node.owner.as_ref() else {
                continue;
            };
            let accounted = incoming[index].saturating_add(1);
            if owner.strong_count() > accounted {
                live[index] = true;
                mark.push(index);
            }
        }
        while let Some(index) = mark.pop() {
            for edge in &graph.nodes[index].edges {
                if !live[*edge] {
                    live[*edge] = true;
                    mark.push(*edge);
                }
            }
        }
        // Ephemerons can form chains: a live key can make a value live, and that value can be a
        // key in another map. Iterate to a fixed point while propagating ordinary strong edges.
        loop {
            let mut activated = false;
            for (map, key, value) in &ephemerons {
                if live[*map] && live[*key] && !live[*value] {
                    live[*value] = true;
                    mark.push(*value);
                    activated = true;
                }
            }
            while let Some(index) = mark.pop() {
                for edge in &graph.nodes[index].edges {
                    if !live[*edge] {
                        live[*edge] = true;
                        mark.push(*edge);
                    }
                }
            }
            if !activated {
                break;
            }
        }
        let live_candidate_count = live.iter().filter(|is_live| **is_live).count();

        // Break only proven-unreachable nodes. WeakRef target upgrades happen after this method
        // returns, so a reclaimed target is absent on the same explicit collection request.
        for (index, node) in graph.nodes.iter().enumerate() {
            if !live[index]
                && let Some(owner) = node.owner.as_ref()
            {
                owner.clear_mutable_edges();
            }
        }
        // Drop entries whose weak key is dead. This releases their conditional values without
        // affecting entries in a map that is itself unreachable (which is cleared above).
        for (index, node) in graph.nodes.iter().enumerate() {
            if live[index]
                && let Some(Owner::Object(object)) = node.owner.as_ref()
            {
                let mut data = object.borrow_mut();
                if let Some(entries) = data.weak_collection_entries.as_mut() {
                    entries.retain(|weak_key, _entry_value| {
                        weak_key.upgrade().is_some_and(|value| {
                            owner_for_value(&value).is_none_or(|owner| {
                                graph
                                    .indices
                                    .get(&owner.key())
                                    .is_some_and(|index| live[*index])
                            })
                        })
                    });
                }
            }
        }
        // Drop the collector's temporary strong owners before checking which weak census entries
        // expired during the edge-clearing pass.
        drop(graph);
        crate::gc_census::prune_dead_candidates();
        self.closures.retain(|closure| closure.strong_count() > 0);
        self.closures_prune_at = self
            .closures
            .len()
            .saturating_add(self.closures.len().max(1024));
        let mut map = module_map.borrow_mut();
        map.gc_closures.retain(|closure| closure.strong_count() > 0);
        map.gc_generators
            .retain(|generator| generator.strong_count() > 0);
        map.gc_weak_ref_objects
            .retain(|object| object.strong_count() > 0);
        map.gc_weak_collections
            .retain(|collection| collection.strong_count() > 0);
        map.gc_weak_collections_prune_at = map
            .gc_weak_collections
            .len()
            .saturating_add(map.gc_weak_collections.len().max(1024));
        map.gc_closures_prune_at = map
            .gc_closures
            .len()
            .saturating_add(map.gc_closures.len().max(1024));
        map.gc_generators_prune_at = map
            .gc_generators
            .len()
            .saturating_add(map.gc_generators.len().max(1024));
        map.gc_weak_ref_objects_prune_at = map
            .gc_weak_ref_objects
            .len()
            .saturating_add(map.gc_weak_ref_objects.len().max(1024));
        map.prune_dead_generations();
        map.gc_cycle_candidate_allocations = 0;
        map.gc_cycle_candidate_threshold = live_candidate_count.max(4096);
        drop(map);
        drop(module_map);
        crate::gc_census::collection_completed(live_candidate_count);
    }

    /// Drop only physical locals proven dead at this suspended resume point. Captured cells
    /// remain in the frame and are kept live by the shared CFG analysis. Running generators
    /// still have an authoritative frame on the VM stack, so their saved state is untouched.
    fn prune_suspended_generator_locals(&mut self, generator: &Rc<GeneratorData>) {
        let (program, function, instruction, running, started, done) = {
            let state = generator.state.borrow();
            (
                Rc::clone(&state.closure.program),
                state.closure.function,
                state.instruction,
                state.running,
                state.started,
                state.done,
            )
        };
        if running {
            return;
        }
        if done {
            return;
        }
        if !started {
            return;
        }
        let proto = match program.functions.get(function as usize) {
            Some(proto) => proto,
            None => return,
        };
        let program_key = Rc::as_ptr(&program) as usize;
        let key = (program_key, function);
        let module_map = self.gc_module_map();
        let cached = {
            let map = module_map.borrow();
            map.gc_suspended_locals.get(&key).and_then(|entry| {
                entry
                    .program
                    .upgrade()
                    .filter(|cached| Rc::ptr_eq(cached, &program))
                    .map(|_| Rc::clone(&entry.by_yield_pc))
            })
        };
        let by_yield_pc = if let Some(cached) = cached {
            cached
        } else {
            let Ok(graph) = build_function_graph(proto, &program) else {
                return;
            };
            let live_by_block = suspended_local_liveness(&graph, proto, &program);
            let mut live_by_pc = HashMap::new();
            for block in &graph.blocks {
                if let kiln_graph::Terminator::Suspend { pc, .. } = &block.terminator
                    && let Some(live) = live_by_block.get(&block.id)
                {
                    live_by_pc.insert(*pc as usize, live.clone());
                }
            }
            if live_by_pc.is_empty() {
                return;
            }
            let live_by_pc = Rc::new(live_by_pc);
            let mut map = module_map.borrow_mut();
            if map.gc_suspended_locals.len() >= map.gc_suspended_locals_prune_at.max(64) {
                map.gc_suspended_locals
                    .retain(|_, entry| entry.program.strong_count() > 0);
                map.gc_suspended_locals_prune_at = map
                    .gc_suspended_locals
                    .len()
                    .saturating_add(map.gc_suspended_locals.len().max(64));
            }
            map.gc_suspended_locals.insert(
                key,
                crate::module_map::SuspendedLocalCache {
                    program: Rc::downgrade(&program),
                    by_yield_pc: Rc::clone(&live_by_pc),
                },
            );
            live_by_pc
        };

        // The saved instruction points after Yield/Await; CFG suspension keys identify the
        // instruction that yielded. Keep unknown conventions conservatively.
        let Some(live) = instruction
            .checked_sub(1)
            .and_then(|yield_pc| by_yield_pc.get(&yield_pc))
        else {
            return;
        };
        let mut state = generator.state.borrow_mut();
        for (slot, value) in state.locals.iter_mut().enumerate() {
            if !live.contains(&(slot as u32)) {
                *value = Value::Undefined;
            }
        }
    }
}

fn owner_for_value(value: &Value) -> Option<Owner> {
    Some(match value {
        Value::Object(value) => Owner::Object(Rc::clone(value)),
        Value::Function(value) => Owner::Closure(Rc::clone(value)),
        Value::Cell(value) => Owner::Cell(Rc::clone(value)),
        Value::Array(value) => Owner::Array(Rc::clone(value)),
        Value::Promise(value) => Owner::Promise(Rc::clone(value)),
        Value::Generator(value) => Owner::Generator(Rc::clone(value)),
        Value::Settler(value, _) => Owner::Promise(Rc::clone(value)),
        Value::Capability(value) => Owner::Capability(Rc::clone(value)),
        Value::Element(value) => Owner::Element(Rc::clone(value)),
        Value::Shared(value) => Owner::Shared(Rc::clone(value)),
        Value::RegExp(value) => Owner::RegExp(Rc::clone(value)),
        Value::Date(value) => Owner::Date(Rc::clone(value)),
        Value::Temporal(value) => Owner::Temporal(Rc::clone(value)),
        Value::Intl(value) => Owner::Intl(Rc::clone(value)),
        Value::TypedArray(value) => Owner::TypedArray(Rc::clone(value)),
        Value::ArrayBuffer(value) => Owner::ArrayBuffer(Rc::clone(value)),
        Value::DataView(value) => Owner::DataView(Rc::clone(value)),
        Value::Proxy(value) => Owner::Proxy(Rc::clone(value)),
        Value::Bound(value) => Owner::Bound(Rc::clone(value)),
        Value::Wrapped(value) => Owner::Wrapped(Rc::clone(value)),
        Value::CommonJsRequire(value) | Value::CommonJsResolve(value) => {
            Owner::CommonJs(Rc::clone(value))
        }
        Value::Symbol(Symbol::Unique(value)) => Owner::UniqueSymbol(Rc::clone(value)),
        _ => return None,
    })
}

fn owner_children(owner: &Owner) -> Vec<Value> {
    let mut values = Vec::new();
    match owner {
        Owner::Object(value) => values.extend(object_children(&value.borrow())),
        Owner::Closure(value) => {
            for cell in &value.upvalues {
                values.push(Value::Cell(Rc::clone(cell)));
            }
            values.extend(value.captured_arguments.iter().cloned());
            values.extend(value.captured_this.iter().cloned());
            values.extend(value.captured_new_target.iter().cloned());
            values.extend(value.prototype.borrow().iter().cloned());
            values.extend(object_children(&value.properties.borrow()));
        }
        Owner::ModuleGeneration(value) => {
            for record in value.borrow().records.values() {
                if let ModuleStatus::Failed(failure) = &record.status {
                    values.push(failure.clone());
                }
                values.extend(record.bindings.values().cloned().map(Value::Cell));
                values.extend(record.namespace.iter().cloned().map(Value::Object));
                values.extend(
                    record
                        .pending
                        .iter()
                        .map(|(_, cell)| Value::Cell(Rc::clone(cell))),
                );
                values.extend(record.commonjs_namespace_failure.iter().cloned());
                values.extend(record.completion.iter().cloned().map(Value::Promise));
            }
        }
        Owner::ModuleExports(exports) => values.extend(exports.values().cloned()),
        Owner::Cell(value) => values.push(value.borrow().clone()),
        Owner::Array(value) => {
            values.extend(value.items.borrow().iter().cloned());
            // Large Map/Set backing arrays keep a duplicate key in this lookup cache. It is a
            // real strong Value edge even though the canonical collection entry is also in
            // `items`; account for both owners so the cache does not look like an external root.
            if let Some(index) = value.collection_index.borrow().as_ref() {
                values.extend(index.keys().map(|key| key.0.clone()));
            }
            for element in value.elements.borrow().iter() {
                match &element.property {
                    Property::Value(value) => values.push(value.clone()),
                    Property::Accessor { get, set } => {
                        values.extend(get.iter().chain(set.iter()).cloned())
                    }
                }
            }
            values.extend(object_children(&value.properties.borrow()));
        }
        Owner::Promise(value) => {
            let state = value.state.borrow();
            if let Some(outcome) = &state.settled {
                values.push(match outcome {
                    Settled::Fulfilled(value) | Settled::Rejected(value) => value.clone(),
                });
            }
            for reaction in &state.reactions {
                values.push(reaction.on_fulfilled.clone());
                values.push(reaction.on_rejected.clone());
                match &reaction.sink {
                    Sink::Promise(promise) => values.push(Value::Promise(Rc::clone(promise))),
                    Sink::ImportRetry(retry) => {
                        values.push(Value::Promise(Rc::clone(&retry.promise)));
                    }
                    Sink::ModulePlanWake(waiter) => {
                        values.push(Value::Promise(Rc::clone(waiter)));
                    }
                    Sink::Replay(promise, outcome) => {
                        values.push(Value::Promise(Rc::clone(promise)));
                        values.push(match outcome {
                            Settled::Fulfilled(value) | Settled::Rejected(value) => value.clone(),
                        });
                    }
                    Sink::Async(generator, promise)
                    | Sink::AsyncGenerator(generator, promise)
                    | Sink::AsyncGeneratorYield(generator, promise) => {
                        values.push(Value::Generator(Rc::clone(generator)));
                        values.push(Value::Promise(Rc::clone(promise)));
                    }
                    // These sinks point through shared Rc containers. Until those containers
                    // have identities in the owner graph, their child refs are deliberately
                    // omitted; the resulting unexplained strong refs conservatively retain the
                    // children instead of counting one shared edge once per reaction.
                    Sink::AsyncDisposal(_) | Sink::ModuleAwait(_) => {}
                    Sink::AsyncGeneratorReturn(generator, promise)
                    | Sink::AsyncGeneratorReturnActive(generator, promise) => {
                        values.push(Value::Generator(Rc::clone(generator)));
                        values.push(Value::Promise(Rc::clone(promise)))
                    }
                    Sink::AsyncGeneratorDelegateReturnMissing(generator, promise)
                    | Sink::AsyncGeneratorDelegateClose(generator, promise) => {
                        values.push(Value::Generator(Rc::clone(generator)));
                        values.push(Value::Promise(Rc::clone(promise)));
                    }
                    Sink::AsyncGeneratorDelegateReturn(generator, promise, _)
                    | Sink::AsyncGeneratorDelegateThrow(generator, promise, _, _) => {
                        values.push(Value::Generator(Rc::clone(generator)));
                        values.push(Value::Promise(Rc::clone(promise)));
                    }
                    Sink::AsyncGeneratorDelegateStep {
                        generator, step, ..
                    } => {
                        values.push(Value::Generator(Rc::clone(generator)));
                        values.push(Value::Promise(Rc::clone(step)));
                    }
                }
            }
            drop(state);
            values.extend(object_children(&value.properties.borrow()));
        }
        Owner::Generator(value) => {
            let state = value.state.borrow();
            values.push(Value::Function(Rc::clone(&state.closure)));
            values.extend(state.locals.iter().cloned());
            values.extend(state.stack.iter().cloned());
            values.extend(state.arguments.iter().cloned());
            values.push(state.this_value.clone());
            values.push(state.new_target.clone());
            values.extend(state.returning.iter().cloned());
            values.extend(state.delegate.iter().map(|(value, _, _)| value.clone()));
            if let Some(requests) = &state.async_requests {
                for request in requests.current.iter().chain(requests.pending.iter()) {
                    values.push(request.value.clone());
                    values.push(Value::Promise(Rc::clone(&request.step)));
                }
            }
            drop(state);
            values.extend(object_children(&value.properties.borrow()));
        }
        Owner::AsyncDisposal(value) => {
            values.push(Value::Promise(Rc::clone(&value.result)));
            values.extend(value.pending.borrow().iter().cloned());
            values.extend(value.thrown.borrow().iter().cloned());
        }
        Owner::ModuleSuspension(value) => {
            let state = value.borrow();
            values.extend(state.stack.iter().cloned());
            values.push(Value::Promise(Rc::clone(&state.completion)));
            if let Some(context) = &state.nested {
                values.extend(context.locals.iter().cloned());
                values.extend(
                    context
                        .self_bindings
                        .iter()
                        .map(|(cell, _)| Value::Cell(Rc::clone(cell))),
                );
                for (namespace, exports) in &context.self_namespaces {
                    values.push(Value::Object(Rc::clone(namespace)));
                    values.extend(exports.iter().filter_map(|(_, source)| match source {
                        ModuleSelfExport::Settled(value) => Some(value.clone()),
                        ModuleSelfExport::Slot(_) => None,
                    }));
                }
                values.extend(context.target.reexports.iter().filter_map(|(_, binding)| {
                    match binding {
                        ModuleBinding::Ready(value) => Some(value.clone()),
                        ModuleBinding::Export { .. } | ModuleBinding::Namespace { .. } => None,
                    }
                }));
                if let Some(PrivateWindow::Open(Some(slot))) = &context.target.private_window {
                    match &slot.property {
                        Property::Value(value) => values.push(value.clone()),
                        Property::Accessor { get, set } => {
                            values.extend(get.iter().chain(set.iter()).cloned());
                        }
                    }
                }
            }
        }
        Owner::AsyncContext(value) => {
            values.extend(value.iter().map(|(_, store)| store.clone()));
        }
        Owner::Capability(value) => {
            values.push(value.resolve.borrow().clone());
            values.push(value.reject.borrow().clone());
        }
        Owner::Element(value) => {
            let _ = value;
        }
        Owner::CombinatorWalk(value) => {
            values.extend(value.values.borrow().iter().cloned());
            values.push(value.resolve.clone());
            values.push(value.reject.clone());
        }
        Owner::RegExp(value) => values.extend(object_children(&value.properties.borrow())),
        Owner::Date(value) => values.extend(object_children(&value.properties.borrow())),
        Owner::Temporal(value) => values.extend(object_children(&value.properties.borrow())),
        Owner::TypedArray(value) => {
            values.push(Value::ArrayBuffer(Rc::clone(&value.buffer)));
            values.extend(object_children(&value.properties.borrow()));
        }
        Owner::ArrayBuffer(value) => values.extend(object_children(&value.properties.borrow())),
        Owner::DataView(value) => {
            values.push(Value::ArrayBuffer(Rc::clone(&value.buffer)));
            values.extend(object_children(&value.properties.borrow()));
        }
        Owner::Proxy(value) => {
            values.push(value.target.clone());
            values.push(value.handler.clone());
            values.extend(object_children(&value.private_elements.borrow()));
        }
        Owner::Bound(value) => {
            values.push(value.target.clone());
            values.push(value.this_value.clone());
            values.extend(value.arguments.iter().cloned());
            values.extend(object_children(&value.properties.borrow()));
        }
        Owner::Wrapped(value) => {
            values.push(value.target.clone());
            values.extend(object_children(&value.properties.borrow()));
        }
        Owner::CommonJs(value) => {
            values.extend(
                value
                    .targets
                    .iter()
                    .map(|(_, _, cell)| Value::Cell(Rc::clone(cell))),
            );
        }
        Owner::NodeServer(value) => {
            let state = value.borrow();
            values.push(state.handler.clone());
            for stream in &state.streams {
                values.extend(stream.socket_object.iter().cloned());
                values.extend(stream.request.iter().cloned());
                values.extend(stream.response.iter().cloned());
                values.extend(stream.client_requests.values().cloned());
                values.extend(stream.client_responses.values().cloned());
                values.extend(
                    stream
                        .http_responses
                        .iter()
                        .map(|(_, response)| response.clone()),
                );
            }
        }
        Owner::NetSocket(value) => {
            let state = value.borrow();
            values.extend(state.corked_callbacks.iter().cloned());
            values.extend(
                state
                    .queued_write_callbacks
                    .iter()
                    .flat_map(|callbacks| callbacks.iter().cloned()),
            );
            values.extend(state.end_callbacks.iter().cloned());
            if let Some((write, end)) = &state.deferred_destroy_callbacks {
                values.extend(write.iter().chain(end).cloned());
            }
            values.extend(state.cancelled_active_write_error.iter().cloned());
            values.extend(
                state
                    .upgrade_write_callbacks
                    .values()
                    .flat_map(|callbacks| callbacks.iter().cloned()),
            );
        }
        Owner::HttpIncoming(_) => {}
        Owner::HttpClientRequest(value) => {
            if let Some(object) = &value.borrow().object {
                values.push(Value::Object(Rc::clone(object)));
            }
        }
        Owner::Readable(value) => {
            let state = value.borrow();
            values.extend(state.buffer.iter().cloned());
            values.extend(state.pipes.iter().cloned());
            values.push(Value::Object(Rc::clone(&state.public)));
        }
        Owner::Writable(value) => values.extend(value.borrow().write.iter().cloned()),
        Owner::Zlib(_) => {}
        Owner::UniqueSymbol(_) | Owner::Shared(_) | Owner::Intl(_) => {}
    }
    values
}

/// Rc containers stored inside promise reactions need identities in the owner graph of their
/// own. Without these edges, Values they contain look like unexplained external references and
/// conservatively pin otherwise-unreachable WeakRef targets.
fn owner_sidecar_children(owner: &Owner) -> Vec<Owner> {
    let mut owners = Vec::new();
    match owner {
        Owner::Object(object) => {
            let data = object.borrow();
            if let Some(generation) = &data.module_generation {
                owners.push(Owner::ModuleGeneration(Rc::clone(generation)));
            }
            match data.node_object.as_ref() {
                Some(NodeObject::NodeServer(state)) => {
                    owners.push(Owner::NodeServer(Rc::clone(state)));
                }
                Some(NodeObject::NetSocket(state)) => {
                    owners.push(Owner::NetSocket(Rc::clone(state)));
                }
                Some(NodeObject::HttpIncoming(state)) => {
                    owners.push(Owner::HttpIncoming(Rc::clone(state)));
                }
                Some(NodeObject::HttpClientRequest(state)) => {
                    owners.push(Owner::HttpClientRequest(Rc::clone(state)));
                }
                Some(NodeObject::Readable(state)) => {
                    owners.push(Owner::Readable(Rc::clone(state)));
                }
                Some(NodeObject::Writable(state)) => {
                    owners.push(Owner::Writable(Rc::clone(state)));
                }
                Some(NodeObject::Zlib(state)) => owners.push(Owner::Zlib(Rc::clone(state))),
                Some(NodeObject::TimerHandle(timer)) => {
                    owners.push(Owner::AsyncContext(Rc::clone(&timer.context)));
                }
                Some(
                    NodeObject::HttpResponse(_)
                    | NodeObject::Url(_)
                    | NodeObject::UrlSearchParams(_)
                    | NodeObject::Hash(_)
                    | NodeObject::CryptoKey(_)
                    | NodeObject::StringDecoder(_)
                    | NodeObject::AsyncLocalStorage(_),
                )
                | None => {}
            }
        }
        Owner::Closure(closure) => {
            if let Some(generation) = closure.module_generation_owner.borrow().as_ref() {
                owners.push(Owner::ModuleGeneration(Rc::clone(generation)));
            }
        }
        Owner::ModuleGeneration(generation) => {
            for record in generation.borrow().records.values() {
                owners.push(Owner::ModuleExports(Rc::clone(&record.exports)));
            }
        }
        Owner::NodeServer(state) => {
            for stream in &state.borrow().streams {
                if let Some(socket) = &stream.socket {
                    owners.push(Owner::NetSocket(Rc::clone(socket)));
                }
            }
        }
        Owner::NetSocket(state) => {
            owners.push(Owner::Readable(Rc::clone(&state.borrow().readable)));
        }
        Owner::HttpIncoming(state) => {
            owners.push(Owner::Readable(Rc::clone(&state.borrow().readable)));
        }
        Owner::HttpClientRequest(state) => {
            if let Some(socket) = &state.borrow().socket {
                owners.push(Owner::NetSocket(Rc::clone(socket)));
            }
        }
        Owner::Zlib(state) => {
            owners.push(Owner::Readable(Rc::clone(&state.readable)));
            owners.push(Owner::Writable(Rc::clone(&state.writable)));
        }
        Owner::Promise(value) => {
            for reaction in &value.state.borrow().reactions {
                owners.push(Owner::AsyncContext(Rc::clone(&reaction.context)));
                match &reaction.sink {
                    Sink::ImportRetry(retry) => {
                        owners.push(Owner::ModuleGeneration(Rc::clone(&retry.generation_owner)))
                    }
                    Sink::AsyncDisposal(state) => {
                        owners.push(Owner::AsyncDisposal(Rc::clone(state)));
                    }
                    Sink::ModuleAwait(state) => {
                        owners.push(Owner::ModuleSuspension(Rc::clone(state)));
                    }
                    Sink::ModulePlanWake(_) => {}
                    _ => {}
                }
            }
        }
        Owner::Element(value) => owners.push(Owner::CombinatorWalk(Rc::clone(&value.walk))),
        Owner::ModuleSuspension(value) => {
            if let Some(generation) = &value.borrow().module_generation {
                owners.push(Owner::ModuleGeneration(Rc::clone(generation)));
            }
            if let Some(generation) = value
                .borrow()
                .nested
                .as_ref()
                .and_then(|context| context.module_generation_owner.as_ref())
            {
                owners.push(Owner::ModuleGeneration(Rc::clone(generation)));
            }
        }
        _ => {}
    }
    owners
}

fn object_children(data: &ObjectData) -> Vec<Value> {
    let mut values = Vec::new();
    for slot in &data.properties {
        match &slot.property {
            Property::Value(value) => values.push(value.clone()),
            Property::Accessor { get, set } => values.extend(get.iter().chain(set.iter()).cloned()),
        }
    }
    values.extend(
        data.namespace_bindings
            .iter()
            .flat_map(|bindings| bindings.cells.iter().cloned()),
    );
    values.extend(data.prototype.iter().cloned());
    values.extend(data.primitive.iter().cloned());
    values.extend(data.error_stack_frames.iter().cloned());
    if let Some(NodeObject::TimerHandle(timer)) = &data.node_object {
        values.push(timer.callback.clone());
        values.extend(timer.arguments.iter().cloned());
    }
    values.extend(data.node_events.iter().flatten().flat_map(|bucket| {
        bucket
            .listeners
            .iter()
            .map(|listener| listener.callback.clone())
    }));
    for cell in &data.finalization_cells {
        values.push(cell.held.clone());
        // `target` and `token` are weak slots. Upgrading them here would invent strong graph
        // edges and could make the incoming-edge count exceed the actual Rc ownership graph.
        let _ = (&cell.target, &cell.token);
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestHost;

    impl HostOps for TestHost {
        fn call(
            &mut self,
            _operation: HostOp,
            _arguments: &[HostValue],
        ) -> Result<HostValue, kiln_host_abi::HostError> {
            Ok(HostValue::Undefined)
        }
    }

    fn test_vm<'a>(host: &'a mut TestHost) -> (Vm<'a, TestHost>, Rc<Program>) {
        let program = Rc::new(Program {
            source: "".into(),
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
            version: kiln_ir::BYTECODE_VERSION,
            source_origin: None,
            static_source_identity: None,
            static_source_provenance: None,
            constants: Vec::new(),
            code: Vec::new(),
            local_count: 0,
            functions: Vec::new(),
        });
        let vm = Vm::with_entropy_seed(
            Rc::clone(&program),
            Rc::new(Strictness::sloppy(&program)),
            host,
            Vec::new(),
            ExecutionLimits::default(),
            &RealmCapabilities::default(),
            ExecutionCapabilities::default(),
        )
        .expect("test realm initializes");
        (vm, program)
    }

    fn add_allocation_pressure(vm: &Vm<'_, TestHost>) {
        loop {
            let module_map = vm.gc_module_map();
            let module_map = module_map.borrow();
            if crate::gc_census::automatic_collection_due(
                module_map.gc_cycle_candidate_allocations,
                module_map.gc_cycle_candidate_threshold,
            ) {
                return;
            }
            drop(module_map);
            drop(tracked_object(ObjectData::default()));
        }
    }

    fn run_empty_boundary(vm: &mut Vm<'_, TestHost>, program: &Rc<Program>) {
        vm.program = Rc::clone(program);
        vm.instruction = 0;
        vm.run_frames(0).expect("boundary program returns");
    }

    #[test]
    fn automatic_cycle_collection_respects_vm_roots_and_job_keepalive() {
        let mut host = TestHost;
        let (mut vm, _) = test_vm(&mut host);
        vm.locals.push(Value::Undefined);
        let other_module_map = SharedModuleMap::default();
        other_module_map.borrow_mut().gc_cycle_candidate_allocations = 4100;

        let mut program = Program {
            source: "".into(),
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
            version: kiln_ir::BYTECODE_VERSION,
            source_origin: None,
            static_source_identity: None,
            static_source_provenance: None,
            constants: Vec::new(),
            code: vec![Instruction::Nop; 2048],
            local_count: 1,
            functions: Vec::new(),
        };
        program.code.push(Instruction::Return);
        let program = Rc::new(program);

        let rooted_target = tracked_object(ObjectData::default());
        rooted_target
            .borrow_mut()
            .set("self", Value::Object(Rc::clone(&rooted_target)));
        let rooted_weak = Rc::downgrade(&rooted_target);
        let rooted_ref = vm
            .construct_weak_ref(&[Value::Object(Rc::clone(&rooted_target))], 0)
            .expect("WeakRef construction succeeds");
        vm.locals[0] = Value::Object(Rc::clone(&rooted_target));

        let job_target = tracked_object(ObjectData::default());
        let job_weak = Rc::downgrade(&job_target);
        let job_ref = vm
            .construct_weak_ref(&[Value::Object(Rc::clone(&job_target))], 0)
            .expect("WeakRef construction succeeds");
        drop(rooted_target);
        drop(job_target);

        // Candidate allocations make the next outermost opcode boundary collect. The current
        // job's WeakRef keepalive and a value in the active VM frame both remain external roots.
        add_allocation_pressure(&vm);
        run_empty_boundary(&mut vm, &program);
        assert_eq!(
            other_module_map.borrow().gc_cycle_candidate_allocations,
            4100,
            "a pass in one module map must not erase another map's pressure"
        );
        assert!(rooted_weak.upgrade().is_some(), "active VM local is a root");
        assert!(
            job_weak.upgrade().is_some(),
            "same-job WeakRef keepalive is a root"
        );

        // Clearing the job keepalive does not affect the active local. The next pressure pass
        // may reclaim the unrelated acyclic target while preserving the VM-rooted cycle.
        vm.module_map.borrow_mut().weak_ref_keepalive.clear();
        add_allocation_pressure(&vm);
        run_empty_boundary(&mut vm, &program);
        assert!(
            rooted_weak.upgrade().is_some(),
            "active VM local remains rooted"
        );
        assert!(
            job_weak.upgrade().is_none(),
            "target dies after the job keepalive is released"
        );

        vm.locals[0] = Value::Undefined;
        add_allocation_pressure(&vm);
        run_empty_boundary(&mut vm, &program);
        assert!(
            rooted_weak.upgrade().is_none(),
            "cycle dies once its VM root is released"
        );
        let _ = (rooted_ref, job_ref);
    }

    #[test]
    fn duplicate_owner_edges_are_counted_when_clearing_unreachable_objects() {
        let mut host = TestHost;
        let (mut vm, _) = test_vm(&mut host);
        let parent = Rc::new(RefCell::new(ObjectData::default()));
        let target = Rc::new(RefCell::new(ObjectData::default()));
        parent
            .borrow_mut()
            .set("first", Value::Object(Rc::clone(&target)));
        parent
            .borrow_mut()
            .set("second", Value::Object(Rc::clone(&target)));
        let weak_target = vm
            .construct_weak_ref(&[Value::Object(Rc::clone(&target))], 0)
            .unwrap();
        vm.module_map.borrow_mut().weak_ref_keepalive.clear();
        drop(parent);
        drop(target);

        vm.collect_closure_cycles();

        assert_eq!(vm.weak_ref_deref(&weak_target, 0), Ok(Value::Undefined));
    }

    fn generator_program() -> Rc<Program> {
        Rc::new(Program {
            source: "function* g() {}".into(),
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
            version: kiln_ir::BYTECODE_VERSION,
            source_origin: None,
            static_source_identity: None,
            static_source_provenance: None,
            constants: Vec::new(),
            code: Vec::new(),
            local_count: 0,
            functions: vec![kiln_ir::FunctionProto {
                inferred_name: None,
                span: kiln_ir::Span::default(),
                prologue_end: 0,
                asynchronous: false,
                name: None,
                arrow: false,
                generator: true,
                async_generator: false,
                is_method: false,
                is_class: false,
                parameter_count: 0,
                declared_length: 0,
                mapped_arguments: false,
                local_count: 1,
                code: vec![Instruction::Return],
                upvalues: Vec::new(),
                type_annotations: None,
            }],
        })
    }

    fn closure_reaching_promise(program: &Rc<Program>, promise: &Rc<PromiseData>) -> Rc<Closure> {
        Rc::new(Closure {
            program: Rc::clone(program),
            function: 0,
            strictness: Rc::new(Strictness::sloppy(program)),
            module_path: None,
            module_generation: 0,
            module_generation_owner: RefCell::new(None),
            name: None,
            inferred_name: None,
            upvalues: Vec::new(),
            realm: RealmId::PRIMARY,
            arguments_length_only: false,
            captured_arguments: Some(Value::Promise(Rc::clone(promise))),
            captured_new_target: None,
            captured_this: None,
            prototype: RefCell::new(None),
            properties: RefCell::new(ObjectData::default()),
        })
    }

    fn inactive_generation_cycle(
        vm: &mut Vm<'_, TestHost>,
        program: &Rc<Program>,
    ) -> (
        std::rc::Weak<RefCell<crate::module_map::ModuleGeneration>>,
        std::rc::Weak<RefCell<ObjectData>>,
        Value,
        Rc<Closure>,
    ) {
        let realm = RealmId::PRIMARY;
        let generation_id = vm.module_map.borrow_mut().reset_generation(realm);
        let generation = vm.module_map.borrow_mut().current_generation_handle(realm);
        let closure = Rc::new(Closure {
            program: Rc::clone(program),
            function: 0,
            strictness: Rc::new(Strictness::sloppy(program)),
            module_path: Some(Rc::from("/inactive/closure.mjs")),
            module_generation: generation_id,
            module_generation_owner: RefCell::new(Some(Rc::clone(&generation))),
            name: None,
            inferred_name: None,
            upvalues: Vec::new(),
            realm,
            arguments_length_only: false,
            captured_arguments: None,
            captured_new_target: None,
            captured_this: None,
            prototype: RefCell::new(None),
            properties: RefCell::new(ObjectData::default()),
        });
        vm.module_map
            .borrow_mut()
            .gc_closures
            .push(Rc::downgrade(&closure));
        vm.module_map.borrow_mut().publish_in_generation(
            realm,
            generation_id,
            "/inactive/closure.mjs",
            HashMap::from([("keep".to_owned(), Value::Function(Rc::clone(&closure)))]),
        );
        let namespace = vm.module_map.borrow_mut().namespace_in_generation(
            realm,
            generation_id,
            "/inactive/closure.mjs",
        );
        let Value::Object(namespace_object) = &namespace else {
            unreachable!();
        };
        let weak_generation = Rc::downgrade(&generation);
        let weak_namespace = Rc::downgrade(namespace_object);
        // The map no longer roots this epoch after a reset. Its record→namespace→generation
        // and exports→closure→generation cycles are the only remaining strong edges.
        vm.module_map.borrow_mut().reset_generation(realm);
        drop(generation);
        (weak_generation, weak_namespace, namespace, closure)
    }

    #[test]
    fn inactive_module_generation_cycle_is_reclaimed() {
        let mut host = TestHost;
        let (mut vm, program) = test_vm(&mut host);
        let (generation, namespace, external, _closure) =
            inactive_generation_cycle(&mut vm, &program);
        drop(external);
        drop(_closure);
        assert!(generation.upgrade().is_some());
        assert!(namespace.upgrade().is_some());
        vm.collect_closure_cycles();
        assert!(generation.upgrade().is_none());
        assert!(namespace.upgrade().is_none());
    }

    #[test]
    fn external_old_namespace_keeps_its_generation_until_released() {
        let mut host = TestHost;
        let (mut vm, program) = test_vm(&mut host);
        let (generation, namespace, external, _closure) =
            inactive_generation_cycle(&mut vm, &program);
        drop(_closure);
        vm.collect_closure_cycles();
        assert!(generation.upgrade().is_some());
        assert!(namespace.upgrade().is_some());
        let Value::Object(external_namespace) = &external else {
            unreachable!();
        };
        let retained_generation = external_namespace
            .borrow()
            .module_generation
            .as_ref()
            .expect("old namespace owns its generation")
            .borrow()
            .id;
        assert!(
            vm.module_map
                .borrow()
                .status_in_generation(
                    RealmId::PRIMARY,
                    retained_generation,
                    "/inactive/closure.mjs",
                )
                .is_some()
        );
        drop(external);
        vm.collect_closure_cycles();
        assert!(generation.upgrade().is_none());
        assert!(namespace.upgrade().is_none());
    }

    #[test]
    fn retained_old_module_closure_keeps_its_generation_until_released() {
        let mut host = TestHost;
        let (mut vm, program) = test_vm(&mut host);
        let (generation, namespace, external_namespace, external_closure) =
            inactive_generation_cycle(&mut vm, &program);
        drop(external_namespace);
        vm.collect_closure_cycles();
        assert!(generation.upgrade().is_some());
        assert!(namespace.upgrade().is_some());
        let captured = external_closure
            .module_generation_owner
            .borrow()
            .clone()
            .expect("retained closure owns its epoch");
        let imported = vm.module_map.borrow_mut().import_generation_handle_for(
            RealmId::PRIMARY,
            external_closure.module_path.as_deref(),
            &captured,
        );
        assert!(Rc::ptr_eq(&captured, &imported));
        assert!(
            vm.module_map
                .borrow()
                .status_in_generation(
                    RealmId::PRIMARY,
                    captured.borrow().id,
                    "/inactive/closure.mjs",
                )
                .is_some()
        );

        drop(external_closure);
        drop(imported);
        drop(captured);
        vm.collect_closure_cycles();
        assert!(generation.upgrade().is_none());
        assert!(namespace.upgrade().is_none());
    }

    #[test]
    fn queued_old_generation_import_keeps_its_epoch_alive() {
        let mut host = TestHost;
        let (mut vm, _) = test_vm(&mut host);
        let realm = RealmId::PRIMARY;
        let generation_id = vm.module_map.borrow_mut().reset_generation(realm);
        let generation = vm.module_map.borrow_mut().current_generation_handle(realm);
        let weak_generation = Rc::downgrade(&generation);
        let promise = PromiseState::pending();
        let job = vm.scheduled(Job::Import(
            Rc::clone(&promise),
            "./later.mjs".to_owned(),
            None,
            Some(Rc::from("/old/importer.mjs")),
            generation_id,
            Rc::clone(&generation),
        ));
        vm.with_realm_mut(|realm| realm.queue_microtask(job));

        vm.module_map.borrow_mut().reset_generation(realm);
        drop(generation);
        vm.collect_closure_cycles();

        assert!(weak_generation.upgrade().is_some());
    }

    #[test]
    fn pending_module_await_keeps_its_generation_until_settled() {
        let mut host = TestHost;
        let (mut vm, program) = test_vm(&mut host);
        let realm = RealmId::PRIMARY;
        let generation_id = vm.module_map.borrow_mut().reset_generation(realm);
        let generation = vm.module_map.borrow_mut().current_generation_handle(realm);
        assert_eq!(generation.borrow().id, generation_id);
        let weak_generation = Rc::downgrade(&generation);
        let state = Rc::new(RefCell::new(ModuleSuspension {
            stack: Vec::new(),
            handlers: Vec::new(),
            instruction: 0,
            module_generation: Some(Rc::clone(&generation)),
            completion: PromiseState::pending(),
            nested: None,
        }));
        let promise = PromiseState::pending();
        promise.state.borrow_mut().reactions.push(Reaction {
            on_fulfilled: Value::Undefined,
            on_rejected: Value::Undefined,
            sink: Sink::ModuleAwait(Rc::clone(&state)),
            passthrough: false,
            context: Rc::new(Vec::new()),
        });
        let closure = closure_reaching_promise(&program, &promise);
        vm.module_map
            .borrow_mut()
            .gc_closures
            .push(Rc::downgrade(&closure));

        vm.module_map.borrow_mut().reset_generation(realm);
        drop(generation);
        vm.collect_closure_cycles();
        assert!(weak_generation.upgrade().is_some());

        drop(closure);
        drop(promise);
        drop(state);
        vm.collect_closure_cycles();
        assert!(weak_generation.upgrade().is_none());
    }

    fn make_registered_suspended_generator(
        vm: &mut Vm<'_, TestHost>,
        program: &Rc<Program>,
        target: Value,
    ) -> Rc<GeneratorData> {
        let closure = Rc::new(Closure {
            program: Rc::clone(program),
            function: 0,
            strictness: Rc::new(Strictness::sloppy(program)),
            module_path: None,
            module_generation: 0,
            module_generation_owner: RefCell::new(None),
            name: None,
            inferred_name: None,
            upvalues: Vec::new(),
            realm: RealmId::PRIMARY,
            arguments_length_only: false,
            captured_arguments: None,
            captured_new_target: None,
            captured_this: None,
            prototype: RefCell::new(None),
            properties: RefCell::new(ObjectData::default()),
        });
        vm.stack.push(Value::Undefined);
        let Value::Generator(generator) = vm
            .build_generator(&closure, 0, 0, Value::Undefined, Value::Undefined)
            .expect("generator construction succeeds")
        else {
            unreachable!();
        };
        {
            let mut state = generator.state.borrow_mut();
            state.locals[0] = target;
            state.started = true;
            state.instruction = 1;
        }
        generator
            .properties
            .borrow_mut()
            .set("self", Value::Generator(Rc::clone(&generator)));
        generator
    }

    #[test]
    fn generator_census_reclaims_suspended_local_cycles_but_respects_live_iterator() {
        let mut host = TestHost;
        let (mut vm, _) = test_vm(&mut host);
        let program = generator_program();

        let unreachable_target = Rc::new(RefCell::new(ObjectData::default()));
        let unreachable_weak = Rc::downgrade(&unreachable_target);
        let unreachable_generator = make_registered_suspended_generator(
            &mut vm,
            &program,
            Value::Object(Rc::clone(&unreachable_target)),
        );
        let unreachable_generator_weak = Rc::downgrade(&unreachable_generator);
        let unreachable_ref = vm
            .construct_weak_ref(&[Value::Object(Rc::clone(&unreachable_target))], 0)
            .expect("object can be held weakly");
        vm.gc_module_map().borrow_mut().weak_ref_keepalive.clear();
        drop(unreachable_generator);
        drop(unreachable_target);

        vm.collect_closure_cycles();

        assert!(unreachable_generator_weak.upgrade().is_none());
        assert!(unreachable_weak.upgrade().is_none());
        assert_eq!(vm.weak_ref_deref(&unreachable_ref, 0), Ok(Value::Undefined));

        let live_target = Rc::new(RefCell::new(ObjectData::default()));
        let live_weak = Rc::downgrade(&live_target);
        let live_generator = make_registered_suspended_generator(
            &mut vm,
            &program,
            Value::Object(Rc::clone(&live_target)),
        );
        let live_generator_weak = Rc::downgrade(&live_generator);
        let live_ref = vm
            .construct_weak_ref(&[Value::Object(Rc::clone(&live_target))], 0)
            .expect("object can be held weakly");
        vm.gc_module_map().borrow_mut().weak_ref_keepalive.clear();
        drop(live_target);

        vm.collect_closure_cycles();
        assert!(live_generator_weak.upgrade().is_some());
        assert!(live_weak.upgrade().is_some());

        drop(live_generator);
        vm.collect_closure_cycles();
        assert!(live_generator_weak.upgrade().is_none());
        assert!(live_weak.upgrade().is_none());
        assert_eq!(vm.weak_ref_deref(&live_ref, 0), Ok(Value::Undefined));
    }

    #[test]
    fn running_generator_vectors_remain_roots_and_return_to_suspended_state() {
        let mut host = TestHost;
        let (mut vm, base_program) = test_vm(&mut host);
        let program = Rc::new(Program {
            functions: vec![kiln_ir::FunctionProto {
                inferred_name: None,
                span: kiln_ir::Span::default(),
                prologue_end: 0,
                asynchronous: false,
                name: None,
                arrow: false,
                generator: true,
                async_generator: false,
                is_method: false,
                is_class: false,
                parameter_count: 0,
                declared_length: 0,
                mapped_arguments: false,
                local_count: 1,
                code: vec![
                    Instruction::PushHandler(6),
                    Instruction::LoadLocal(0),
                    Instruction::Pop,
                    Instruction::Yield,
                    Instruction::PopHandler,
                    Instruction::Return,
                    Instruction::LoadLocal(0),
                    Instruction::Yield,
                    Instruction::Return,
                ],
                upvalues: Vec::new(),
                type_annotations: None,
            }],
            ..(*base_program).clone()
        });
        let closure = Rc::new(Closure {
            program: Rc::clone(&program),
            function: 0,
            strictness: Rc::new(Strictness::sloppy(&program)),
            module_path: None,
            module_generation: 0,
            module_generation_owner: RefCell::new(None),
            name: None,
            inferred_name: None,
            upvalues: Vec::new(),
            realm: RealmId::PRIMARY,
            arguments_length_only: false,
            captured_arguments: None,
            captured_new_target: None,
            captured_this: None,
            prototype: RefCell::new(None),
            properties: RefCell::new(ObjectData::default()),
        });
        vm.stack.push(Value::Undefined);
        let Value::Generator(generator) = vm
            .build_generator(&closure, 0, 0, Value::Undefined, Value::Undefined)
            .expect("generator construction succeeds")
        else {
            unreachable!();
        };

        let local = tracked_object(ObjectData::default());
        let local_weak = Rc::downgrade(&local);
        let saved_operand = tracked_object(ObjectData::default());
        let saved_operand_weak = Rc::downgrade(&saved_operand);
        {
            let mut state = generator.state.borrow_mut();
            state.locals[0] = Value::Object(Rc::clone(&local));
            state.stack.push(Value::Object(Rc::clone(&saved_operand)));
        }
        drop(local);
        drop(saved_operand);
        vm.stack.push(Value::Number(99.0));

        // The outermost run collects before its first opcode. At that point the saved vectors
        // have been moved into the active VM frame/stack, so both values must still be roots.
        {
            let module_map = vm.gc_module_map();
            let mut module_map = module_map.borrow_mut();
            module_map.gc_cycle_candidate_allocations = 4096;
            module_map.gc_cycle_candidate_threshold = 4096;
        }
        let (yielded, done) = vm
            .resume_generator(&generator, Value::Undefined, 0)
            .expect("generator resumes through its yield");
        assert!(!done);
        assert!(
            local_weak.upgrade().is_some(),
            "active local remains rooted"
        );
        let expected_operand = saved_operand_weak
            .upgrade()
            .expect("saved operand remains rooted during collection");
        let Value::Object(yielded) = yielded else {
            panic!("generator yields its saved operand");
        };
        assert!(Rc::ptr_eq(&yielded, &expected_operand));
        assert_eq!(vm.stack, [Value::Number(99.0)], "caller stack is restored");
        assert_eq!(
            vm.gc_module_map().borrow().gc_cycle_candidate_allocations,
            0,
            "the first outermost opcode boundary performed the requested collection"
        );

        {
            let state = generator.state.borrow();
            assert!(state.started);
            assert!(state.stack.is_empty());
            assert_eq!(
                state.handlers,
                [(6, 0, 1)],
                "handler is moved back as offsets"
            );
            let Value::Object(restored_local) = &state.locals[0] else {
                panic!("active local is moved back into suspended generator state");
            };
            assert!(Rc::ptr_eq(
                restored_local,
                &local_weak
                    .upgrade()
                    .expect("suspended local remains rooted")
            ));
        }

        let (caught_yield, done) = vm
            .resume_generator_with(&generator, Resume::Throw(Value::Number(7.0)), 0)
            .expect("suspended handler is restored and catches the injected throw");
        assert!(!done);
        let Value::Object(caught_yield) = caught_yield else {
            panic!("restored handler resumes at its target");
        };
        assert!(Rc::ptr_eq(
            &caught_yield,
            &local_weak
                .upgrade()
                .expect("local survives handler resumption")
        ));
    }

    #[test]
    fn async_disposal_promise_edges_reclaim_dead_target_and_preserve_external_owner() {
        let mut host = TestHost;
        let (mut vm, program) = test_vm(&mut host);
        let target = Value::Object(Rc::new(RefCell::new(ObjectData::default())));
        let Value::Object(target_object) = &target else {
            unreachable!();
        };
        let weak_target = Rc::downgrade(target_object);
        let keeper = Rc::clone(target_object);
        let state = Rc::new(AsyncDisposal {
            result: PromiseState::pending(),
            pending: RefCell::new(vec![target.clone()]),
            thrown: RefCell::new(None),
            needs_await: std::cell::Cell::new(false),
            has_awaited: std::cell::Cell::new(false),
        });
        let promise = PromiseState::pending();
        promise.state.borrow_mut().reactions.push(Reaction {
            on_fulfilled: Value::Undefined,
            on_rejected: Value::Undefined,
            sink: Sink::AsyncDisposal(Rc::clone(&state)),
            passthrough: false,
            context: Rc::new(Vec::new()),
        });
        let closure = closure_reaching_promise(&program, &promise);
        vm.module_map
            .borrow_mut()
            .gc_closures
            .push(Rc::downgrade(&closure));
        let weak_ref = vm
            .construct_weak_ref(&[target.clone()], 0)
            .expect("object can be held weakly");
        vm.module_map.borrow_mut().weak_ref_keepalive.clear();
        drop(target);
        drop(promise);
        drop(state);
        drop(closure);

        vm.collect_closure_cycles();
        assert!(
            weak_target.upgrade().is_some(),
            "external owner keeps target alive"
        );
        drop(keeper);
        vm.collect_closure_cycles();
        assert!(weak_target.upgrade().is_none());
        assert_eq!(vm.weak_ref_deref(&weak_ref, 0), Ok(Value::Undefined));
    }

    #[test]
    fn array_collection_lookup_cache_is_counted_as_a_strong_edge() {
        let mut host = TestHost;
        let (mut vm, _) = test_vm(&mut host);
        let target = Rc::new(RefCell::new(ObjectData::default()));
        let weak_target = Rc::downgrade(&target);
        let keeper = Rc::clone(&target);
        let mut entries = vec![Value::Object(Rc::clone(&target))];
        entries.extend((0..15).map(|index| Value::Number(index as f64)));
        let backing = ArrayData::new(entries);
        target
            .borrow_mut()
            .set("backing", Value::Array(Rc::clone(&backing)));
        assert_eq!(
            find_collection_entry(&backing, &Value::Object(Rc::clone(&target)), false),
            Some(0)
        );
        let weak_ref = vm
            .construct_weak_ref(&[Value::Object(Rc::clone(&target))], 0)
            .expect("object can be held weakly");
        vm.module_map.borrow_mut().weak_ref_keepalive.clear();
        drop(target);
        drop(backing);

        vm.collect_closure_cycles();

        assert!(
            weak_target.upgrade().is_some(),
            "the cached key remains live while an external owner holds it"
        );
        drop(keeper);
        vm.collect_closure_cycles();

        assert!(
            weak_target.upgrade().is_none(),
            "an unreachable array/index cycle must not be mistaken for an external owner"
        );
        assert_eq!(vm.weak_ref_deref(&weak_ref, 0), Ok(Value::Undefined));
    }

    #[test]
    fn module_await_promise_edges_reclaim_dead_target_and_preserve_external_owner() {
        let mut host = TestHost;
        let (mut vm, program) = test_vm(&mut host);
        let target = Value::Object(Rc::new(RefCell::new(ObjectData::default())));
        let Value::Object(target_object) = &target else {
            unreachable!();
        };
        let weak_target = Rc::downgrade(target_object);
        let keeper = Rc::clone(target_object);
        let state = Rc::new(RefCell::new(ModuleSuspension {
            stack: vec![target.clone()],
            handlers: Vec::new(),
            instruction: 0,
            module_generation: None,
            completion: PromiseState::pending(),
            nested: None,
        }));
        let promise = PromiseState::pending();
        promise.state.borrow_mut().reactions.push(Reaction {
            on_fulfilled: Value::Undefined,
            on_rejected: Value::Undefined,
            sink: Sink::ModuleAwait(Rc::clone(&state)),
            passthrough: false,
            context: Rc::new(Vec::new()),
        });
        let closure = closure_reaching_promise(&program, &promise);
        vm.module_map
            .borrow_mut()
            .gc_closures
            .push(Rc::downgrade(&closure));
        let weak_ref = vm
            .construct_weak_ref(&[target.clone()], 0)
            .expect("object can be held weakly");
        vm.module_map.borrow_mut().weak_ref_keepalive.clear();
        drop(target);
        drop(promise);
        drop(state);
        drop(closure);

        vm.collect_closure_cycles();
        assert!(
            weak_target.upgrade().is_some(),
            "external owner keeps target alive"
        );
        drop(keeper);
        vm.collect_closure_cycles();
        assert!(weak_target.upgrade().is_none());
        assert_eq!(vm.weak_ref_deref(&weak_ref, 0), Ok(Value::Undefined));
    }

    #[test]
    fn event_listener_sidecar_cycle_does_not_keep_weak_target_alive() {
        let mut host = TestHost;
        let (mut vm, _) = test_vm(&mut host);
        let target = Rc::new(RefCell::new(ObjectData::default()));
        target.borrow_mut().node_events = Some(vec![NodeEventBucket {
            name: "tick".into(),
            listeners: vec![NodeEventListener {
                callback: Value::Bound(Rc::new(BoundFunction {
                    target: Value::Undefined,
                    this_value: Value::Object(Rc::clone(&target)),
                    arguments: Vec::new(),
                    properties: RefCell::new(ObjectData::default()),
                })),
                once: false,
            }],
        }]);
        let weak_target = Rc::downgrade(&target);
        let keeper = Rc::clone(&target);
        let weak_ref = vm
            .construct_weak_ref(&[Value::Object(Rc::clone(&target))], 0)
            .expect("object can be held weakly");
        vm.module_map.borrow_mut().weak_ref_keepalive.clear();
        drop(target);

        vm.collect_closure_cycles();
        assert!(
            weak_target.upgrade().is_some(),
            "external owner keeps it alive"
        );
        drop(keeper);
        vm.collect_closure_cycles();
        assert!(weak_target.upgrade().is_none());
        assert_eq!(vm.weak_ref_deref(&weak_ref, 0), Ok(Value::Undefined));
    }

    #[test]
    fn proxy_private_slot_self_cycle_is_traced_and_reclaimed() {
        let mut host = TestHost;
        let (mut vm, _) = test_vm(&mut host);
        let proxy_data = Rc::new(ProxyData {
            target: Value::Object(Rc::new(RefCell::new(ObjectData::default()))),
            handler: Value::Object(Rc::new(RefCell::new(ObjectData::default()))),
            private_elements: RefCell::new(ObjectData::default()),
            revoked: std::cell::Cell::new(false),
        });
        crate::gc_census::register_proxy(&proxy_data);
        let proxy = Value::Proxy(Rc::clone(&proxy_data));
        let weak_target = Rc::downgrade(&proxy_data);
        proxy_data
            .private_elements
            .borrow_mut()
            .set("@@private:cycle:%brand", proxy.clone());

        drop(proxy);
        drop(proxy_data);
        vm.collect_closure_cycles();

        assert!(
            weak_target.upgrade().is_none(),
            "private-slot edges are accounted for and broken on unreachable Proxy cycles"
        );
    }

    #[test]
    fn proxy_private_slot_cycle_survives_external_owner() {
        let mut host = TestHost;
        let (mut vm, _) = test_vm(&mut host);
        let proxy_data = Rc::new(ProxyData {
            target: Value::Object(Rc::new(RefCell::new(ObjectData::default()))),
            handler: Value::Object(Rc::new(RefCell::new(ObjectData::default()))),
            private_elements: RefCell::new(ObjectData::default()),
            revoked: std::cell::Cell::new(false),
        });
        crate::gc_census::register_proxy(&proxy_data);
        let proxy = Value::Proxy(Rc::clone(&proxy_data));
        let weak_target = Rc::downgrade(&proxy_data);
        proxy_data
            .private_elements
            .borrow_mut()
            .set("@@private:cycle:%brand", proxy.clone());
        let external_owner = proxy.clone();

        drop(proxy);
        drop(proxy_data);
        vm.collect_closure_cycles();

        let Value::Proxy(proxy_data) = &external_owner else {
            unreachable!();
        };
        assert!(
            matches!(
                proxy_data
                    .private_elements
                    .borrow()
                    .get_own_property("@@private:cycle:%brand"),
                Some(Property::Value(value)) if value == &external_owner
            ),
            "an external Proxy owner retains the private-slot cycle"
        );
        assert!(weak_target.upgrade().is_some());
    }

    #[test]
    fn active_node_socket_registry_is_a_strong_weakref_root_until_removed() {
        let mut host = TestHost;
        let (mut vm, _) = test_vm(&mut host);
        let realm = ModuleRealmSession::default();
        vm.module_realm = Some(Rc::clone(&realm.shared));
        let object = Rc::new(RefCell::new(ObjectData::default()));
        let weak_object = Rc::downgrade(&object);
        let weak_ref = vm
            .construct_weak_ref(&[Value::Object(Rc::clone(&object))], 0)
            .expect("object can be held weakly");
        vm.gc_module_map().borrow_mut().weak_ref_keepalive.clear();
        realm
            .shared
            .borrow_mut()
            .active_node_socket_objects
            .insert(17, Value::Object(Rc::clone(&object)));
        drop(object);

        vm.collect_closure_cycles();
        assert!(
            weak_object.upgrade().is_some(),
            "active registry roots socket"
        );
        realm
            .shared
            .borrow_mut()
            .active_node_socket_objects
            .remove(&17);
        vm.collect_closure_cycles();
        assert!(weak_object.upgrade().is_none());
        assert_eq!(vm.weak_ref_deref(&weak_ref, 0), Ok(Value::Undefined));
    }

    #[test]
    fn node_object_http_client_sidecar_cycle_respects_external_owners() {
        let mut host = TestHost;
        let (mut vm, _) = test_vm(&mut host);
        let object = Rc::new(RefCell::new(ObjectData::default()));
        let state = Rc::new(RefCell::new(HttpClientRequestState {
            host: "localhost".into(),
            port: 80,
            method: "GET".into(),
            path: "/".into(),
            headers: Vec::new(),
            body: Vec::new(),
            socket: None,
            object: Some(Rc::clone(&object)),
            ended: false,
            started: false,
            end_sent: false,
            cancelled: false,
            request_id: 1,
        }));
        object.borrow_mut().node_object = Some(NodeObject::HttpClientRequest(Rc::clone(&state)));
        let weak_object = Rc::downgrade(&object);
        let keeper = Rc::clone(&object);
        let weak_ref = vm
            .construct_weak_ref(&[Value::Object(Rc::clone(&object))], 0)
            .expect("object can be held weakly");
        vm.module_map.borrow_mut().weak_ref_keepalive.clear();
        drop(object);
        drop(state);

        vm.collect_closure_cycles();
        assert!(
            weak_object.upgrade().is_some(),
            "external owner keeps it alive"
        );
        drop(keeper);
        vm.collect_closure_cycles();
        assert!(weak_object.upgrade().is_none());
        assert_eq!(vm.weak_ref_deref(&weak_ref, 0), Ok(Value::Undefined));
    }

    #[test]
    fn shared_readable_sidecar_is_counted_once_for_two_wrappers() {
        let mut host = TestHost;
        let (mut vm, _) = test_vm(&mut host);
        let target = Rc::new(RefCell::new(ObjectData::default()));
        let weak_target = Rc::downgrade(&target);
        let state = new_readable_state(16, None);
        state
            .borrow_mut()
            .buffer
            .push(Value::Object(Rc::clone(&target)));
        let first = Rc::new(RefCell::new(ObjectData {
            node_object: Some(NodeObject::Readable(Rc::clone(&state))),
            ..Default::default()
        }));
        let second = Rc::new(RefCell::new(ObjectData {
            node_object: Some(NodeObject::Readable(Rc::clone(&state))),
            ..Default::default()
        }));
        let weak_first = Rc::downgrade(&first);
        let weak_second = Rc::downgrade(&second);
        let first_weak_ref = vm
            .construct_weak_ref(&[Value::Object(Rc::clone(&first))], 0)
            .expect("first wrapper can be held weakly");
        let second_weak_ref = vm
            .construct_weak_ref(&[Value::Object(Rc::clone(&second))], 0)
            .expect("second wrapper can be held weakly");
        let target_weak_ref = vm
            .construct_weak_ref(&[Value::Object(Rc::clone(&target))], 0)
            .expect("buffered target can be held weakly");
        vm.module_map.borrow_mut().weak_ref_keepalive.clear();
        let keeper = Rc::clone(&first);
        drop(first);
        drop(second);
        drop(target);
        drop(state);

        vm.collect_closure_cycles();
        assert!(weak_first.upgrade().is_some());
        assert!(weak_second.upgrade().is_none());
        assert!(
            weak_target.upgrade().is_some(),
            "live wrapper keeps shared state live"
        );
        assert_eq!(vm.weak_ref_deref(&second_weak_ref, 0), Ok(Value::Undefined));
        drop(keeper);
        vm.collect_closure_cycles();
        assert!(weak_target.upgrade().is_none());
        assert_eq!(vm.weak_ref_deref(&first_weak_ref, 0), Ok(Value::Undefined));
        assert_eq!(vm.weak_ref_deref(&target_weak_ref, 0), Ok(Value::Undefined));
    }

    #[test]
    fn inactive_timer_handle_traces_callback_arguments_and_context() {
        let mut host = TestHost;
        let (mut vm, _) = test_vm(&mut host);
        let timer = Rc::new(RefCell::new(ObjectData::default()));
        let target = Rc::new(RefCell::new(ObjectData::default()));
        let context = Rc::new(vec![(
            Rc::new(AsyncStorage {
                disabled: std::cell::Cell::new(false),
            }),
            Value::Object(Rc::clone(&target)),
        )]);
        timer.borrow_mut().node_object = Some(NodeObject::TimerHandle(Box::new(TimerRecord {
            id: 1,
            delay: std::time::Duration::ZERO,
            interval: false,
            immediate: false,
            callback: Value::Bound(Rc::new(BoundFunction {
                target: Value::Undefined,
                this_value: Value::Object(Rc::clone(&timer)),
                arguments: Vec::new(),
                properties: RefCell::new(ObjectData::default()),
            })),
            arguments: vec![Value::Object(Rc::clone(&target))],
            context,
            ran: true,
            unreferenced: false,
            cleared: false,
        })));
        let weak_timer = Rc::downgrade(&timer);
        let weak_target = Rc::downgrade(&target);
        let timer_weak_ref = vm
            .construct_weak_ref(&[Value::Object(Rc::clone(&timer))], 0)
            .expect("timer handle can be held weakly");
        let target_weak_ref = vm
            .construct_weak_ref(&[Value::Object(Rc::clone(&target))], 0)
            .expect("context target can be held weakly");
        vm.module_map.borrow_mut().weak_ref_keepalive.clear();
        drop(timer);
        drop(target);

        vm.collect_closure_cycles();
        assert!(weak_timer.upgrade().is_none());
        assert!(weak_target.upgrade().is_none());
        assert_eq!(vm.weak_ref_deref(&timer_weak_ref, 0), Ok(Value::Undefined));
        assert_eq!(vm.weak_ref_deref(&target_weak_ref, 0), Ok(Value::Undefined));
    }

    #[test]
    fn weak_map_ephemerons_collect_value_to_key_cycles_but_follow_live_keys() {
        let mut host = TestHost;
        let (mut vm, _) = test_vm(&mut host);
        let map = Rc::new(RefCell::new(ObjectData::default()));
        map.borrow_mut().weak_collection_entries = Some(Box::default());
        map.borrow_mut().set("@@map", Value::Boolean(true));
        vm.module_map
            .borrow_mut()
            .gc_weak_collections
            .push(Rc::downgrade(&map));
        let key = Rc::new(RefCell::new(ObjectData::default()));
        let value = Rc::new(RefCell::new(ObjectData::default()));
        value
            .borrow_mut()
            .set("key", Value::Object(Rc::clone(&key)));
        map.borrow_mut()
            .weak_collection_entries
            .as_mut()
            .unwrap()
            .insert(
                WeakCollectionKey::Object(Rc::downgrade(&key)),
                Value::Object(Rc::clone(&value)),
            );
        let weak_map = vm
            .construct_weak_ref(&[Value::Object(Rc::clone(&map))], 0)
            .unwrap();
        let weak_key = vm
            .construct_weak_ref(&[Value::Object(Rc::clone(&key))], 0)
            .unwrap();
        let weak_value = vm
            .construct_weak_ref(&[Value::Object(Rc::clone(&value))], 0)
            .unwrap();
        vm.module_map.borrow_mut().weak_ref_keepalive.clear();
        drop(key);
        drop(value);

        vm.collect_closure_cycles();
        assert!(matches!(
            vm.weak_ref_deref(&weak_map, 0),
            Ok(Value::Object(_))
        ));
        assert_eq!(vm.weak_ref_deref(&weak_key, 0), Ok(Value::Undefined));
        assert_eq!(vm.weak_ref_deref(&weak_value, 0), Ok(Value::Undefined));
        assert!(
            map.borrow()
                .weak_collection_entries
                .as_ref()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn weak_map_value_does_not_keep_its_key_if_value_is_externally_held() {
        let mut host = TestHost;
        let (mut vm, _) = test_vm(&mut host);
        let map = Rc::new(RefCell::new(ObjectData::default()));
        map.borrow_mut().weak_collection_entries = Some(Box::default());
        map.borrow_mut().set("@@map", Value::Boolean(true));
        vm.module_map
            .borrow_mut()
            .gc_weak_collections
            .push(Rc::downgrade(&map));
        let key = Rc::new(RefCell::new(ObjectData::default()));
        let value = Rc::new(RefCell::new(ObjectData::default()));
        map.borrow_mut()
            .weak_collection_entries
            .as_mut()
            .unwrap()
            .insert(
                WeakCollectionKey::Object(Rc::downgrade(&key)),
                Value::Object(Rc::clone(&value)),
            );
        let weak_key = vm
            .construct_weak_ref(&[Value::Object(Rc::clone(&key))], 0)
            .unwrap();
        vm.module_map.borrow_mut().weak_ref_keepalive.clear();
        let keeper = Rc::clone(&value);
        drop(key);
        drop(value);

        vm.collect_closure_cycles();
        assert_eq!(vm.weak_ref_deref(&weak_key, 0), Ok(Value::Undefined));
        assert!(
            map.borrow()
                .weak_collection_entries
                .as_ref()
                .unwrap()
                .is_empty()
        );
        assert!(Rc::strong_count(&keeper) >= 1);
    }

    #[test]
    fn weak_map_keeps_value_when_key_is_independently_reachable() {
        let mut host = TestHost;
        let (mut vm, _) = test_vm(&mut host);
        let map = Rc::new(RefCell::new(ObjectData::default()));
        map.borrow_mut().weak_collection_entries = Some(Box::default());
        map.borrow_mut().set("@@map", Value::Boolean(true));
        vm.module_map
            .borrow_mut()
            .gc_weak_collections
            .push(Rc::downgrade(&map));
        let key = Rc::new(RefCell::new(ObjectData::default()));
        let value = Rc::new(RefCell::new(ObjectData::default()));
        map.borrow_mut()
            .weak_collection_entries
            .as_mut()
            .unwrap()
            .insert(
                WeakCollectionKey::Object(Rc::downgrade(&key)),
                Value::Object(Rc::clone(&value)),
            );
        let weak_value = vm
            .construct_weak_ref(&[Value::Object(Rc::clone(&value))], 0)
            .unwrap();
        vm.module_map.borrow_mut().weak_ref_keepalive.clear();
        let keeper = Rc::clone(&key);
        drop(key);
        drop(value);

        vm.collect_closure_cycles();
        assert!(matches!(
            vm.weak_ref_deref(&weak_value, 0),
            Ok(Value::Object(_))
        ));
        assert_eq!(
            map.borrow().weak_collection_entries.as_ref().unwrap().len(),
            1
        );
        drop(keeper);
        vm.module_map.borrow_mut().weak_ref_keepalive.clear();
        vm.collect_closure_cycles();
        assert_eq!(vm.weak_ref_deref(&weak_value, 0), Ok(Value::Undefined));
    }

    #[test]
    fn overlapping_weak_maps_activate_ephemeron_chains() {
        let mut host = TestHost;
        let (mut vm, _) = test_vm(&mut host);
        let first = Rc::new(RefCell::new(ObjectData::default()));
        let second = Rc::new(RefCell::new(ObjectData::default()));
        for map in [&first, &second] {
            map.borrow_mut().weak_collection_entries = Some(Box::default());
            map.borrow_mut().set("@@map", Value::Boolean(true));
            vm.module_map
                .borrow_mut()
                .gc_weak_collections
                .push(Rc::downgrade(map));
        }
        let root_key = Rc::new(RefCell::new(ObjectData::default()));
        let chained_key = Rc::new(RefCell::new(ObjectData::default()));
        let final_value = Rc::new(RefCell::new(ObjectData::default()));
        first
            .borrow_mut()
            .weak_collection_entries
            .as_mut()
            .unwrap()
            .insert(
                WeakCollectionKey::Object(Rc::downgrade(&root_key)),
                Value::Object(Rc::clone(&chained_key)),
            );
        second
            .borrow_mut()
            .weak_collection_entries
            .as_mut()
            .unwrap()
            .insert(
                WeakCollectionKey::Object(Rc::downgrade(&chained_key)),
                Value::Object(Rc::clone(&final_value)),
            );
        let weak_final = vm
            .construct_weak_ref(&[Value::Object(Rc::clone(&final_value))], 0)
            .unwrap();
        vm.module_map.borrow_mut().weak_ref_keepalive.clear();
        let key_keeper = Rc::clone(&root_key);
        drop(root_key);
        drop(chained_key);
        drop(final_value);

        vm.collect_closure_cycles();
        assert!(matches!(
            vm.weak_ref_deref(&weak_final, 0),
            Ok(Value::Object(_))
        ));
        drop(key_keeper);
    }

    #[test]
    fn weak_map_methods_support_closure_keys_and_live_key_operations() {
        let mut host = TestHost;
        let (mut vm, program) = test_vm(&mut host);
        let map = vm
            .construct_collection(Native::WeakMapConstructor, &[], 0)
            .unwrap();
        let key = Value::Function(closure_reaching_promise(&program, &PromiseState::pending()));
        let value = Value::String("held".into());
        let call = |vm: &Vm<'_, TestHost>, method, args: &[Value]| {
            crate::intrinsics::call_native(&vm.intrinsics, method, &map, args)
        };
        assert_eq!(
            call(&vm, Native::WeakMapSet, &[key.clone(), value.clone()]),
            Ok(map.clone())
        );
        assert_eq!(
            call(&vm, Native::WeakMapHas, &[key.clone()]),
            Ok(Value::Boolean(true))
        );
        assert_eq!(call(&vm, Native::WeakMapGet, &[key.clone()]), Ok(value));
        assert_eq!(
            call(&vm, Native::WeakMapDelete, &[key.clone()]),
            Ok(Value::Boolean(true))
        );
        assert_eq!(
            call(&vm, Native::WeakMapHas, &[key]),
            Ok(Value::Boolean(false))
        );
    }

    #[test]
    fn weak_collection_index_preserves_identity_and_reinsert_semantics() {
        let mut host = TestHost;
        let (mut vm, _) = test_vm(&mut host);
        let map = vm
            .construct_collection(Native::WeakMapConstructor, &[], 0)
            .unwrap();
        let Value::Object(map_object) = &map else {
            unreachable!()
        };
        let object_key = Value::Object(Rc::new(RefCell::new(ObjectData::default())));
        let array_key = Value::Array(ArrayData::new(Vec::new()));
        let native_key = Value::Native(Native::ArrayPush, RealmId::PRIMARY);
        let other_native_key = Value::Native(Native::ArrayPop, RealmId::PRIMARY);
        let call = |vm: &Vm<'_, TestHost>, method, args: &[Value]| {
            crate::intrinsics::call_native(&vm.intrinsics, method, &map, args)
        };

        for (key, value) in [
            (&object_key, 11.0),
            (&array_key, 22.0),
            (&native_key, 33.0),
            (&other_native_key, 44.0),
        ] {
            assert_eq!(
                call(
                    &vm,
                    Native::WeakMapSet,
                    &[(*key).clone(), Value::Number(value)]
                ),
                Ok(map.clone())
            );
        }
        assert_eq!(
            map_object
                .borrow()
                .weak_collection_entries
                .as_ref()
                .unwrap()
                .len(),
            4
        );
        for (key, value) in [
            (&object_key, 11.0),
            (&array_key, 22.0),
            (&native_key, 33.0),
            (&other_native_key, 44.0),
        ] {
            assert_eq!(
                call(&vm, Native::WeakMapGet, &[(*key).clone()]),
                Ok(Value::Number(value))
            );
        }
        assert_eq!(
            call(
                &vm,
                Native::WeakMapSet,
                &[object_key.clone(), Value::Number(55.0)]
            ),
            Ok(map.clone())
        );
        assert_eq!(
            map_object
                .borrow()
                .weak_collection_entries
                .as_ref()
                .unwrap()
                .len(),
            4,
            "setting an existing identity must update, not duplicate"
        );
        assert_eq!(
            call(&vm, Native::WeakMapDelete, &[object_key.clone()]),
            Ok(Value::Boolean(true))
        );
        assert_eq!(
            call(&vm, Native::WeakMapDelete, &[object_key.clone()]),
            Ok(Value::Boolean(false))
        );
        assert_eq!(
            call(
                &vm,
                Native::WeakMapSet,
                &[object_key.clone(), Value::Number(66.0)]
            ),
            Ok(map.clone())
        );
        assert_eq!(
            call(&vm, Native::WeakMapGet, &[object_key.clone()]),
            Ok(Value::Number(66.0))
        );

        let set = vm
            .construct_collection(Native::WeakSetConstructor, &[], 0)
            .unwrap();
        assert_eq!(
            crate::intrinsics::call_native(
                &vm.intrinsics,
                Native::WeakSetAdd,
                &set,
                &[array_key.clone()]
            ),
            Ok(set.clone())
        );
        assert_eq!(
            crate::intrinsics::call_native(
                &vm.intrinsics,
                Native::WeakSetHas,
                &set,
                &[array_key.clone()]
            ),
            Ok(Value::Boolean(true))
        );
        assert_eq!(
            crate::intrinsics::call_native(
                &vm.intrinsics,
                Native::WeakSetDelete,
                &set,
                &[array_key.clone()]
            ),
            Ok(Value::Boolean(true))
        );
        assert_eq!(
            crate::intrinsics::call_native(&vm.intrinsics, Native::WeakSetHas, &set, &[array_key]),
            Ok(Value::Boolean(false))
        );
    }

    #[test]
    fn weak_map_closure_key_and_value_cycle_is_collectible() {
        let mut host = TestHost;
        let (mut vm, program) = test_vm(&mut host);
        let map = Rc::new(RefCell::new(ObjectData::default()));
        map.borrow_mut().weak_collection_entries = Some(Box::default());
        map.borrow_mut().set("@@map", Value::Boolean(true));
        vm.module_map
            .borrow_mut()
            .gc_weak_collections
            .push(Rc::downgrade(&map));
        let value = Rc::new(RefCell::new(ObjectData::default()));
        let closure = Rc::new(Closure {
            program: Rc::clone(&program),
            function: 0,
            strictness: Rc::new(Strictness::sloppy(&program)),
            module_path: None,
            module_generation: 0,
            module_generation_owner: RefCell::new(None),
            name: None,
            inferred_name: None,
            upvalues: Vec::new(),
            realm: RealmId::PRIMARY,
            arguments_length_only: false,
            captured_arguments: Some(Value::Object(Rc::clone(&value))),
            captured_new_target: None,
            captured_this: None,
            prototype: RefCell::new(None),
            properties: RefCell::new(ObjectData::default()),
        });
        value
            .borrow_mut()
            .set("closure", Value::Function(Rc::clone(&closure)));
        map.borrow_mut()
            .weak_collection_entries
            .as_mut()
            .unwrap()
            .insert(
                WeakCollectionKey::Closure(Rc::downgrade(&closure)),
                Value::Object(Rc::clone(&value)),
            );
        let weak_closure = Rc::downgrade(&closure);
        let weak_value = Rc::downgrade(&value);
        drop(closure);
        drop(value);

        vm.collect_closure_cycles();
        assert!(weak_closure.upgrade().is_none());
        assert!(weak_value.upgrade().is_none());
        assert!(
            map.borrow()
                .weak_collection_entries
                .as_ref()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn promise_reaction_context_is_a_counted_owner_edge() {
        let mut host = TestHost;
        let (mut vm, program) = test_vm(&mut host);
        let target = Value::Object(Rc::new(RefCell::new(ObjectData::default())));
        let Value::Object(target_object) = &target else {
            unreachable!();
        };
        let weak_target = Rc::downgrade(target_object);
        let keeper = Rc::clone(target_object);
        let context: AsyncContext = Rc::new(vec![(
            Rc::new(crate::async_hooks::AsyncStorage {
                disabled: std::cell::Cell::new(false),
            }),
            target.clone(),
        )]);
        let promise = PromiseState::pending();
        let closure = closure_reaching_promise(&program, &promise);
        promise.state.borrow_mut().reactions.push(Reaction {
            on_fulfilled: Value::Function(Rc::clone(&closure)),
            on_rejected: Value::Undefined,
            sink: Sink::Promise(PromiseState::pending()),
            passthrough: false,
            context: Rc::clone(&context),
        });
        vm.module_map
            .borrow_mut()
            .gc_closures
            .push(Rc::downgrade(&closure));
        let weak_ref = vm
            .construct_weak_ref(&[target.clone()], 0)
            .expect("object can be held weakly");
        vm.module_map.borrow_mut().weak_ref_keepalive.clear();
        drop(target);
        drop(promise);
        drop(closure);

        vm.collect_closure_cycles();
        assert!(
            weak_target.upgrade().is_some(),
            "external context keeps target alive"
        );
        drop(keeper);
        drop(context);
        vm.collect_closure_cycles();
        assert!(weak_target.upgrade().is_none());
        assert_eq!(vm.weak_ref_deref(&weak_ref, 0), Ok(Value::Undefined));
    }

    #[test]
    fn combinator_walk_edges_reclaim_dead_target_and_preserve_external_owner() {
        let mut host = TestHost;
        let (mut vm, program) = test_vm(&mut host);
        let target = Value::Object(Rc::new(RefCell::new(ObjectData::default())));
        let Value::Object(target_object) = &target else {
            unreachable!();
        };
        let weak_target = Rc::downgrade(target_object);
        let keeper = Rc::clone(target_object);
        let walk = Rc::new(CombinatorWalk {
            native: Native::PromiseAll,
            values: RefCell::new(vec![target.clone()]),
            remaining: std::cell::Cell::new(1),
            resolve: Value::Undefined,
            reject: Value::Undefined,
        });
        let element = Rc::new(ElementFunction {
            walk,
            index: 0,
            rejects: false,
            already_called: Rc::new(std::cell::Cell::new(false)),
        });
        let promise = PromiseState::pending();
        let closure = closure_reaching_promise(&program, &promise);
        promise.state.borrow_mut().reactions.push(Reaction {
            on_fulfilled: Value::Element(Rc::clone(&element)),
            on_rejected: Value::Undefined,
            sink: Sink::Promise(PromiseState::pending()),
            passthrough: false,
            context: Rc::new(Vec::new()),
        });
        vm.module_map
            .borrow_mut()
            .gc_closures
            .push(Rc::downgrade(&closure));
        let weak_ref = vm
            .construct_weak_ref(&[target.clone()], 0)
            .expect("object can be held weakly");
        vm.module_map.borrow_mut().weak_ref_keepalive.clear();
        drop(target);
        drop(promise);
        drop(closure);
        drop(element);

        vm.collect_closure_cycles();
        assert!(
            weak_target.upgrade().is_some(),
            "external owner keeps target alive"
        );
        drop(keeper);
        vm.collect_closure_cycles();
        assert!(weak_target.upgrade().is_none());
        assert_eq!(vm.weak_ref_deref(&weak_ref, 0), Ok(Value::Undefined));
    }

    #[test]
    fn capability_record_edges_reclaim_dead_target_and_preserve_external_owner() {
        let mut host = TestHost;
        let (mut vm, program) = test_vm(&mut host);
        let target = Value::Object(Rc::new(RefCell::new(ObjectData::default())));
        let Value::Object(target_object) = &target else {
            unreachable!();
        };
        let weak_target = Rc::downgrade(target_object);
        let keeper = Rc::clone(target_object);
        let capability = Rc::new(CapabilityRecord {
            resolve: RefCell::new(target.clone()),
            reject: RefCell::new(Value::Undefined),
        });
        let promise = PromiseState::pending();
        let closure = closure_reaching_promise(&program, &promise);
        promise.state.borrow_mut().reactions.push(Reaction {
            on_fulfilled: Value::Capability(Rc::clone(&capability)),
            on_rejected: Value::Undefined,
            sink: Sink::Promise(PromiseState::pending()),
            passthrough: false,
            context: Rc::new(Vec::new()),
        });
        vm.module_map
            .borrow_mut()
            .gc_closures
            .push(Rc::downgrade(&closure));
        let weak_ref = vm
            .construct_weak_ref(&[target.clone()], 0)
            .expect("object can be held weakly");
        vm.module_map.borrow_mut().weak_ref_keepalive.clear();
        drop(target);
        drop(promise);
        drop(closure);
        drop(capability);

        vm.collect_closure_cycles();
        assert!(
            weak_target.upgrade().is_some(),
            "external owner keeps target alive"
        );
        drop(keeper);
        vm.collect_closure_cycles();
        assert!(weak_target.upgrade().is_none());
        assert_eq!(vm.weak_ref_deref(&weak_ref, 0), Ok(Value::Undefined));
    }

    #[test]
    fn finalization_registry_targets_and_tokens_are_weak_but_holdings_are_strong() {
        let mut host = TestHost;
        let (mut vm, _) = test_vm(&mut host);

        let target = tracked_object(ObjectData::default());
        target
            .borrow_mut()
            .set("self", Value::Object(Rc::clone(&target)));
        let weak_target = Rc::downgrade(&target);
        let token = tracked_object(ObjectData::default());
        let weak_token = Rc::downgrade(&token);

        let registry = vm
            .construct_finalization_registry(
                &[Value::Native(Native::ObjectToStringTag, RealmId::PRIMARY)],
                0,
            )
            .expect("callable cleanup callback constructs a registry");
        vm.finalization_registry_register(
            &registry,
            &[
                Value::Object(Rc::clone(&target)),
                Value::String("holding".into()),
                Value::Object(Rc::clone(&token)),
            ],
            0,
        )
        .expect("valid target, holding, and unregister token register");
        assert_eq!(
            vm.finalization_registry_unregister(&registry, &[Value::Object(Rc::clone(&token))], 0,),
            Ok(Value::Boolean(true))
        );
        assert_eq!(
            vm.finalization_registry_unregister(&registry, &[Value::Object(Rc::clone(&token))], 0,),
            Ok(Value::Boolean(false))
        );
        vm.finalization_registry_register(
            &registry,
            &[
                Value::Object(Rc::clone(&target)),
                Value::String("holding".into()),
                Value::Object(Rc::clone(&token)),
            ],
            0,
        )
        .expect("a token can be registered again after unregister");
        let weak_ref = vm
            .construct_weak_ref(&[Value::Object(Rc::clone(&target))], 0)
            .expect("object can be held weakly");
        vm.module_map.borrow_mut().weak_ref_keepalive.clear();
        drop(target);
        drop(token);

        vm.collect_closure_cycles();
        assert!(
            weak_target.upgrade().is_none(),
            "weak target cycle is reclaimed"
        );
        assert!(weak_token.upgrade().is_none(), "unregister token is weak");
        assert_eq!(vm.weak_ref_deref(&weak_ref, 0), Ok(Value::Undefined));

        // A live holding that points to a target is an ordinary strong path and must keep it
        // alive. Once that path is removed, the target's self-cycle becomes collectible.
        let held_target = tracked_object(ObjectData::default());
        held_target
            .borrow_mut()
            .set("self", Value::Object(Rc::clone(&held_target)));
        let held_target_weak = Rc::downgrade(&held_target);
        let holding = tracked_object(ObjectData::default());
        holding
            .borrow_mut()
            .set("target", Value::Object(Rc::clone(&held_target)));
        vm.finalization_registry_register(
            &registry,
            &[
                Value::Object(Rc::clone(&held_target)),
                Value::Object(Rc::clone(&holding)),
            ],
            0,
        )
        .expect("holding object registration succeeds");
        drop(held_target);
        vm.collect_closure_cycles();
        assert!(
            held_target_weak.upgrade().is_some(),
            "holding value reaches target"
        );
        holding.borrow_mut().set("target", Value::Undefined);
        vm.collect_closure_cycles();
        assert!(held_target_weak.upgrade().is_none());

        // A target with an ordinary external owner stays live through collection.
        let rooted = tracked_object(ObjectData::default());
        let rooted_weak = Rc::downgrade(&rooted);
        vm.finalization_registry_register(
            &registry,
            &[
                Value::Object(Rc::clone(&rooted)),
                Value::String("rooted".into()),
            ],
            0,
        )
        .expect("rooted target registration succeeds");
        vm.collect_closure_cycles();
        assert!(
            rooted_weak.upgrade().is_some(),
            "external target owner is respected"
        );
    }
}
