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
}

enum Owner {
    Object(Rc<RefCell<ObjectData>>),
    Closure(Rc<Closure>),
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
}

impl Owner {
    fn key(&self) -> Key {
        match self {
            Self::Object(value) => Key::Object(Rc::as_ptr(value) as usize),
            Self::Closure(value) => Key::Closure(Rc::as_ptr(value) as usize),
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
        }
    }

    fn strong_count(&self) -> usize {
        match self {
            Self::Object(value) => Rc::strong_count(value),
            Self::Closure(value) => Rc::strong_count(value),
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
            }
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
            | Self::Proxy(_)
            | Self::CommonJs(_) => {}
        }
    }
}

#[derive(Default)]
struct Node {
    owner: Option<Owner>,
    edges: Vec<Key>,
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
        let mut nodes: HashMap<Key, Node> = HashMap::new();
        let mut work = Vec::new();
        // WeakMap values own their Rc payloads, but only conditionally keep them reachable.
        // Store identities here; the value itself remains owned only by its weak-map entry.
        let mut ephemerons = Vec::<(Key, Key, Key)>::new();

        let module_map = self.gc_module_map();
        let (shared_closures, shared_weak_objects, shared_weak_collections) = {
            let map = module_map.borrow();
            (
                map.gc_closures.clone(),
                map.gc_weak_ref_objects.clone(),
                map.gc_weak_collections.clone(),
            )
        };
        for weak in &shared_closures {
            if let Some(closure) = weak.upgrade() {
                let owner = Owner::Closure(closure);
                let key = owner.key();
                if let std::collections::hash_map::Entry::Vacant(entry) = nodes.entry(key) {
                    entry.insert(Node {
                        owner: Some(owner),
                        edges: Vec::new(),
                    });
                    work.push(key);
                }
            }
        }

        // WeakRef targets are another non-rooting way for the VM to know an object's identity.
        // This catches object-only cycles that have no function edge at all.
        for weak in &shared_weak_objects {
            if let Some(object) = weak.upgrade() {
                let owner = Owner::Object(object);
                let key = owner.key();
                if let std::collections::hash_map::Entry::Vacant(entry) = nodes.entry(key) {
                    entry.insert(Node {
                        owner: Some(owner),
                        edges: Vec::new(),
                    });
                    work.push(key);
                }
            }
        }
        for weak in &shared_weak_collections {
            if let Some(object) = weak.upgrade() {
                let owner = Owner::Object(object);
                let key = owner.key();
                if let std::collections::hash_map::Entry::Vacant(entry) = nodes.entry(key) {
                    entry.insert(Node {
                        owner: Some(owner),
                        edges: Vec::new(),
                    });
                    work.push(key);
                }
            }
        }

        while let Some(parent) = work.pop() {
            if let Some(Node {
                owner: Some(Owner::Generator(generator)),
                ..
            }) = nodes.get(&parent)
            {
                self.prune_suspended_generator_locals(generator);
            }
            let weak_entries = nodes
                .get(&parent)
                .and_then(|node| match node.owner.as_ref() {
                    Some(Owner::Object(object)) => object.borrow().weak_collection_entries.clone(),
                    _ => None,
                })
                .unwrap_or_default();
            for entry in weak_entries {
                let Some(key_value) = entry.key.upgrade() else {
                    continue;
                };
                let Some(key_owner) = owner_for_value(&key_value) else {
                    // Unknown exotic identities are conservatively kept live until their
                    // representation has an owner-graph node of its own.
                    continue;
                };
                let key = key_owner.key();
                if let std::collections::hash_map::Entry::Vacant(slot) = nodes.entry(key) {
                    slot.insert(Node {
                        owner: Some(key_owner),
                        edges: Vec::new(),
                    });
                    work.push(key);
                }
                if let Some(value_owner) = owner_for_value(&entry.value) {
                    let value = value_owner.key();
                    if let std::collections::hash_map::Entry::Vacant(slot) = nodes.entry(value) {
                        slot.insert(Node {
                            owner: Some(value_owner),
                            edges: Vec::new(),
                        });
                        work.push(value);
                    }
                    ephemerons.push((parent, key, value));
                }
            }
            let owner = nodes
                .get(&parent)
                .and_then(|node| node.owner.as_ref())
                .expect("work node exists");
            let children = owner_children(owner);
            for child_owner in owner_sidecar_children(owner) {
                let key = child_owner.key();
                nodes
                    .get_mut(&parent)
                    .expect("parent exists")
                    .edges
                    .push(key);
                if let std::collections::hash_map::Entry::Vacant(entry) = nodes.entry(key) {
                    entry.insert(Node {
                        owner: Some(child_owner),
                        edges: Vec::new(),
                    });
                    work.push(key);
                }
            }
            for child in children {
                if let Some(owner) = owner_for_value(&child) {
                    let key = owner.key();
                    nodes
                        .get_mut(&parent)
                        .expect("parent exists")
                        .edges
                        .push(key);
                    if let std::collections::hash_map::Entry::Vacant(entry) = nodes.entry(key) {
                        entry.insert(Node {
                            owner: Some(owner),
                            edges: Vec::new(),
                        });
                        work.push(key);
                    }
                }
            }
        }

        let mut incoming: HashMap<Key, usize> = HashMap::new();
        for node in nodes.values() {
            for edge in &node.edges {
                *incoming.entry(*edge).or_default() += 1;
            }
        }
        // Count the real Rc owner in every weak-map value even before its ephemeron activates.
        // This is ownership accounting only; it does not make the value reachable.
        for (_, _, value) in &ephemerons {
            *incoming.entry(*value).or_default() += 1;
        }

        // Nodes whose counts exceed the collector's one owner plus all incoming edges have an
        // owner outside this discovered graph. Unknown or unaccounted owners are always live.
        let mut live = HashSet::new();
        let mut mark = Vec::new();
        for (key, node) in &nodes {
            let Some(owner) = node.owner.as_ref() else {
                continue;
            };
            let accounted = incoming.get(key).copied().unwrap_or(0).saturating_add(1);
            if owner.strong_count() > accounted {
                live.insert(*key);
                mark.push(*key);
            }
        }
        while let Some(key) = mark.pop() {
            if let Some(node) = nodes.get(&key) {
                for edge in &node.edges {
                    if live.insert(*edge) {
                        mark.push(*edge);
                    }
                }
            }
        }
        // Ephemerons can form chains: a live key can make a value live, and that value can be a
        // key in another map. Iterate to a fixed point while propagating ordinary strong edges.
        loop {
            let mut activated = false;
            for (map, key, value) in &ephemerons {
                if live.contains(map) && live.contains(key) && live.insert(*value) {
                    mark.push(*value);
                    activated = true;
                }
            }
            while let Some(key) = mark.pop() {
                if let Some(node) = nodes.get(&key) {
                    for edge in &node.edges {
                        if live.insert(*edge) {
                            mark.push(*edge);
                        }
                    }
                }
            }
            if !activated {
                break;
            }
        }

        // Break only proven-unreachable nodes. WeakRef target upgrades happen after this method
        // returns, so a reclaimed target is absent on the same explicit collection request.
        for (key, node) in &nodes {
            if !live.contains(key)
                && let Some(owner) = node.owner.as_ref()
            {
                owner.clear_mutable_edges();
            }
        }
        // Drop entries whose weak key is dead. This releases their conditional values without
        // affecting entries in a map that is itself unreachable (which is cleared above).
        for (key, node) in &nodes {
            if live.contains(key)
                && let Some(Owner::Object(object)) = node.owner.as_ref()
            {
                let mut data = object.borrow_mut();
                if let Some(entries) = data.weak_collection_entries.as_mut() {
                    entries.retain(|entry| {
                        entry.key.upgrade().is_some_and(|value| {
                            owner_for_value(&value).is_none_or(|owner| live.contains(&owner.key()))
                        })
                    });
                }
            }
        }
        // Drop the collector's temporary strong owners before checking which weak census entries
        // expired during the edge-clearing pass.
        drop(nodes);
        self.closures.retain(|closure| closure.strong_count() > 0);
        self.closures_prune_at = self
            .closures
            .len()
            .saturating_add(self.closures.len().max(1024));
        let mut map = module_map.borrow_mut();
        map.gc_closures.retain(|closure| closure.strong_count() > 0);
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
        map.gc_weak_ref_objects_prune_at = map
            .gc_weak_ref_objects
            .len()
            .saturating_add(map.gc_weak_ref_objects.len().max(1024));
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
                    .map(|_| entry.by_yield_pc.clone())
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
                    by_yield_pc: live_by_pc.clone(),
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
        Owner::Cell(value) => values.push(value.borrow().clone()),
        Owner::Array(value) => {
            values.extend(value.items.borrow().iter().cloned());
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
                    Sink::AsyncGeneratorReturn(promise) => {
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
            drop(state);
            values.extend(object_children(&value.properties.borrow()));
        }
        Owner::AsyncDisposal(value) => {
            values.push(Value::Promise(Rc::clone(&value.result)));
            values.extend(value.pending.borrow().iter().cloned());
            values.extend(value.thrown.borrow().iter().cloned());
        }
        Owner::ModuleSuspension(value) => {
            values.extend(value.borrow().stack.iter().cloned());
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
        Owner::Promise(value) => {
            for reaction in &value.state.borrow().reactions {
                owners.push(Owner::AsyncContext(Rc::clone(&reaction.context)));
                match &reaction.sink {
                    Sink::AsyncDisposal(state) => {
                        owners.push(Owner::AsyncDisposal(Rc::clone(state)));
                    }
                    Sink::ModuleAwait(state) => {
                        owners.push(Owner::ModuleSuspension(Rc::clone(state)));
                    }
                    _ => {}
                }
            }
        }
        Owner::Element(value) => owners.push(Owner::CombinatorWalk(Rc::clone(&value.walk))),
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
    values.extend(data.namespace_cells.iter().map(|(_, value)| value.clone()));
    values.extend(data.prototype.iter().cloned());
    values.extend(data.primitive.iter().cloned());
    values.extend(data.error_stack_frames.iter().cloned());
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

    fn closure_reaching_promise(program: &Rc<Program>, promise: &Rc<PromiseData>) -> Rc<Closure> {
        Rc::new(Closure {
            program: Rc::clone(program),
            function: 0,
            strictness: Rc::new(Strictness::sloppy(program)),
            module_path: None,
            name: None,
            inferred_name: None,
            upvalues: Vec::new(),
            realm: RealmId::PRIMARY,
            captured_arguments: Some(Value::Promise(Rc::clone(promise))),
            captured_new_target: None,
            captured_this: None,
            prototype: RefCell::new(None),
            properties: RefCell::new(ObjectData::default()),
        })
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
    fn weak_map_ephemerons_collect_value_to_key_cycles_but_follow_live_keys() {
        let mut host = TestHost;
        let (mut vm, _) = test_vm(&mut host);
        let map = Rc::new(RefCell::new(ObjectData::default()));
        map.borrow_mut().weak_collection_entries = Some(Vec::new());
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
            .push(WeakCollectionEntry {
                key: WeakCollectionKey::Object(Rc::downgrade(&key)),
                value: Value::Object(Rc::clone(&value)),
            });
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
        map.borrow_mut().weak_collection_entries = Some(Vec::new());
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
            .push(WeakCollectionEntry {
                key: WeakCollectionKey::Object(Rc::downgrade(&key)),
                value: Value::Object(Rc::clone(&value)),
            });
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
        map.borrow_mut().weak_collection_entries = Some(Vec::new());
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
            .push(WeakCollectionEntry {
                key: WeakCollectionKey::Object(Rc::downgrade(&key)),
                value: Value::Object(Rc::clone(&value)),
            });
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
            map.borrow_mut().weak_collection_entries = Some(Vec::new());
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
            .push(WeakCollectionEntry {
                key: WeakCollectionKey::Object(Rc::downgrade(&root_key)),
                value: Value::Object(Rc::clone(&chained_key)),
            });
        second
            .borrow_mut()
            .weak_collection_entries
            .as_mut()
            .unwrap()
            .push(WeakCollectionEntry {
                key: WeakCollectionKey::Object(Rc::downgrade(&chained_key)),
                value: Value::Object(Rc::clone(&final_value)),
            });
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
    fn weak_map_closure_key_and_value_cycle_is_collectible() {
        let mut host = TestHost;
        let (mut vm, program) = test_vm(&mut host);
        let map = Rc::new(RefCell::new(ObjectData::default()));
        map.borrow_mut().weak_collection_entries = Some(Vec::new());
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
            name: None,
            inferred_name: None,
            upvalues: Vec::new(),
            realm: RealmId::PRIMARY,
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
            .push(WeakCollectionEntry {
                key: WeakCollectionKey::Closure(Rc::downgrade(&closure)),
                value: Value::Object(Rc::clone(&value)),
            });
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
}
