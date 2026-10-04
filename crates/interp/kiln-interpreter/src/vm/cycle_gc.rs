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
}

enum Owner {
    Object(Rc<RefCell<ObjectData>>),
    Closure(Rc<Closure>),
    Cell(Cell),
    Array(Rc<ArrayData>),
    Promise(Rc<PromiseData>),
    Generator(Rc<GeneratorData>),
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

        let module_map = self.gc_module_map();
        let (shared_closures, shared_weak_objects) = {
            let map = module_map.borrow();
            (map.gc_closures.clone(), map.gc_weak_ref_objects.clone())
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

        while let Some(parent) = work.pop() {
            if let Some(Node {
                owner: Some(Owner::Generator(generator)),
                ..
            }) = nodes.get(&parent)
            {
                self.prune_suspended_generator_locals(generator);
            }
            let children = owner_children(
                nodes
                    .get(&parent)
                    .and_then(|node| node.owner.as_ref())
                    .expect("work node exists"),
            );
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

        // Break only proven-unreachable nodes. WeakRef target upgrades happen after this method
        // returns, so a reclaimed target is absent on the same explicit collection request.
        for (key, node) in &nodes {
            if !live.contains(key)
                && let Some(owner) = node.owner.as_ref()
            {
                owner.clear_mutable_edges();
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
    }
    values
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
    for cell in &data.finalization_cells {
        values.push(cell.held.clone());
        // `target` and `token` are weak slots. Upgrading them here would invent strong graph
        // edges and could make the incoming-edge count exceed the actual Rc ownership graph.
        let _ = (&cell.target, &cell.token);
    }
    values
}
