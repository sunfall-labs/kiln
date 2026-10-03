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
}

impl ModuleRecord {
    fn new() -> Self {
        Self {
            status: ModuleStatus::Linked,
            exports: Rc::new(HashMap::new()),
            bindings: HashMap::new(),
            namespace: None,
            pending: Vec::new(),
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
    records: HashMap<(RealmId, String), ModuleRecord>,
    runner_candidates: HashMap<RealmId, std::collections::HashSet<String>>,
    runner_referrers: HashMap<RealmId, String>,
    spy_adapter_referrers: HashMap<RealmId, String>,
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
    fn record(&mut self, realm: RealmId, key: &str) -> &mut ModuleRecord {
        self.records
            .entry((realm, key.to_owned()))
            .or_insert_with(ModuleRecord::new)
    }

    /// Where `key` is in its evaluation, or `None` if nothing has asked for it.
    pub fn status(&self, realm: RealmId, key: &str) -> Option<ModuleStatus> {
        self.records
            .get(&(realm, key.to_owned()))
            .map(|record| record.status.clone())
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
        let record = self.records.get(&(realm, key.to_owned()))?;
        matches!(record.status, ModuleStatus::Evaluated).then(|| Rc::clone(&record.exports))
    }

    /// Mark `key` as evaluating. The caller is the one evaluation it will ever have.
    pub fn begin(&mut self, realm: RealmId, key: &str) {
        self.record(realm, key).status = ModuleStatus::Evaluating;
    }

    /// Record that evaluating `key` threw `error`.
    pub fn fail(&mut self, realm: RealmId, key: &str, error: Value) {
        self.record(realm, key).status = ModuleStatus::Failed(error);
    }

    /// Publish `key`'s exports and mark it evaluated. Every cell handed out for one of its names
    /// while it was running now holds the value, and a namespace created early is filled in.
    pub fn publish(&mut self, realm: RealmId, key: &str, exports: HashMap<String, Value>) {
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
        let record = self.record(realm, key);
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
                let namespace = self.namespace(realm, key);
                *cell.borrow_mut() = namespace;
                continue;
            }
            let record = self.record(realm, key);
            match record.bindings.get(&name) {
                Some(value) => {
                    *cell.borrow_mut() = read_through_cell(&Value::Cell(Rc::clone(value)))
                }
                None => waiting.push((name, cell)),
            }
        }
        self.record(realm, key).pending = waiting;
    }

    /// A dead-zone cell for `name` of `key`, filled when `key` publishes. One cell per name, so
    /// every crossing of a cycle lands on the same place. `NAMESPACE_IMPORT` names the namespace.
    pub fn pending_cell(&mut self, realm: RealmId, key: &str, name: &str) -> Cell {
        let record = self.record(realm, key);
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
        if let Some(record) = self.records.get(&(realm, key.to_owned())) {
            if let Some(cell) = record.bindings.get(name) {
                return Some(Value::Cell(Rc::clone(cell)));
            }
            if let Some(value) = lookup_export(&record.exports, name) {
                return Some(value);
            }
        }
        Some(Value::Cell(self.pending_cell(realm, key, name)))
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

    /// Register the one authorized spy adapter for this realm after its runner is known.
    pub fn register_spy_adapter(&mut self, realm: RealmId, key: &str) -> bool {
        if !self.runner_referrers.contains_key(&realm)
            || !self.records.contains_key(&(realm, key.to_owned()))
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
        for ((record_realm, _), record) in &mut self.records {
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
        let record = self.record(realm, key);
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
}
