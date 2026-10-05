//! One module map per realm session: a record per canonical module key.
//!
//! Before this existed the static graph executor kept exports in a table of its own and every
//! machine that ran `import()` kept a second cache on itself, so a module the graph had already
//! run was run again by the first dynamic import of it, and two modules that each imported it
//! dynamically got two instances. ECMA-262 has one `[[ModuleMap]]` per realm (HostLoadImportedModule
//! must answer the same Module Record for the same request), and this is that map. The static
//! executor, the lazy CommonJS/builtin loader and dynamic `import()` all read and write it, so
//! whichever reaches a module first evaluates it and every later importer links to that record.

use super::*;
use std::collections::HashMap;
use std::rc::Weak;

/// Where one module is in its evaluation.
#[derive(Clone, Debug)]
pub enum ModuleStatus {
    /// Known to the map -- something holds a cell or the namespace of it -- but not started.
    Linked,
    /// Its body is running, or suspended at a top-level `await`.
    Evaluating,
    /// Its body finished; its exports are final bindings.
    Evaluated,
    /// Its body threw this value. Every later import rethrows the same value (ECMA-262
    /// 16.2.1.5.3 `Evaluate` step 4: a module whose `[[EvaluationError]]` is set answers it).
    Failed(Value),
}

/// One module's entry in the map.
#[derive(Clone)]
pub struct ModuleRecord {
    pub(super) status: ModuleStatus,
    /// Every name the module exports, as the binding the importer links to: a live cell where
    /// the binding is mutable, the value where it is not. Empty until the module is evaluated.
    pub(super) exports: Rc<HashMap<String, Value>>,
    /// Stable cells for every published export. Even an immutable binding has a stable cell:
    /// the module loader can replace its value for an authorized test spy, and all linked
    /// imports and namespace views observe that replacement without changing namespace shape.
    pub(super) bindings: HashMap<String, Cell>,
    /// The module's namespace object, created on first demand and never replaced, so a static
    /// `import * as ns` and every `import()` of the module see the same object.
    pub(super) namespace: Option<Rc<RefCell<ObjectData>>>,
    /// Cells handed out for names whose module has not finished: an import across a cycle, or a
    /// dynamic import that links to a module still evaluating. Each starts in its dead zone and
    /// is filled when the module publishes.
    pub(super) pending: Vec<(String, Cell)>,
    /// Shared completion identity for dynamic imports waiting on this module's TLA. Installed
    /// before the current job returns, so reentrant queued imports adopt the same evaluation.
    pub(super) completion: Option<Rc<PromiseData>>,
    /// CommonJS body evaluation and its ESM namespace snapshot are separate operations.
    pub(super) commonjs_namespace_ready: bool,
    pub(super) commonjs_namespace_failure: Option<Value>,
}

impl ModuleRecord {
    fn new() -> Self {
        Self {
            status: ModuleStatus::Linked,
            exports: Rc::new(HashMap::new()),
            bindings: HashMap::new(),
            namespace: None,
            pending: Vec::new(),
            completion: None,
            commonjs_namespace_ready: false,
            commonjs_namespace_failure: None,
        }
    }
}

/// One source-module import generation. The realm keeps its current and static generations
/// strongly and records every other generation weakly. Closures/namespaces/jobs retain an
/// inactive generation only while the language or VM can still observe its records.
pub struct ModuleGeneration {
    pub(crate) realm: RealmId,
    pub(crate) id: u64,
    pub(super) records: HashMap<String, ModuleRecord>,
}

impl std::fmt::Debug for ModuleGeneration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ModuleGeneration")
            .field("realm", &self.realm)
            .field("id", &self.id)
            .field("record_count", &self.records.len())
            .finish()
    }
}

pub type ModuleGenerationHandle = Rc<RefCell<ModuleGeneration>>;

/// The realm session's module map, keyed by realm and canonical module key (the module's path).
///
/// Keyed by realm as well as by module, because each realm has its own map: two realms
/// importing one specifier get two independent instances, which is what keeps a `ShadowRealm`
/// from sharing module state with the realm that made it.
#[derive(Default)]
pub struct ModuleMap {
    /// The strong records are owned by their generation object. Keeping the generation handle
    /// weak here is what lets an inactive epoch and all of its module records die together.
    generations_by_id: HashMap<(RealmId, u64), Weak<RefCell<ModuleGeneration>>>,
    generations_prune_at: usize,
    generations: HashMap<RealmId, u64>,
    current_generations: HashMap<RealmId, ModuleGenerationHandle>,
    /// Generation zero is also the static graph's initial module map. It remains rooted for the
    /// realm lifetime; dynamic generations created by reset are independently reclaimable.
    static_generations: HashMap<RealmId, ModuleGenerationHandle>,
    runner_candidates: HashMap<RealmId, std::collections::HashSet<String>>,
    runner_referrers: HashMap<RealmId, String>,
    spy_adapter_referrers: HashMap<RealmId, String>,
    /// Weak graph census shared by every VM in this realm session. Module execution allocates
    /// closures in distinct machines, while explicit collection can be requested from any one
    /// of them; keeping weak identities here lets that request see cross-module cycles.
    pub(super) gc_closures: Vec<Weak<Closure>>,
    pub(super) gc_closures_prune_at: usize,
    /// Suspended generators retain locals outside ordinary object edges; census them weakly so
    /// those references are accounted for without making abandoned generators roots.
    pub(super) gc_generators: Vec<Weak<GeneratorData>>,
    pub(super) gc_generators_prune_at: usize,
    pub(super) gc_weak_ref_objects: Vec<Weak<RefCell<ObjectData>>>,
    pub(super) gc_weak_ref_objects_prune_at: usize,
    /// Weak collections are census seeds so their conditional values can be traced even when a
    /// map has no WeakRef target pointing at the map itself.
    pub(super) gc_weak_collections: Vec<Weak<RefCell<ObjectData>>>,
    pub(super) gc_weak_collections_prune_at: usize,
    /// Strong targets kept alive through the current ECMAScript job. The job can resume in a
    /// different VM, so its keepalive set belongs to the realm session rather than one machine.
    pub(super) weak_ref_keepalive: Vec<Value>,
    /// Cached resume-point local liveness, keyed by the owning program allocation and function.
    /// The weak program edge keeps cached analyses from extending a dead module's lifetime.
    pub(super) gc_suspended_locals: HashMap<(usize, u32), SuspendedLocalCache>,
    pub(super) gc_suspended_locals_prune_at: usize,
}

pub(super) struct SuspendedLocalCache {
    pub(super) program: Weak<Program>,
    pub(super) by_yield_pc: HashMap<usize, std::collections::HashSet<u32>>,
}

/// The map as the session shares it between every machine that runs one of its modules.
pub type SharedModuleMap = Rc<RefCell<ModuleMap>>;

/// The dead-zone marker a cell holds before the binding it stands for exists.
fn dead_zone_cell() -> Cell {
    Rc::new(RefCell::new(Value::Native(
        Native::Uninitialized,
        RealmId::PRIMARY,
    )))
}

/// Look one name up in a module's exports. A CommonJS module's names are the own data
/// properties of its `module.exports`, which its `default` is, so a name the table does not
/// list is read from there -- the same answer a static import of the name gets.
pub fn lookup_export(exports: &HashMap<String, Value>, name: &str) -> Option<Value> {
    exports.get(name).cloned().or_else(|| {
        exports
            .get("default")
            .and_then(|default| own_data_property(default, name))
    })
}

impl ModuleMap {
    /// Current dynamic-module generation for this realm. Static graph records start in
    /// generation zero; resetModules advances the dynamic import generation without invalidating
    /// cells or namespaces already linked by that graph.
    pub fn generation(&self, realm: RealmId) -> u64 {
        self.generations.get(&realm).copied().unwrap_or(0)
    }

    pub(super) fn weak_generation_census(&self) -> Vec<Weak<RefCell<ModuleGeneration>>> {
        self.generations_by_id.values().cloned().collect()
    }

    pub(super) fn prune_dead_generations(&mut self) {
        self.generations_by_id
            .retain(|_, generation| generation.strong_count() > 0);
        self.generations_prune_at = self
            .generations_by_id
            .len()
            .saturating_add(self.generations_by_id.len().max(1024));
    }

    fn track_generation(
        &mut self,
        realm: RealmId,
        generation: u64,
        handle: &ModuleGenerationHandle,
    ) {
        if self.generations_by_id.len() >= self.generations_prune_at {
            self.generations_by_id
                .retain(|_, generation| generation.strong_count() > 0);
            self.generations_prune_at = self
                .generations_by_id
                .len()
                .saturating_add(self.generations_by_id.len().max(1024));
        }
        self.generations_by_id
            .insert((realm, generation), Rc::downgrade(handle));
    }

    pub(super) fn generation_handle(
        &self,
        realm: RealmId,
        generation: u64,
    ) -> Option<ModuleGenerationHandle> {
        if let Some(current) = self.current_generations.get(&realm)
            && current.borrow().id == generation
        {
            return Some(Rc::clone(current));
        }
        if let Some(static_generation) = self.static_generations.get(&realm)
            && static_generation.borrow().id == generation
        {
            return Some(Rc::clone(static_generation));
        }
        self.generations_by_id
            .get(&(realm, generation))
            .and_then(Weak::upgrade)
    }

    fn ensure_generation_handle(
        &mut self,
        realm: RealmId,
        generation: u64,
    ) -> ModuleGenerationHandle {
        if let Some(existing) = self.generation_handle(realm, generation) {
            return existing;
        }
        let handle = Rc::new(RefCell::new(ModuleGeneration {
            realm,
            id: generation,
            records: HashMap::new(),
        }));
        self.track_generation(realm, generation, &handle);
        if generation == 0 {
            self.static_generations
                .entry(realm)
                .or_insert_with(|| Rc::clone(&handle));
        }
        if self.generation(realm) == generation {
            self.current_generations
                .entry(realm)
                .or_insert_with(|| Rc::clone(&handle));
        }
        handle
    }

    pub fn current_generation_handle(&mut self, realm: RealmId) -> ModuleGenerationHandle {
        let generation = self.generation(realm);
        self.ensure_generation_handle(realm, generation)
    }

    /// Start a fresh dynamic module graph while retaining prior records for existing bindings.
    pub fn reset_generation(&mut self, realm: RealmId) -> u64 {
        let previous = self.current_generation_handle(realm);
        let next = self
            .generation(realm)
            .checked_add(1)
            .expect("dynamic module generation counter exhausted");
        self.generations.insert(realm, next);
        let current = Rc::new(RefCell::new(ModuleGeneration {
            realm,
            id: next,
            records: HashMap::new(),
        }));
        self.track_generation(realm, next, &current);
        self.current_generations.insert(realm, current);
        drop(previous);
        next
    }

    /// Make a fresh graph's host-module record refer to the cached host instance. Source modules
    /// are reevaluated per generation; CommonJS and builtin modules retain their host cache and
    /// export-cell identity across `resetModules`, while each generation gets its own namespace.
    pub fn alias_generation(
        &mut self,
        realm: RealmId,
        from_generation: u64,
        to_generation: u64,
        key: &str,
    ) -> bool {
        let Some(source) = self.generation_handle(realm, from_generation) else {
            return false;
        };
        let Some(record) = source.borrow().records.get(key).cloned() else {
            return false;
        };
        if !matches!(
            record.status,
            ModuleStatus::Evaluated | ModuleStatus::Failed(_)
        ) {
            return false;
        }
        let destination = self.ensure_generation_handle(realm, to_generation);
        let (old_generation_namespace, pending) = {
            let mut destination = destination.borrow_mut();
            destination
                .records
                .entry(key.to_owned())
                .or_insert_with(ModuleRecord::new);
            let existing = destination.records.get_mut(key).expect("record inserted");
            (
                existing.namespace.clone(),
                std::mem::take(&mut existing.pending),
            )
        };
        let mut alias = record;
        // Namespace wrappers belong to the module registry generation. Keep a namespace that
        // was already requested in the destination generation, but never carry over the old
        // wrapper merely because the host exports are cached.
        alias.namespace = old_generation_namespace;
        alias.pending = pending;
        let pending = std::mem::take(&mut alias.pending);
        let namespace = {
            let mut destination = destination.borrow_mut();
            destination.records.insert(key.to_owned(), alias);
            destination.records.get(key).and_then(|record| {
                record
                    .namespace
                    .as_ref()
                    .map(|namespace| (Rc::clone(namespace), build_namespace(&record.bindings)))
            })
        };
        if let Some((namespace, Value::Object(fresh))) = namespace {
            *namespace.borrow_mut() = std::mem::take(&mut *fresh.borrow_mut());
        }
        let mut waiting = Vec::new();
        for (name, cell) in pending {
            if name == kiln_ir::NAMESPACE_IMPORT {
                let namespace = self.namespace_in_generation(realm, to_generation, key);
                *cell.borrow_mut() = namespace;
            } else if let Some(binding) = destination
                .borrow()
                .records
                .get(key)
                .and_then(|record| record.bindings.get(&name).cloned())
            {
                *cell.borrow_mut() = Value::Cell(binding);
            } else {
                waiting.push((name, cell));
            }
        }
        if let Some(record) = destination.borrow_mut().records.get_mut(key) {
            record.pending = waiting;
        }
        true
    }

    fn record(&mut self, realm: RealmId, key: &str) -> ModuleGenerationHandle {
        let generation = self.generation(realm);
        self.ensure_record_in_generation(realm, generation, key)
    }

    fn ensure_record_in_generation(
        &mut self,
        realm: RealmId,
        generation: u64,
        key: &str,
    ) -> ModuleGenerationHandle {
        let handle = self.ensure_generation_handle(realm, generation);
        handle
            .borrow_mut()
            .records
            .entry(key.to_owned())
            .or_insert_with(ModuleRecord::new);
        handle
    }

    pub fn status_in_generation(
        &self,
        realm: RealmId,
        generation: u64,
        key: &str,
    ) -> Option<ModuleStatus> {
        self.generation_handle(realm, generation)
            .and_then(|handle| {
                handle
                    .borrow()
                    .records
                    .get(key)
                    .map(|record| record.status.clone())
            })
    }

    /// Where `key` is in its evaluation, or `None` if nothing has asked for it.
    pub fn status(&self, realm: RealmId, key: &str) -> Option<ModuleStatus> {
        self.status_in_generation(realm, self.generation(realm), key)
    }

    /// Whether `key` has finished evaluating successfully.
    pub fn is_evaluated(&self, realm: RealmId, key: &str) -> bool {
        matches!(self.status(realm, key), Some(ModuleStatus::Evaluated))
    }

    /// Whether `key` has started evaluating, whatever came of it.
    pub fn is_started(&self, realm: RealmId, key: &str) -> bool {
        matches!(
            self.status(realm, key),
            Some(ModuleStatus::Evaluating | ModuleStatus::Evaluated | ModuleStatus::Failed(_))
        )
    }

    /// The finished exports of `key`; `None` until it has been evaluated.
    pub fn exports(&self, realm: RealmId, key: &str) -> Option<Rc<HashMap<String, Value>>> {
        self.exports_in_generation(realm, self.generation(realm), key)
    }

    pub fn exports_in_generation(
        &self,
        realm: RealmId,
        generation: u64,
        key: &str,
    ) -> Option<Rc<HashMap<String, Value>>> {
        let handle = self.generation_handle(realm, generation)?;
        let handle = handle.borrow();
        let record = handle.records.get(key)?;
        matches!(record.status, ModuleStatus::Evaluated).then(|| Rc::clone(&record.exports))
    }

    /// Mark `key` as evaluating. The caller is the one evaluation it will ever have.
    pub fn begin(&mut self, realm: RealmId, key: &str) {
        let record = self.record(realm, key);
        record
            .borrow_mut()
            .records
            .get_mut(key)
            .expect("record inserted")
            .status = ModuleStatus::Evaluating;
    }

    pub fn begin_in_generation(&mut self, realm: RealmId, generation: u64, key: &str) {
        let record = self.ensure_record_in_generation(realm, generation, key);
        record
            .borrow_mut()
            .records
            .get_mut(key)
            .expect("record inserted")
            .status = ModuleStatus::Evaluating;
    }

    pub fn completion_in_generation(
        &self,
        realm: RealmId,
        generation: u64,
        key: &str,
    ) -> Option<Rc<PromiseData>> {
        self.generation_handle(realm, generation).and_then(|owner| {
            owner
                .borrow()
                .records
                .get(key)
                .and_then(|record| record.completion.clone())
        })
    }

    pub fn set_completion_in_generation(
        &mut self,
        realm: RealmId,
        generation: u64,
        key: &str,
        completion: Rc<PromiseData>,
    ) {
        let owner = self.ensure_record_in_generation(realm, generation, key);
        owner
            .borrow_mut()
            .records
            .get_mut(key)
            .expect("record inserted")
            .completion = Some(completion);
    }

    /// Record that evaluating `key` threw `error`.
    pub fn fail(&mut self, realm: RealmId, key: &str, error: Value) {
        let record = self.record(realm, key);
        let mut owner = record.borrow_mut();
        let record = owner.records.get_mut(key).expect("record inserted");
        record.status = ModuleStatus::Failed(error);
        record.completion = None;
    }

    pub fn fail_in_generation(&mut self, realm: RealmId, generation: u64, key: &str, error: Value) {
        let record = self.ensure_record_in_generation(realm, generation, key);
        let mut owner = record.borrow_mut();
        let record = owner.records.get_mut(key).expect("record inserted");
        record.status = ModuleStatus::Failed(error);
        record.completion = None;
    }

    /// Publish `key`'s exports and mark it evaluated. Every cell handed out for one of its names
    /// while it was running now holds the value, and a namespace created early is filled in.
    pub fn publish(&mut self, realm: RealmId, key: &str, exports: HashMap<String, Value>) {
        let generation = self.generation(realm);
        self.publish_in_generation(realm, generation, key, exports);
    }

    pub fn publish_in_generation(
        &mut self,
        realm: RealmId,
        generation: u64,
        key: &str,
        exports: HashMap<String, Value>,
    ) {
        let bindings: HashMap<String, Cell> = exports
            .iter()
            .map(|(name, value)| {
                let cell = match value {
                    Value::Cell(cell) => Rc::clone(cell),
                    other => Rc::new(RefCell::new(other.clone())),
                };
                (name.clone(), cell)
            })
            .collect();
        let record = self.ensure_record_in_generation(realm, generation, key);
        let pending = {
            let mut generation = record.borrow_mut();
            let record = generation.records.get_mut(key).expect("record inserted");
            record.status = ModuleStatus::Evaluated;
            record.exports = Rc::new(exports);
            record.bindings = bindings;
            record.completion = None;
            if let Some(namespace) = &record.namespace
                && let Value::Object(fresh) = build_namespace(&record.bindings)
            {
                *namespace.borrow_mut() = std::mem::take(&mut *fresh.borrow_mut());
            }
            std::mem::take(&mut record.pending)
        };
        let mut waiting = Vec::new();
        for (name, cell) in pending {
            if name == kiln_ir::NAMESPACE_IMPORT {
                let namespace = self.namespace_in_generation(realm, generation, key);
                *cell.borrow_mut() = namespace;
                continue;
            }
            let binding = record
                .borrow()
                .records
                .get(key)
                .and_then(|record| record.bindings.get(&name).cloned());
            match binding {
                Some(value) => {
                    /* Keep an import linked to the canonical export cell. Copying the current
                     * value breaks live-binding updates (including an authorized test spy) for
                     * modules that linked across a cycle before the exporter published. */
                    *cell.borrow_mut() = Value::Cell(value);
                }
                None => waiting.push((name, cell)),
            }
        }
        record
            .borrow_mut()
            .records
            .get_mut(key)
            .expect("record inserted")
            .pending = waiting;
    }

    pub fn commonjs_namespace_ready_in_generation(
        &self,
        realm: RealmId,
        generation: u64,
        key: &str,
    ) -> bool {
        self.generation_handle(realm, generation)
            .and_then(|generation| {
                generation
                    .borrow()
                    .records
                    .get(key)
                    .map(|record| record.commonjs_namespace_ready)
            })
            .unwrap_or(false)
    }

    pub fn commonjs_namespace_failure_in_generation(
        &self,
        realm: RealmId,
        generation: u64,
        key: &str,
    ) -> Option<Value> {
        self.generation_handle(realm, generation)
            .and_then(|generation| {
                generation
                    .borrow()
                    .records
                    .get(key)
                    .and_then(|record| record.commonjs_namespace_failure.clone())
            })
    }

    pub fn fail_commonjs_namespace_in_generation(
        &mut self,
        realm: RealmId,
        generation: u64,
        key: &str,
        failure: Value,
    ) {
        let generation = self.ensure_record_in_generation(realm, generation, key);
        generation
            .borrow_mut()
            .records
            .get_mut(key)
            .expect("record inserted")
            .commonjs_namespace_failure = Some(failure);
    }

    pub fn publish_commonjs_namespace_in_generation(
        &mut self,
        realm: RealmId,
        generation: u64,
        key: &str,
        exports: HashMap<String, Value>,
    ) {
        self.publish_in_generation(realm, generation, key, exports);
        let generation = self.ensure_record_in_generation(realm, generation, key);
        generation
            .borrow_mut()
            .records
            .get_mut(key)
            .expect("record inserted")
            .commonjs_namespace_ready = true;
    }

    /// Whether a CommonJS module has snapshotted its ESM namespace already.
    pub fn commonjs_namespace_ready(&self, realm: RealmId, key: &str) -> bool {
        self.commonjs_namespace_ready_in_generation(realm, self.generation(realm), key)
    }

    /// An ESM namespace getter failure does not invalidate the require cache.
    pub fn commonjs_namespace_failure(&self, realm: RealmId, key: &str) -> Option<Value> {
        self.commonjs_namespace_failure_in_generation(realm, self.generation(realm), key)
    }

    pub fn fail_commonjs_namespace(&mut self, realm: RealmId, key: &str, failure: Value) {
        let generation = self.generation(realm);
        self.fail_commonjs_namespace_in_generation(realm, generation, key, failure);
    }

    /// Publish the namespace snapshot independently of the lazy require cache.
    pub fn publish_commonjs_namespace(
        &mut self,
        realm: RealmId,
        key: &str,
        exports: HashMap<String, Value>,
    ) {
        let generation = self.generation(realm);
        self.publish_commonjs_namespace_in_generation(realm, generation, key, exports);
    }

    /// A dead-zone cell for `name` of `key`, filled when `key` publishes. One cell per name, so
    /// every crossing of a cycle lands on the same place. `NAMESPACE_IMPORT` names the namespace.
    pub fn pending_cell(&mut self, realm: RealmId, key: &str, name: &str) -> Cell {
        let generation = self.generation(realm);
        self.pending_cell_in_generation(realm, generation, key, name)
    }

    pub fn pending_cell_in_generation(
        &mut self,
        realm: RealmId,
        generation: u64,
        key: &str,
        name: &str,
    ) -> Cell {
        let generation = self.ensure_record_in_generation(realm, generation, key);
        let mut generation = generation.borrow_mut();
        let record = generation.records.get_mut(key).expect("record inserted");
        if let Some((_, cell)) = record.pending.iter().find(|(held, _)| held == name) {
            return Rc::clone(cell);
        }
        let cell = dead_zone_cell();
        record.pending.push((name.to_owned(), Rc::clone(&cell)));
        cell
    }

    /// What an importer of `name` from `key` links to: the binding if `key` has finished, and a
    /// dead-zone cell that will hold it otherwise.
    pub fn binding(&mut self, realm: RealmId, key: &str, name: &str) -> Option<Value> {
        let generation = self.generation(realm);
        self.binding_in_generation(realm, generation, key, name)
    }

    pub fn binding_in_generation(
        &mut self,
        realm: RealmId,
        generation: u64,
        key: &str,
        name: &str,
    ) -> Option<Value> {
        if let Some(handle) = self.generation_handle(realm, generation) {
            let binding = {
                let handle = handle.borrow();
                handle.records.get(key).and_then(|record| {
                    record
                        .bindings
                        .get(name)
                        .cloned()
                        .map(Value::Cell)
                        .or_else(|| lookup_export(&record.exports, name))
                })
            };
            if binding.is_some() {
                return binding;
            }
        }
        Some(Value::Cell(
            self.pending_cell_in_generation(realm, generation, key, name),
        ))
    }

    /// Register the runner's source path, which is the prerequisite for a separate spy-adapter
    /// registration. Paths arrive as file URLs from the adapter and canonical paths from hosts.
    pub fn add_runner_candidate(&mut self, realm: RealmId, key: &str) {
        self.runner_candidates
            .entry(realm)
            .or_default()
            .insert(key.to_owned());
    }

    pub fn register_runner(&mut self, realm: RealmId, key: &str) -> bool {
        if !self
            .runner_candidates
            .get(&realm)
            .is_some_and(|candidates| candidates.contains(key))
        {
            return false;
        }
        self.runner_referrers.insert(realm, key.to_owned());
        true
    }

    pub fn has_registered_runner(&self, realm: RealmId) -> bool {
        self.runner_referrers.contains_key(&realm)
    }

    /// Dynamic imports written in the registered runner observe the active reset generation,
    /// even when reset was called through a helper closure from another module. Ordinary module
    /// closures keep their captured generation so retained old instances remain coherent.
    pub fn import_generation_for(
        &self,
        realm: RealmId,
        importer: Option<&str>,
        captured: u64,
    ) -> u64 {
        if importer.is_some_and(|importer| {
            self.runner_referrers
                .get(&realm)
                .is_some_and(|runner| runner == importer)
        }) {
            self.generation(realm)
        } else {
            captured
        }
    }

    pub fn import_generation_handle_for(
        &mut self,
        realm: RealmId,
        importer: Option<&str>,
        captured: &ModuleGenerationHandle,
    ) -> ModuleGenerationHandle {
        let captured_id = captured.borrow().id;
        let selected = self.import_generation_for(realm, importer, captured_id);
        if selected == captured_id {
            Rc::clone(captured)
        } else {
            self.ensure_generation_handle(realm, selected)
        }
    }

    pub fn import_generation_handle_for_id(
        &mut self,
        realm: RealmId,
        importer: Option<&str>,
        captured: u64,
    ) -> ModuleGenerationHandle {
        let selected = self.import_generation_for(realm, importer, captured);
        self.ensure_generation_handle(realm, selected)
    }

    /// Register the one authorized spy adapter for this realm after its runner is known.
    pub fn register_spy_adapter(&mut self, realm: RealmId, key: &str) -> bool {
        if !self.runner_referrers.contains_key(&realm)
            || !self
                .live_generation_handles(realm)
                .iter()
                .any(|generation| generation.borrow().records.contains_key(key))
        {
            return false;
        }
        self.spy_adapter_referrers.insert(realm, key.to_owned());
        true
    }

    /// Replace one export cell only for the registered spy adapter. The namespace remains the
    /// same immutable object and every import retains the same underlying binding cell.
    pub fn spy_export(
        &mut self,
        realm: RealmId,
        namespace: &Value,
        name: &str,
        value: Value,
        adapter: &str,
    ) -> bool {
        if self.spy_adapter_referrers.get(&realm).map(String::as_str) != Some(adapter) {
            return false;
        }
        let Value::Object(namespace) = namespace else {
            return false;
        };
        for generation in self.live_generation_handles(realm) {
            let generation = generation.borrow();
            for record in generation.records.values() {
                if !record
                    .namespace
                    .as_ref()
                    .is_some_and(|candidate| Rc::ptr_eq(candidate, namespace))
                {
                    continue;
                }
                let Some(cell) = record.bindings.get(name) else {
                    return false;
                };
                *cell.borrow_mut() = value;
                return true;
            }
        }
        false
    }

    /// `key`'s namespace object: one per module, created now if nothing has asked before. A
    /// namespace asked for before the module finished is empty until it publishes, and is then
    /// filled in place, so its identity never changes.
    pub fn namespace(&mut self, realm: RealmId, key: &str) -> Value {
        let generation = self.generation(realm);
        self.namespace_in_generation(realm, generation, key)
    }

    pub fn namespace_in_generation(&mut self, realm: RealmId, generation: u64, key: &str) -> Value {
        let owner = self.ensure_record_in_generation(realm, generation, key);
        let mut owner_data = owner.borrow_mut();
        let record = owner_data.records.get_mut(key).expect("record inserted");
        if let Some(namespace) = &record.namespace {
            return Value::Object(Rc::clone(namespace));
        }
        let built = build_namespace(&record.bindings);
        if let Value::Object(object) = &built {
            object.borrow_mut().module_generation = Some(Rc::clone(&owner));
            record.namespace = Some(Rc::clone(object));
        }
        built
    }

    fn live_generation_handles(&self, realm: RealmId) -> Vec<ModuleGenerationHandle> {
        let mut handles = Vec::new();
        if let Some(current) = self.current_generations.get(&realm) {
            handles.push(Rc::clone(current));
        }
        if let Some(static_generation) = self.static_generations.get(&realm)
            && !handles
                .iter()
                .any(|existing| Rc::ptr_eq(existing, static_generation))
        {
            handles.push(Rc::clone(static_generation));
        }
        let inactive: Vec<_> = self
            .generations_by_id
            .iter()
            .filter(|((record_realm, _), _)| *record_realm == realm)
            .filter_map(|(_, generation)| generation.upgrade())
            .collect();
        for candidate in inactive {
            if !handles
                .iter()
                .any(|existing| Rc::ptr_eq(existing, &candidate))
            {
                handles.push(candidate);
            }
        }
        handles
    }
}

/// A namespace over `exports`, in the order a namespace holds its names. Cells are passed as
/// they are, so the namespace tracks the live bindings behind them.
fn build_namespace(exports: &HashMap<String, Cell>) -> Value {
    let mut entries: Vec<(String, Value)> = exports
        .iter()
        .map(|(name, value)| (name.clone(), Value::Cell(Rc::clone(value))))
        .collect();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    namespace_object(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runner_spy_replaces_const_binding_without_changing_namespace_shape() {
        let realm = RealmId::PRIMARY;
        let mut modules = ModuleMap::default();
        modules.begin(realm, "/test.mjs");
        modules.begin(realm, "/spy.mjs");
        modules.publish(
            realm,
            "/test.mjs",
            HashMap::from([("run".to_owned(), Value::Number(1.0))]),
        );
        modules.publish(realm, "/spy.mjs", HashMap::new());
        modules.add_runner_candidate(realm, "/test.mjs");
        assert!(!modules.register_runner(realm, "/unrelated.mjs"));
        assert!(modules.register_runner(realm, "/test.mjs"));
        assert!(modules.register_spy_adapter(realm, "/spy.mjs"));

        let namespace = modules.namespace(realm, "/test.mjs");
        let Value::Object(object) = &namespace else {
            panic!("module namespace must be an object");
        };
        let attributes = object.borrow().get_own_attributes("run");
        let replacement = Value::Number(2.0);
        assert!(!modules.spy_export(
            realm,
            &namespace,
            "run",
            replacement.clone(),
            "/unrelated.mjs"
        ));
        assert!(modules.spy_export(realm, &namespace, "run", replacement.clone(), "/spy.mjs"));
        let Value::Cell(binding) = modules.binding(realm, "/test.mjs", "run").unwrap() else {
            panic!("published exports link through their stable cells");
        };
        assert_eq!(*binding.borrow(), replacement);
        object.borrow_mut().refresh_namespace();
        assert_eq!(object.borrow().get_own("run"), Some(&replacement));
        assert_eq!(object.borrow().get_own_attributes("run"), attributes);
        assert!(!object.borrow().is_extensible());
        assert!(object.borrow().is_module_namespace());
    }

    #[test]
    fn reset_generation_keeps_old_static_bindings_and_namespaces() {
        let realm = RealmId::PRIMARY;
        let mut modules = ModuleMap::default();
        modules.begin_in_generation(realm, 0, "/effect.mjs");
        modules.publish_in_generation(
            realm,
            0,
            "/effect.mjs",
            HashMap::from([("value".to_owned(), Value::Number(1.0))]),
        );
        let old_namespace = modules.namespace_in_generation(realm, 0, "/effect.mjs");
        let old_binding = modules
            .binding_in_generation(realm, 0, "/effect.mjs", "value")
            .unwrap();

        assert_eq!(modules.reset_generation(realm), 1);
        modules.begin_in_generation(realm, 1, "/effect.mjs");
        modules.publish_in_generation(
            realm,
            1,
            "/effect.mjs",
            HashMap::from([("value".to_owned(), Value::Number(2.0))]),
        );
        let new_namespace = modules.namespace_in_generation(realm, 1, "/effect.mjs");
        let new_binding = modules
            .binding_in_generation(realm, 1, "/effect.mjs", "value")
            .unwrap();

        let (Value::Object(old_namespace), Value::Object(new_namespace)) =
            (old_namespace, new_namespace)
        else {
            panic!("module namespace expected");
        };
        assert!(!Rc::ptr_eq(&old_namespace, &new_namespace));
        let Value::Cell(old_cell) = old_binding else {
            panic!("old import must remain a live cell");
        };
        let Value::Cell(new_cell) = new_binding else {
            panic!("new import must be a live cell");
        };
        assert_eq!(*old_cell.borrow(), Value::Number(1.0));
        assert_eq!(*new_cell.borrow(), Value::Number(2.0));
    }

    #[test]
    fn inactive_empty_generations_are_only_weakly_indexed() {
        let realm = RealmId::PRIMARY;
        let mut modules = ModuleMap::default();
        let old_generation = modules.reset_generation(realm);
        let owner = modules.current_generation_handle(realm);
        let weak = Rc::downgrade(&owner);
        assert!(modules.generation_handle(realm, old_generation).is_some());

        assert_eq!(modules.reset_generation(realm), old_generation + 1);
        drop(owner);
        assert!(weak.upgrade().is_none());
        assert!(modules.generation_handle(realm, old_generation).is_none());
        modules.prune_dead_generations();
        assert!(
            !modules
                .generations_by_id
                .contains_key(&(realm, old_generation))
        );
    }

    #[test]
    fn captured_import_handle_stays_old_while_runner_selects_current_epoch() {
        let realm = RealmId::PRIMARY;
        let mut modules = ModuleMap::default();
        modules.add_runner_candidate(realm, "/test.mjs");
        assert!(modules.register_runner(realm, "/test.mjs"));
        let old_generation = modules.current_generation_handle(realm);
        let old_id = old_generation.borrow().id;
        assert_eq!(modules.reset_generation(realm), old_id + 1);

        let old_import =
            modules.import_generation_handle_for(realm, Some("/ordinary.mjs"), &old_generation);
        let runner_import =
            modules.import_generation_handle_for(realm, Some("/test.mjs"), &old_generation);
        assert!(Rc::ptr_eq(&old_import, &old_generation));
        assert_eq!(old_import.borrow().id, old_id);
        assert_eq!(runner_import.borrow().id, old_id + 1);
        assert!(!Rc::ptr_eq(&runner_import, &old_generation));
    }

    #[test]
    fn host_generation_alias_keeps_export_cell_but_scopes_namespace_identity() {
        let realm = RealmId::PRIMARY;
        let mut modules = ModuleMap::default();
        modules.publish_commonjs_namespace_in_generation(
            realm,
            0,
            "/host.cjs",
            HashMap::from([("value".to_owned(), Value::Number(1.0))]),
        );
        let old_namespace = modules.namespace_in_generation(realm, 0, "/host.cjs");
        let old_binding = modules
            .binding_in_generation(realm, 0, "/host.cjs", "value")
            .unwrap();
        let old_exports = modules
            .exports_in_generation(realm, 0, "/host.cjs")
            .expect("host exports are published");

        assert_eq!(modules.reset_generation(realm), 1);
        assert!(modules.alias_generation(realm, 0, 1, "/host.cjs"));
        assert!(modules.commonjs_namespace_ready_in_generation(realm, 1, "/host.cjs"));
        let new_namespace = modules.namespace_in_generation(realm, 1, "/host.cjs");
        let new_binding = modules
            .binding_in_generation(realm, 1, "/host.cjs", "value")
            .unwrap();
        let new_exports = modules
            .exports_in_generation(realm, 1, "/host.cjs")
            .expect("aliased host exports are published");
        let (Value::Object(old_namespace), Value::Object(new_namespace)) =
            (old_namespace, new_namespace)
        else {
            panic!("module namespace expected");
        };
        assert!(!Rc::ptr_eq(&old_namespace, &new_namespace));
        assert!(Rc::ptr_eq(&old_exports, &new_exports));
        let (Value::Cell(old_cell), Value::Cell(new_cell)) = (old_binding, new_binding) else {
            panic!("host exports must remain linked cells");
        };
        assert!(Rc::ptr_eq(&old_cell, &new_cell));
    }

    #[test]
    fn host_generation_alias_keeps_cached_namespace_failure() {
        let realm = RealmId::PRIMARY;
        let mut modules = ModuleMap::default();
        modules.publish_in_generation(
            realm,
            0,
            "/host.cjs",
            HashMap::from([("default".to_owned(), Value::Number(1.0))]),
        );
        let failure = Value::Object(tracked_object(ObjectData::default()));
        modules.fail_commonjs_namespace_in_generation(realm, 0, "/host.cjs", failure.clone());

        assert_eq!(modules.reset_generation(realm), 1);
        assert!(modules.alias_generation(realm, 0, 1, "/host.cjs"));
        assert!(matches!(
            modules.commonjs_namespace_failure_in_generation(realm, 1, "/host.cjs"),
            Some(Value::Object(value)) if matches!(failure, Value::Object(ref original) if Rc::ptr_eq(&value, original))
        ));
    }

    #[test]
    fn import_linked_before_publish_keeps_the_export_cell_through_a_cycle() {
        let realm = RealmId::PRIMARY;
        let mut modules = ModuleMap::default();
        modules.begin(realm, "/a.mjs");
        modules.begin(realm, "/b.mjs");

        // Both sides link the other's export before either finishes, as in an ESM cycle.
        let a_imports_b = modules.binding(realm, "/b.mjs", "run").unwrap();
        let b_imports_a = modules.binding(realm, "/a.mjs", "run").unwrap();
        let a_canonical = Rc::new(RefCell::new(Value::Number(1.0)));
        let b_canonical = Rc::new(RefCell::new(Value::Number(3.0)));
        modules.publish(
            realm,
            "/a.mjs",
            HashMap::from([("run".to_owned(), Value::Cell(Rc::clone(&a_canonical)))]),
        );
        assert_eq!(read_through_cell(&b_imports_a), Value::Number(1.0));
        modules.publish(
            realm,
            "/b.mjs",
            HashMap::from([("run".to_owned(), Value::Cell(Rc::clone(&b_canonical)))]),
        );
        assert_eq!(read_through_cell(&a_imports_b), Value::Number(3.0));
        modules.add_runner_candidate(realm, "/test.mjs");
        modules.begin(realm, "/test.mjs");
        modules.publish(realm, "/test.mjs", HashMap::new());
        assert!(modules.register_runner(realm, "/test.mjs"));
        modules.begin(realm, "/spy.mjs");
        modules.publish(realm, "/spy.mjs", HashMap::new());
        assert!(modules.register_spy_adapter(realm, "/spy.mjs"));

        let namespace = modules.namespace(realm, "/a.mjs");
        assert!(modules.spy_export(realm, &namespace, "run", Value::Number(2.0), "/spy.mjs"));
        assert_eq!(read_through_cell(&b_imports_a), Value::Number(2.0));
        let b_namespace = modules.namespace(realm, "/b.mjs");
        assert!(modules.spy_export(realm, &b_namespace, "run", Value::Number(4.0), "/spy.mjs"));
        assert_eq!(read_through_cell(&a_imports_b), Value::Number(4.0));
    }

    #[test]
    fn cyclic_cell_aliases_stay_in_the_dead_zone_instead_of_looping() {
        let cell = Rc::new(RefCell::new(Value::Undefined));
        *cell.borrow_mut() = Value::Cell(Rc::clone(&cell));
        assert_eq!(
            read_through_cell(&Value::Cell(cell)),
            Value::Native(Native::Uninitialized, RealmId::PRIMARY)
        );
    }
}
