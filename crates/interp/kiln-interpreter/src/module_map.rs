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
    status: ModuleStatus,
    /// Every name the module exports, as the binding the importer links to: a live cell where
    /// the binding is mutable, the value where it is not. Empty until the module is evaluated.
    exports: Rc<HashMap<String, Value>>,
    /// Stable cells for every published export. Even an immutable binding has a stable cell:
    /// the module loader can replace its value for an authorized test spy, and all linked
    /// imports and namespace views observe that replacement without changing namespace shape.
    bindings: HashMap<String, Cell>,
    /// The module's namespace object, created on first demand and never replaced, so a static
    /// `import * as ns` and every `import()` of the module see the same object.
    namespace: Option<Rc<RefCell<ObjectData>>>,
    /// Cells handed out for names whose module has not finished: an import across a cycle, or a
    /// dynamic import that links to a module still evaluating. Each starts in its dead zone and
    /// is filled when the module publishes.
    pending: Vec<(String, Cell)>,
    /// CommonJS body evaluation and its ESM namespace snapshot are separate operations.
    commonjs_namespace_ready: bool,
    commonjs_namespace_failure: Option<Value>,
}

impl ModuleRecord {
    fn new() -> Self {
        Self {
            status: ModuleStatus::Linked,
            exports: Rc::new(HashMap::new()),
            bindings: HashMap::new(),
            namespace: None,
            pending: Vec::new(),
            commonjs_namespace_ready: false,
            commonjs_namespace_failure: None,
        }
    }
}

/// The realm session's module map, keyed by realm and canonical module key (the module's path).
///
/// Keyed by realm as well as by module, because each realm has its own map: two realms
/// importing one specifier get two independent instances, which is what keeps a `ShadowRealm`
/// from sharing module state with the realm that made it.
#[derive(Default)]
pub struct ModuleMap {
    records: HashMap<(RealmId, u64, String), ModuleRecord>,
    generations: HashMap<RealmId, u64>,
    runner_candidates: HashMap<RealmId, std::collections::HashSet<String>>,
    runner_referrers: HashMap<RealmId, String>,
    spy_adapter_referrers: HashMap<RealmId, String>,
    /// Weak graph census shared by every VM in this realm session. Module execution allocates
    /// closures in distinct machines, while explicit collection can be requested from any one
    /// of them; keeping weak identities here lets that request see cross-module cycles.
    pub(super) gc_closures: Vec<Weak<Closure>>,
    pub(super) gc_closures_prune_at: usize,
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

    /// Start a fresh dynamic module graph while retaining prior records for existing bindings.
    pub fn reset_generation(&mut self, realm: RealmId) -> u64 {
        let next = self
            .generation(realm)
            .checked_add(1)
            .expect("dynamic module generation counter exhausted");
        self.generations.insert(realm, next);
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
        let Some(record) = self
            .records
            .get(&(realm, from_generation, key.to_owned()))
            .cloned()
        else {
            return false;
        };
        if !matches!(
            record.status,
            ModuleStatus::Evaluated | ModuleStatus::Failed(_)
        ) {
            return false;
        }
        let old_generation_namespace = self
            .records
            .get(&(realm, to_generation, key.to_owned()))
            .and_then(|existing| existing.namespace.clone());
        let pending = self
            .records
            .get_mut(&(realm, to_generation, key.to_owned()))
            .map(|existing| std::mem::take(&mut existing.pending))
            .unwrap_or_default();
        let mut alias = record;
        // Namespace wrappers belong to the module registry generation. Keep a namespace that
        // was already requested in the destination generation, but never carry over the old
        // wrapper merely because the host exports are cached.
        alias.namespace = old_generation_namespace;
        alias.pending = pending;
        self.records
            .insert((realm, to_generation, key.to_owned()), alias);
        if let Some(namespace) = self
            .records
            .get(&(realm, to_generation, key.to_owned()))
            .and_then(|record| record.namespace.clone())
            && let Value::Object(fresh) =
                build_namespace(&self.records[&(realm, to_generation, key.to_owned())].bindings)
        {
            *namespace.borrow_mut() = std::mem::take(&mut *fresh.borrow_mut());
        }
        let pending =
            std::mem::take(&mut self.record_in_generation(realm, to_generation, key).pending);
        let mut waiting = Vec::new();
        for (name, cell) in pending {
            if name == kiln_ir::NAMESPACE_IMPORT {
                let namespace = self.namespace_in_generation(realm, to_generation, key);
                *cell.borrow_mut() = namespace;
            } else if let Some(binding) = self
                .record_in_generation(realm, to_generation, key)
                .bindings
                .get(&name)
                .cloned()
            {
                *cell.borrow_mut() = Value::Cell(binding);
            } else {
                waiting.push((name, cell));
            }
        }
        self.record_in_generation(realm, to_generation, key).pending = waiting;
        true
    }

    fn record(&mut self, realm: RealmId, key: &str) -> &mut ModuleRecord {
        let generation = self.generation(realm);
        self.record_in_generation(realm, generation, key)
    }

    fn record_in_generation(
        &mut self,
        realm: RealmId,
        generation: u64,
        key: &str,
    ) -> &mut ModuleRecord {
        self.records
            .entry((realm, generation, key.to_owned()))
            .or_insert_with(ModuleRecord::new)
    }

    pub fn status_in_generation(
        &self,
        realm: RealmId,
        generation: u64,
        key: &str,
    ) -> Option<ModuleStatus> {
        self.records
            .get(&(realm, generation, key.to_owned()))
            .map(|record| record.status.clone())
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
        let record = self.records.get(&(realm, generation, key.to_owned()))?;
        matches!(record.status, ModuleStatus::Evaluated).then(|| Rc::clone(&record.exports))
    }

    /// Mark `key` as evaluating. The caller is the one evaluation it will ever have.
    pub fn begin(&mut self, realm: RealmId, key: &str) {
        self.record(realm, key).status = ModuleStatus::Evaluating;
    }

    pub fn begin_in_generation(&mut self, realm: RealmId, generation: u64, key: &str) {
        self.record_in_generation(realm, generation, key).status = ModuleStatus::Evaluating;
    }

    /// Record that evaluating `key` threw `error`.
    pub fn fail(&mut self, realm: RealmId, key: &str, error: Value) {
        self.record(realm, key).status = ModuleStatus::Failed(error);
    }

    pub fn fail_in_generation(&mut self, realm: RealmId, generation: u64, key: &str, error: Value) {
        self.record_in_generation(realm, generation, key).status = ModuleStatus::Failed(error);
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
        let record = self.record_in_generation(realm, generation, key);
        record.status = ModuleStatus::Evaluated;
        record.exports = Rc::new(exports);
        record.bindings = bindings;
        if let Some(namespace) = &record.namespace {
            if let Value::Object(fresh) = build_namespace(&record.bindings) {
                *namespace.borrow_mut() = std::mem::take(&mut *fresh.borrow_mut());
            }
        }
        let pending = std::mem::take(&mut record.pending);
        let mut waiting = Vec::new();
        for (name, cell) in pending {
            if name == kiln_ir::NAMESPACE_IMPORT {
                let namespace = self.namespace_in_generation(realm, generation, key);
                *cell.borrow_mut() = namespace;
                continue;
            }
            let record = self.record_in_generation(realm, generation, key);
            match record.bindings.get(&name) {
                Some(value) => {
                    /* Keep an import linked to the canonical export cell. Copying the current
                     * value breaks live-binding updates (including an authorized test spy) for
                     * modules that linked across a cycle before the exporter published. */
                    *cell.borrow_mut() = Value::Cell(Rc::clone(value));
                }
                None => waiting.push((name, cell)),
            }
        }
        self.record_in_generation(realm, generation, key).pending = waiting;
    }

    pub fn commonjs_namespace_ready_in_generation(
        &self,
        realm: RealmId,
        generation: u64,
        key: &str,
    ) -> bool {
        self.records
            .get(&(realm, generation, key.to_owned()))
            .is_some_and(|record| record.commonjs_namespace_ready)
    }

    pub fn commonjs_namespace_failure_in_generation(
        &self,
        realm: RealmId,
        generation: u64,
        key: &str,
    ) -> Option<Value> {
        self.records
            .get(&(realm, generation, key.to_owned()))
            .and_then(|record| record.commonjs_namespace_failure.clone())
    }

    pub fn fail_commonjs_namespace_in_generation(
        &mut self,
        realm: RealmId,
        generation: u64,
        key: &str,
        failure: Value,
    ) {
        self.record_in_generation(realm, generation, key)
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
        self.record_in_generation(realm, generation, key)
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
        let record = self.record_in_generation(realm, generation, key);
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
        if let Some(record) = self.records.get(&(realm, generation, key.to_owned())) {
            if let Some(cell) = record.bindings.get(name) {
                return Some(Value::Cell(Rc::clone(cell)));
            }
            if let Some(value) = lookup_export(&record.exports, name) {
                return Some(value);
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

    /// Register the one authorized spy adapter for this realm after its runner is known.
    pub fn register_spy_adapter(&mut self, realm: RealmId, key: &str) -> bool {
        if !self.runner_referrers.contains_key(&realm)
            || !self
                .records
                .keys()
                .any(|(record_realm, _, record_key)| *record_realm == realm && record_key == key)
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
        for ((record_realm, _, _), record) in &mut self.records {
            if *record_realm != realm
                || !record
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
        let record = self.record_in_generation(realm, generation, key);
        if let Some(namespace) = &record.namespace {
            return Value::Object(Rc::clone(namespace));
        }
        let built = build_namespace(&record.bindings);
        if let Value::Object(object) = &built {
            record.namespace = Some(Rc::clone(object));
        }
        built
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

        assert!(modules.alias_generation(realm, 0, 1, "/host.cjs"));
        assert!(modules.commonjs_namespace_ready_in_generation(realm, 1, "/host.cjs"));
        let new_namespace = modules.namespace_in_generation(realm, 1, "/host.cjs");
        let new_binding = modules
            .binding_in_generation(realm, 1, "/host.cjs", "value")
            .unwrap();
        let (Value::Object(old_namespace), Value::Object(new_namespace)) =
            (old_namespace, new_namespace)
        else {
            panic!("module namespace expected");
        };
        assert!(!Rc::ptr_eq(&old_namespace, &new_namespace));
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
        let failure = Value::Object(Rc::new(RefCell::new(ObjectData::default())));
        modules.fail_commonjs_namespace_in_generation(realm, 0, "/host.cjs", failure.clone());

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
