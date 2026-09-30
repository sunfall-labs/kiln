use super::*;

impl<'host, H: HostOps + ?Sized> Vm<'host, H> {
    /// Constructs a VM whose deterministic entropy is owned by its realm runtime policy.
    ///
    /// Keeping the seed as a plain value here avoids leaking Tokio or runtime ownership into the
    /// language machine while making ADR 0031 authoritative for every entropy consumer.
    pub(super) fn with_entropy_seed(
        program: Rc<Program>,
        strictness: Rc<Strictness>,
        host: &'host mut H,
        locals: Vec<Value>,
        limits: ExecutionLimits,
        capabilities: &RealmCapabilities,
        execution: ExecutionCapabilities,
    ) -> Result<Self, ExecutionError> {
        let timezone = resolve_timezone(host, &capabilities.timezone)?;
        let default_locale = resolve_locale(host, &capabilities.locale)?;
        let entropy = capabilities.entropy;
        // One group per execution, made here so that every entry point gets it and none has to
        // remember to. An agent's own machine replaces this with the group it was started from;
        // the empty one it was given costs a mutex and stops nothing when it is dropped.
        let agents = execution
            .host_agents()
            .map(|_| AgentGroup::new(execution.clone(), limits));
        Ok(Self {
            program,
            strict: strictness.body,
            strictness,
            host,
            stack: Vec::new(),
            locals,
            function: None,
            suspended: false,
            handlers: Vec::new(),
            arguments: Vec::new(),
            arguments_object: None,
            // The module body has no `arguments` at all, mapped or not.
            mapped_arguments: false,
            current_realm: RealmId::PRIMARY,
            shared_facades: Rc::new(RefCell::new(SharedCache::default())),
            isolated_graphs: Rc::new(RefCell::new(Vec::new())),
            actor_realm_id: kiln_runtime::workers::actor_request_id(),
            actor_graph_requests: Rc::new(RefCell::new(std::collections::HashMap::new())),
            async_context: AsyncContext::default(),
            esm_body_pending_checkpoint: false,
            this_value: Value::Undefined,
            new_target: Value::Undefined,
            suspended_awaiting: false,
            suspended_delegate: None,
            global_objects: vec![None],
            regexp_legacy_captures: vec![std::array::from_fn(|_| StringUnits::default())],
            process_object: None,
            intrinsics: Rc::new(Intrinsics::default()),
            native_depth: 0,
            frames: Vec::new(),
            global_base: 0,
            instruction: 0,
            steps: 0,
            checkpoint: if kiln_runtime::workers::current().is_some() {
                crate::vm::WORKER_QUANTUM.min(limits.max_steps)
            } else {
                limits.max_steps
            },
            agent_owner: agents.clone().map(AgentOwner),
            agents,
            agent_id: 0,
            async_waits: Vec::new(),
            symbol_serial: 0,
            realm: Realm::new(),
            clock_capability: capabilities.clock,
            timezone,
            default_locale,
            source_compiler: execution.source_compiler(),
            realm_bootstrap: execution.realm_bootstrap(),
            module_loader: execution.module_loader(),
            worker_host: execution.worker_host(),
            in_worker: kiln_runtime::workers::current().is_some(),
            module_path: None,
            host_realms: execution.host_realms(),
            host_agents: execution.host_agents(),
            argv: execution.argv().to_vec(),
            pending_host: Vec::new(),
            pending_node: Vec::new(),
            node_servers: Vec::new(),
            http_client_requests: std::collections::BTreeMap::new(),
            next_http_client_request_id: 1,
            module_realm: None,
            async_host: None,
            http_runtime: None,
            commonjs_loader: None,
            caught: None,
            fd_table: fs::VirtualFdTable::new(),
            uncaught_value: None,
            rejections: Rejections::default(),
            module_suspension: None,
            module_completion: None,
            self_bindings: Vec::new(),
            self_namespaces: Vec::new(),
            symbol_registry: Vec::new(),
            symbols: Vec::new(),
            template_objects: std::collections::HashMap::new(),
            random_state: match entropy {
                EntropyCapability::Deterministic | EntropyCapability::Host => DEFAULT_ENTROPY_SEED,
                EntropyCapability::Seeded(seed) => seed,
            },
            entropy,
            closures: Vec::new(),
            dynamic_modules: std::collections::HashMap::new(),
            dynamic_namespaces: std::collections::HashMap::new(),
            limits,
        })
    }

    /// Read the realm's civil-time clock.
    ///
    /// The scheduler clock remains virtual even in a host-clock realm: timers, `performance.now`
    /// and `process.hrtime` are monotonic measurements, not wall time. Only APIs whose meaning is
    /// an epoch instant cross HostOps, and only when the caller explicitly selected that policy.
    pub(super) fn wall_clock_milliseconds(
        &mut self,
        capability: ClockCapability,
        instruction: usize,
    ) -> Result<f64, ExecutionError> {
        match capability {
            ClockCapability::Virtual => Ok(self.with_realm(kiln_runtime::Realm::now_milliseconds)),
            ClockCapability::Host => self
                .host
                .call(HostOp::ClockNow, &[])
                .map_err(|error| ExecutionError::Host {
                    instruction,
                    message: error.to_string(),
                })?
                .as_number()
                .filter(|milliseconds| milliseconds.is_finite())
                .ok_or_else(|| ExecutionError::Host {
                    instruction,
                    message: "HostOp::ClockNow must return finite epoch milliseconds".to_owned(),
                }),
        }
    }

    pub(super) fn entropy_word(&mut self, instruction: usize) -> Result<u64, ExecutionError> {
        match self.entropy {
            EntropyCapability::Deterministic | EntropyCapability::Seeded(_) => {
                Ok(split_mix_64(&mut self.random_state))
            }
            EntropyCapability::Host => {
                let value = self.host.call(HostOp::EntropyU64, &[]).map_err(|error| {
                    ExecutionError::Host {
                        instruction,
                        message: error.to_string(),
                    }
                })?;
                match value {
                    HostValue::Number(bits) => Ok(bits),
                    _ => Err(ExecutionError::Host {
                        instruction,
                        message: "HostOp::EntropyU64 must return one u64 word".to_owned(),
                    }),
                }
            }
        }
    }

    pub(super) fn random_double(&mut self, instruction: usize) -> Result<f64, ExecutionError> {
        Ok((self.entropy_word(instruction)? >> 11) as f64 / 9_007_199_254_740_992.0)
    }

    pub(super) fn random_bytes(
        &mut self,
        size: usize,
        instruction: usize,
    ) -> Result<Vec<u8>, ExecutionError> {
        let mut bytes = Vec::with_capacity(size);
        while bytes.len() < size {
            let word = self.entropy_word(instruction)?.to_le_bytes();
            let take = (size - bytes.len()).min(8);
            bytes.extend_from_slice(&word[..take]);
        }
        Ok(bytes)
    }

    /// Cut the reference cycles this run created, so the process can reclaim what it allocated.
    ///
    /// A closure owns exactly three edges that the object graph can point back along: the
    /// `.prototype` object whose `constructor` is the closure itself, the closure's own
    /// properties, where a class keeps its statics, and the cells it captured, any of which may
    /// hold the closure. Cutting all three leaves every remaining edge pointing one way, which
    /// is all `Rc` needs to free the graph.
    ///
    /// This is sound only once no value from the run can be observed again, which is why it is
    /// not `Drop` and why [`execute_reclaiming`] is its only caller: `execute_module` hands live
    /// functions to the next module in the graph, and clearing those would hollow out an export.
    pub(super) fn release_cycles(&mut self) {
        for weak in self.closures.drain(..) {
            let Some(closure) = weak.upgrade() else {
                continue;
            };
            closure.prototype.replace(None);
            closure.properties.replace(ObjectData::default());
            for cell in &closure.upvalues {
                cell.replace(Value::Undefined);
            }
        }
        // The built-in store is a realm root too, and one that can hold anything: a program
        // that writes a closure onto `Array.prototype` puts a whole object graph in here.
        // Dropped rather than emptied slot by slot, because nothing points *into* it.
        self.intrinsics.entries.borrow_mut().clear();
        // The realm's own roots. `globalThis.globalThis` is the global object itself, so the
        // realm is a cycle on its own and survives the machine that built it -- taking with it
        // everything the program hung off the global object.
        let roots: Vec<Option<Value>> = self
            .global_objects
            .iter_mut()
            .map(Option::take)
            .chain(std::iter::once(self.process_object.take()))
            .collect();
        for root in roots
            .into_iter()
            .flatten()
            .chain(self.intrinsics.take_error_prototypes())
            .chain(self.intrinsics.take_unscopables())
        {
            if let Value::Object(object) = root {
                object.replace(ObjectData::default());
            }
        }
    }
}
