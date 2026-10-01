use super::*;

fn node_stream_write_bytes(value: Option<&Value>) -> Result<Vec<u8>, ExecutionError> {
    match value {
        Some(Value::String(_)) => node_body_bytes(value),
        Some(Value::TypedArray(view)) if view.kind == TypedKind::Uint8 => node_body_bytes(value),
        _ => Err(ExecutionError::TypeErrorMessage {
            message: "The \"chunk\" argument must be of type string or an instance of Buffer or Uint8Array".into(),
        }),
    }
}

impl<'host, H: HostOps + ?Sized> Vm<'host, H> {
    fn take_node_socket_queued_state(
        state: &Rc<RefCell<crate::net::NetSocketState>>,
    ) -> (Vec<Value>, Vec<Value>) {
        let mut state = state.borrow_mut();
        let mut write_callbacks = Vec::new();
        for callbacks in std::mem::take(&mut state.queued_write_callbacks) {
            write_callbacks.extend(callbacks);
        }
        write_callbacks.extend(std::mem::take(&mut state.corked_callbacks));
        let end_callbacks = std::mem::take(&mut state.end_callbacks);
        state.pending_write.clear();
        state.corked_write.clear();
        state.cork_depth = 0;
        state.shutdown_after_upgrade_fence = None;
        (write_callbacks, end_callbacks)
    }

    fn take_next_queued_write_callbacks(
        state: &Rc<RefCell<crate::net::NetSocketState>>,
    ) -> Vec<Value> {
        state
            .borrow_mut()
            .queued_write_callbacks
            .pop_front()
            .unwrap_or_default()
    }

    fn schedule_node_socket_queued_callbacks(
        &mut self,
        write_callbacks: Vec<Value>,
        end_callbacks: Vec<Value>,
    ) {
        let error = if !write_callbacks.is_empty() || !end_callbacks.is_empty() {
            let error = self.construct_error(
                ErrorKind::Error,
                &[Value::String("socket destroyed".into())],
            );
            if let Value::Object(object) = &error {
                object
                    .borrow_mut()
                    .set_non_enumerable("code", Value::String("ERR_STREAM_DESTROYED".into()));
            }
            Some(error)
        } else {
            None
        };
        if let Some(error) = error.as_ref() {
            for callback in write_callbacks {
                let scheduled = self.scheduled(Job::NextTick(callback, vec![error.clone()]));
                self.with_realm_mut(|realm| realm.queue_next_tick(scheduled));
            }
            for callback in end_callbacks {
                let scheduled = self.scheduled(Job::NextTick(callback, vec![error.clone()]));
                self.with_realm_mut(|realm| realm.queue_next_tick(scheduled));
            }
        }
    }

    fn call_node_socket_end_callbacks(
        &mut self,
        state: &Rc<RefCell<crate::net::NetSocketState>>,
        callback_error: Value,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let callbacks = std::mem::take(&mut state.borrow_mut().end_callbacks);
        for callback in callbacks {
            self.call_value(
                &callback,
                Value::Undefined,
                vec![callback_error.clone()],
                instruction,
            )?;
        }
        Ok(())
    }

    fn schedule_node_write_after_end(
        &mut self,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let error =
            self.construct_error(ErrorKind::Error, &[Value::String("write after end".into())]);
        if let Value::Object(error_object) = &error {
            error_object
                .borrow_mut()
                .set_non_enumerable("code", Value::String("ERR_STREAM_WRITE_AFTER_END".into()));
        }
        let callback = arguments
            .iter()
            .skip(1)
            .rev()
            .find(|value| is_callable(value));
        if let Some(callback) = callback {
            let callback = Value::Bound(Rc::new(BoundFunction {
                target: callback.clone(),
                this_value: Value::Undefined,
                arguments: vec![error.clone()],
                properties: RefCell::new(ObjectData::default()),
            }));
            let scheduled = self.scheduled(Job::NextTick(callback, Vec::new()));
            self.with_realm_mut(|realm| realm.queue_next_tick(scheduled));
        }
        let emit = self.read_property(receiver, "emit", instruction)?;
        let emit = Value::Bound(Rc::new(BoundFunction {
            target: emit,
            this_value: receiver.clone(),
            arguments: vec![Value::String("error".into()), error],
            properties: RefCell::new(ObjectData::default()),
        }));
        let scheduled = self.scheduled(Job::NextTick(emit, Vec::new()));
        self.with_realm_mut(|realm| realm.queue_next_tick(scheduled));
        Ok(())
    }

    fn node_connect_arguments(
        &mut self,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<(u16, String, Option<Value>), ExecutionError> {
        let options = matches!(arguments.first(), Some(Value::Object(_)));
        let (port_value, host_value) = if options {
            let bag = &arguments[0];
            // Keep Node's observable Get order: port, host, then hostname only when host is
            // nullish. These are ordinary Gets so inherited properties and accessors participate.
            let port = self.read_property(bag, "port", instruction)?;
            let mut host = self.read_property(bag, "host", instruction)?;
            if matches!(host, Value::Undefined | Value::Null) {
                host = self.read_property(bag, "hostname", instruction)?;
            }
            (port, host)
        } else {
            (
                arguments.first().cloned().unwrap_or(Value::Undefined),
                // Node overloads `connect(port, callback)` as an omitted-host call. Treat the
                // callback as the callback argument, not as a host to stringify.
                if arguments.get(1).is_some_and(is_callable) {
                    Value::Undefined
                } else {
                    arguments.get(1).cloned().unwrap_or(Value::Undefined)
                },
            )
        };
        if !matches!(port_value, Value::Number(_) | Value::String(_)) {
            return Err(ExecutionError::TypeErrorMessage {
                message: "The options.port property must be one of type number or string".into(),
            });
        }
        let port = self.as_number_argument(Some(&port_value), instruction)?;
        if !port.is_finite() || port.fract() != 0.0 || !(0.0..=65535.0).contains(&port) {
            return Err(ExecutionError::InvalidPort);
        }
        let host = if matches!(host_value, Value::Undefined | Value::Null) {
            "localhost".to_owned()
        } else {
            self.as_string(&host_value, instruction)?
        };
        let callback_index = if options {
            1
        } else if arguments.get(1).is_some_and(is_callable) {
            1
        } else {
            2
        };
        let callback = arguments
            .get(callback_index)
            .filter(|value| is_callable(value))
            .cloned();
        Ok((port as u16, host, callback))
    }

    /// Stamp a job with the context it is being queued in.
    pub(crate) fn scheduled(&self, job: Job) -> Scheduled {
        Scheduled {
            context: Rc::clone(&self.async_context),
            job,
        }
    }

    pub(crate) fn with_realm<R>(&self, operation: impl FnOnce(&Realm<Scheduled, Timer>) -> R) -> R {
        match &self.module_realm {
            Some(shared) => operation(&shared.borrow().realm),
            None => operation(&self.realm),
        }
    }

    pub(crate) fn with_realm_mut<R>(
        &mut self,
        operation: impl FnOnce(&mut Realm<Scheduled, Timer>) -> R,
    ) -> R {
        match &self.module_realm {
            Some(shared) => operation(&mut shared.borrow_mut().realm),
            None => operation(&mut self.realm),
        }
    }

    /// The realm session's rejection queue, or this machine's own when it has no session.
    pub(crate) fn with_rejections_mut<R>(
        &mut self,
        operation: impl FnOnce(&mut Rejections) -> R,
    ) -> R {
        match &self.module_realm {
            Some(shared) => operation(&mut shared.borrow_mut().rejections),
            None => operation(&mut self.rejections),
        }
    }

    pub(crate) fn push_pending_node(&mut self, pending: PendingNodeRequest) {
        if let Some(realm) = &self.module_realm {
            realm.borrow_mut().pending_node.push(pending);
        } else {
            self.pending_node.push(pending);
        }
    }

    fn take_pending_node_request(
        &mut self,
        request: kiln_host_abi::RequestId,
    ) -> Option<PendingNodeRequest> {
        if let Some(realm) = &self.module_realm {
            let mut realm = realm.borrow_mut();
            let index = realm
                .pending_node
                .iter()
                .position(|pending| pending.request() == request)?;
            Some(realm.pending_node.remove(index))
        } else {
            let index = self
                .pending_node
                .iter()
                .position(|pending| pending.request() == request)?;
            Some(self.pending_node.remove(index))
        }
    }

    fn close_late_connect_resource(
        &mut self,
        resource: kiln_host_abi::ResourceId,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let host = self.async_host.ok_or_else(|| ExecutionError::Host {
            instruction,
            message: "node:net requires the Node execution Adapter".into(),
        })?;
        let request = unsafe {
            ActiveAsyncHostOps::start(&mut *host, AsyncHostOp::CloseResource { resource }, false)
        };
        self.push_pending_node(PendingNodeRequest::CloseResource { request });
        Ok(())
    }

    pub(crate) fn push_pending_host(&mut self, pending: PendingHostRequest) {
        if let Some(realm) = &self.module_realm {
            realm.borrow_mut().pending_host.push(pending);
        } else {
            self.pending_host.push(pending);
        }
    }

    pub(crate) fn register_node_server(&mut self, state: &Rc<RefCell<NodeServerState>>) {
        if let Some(realm) = &self.module_realm {
            realm.borrow_mut().node_servers.push(Rc::clone(state));
        } else {
            self.node_servers.push(Rc::clone(state));
        }
    }

    fn node_server_identity(state: &Rc<RefCell<NodeServerState>>) -> usize {
        Rc::as_ptr(state) as usize
    }

    fn retain_active_node_server_object(
        &mut self,
        state: &Rc<RefCell<NodeServerState>>,
        object: Value,
    ) {
        let identity = Self::node_server_identity(state);
        if let Some(realm) = &self.module_realm {
            realm
                .borrow_mut()
                .active_node_server_objects
                .insert(identity, object);
        } else {
            self.active_node_server_objects.insert(identity, object);
        }
    }

    fn active_node_server_object(&self, state: &Rc<RefCell<NodeServerState>>) -> Option<Value> {
        let identity = Self::node_server_identity(state);
        if let Some(realm) = &self.module_realm {
            realm
                .borrow()
                .active_node_server_objects
                .get(&identity)
                .cloned()
        } else {
            self.active_node_server_objects.get(&identity).cloned()
        }
    }

    fn take_active_node_server_object(
        &mut self,
        state: &Rc<RefCell<NodeServerState>>,
    ) -> Option<Value> {
        let identity = Self::node_server_identity(state);
        if let Some(realm) = &self.module_realm {
            realm
                .borrow_mut()
                .active_node_server_objects
                .remove(&identity)
        } else {
            self.active_node_server_objects.remove(&identity)
        }
    }

    pub(crate) fn allocate_http_client_request_id(&mut self) -> u64 {
        if let Some(realm) = &self.module_realm {
            let mut realm = realm.borrow_mut();
            let id = realm.next_http_client_request_id;
            realm.next_http_client_request_id = id.wrapping_add(1).max(1);
            id
        } else {
            let id = self.next_http_client_request_id;
            self.next_http_client_request_id = id.wrapping_add(1).max(1);
            id
        }
    }

    pub(crate) fn register_http_client_request(
        &mut self,
        id: u64,
        state: &Rc<RefCell<HttpClientRequestState>>,
    ) {
        if let Some(realm) = &self.module_realm {
            realm
                .borrow_mut()
                .http_client_requests
                .insert(id, Rc::clone(state));
        } else {
            self.http_client_requests.insert(id, Rc::clone(state));
        }
    }

    pub(crate) fn http_client_request(
        &self,
        id: u64,
    ) -> Option<Rc<RefCell<HttpClientRequestState>>> {
        self.module_realm
            .as_ref()
            .and_then(|realm| realm.borrow().http_client_requests.get(&id).cloned())
            .or_else(|| self.http_client_requests.get(&id).cloned())
    }

    pub(crate) fn remove_http_client_request(&mut self, id: u64) {
        if let Some(realm) = &self.module_realm {
            realm.borrow_mut().http_client_requests.remove(&id);
        } else {
            self.http_client_requests.remove(&id);
        }
    }

    pub(crate) fn get_template_object(&self, site: u32) -> Option<Value> {
        let key = (self.current_realm, Rc::as_ptr(&self.program) as usize, site);
        if let Some(realm) = &self.module_realm {
            realm
                .borrow()
                .template_objects
                .get(&key)
                .map(|(_, value)| value.clone())
        } else {
            self.template_objects
                .get(&key)
                .map(|(_, value)| value.clone())
        }
    }

    pub(crate) fn insert_template_object(&mut self, site: u32, value: Value) {
        let key = (self.current_realm, Rc::as_ptr(&self.program) as usize, site);
        if let Some(realm) = &self.module_realm {
            realm
                .borrow_mut()
                .template_objects
                .insert(key, (Rc::downgrade(&self.program), value));
        } else {
            self.template_objects
                .insert(key, (Rc::downgrade(&self.program), value));
        }
    }

    pub(crate) fn dispatch_node_net(
        &mut self,
        native: Native,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        match native {
            Native::NodeNetCreateServer => {
                let first = arguments.first().cloned().unwrap_or(Value::Undefined);
                let (options, handler) = if is_callable(&first) {
                    (None, first)
                } else {
                    if !matches!(first, Value::Undefined | Value::Null | Value::Object(_)) {
                        return self.throw_node_invalid_arg_type(&format!(
                            "The \"options\" argument must be of type object. Received type {}",
                            first.type_name()
                        ));
                    }
                    let handler = arguments
                        .get(1)
                        .filter(|value| is_callable(value))
                        .cloned()
                        .unwrap_or(Value::Undefined);
                    (matches!(first, Value::Object(_)).then_some(first), handler)
                };
                let allow_half_open_value = if let Some(options) = options {
                    let value = self.read_property(&options, "allowHalfOpen", instruction)?;
                    if value.is_truthy() {
                        value
                    } else {
                        Value::Boolean(false)
                    }
                } else {
                    Value::Boolean(false)
                };
                let allow_half_open = allow_half_open_value.is_truthy();
                let state = Rc::new(RefCell::new(NodeServerState {
                    driver: TcpServerDriver::default(),
                    handler: handler.clone(),
                    protocol: ServerProtocol::Net,
                    allow_half_open,
                    timeout_milliseconds: 0.0,
                    streams: Vec::new(),
                    object: None,
                    fetch: false,
                    close_deferred: false,
                }));
                let object = node_server_object(&state);
                if let Value::Object(object) = &object {
                    object
                        .borrow_mut()
                        .set("allowHalfOpen", allow_half_open_value);
                }
                if is_callable(&handler) {
                    self.dispatch_node_event(
                        Native::NodeEventOn,
                        &object,
                        &[Value::String("connection".into()), handler],
                        instruction,
                    )?;
                }
                self.register_node_server(&state);
                Ok(object)
            }
            Native::NodeNetConnect => {
                let (port, host, callback) = self.node_connect_arguments(arguments, instruction)?;
                let state = Rc::new(RefCell::new(NetSocketState {
                    owner: None,
                    resource: None,
                    object: None,
                    readable: new_readable_state(DEFAULT_HIGH_WATER_MARK, None),
                    read_paused: false,
                    connecting: true,
                    pending_write: Vec::new(),
                    cork_depth: 0,
                    corked_write: Vec::new(),
                    corked_callbacks: Vec::new(),
                    queued_write_callbacks: Default::default(),
                    end_callbacks: Vec::new(),
                    upgrade_write_callbacks: Default::default(),
                    shutdown_after_upgrade_fence: None,
                    end_requested: false,
                    destroyed: false,
                    connect_request: None,
                    http_agent_request: None,
                }));
                let socket = node_socket_object(&state);
                self.decorate_node_socket(&socket, instruction)?;
                if let Some(callback) = callback {
                    self.dispatch_node_event(
                        Native::NodeEventOnce,
                        &socket,
                        &[Value::String("connect".into()), callback],
                        instruction,
                    )?;
                }
                let host_ops = self.async_host.ok_or_else(|| ExecutionError::Host {
                    instruction,
                    message: "node:net requires the Node execution Adapter".into(),
                })?;
                let request = unsafe {
                    ActiveAsyncHostOps::start(
                        &mut *host_ops,
                        AsyncHostOp::TcpConnect { host, port },
                        true,
                    )
                };
                state.borrow_mut().connect_request = Some(request);
                self.push_pending_node(PendingNodeRequest::Connect {
                    request,
                    target: PendingConnect::Net(state),
                });
                Ok(socket)
            }
            Native::NodeSocketWrite | Native::NodeSocketEnd | Native::NodeSocketDestroy => {
                self.dispatch_node_socket(native, receiver, arguments, instruction)
            }
            Native::NodeSocketCork | Native::NodeSocketUncork => {
                let socket = arguments
                    .first()
                    .ok_or_else(|| ExecutionError::TypeErrorMessage {
                        message: "Socket cork operation requires a socket receiver".into(),
                    })?;
                self.dispatch_node_socket(native, socket, &[], instruction)
            }
            _ => unreachable!("only node:net natives dispatch here"),
        }
    }

    fn decorate_node_socket(
        &mut self,
        socket: &Value,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        // The facade may have been initialized by another machine in this module realm.
        // Resolve the realm's shared process even when this machine has not read it yet.
        let process = self.process_object();
        let decorator = self.read_property(&process, "__kilnSocketDecorator", instruction)?;
        if is_callable(&decorator) {
            self.call_value(
                &decorator,
                Value::Undefined,
                vec![socket.clone()],
                instruction,
            )?;
        }
        Ok(())
    }

    pub(crate) fn dispatch_node_socket(
        &mut self,
        native: Native,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let state = node_net_socket_state(receiver)?;
        if native == Native::NodeSocketEnd {
            if let Some(chunk) = arguments.first().filter(|value| !is_callable(value)) {
                let _ = node_body_bytes(Some(chunk))?;
            }
        }
        if native == Native::NodeSocketEnd {
            if let Some(callback) = arguments.iter().rev().find(|value| is_callable(value)) {
                state.borrow_mut().end_callbacks.push(callback.clone());
            }
        }
        if native == Native::NodeSocketCork {
            let mut state = state.borrow_mut();
            if state.cork_depth >= u32::MAX as usize {
                return Err(ExecutionError::RangeErrorMessage {
                    message: "socket cork depth overflow".into(),
                });
            }
            state.cork_depth += 1;
            return Ok(Value::Undefined);
        }
        if native == Native::NodeSocketUncork {
            let bytes = {
                let mut state = state.borrow_mut();
                if state.cork_depth != 0 {
                    state.cork_depth -= 1;
                }
                if state.cork_depth != 0
                    || (state.corked_write.is_empty() && state.corked_callbacks.is_empty())
                {
                    return Ok(Value::Undefined);
                }
                let bytes = std::mem::take(&mut state.corked_write);
                if state.connecting && state.resource.is_none() {
                    state.pending_write.extend(bytes);
                    return Ok(Value::Undefined);
                }
                bytes
            };
            self.dispatch_node_socket(
                Native::NodeSocketWrite,
                receiver,
                &[crate::buffer::node_bytes_value(bytes)],
                instruction,
            )?;
            return Ok(Value::Undefined);
        }
        let (connecting, resource, destroyed, connect_request, agent_request) = {
            let state = state.borrow();
            (
                state.connecting,
                state.resource,
                state.destroyed,
                state.connect_request,
                state.http_agent_request.clone(),
            )
        };
        if native == Native::NodeSocketEnd && destroyed {
            let (write_callbacks, end_callbacks) = Self::take_node_socket_queued_state(&state);
            self.schedule_node_socket_queued_callbacks(write_callbacks, end_callbacks);
            return Ok(receiver.clone());
        }
        if connecting && resource.is_none() {
            match native {
                Native::NodeSocketWrite => {
                    let bytes = node_stream_write_bytes(arguments.first())?;
                    let mut state = state.borrow_mut();
                    if state.end_requested {
                        drop(state);
                        self.schedule_node_write_after_end(receiver, arguments, instruction)?;
                        return Ok(Value::Boolean(false));
                    }
                    if state.cork_depth != 0 {
                        state.corked_write.extend(bytes);
                        if let Some(callback) = arguments
                            .iter()
                            .skip(1)
                            .rev()
                            .find(|value| is_callable(value))
                        {
                            state.corked_callbacks.push(callback.clone());
                        }
                    } else {
                        state.pending_write.extend(bytes);
                        if let Some(callback) = arguments
                            .iter()
                            .skip(1)
                            .rev()
                            .find(|value| is_callable(value))
                        {
                            state.corked_callbacks.push(callback.clone());
                        }
                    }
                    let buffered = state
                        .pending_write
                        .len()
                        .saturating_add(state.corked_write.len());
                    return Ok(Value::Boolean(buffered < DEFAULT_HIGH_WATER_MARK));
                }
                Native::NodeSocketEnd => {
                    let data = arguments
                        .first()
                        .filter(|value| !matches!(value, Value::Undefined) && !is_callable(value))
                        .map(|value| node_body_bytes(Some(value)))
                        .transpose()?;
                    let mut state = state.borrow_mut();
                    state.cork_depth = 0;
                    let corked = std::mem::take(&mut state.corked_write);
                    state.pending_write.extend(corked);
                    if let Some(data) = data {
                        state.pending_write.extend(data);
                    }
                    state.end_requested = true;
                    if let Value::Object(object) = receiver {
                        object
                            .borrow_mut()
                            .set("readyState", Value::String("readOnly".into()));
                        object.borrow_mut().set("writable", Value::Boolean(false));
                    }
                    return Ok(receiver.clone());
                }
                Native::NodeSocketDestroy => {
                    if destroyed {
                        return Ok(receiver.clone());
                    }
                    {
                        let mut state = state.borrow_mut();
                        state.destroyed = true;
                        state.connecting = false;
                    }
                    let (queued_write_callbacks, end_callbacks) =
                        Self::take_node_socket_queued_state(&state);
                    if let Value::Object(object) = receiver {
                        let mut object = object.borrow_mut();
                        object.set("connecting", Value::Boolean(false));
                        object.set("destroyed", Value::Boolean(true));
                        object.set("readyState", Value::String("closed".into()));
                    }
                    self.schedule_node_socket_queued_callbacks(
                        queued_write_callbacks,
                        end_callbacks,
                    );
                    if let Some(request) = connect_request {
                        let canceled = if let Some(host) = self.async_host {
                            unsafe { ActiveAsyncHostOps::cancel(&mut *host, request) }
                        } else {
                            false
                        };
                        if canceled {
                            if let Some(realm) = &self.module_realm {
                                realm
                                    .borrow_mut()
                                    .pending_node
                                    .retain(|pending| pending.request() != request);
                            } else {
                                self.pending_node
                                    .retain(|pending| pending.request() != request);
                            }
                        }
                    }
                    if let Some((host, port, request_id)) = agent_request {
                        if let Some(runtime) = self.http_runtime {
                            let cancellation = unsafe {
                                ActiveHttpRuntime::cancel_client_connection(
                                    &mut *runtime,
                                    &HttpOrigin::http(&host, port),
                                    request_id,
                                )
                            };
                            if let AgentCancellation::Connecting {
                                next: Some(next_id),
                            } = cancellation
                            {
                                let queued = self.http_client_request(next_id).ok_or_else(|| {
                                    ExecutionError::Host {
                                        instruction,
                                        message: "HTTP Agent lost a queued ClientRequest after socket cancellation".into(),
                                    }
                                })?;
                                self.start_http_client_connection(&queued, instruction)?;
                            }
                            self.remove_http_client_request(request_id);
                        }
                    }
                    let emit = self.read_property(receiver, "emit", instruction)?;
                    let callback = Value::Bound(Rc::new(BoundFunction {
                        target: emit,
                        this_value: receiver.clone(),
                        arguments: vec![Value::String("close".into())],
                        properties: RefCell::new(ObjectData::default()),
                    }));
                    let scheduled = self.scheduled(Job::NextTick(callback, Vec::new()));
                    self.with_realm_mut(|realm| realm.queue_next_tick(scheduled));
                    return Ok(receiver.clone());
                }
                _ => {}
            }
        }
        // A refused connection has no server owner or stream resource. `destroy()` is still
        // valid in this terminal state; it closes the socket object locally and reports close
        // on the next tick instead of routing through a server that never owned it.
        if native == Native::NodeSocketDestroy && resource.is_none() {
            if destroyed {
                return Ok(receiver.clone());
            }
            if let Some(request) = connect_request {
                let canceled = if let Some(host) = self.async_host {
                    unsafe { ActiveAsyncHostOps::cancel(&mut *host, request) }
                } else {
                    false
                };
                if canceled {
                    if let Some(realm) = &self.module_realm {
                        realm
                            .borrow_mut()
                            .pending_node
                            .retain(|pending| pending.request() != request);
                    } else {
                        self.pending_node
                            .retain(|pending| pending.request() != request);
                    }
                }
            }
            {
                let mut socket_state = state.borrow_mut();
                socket_state.destroyed = true;
                socket_state.connecting = false;
            }
            let (write_callbacks, end_callbacks) = Self::take_node_socket_queued_state(&state);
            self.schedule_node_socket_queued_callbacks(write_callbacks, end_callbacks);
            if let Value::Object(object) = receiver {
                let mut object = object.borrow_mut();
                object.set("connecting", Value::Boolean(false));
                object.set("destroyed", Value::Boolean(true));
                object.set("readyState", Value::String("closed".into()));
            }
            let emit = self.read_property(receiver, "emit", instruction)?;
            let callback = Value::Bound(Rc::new(BoundFunction {
                target: emit,
                this_value: receiver.clone(),
                arguments: vec![Value::String("close".into())],
                properties: RefCell::new(ObjectData::default()),
            }));
            let scheduled = self.scheduled(Job::NextTick(callback, Vec::new()));
            self.with_realm_mut(|realm| realm.queue_next_tick(scheduled));
            return Ok(receiver.clone());
        }
        let (server, resource) = {
            let state = state.borrow();
            let server = state
                .owner
                .as_ref()
                .and_then(Weak::upgrade)
                .ok_or_else(|| ExecutionError::Host {
                    instruction,
                    message: "node:net socket lost its server".into(),
                })?;
            let resource = state.resource.ok_or_else(|| ExecutionError::Host {
                instruction,
                message: "node:net socket is not connected".into(),
            })?;
            (server, resource)
        };
        let mut destroyed_queued_callbacks = None;
        if native == Native::NodeSocketDestroy {
            if destroyed {
                return Ok(receiver.clone());
            }
            {
                let mut socket_state = state.borrow_mut();
                socket_state.destroyed = true;
                socket_state.connecting = false;
            }
            destroyed_queued_callbacks = Some(Self::take_node_socket_queued_state(&state));
            if let Value::Object(object) = receiver {
                let mut object = object.borrow_mut();
                object.set("destroyed", Value::Boolean(true));
                object.set("readyState", Value::String("closed".into()));
            }
        }
        if native == Native::NodeSocketDestroy
            && server.borrow().protocol != ServerProtocol::Net
            && server
                .borrow()
                .streams
                .iter()
                .find(|stream| stream.resource() == resource)
                .is_some_and(|stream| stream.runtime_http)
        {
            let runtime = self.http_runtime.ok_or_else(|| ExecutionError::Host {
                instruction,
                message: "node:http requires the realm HTTP runtime".into(),
            })?;
            // A connection the engine has already let go of (its `Closed` may still be queued)
            // makes this the no-op that Node's `destroy` on a closed socket is.
            let _ = unsafe {
                ActiveHttpRuntime::close_connection(&mut *runtime, resource, HttpCloseMode::Abort)
            };
            if let Some((write_callbacks, end_callbacks)) = destroyed_queued_callbacks.take() {
                self.schedule_node_socket_queued_callbacks(write_callbacks, end_callbacks);
            }
            return Ok(receiver.clone());
        }
        if native == Native::NodeSocketWrite
            && server
                .borrow()
                .streams
                .iter()
                .find(|stream| stream.resource() == resource)
                .is_some_and(|stream| {
                    stream.driver.state().end_requested() || stream.driver.state().write_closed()
                })
        {
            // Keep Node's chunk validation before the asynchronous write-after-end report.
            let _ = node_stream_write_bytes(arguments.first())?;
            self.schedule_node_write_after_end(receiver, arguments, instruction)?;
            return Ok(Value::Boolean(false));
        }
        if native == Native::NodeSocketWrite {
            let bytes = node_stream_write_bytes(arguments.first())?;
            let mut state = state.borrow_mut();
            if state.cork_depth != 0 {
                state.corked_write.extend(bytes);
                if let Some(callback) = arguments
                    .iter()
                    .skip(1)
                    .rev()
                    .find(|value| is_callable(value))
                {
                    state.corked_callbacks.push(callback.clone());
                }
                let buffered = state
                    .pending_write
                    .len()
                    .saturating_add(state.corked_write.len());
                return Ok(Value::Boolean(buffered < DEFAULT_HIGH_WATER_MARK));
            }
        }
        let mut write_callbacks =
            if matches!(native, Native::NodeSocketWrite | Native::NodeSocketEnd) {
                std::mem::take(&mut state.borrow_mut().corked_callbacks)
            } else {
                Vec::new()
            };
        if native == Native::NodeSocketWrite
            && let Some(callback) = arguments
                .iter()
                .skip(1)
                .rev()
                .find(|value| is_callable(value))
        {
            write_callbacks.push(callback.clone());
        }
        if matches!(native, Native::NodeSocketWrite | Native::NodeSocketEnd)
            && let Some(id) = server
                .borrow()
                .streams
                .iter()
                .find(|stream| stream.resource() == resource)
                .and_then(|stream| stream.upgrade_request_id)
        {
            let bytes = if native == Native::NodeSocketWrite {
                node_stream_write_bytes(arguments.first())?
            } else {
                let mut bytes = state.borrow().corked_write.clone();
                if let Some(data) = arguments
                    .first()
                    .filter(|value| !matches!(value, Value::Undefined) && !is_callable(value))
                    .map(|value| node_body_bytes(Some(value)))
                    .transpose()?
                {
                    bytes.extend(data);
                }
                bytes
            };
            let runtime = self.http_runtime.ok_or_else(|| ExecutionError::Host {
                instruction,
                message: "node:http requires the realm HTTP runtime".into(),
            })?;
            let receipt =
                unsafe { ActiveHttpRuntime::write_upgrade(&mut *runtime, resource, id, &bytes) }
                    .ok_or_else(|| ExecutionError::Host {
                        instruction,
                        message: "HTTP upgrade write is no longer available".into(),
                    })?;
            let mut callbacks = std::mem::take(&mut state.borrow_mut().corked_callbacks);
            callbacks.extend(write_callbacks);
            if !callbacks.is_empty() {
                state
                    .borrow_mut()
                    .upgrade_write_callbacks
                    .entry(receipt.fence)
                    .or_default()
                    .extend(callbacks);
            }
            if native == Native::NodeSocketEnd {
                let mut socket_state = state.borrow_mut();
                socket_state.cork_depth = 0;
                socket_state.corked_write.clear();
                socket_state.end_requested = true;
                socket_state.shutdown_after_upgrade_fence = Some(receipt.fence);
                if let Value::Object(object) = receiver {
                    object
                        .borrow_mut()
                        .set("readyState", Value::String("readOnly".into()));
                    object.borrow_mut().set("writable", Value::Boolean(false));
                }
                return Ok(receiver.clone());
            }
            return Ok(Value::Boolean(receipt.disposition.ready));
        }
        let mut deferred_write = native == Native::NodeSocketWrite;
        let (result, actions) = {
            let mut server_state = server.borrow_mut();
            let connection = server_state
                .streams
                .iter_mut()
                .find(|stream| stream.resource() == resource)
                .ok_or_else(|| ExecutionError::Host {
                    instruction,
                    message: "node:net socket lost its stream".into(),
                })?;
            match native {
                Native::NodeSocketWrite => {
                    let (disposition, actions) =
                        connection.driver.write(node_body_bytes(arguments.first())?);
                    (Value::Boolean(disposition.ready), actions)
                }
                Native::NodeSocketEnd => {
                    let mut data = arguments
                        .first()
                        .filter(|value| !matches!(value, Value::Undefined) && !is_callable(value))
                        .map(|value| node_body_bytes(Some(value)))
                        .transpose()?;
                    let corked = {
                        let mut state = state.borrow_mut();
                        state.cork_depth = 0;
                        std::mem::take(&mut state.corked_write)
                    };
                    if !corked.is_empty() {
                        let mut bytes = corked;
                        if let Some(data) = data.take() {
                            bytes.extend(data);
                        }
                        data = Some(bytes);
                    }
                    deferred_write = data.as_ref().is_some_and(|data| !data.is_empty());
                    let mut actions = Vec::new();
                    if !deferred_write && !write_callbacks.is_empty() {
                        // `write('', callback)` is still an ordered write even though `end('')`
                        // itself does not create a transport write. Keep callbacks for corked
                        // empty writes behind earlier bytes by placing an empty write in the
                        // driver's queue before requesting shutdown.
                        let (_, empty_write) = connection.driver.write(Vec::new());
                        actions.extend(empty_write);
                        deferred_write = true;
                    }
                    let (_, end_actions) = connection.driver.end(data);
                    actions.extend(end_actions);
                    (receiver.clone(), actions)
                }
                Native::NodeSocketDestroy => (receiver.clone(), connection.driver.close_actions()),
                _ => unreachable!("only socket natives dispatch here"),
            }
        };
        sync_node_socket_ready_state(&server, resource);
        let mut write_submitted = false;
        for action in actions {
            if matches!(&action, StreamAction::Start(AsyncHostOp::TcpWrite { .. })) {
                write_submitted = true;
                self.start_http_stream_action_with_callbacks(
                    &server,
                    resource,
                    action,
                    instruction,
                    write_callbacks,
                )?;
                write_callbacks = Vec::new();
            } else {
                self.start_http_stream_action(&server, resource, action, instruction)?;
            }
        }
        if deferred_write && !write_submitted {
            // The driver accepted the bytes but another write is still in flight. Keep this
            // callback group paired with that queue entry; the next TCP_WRITE action consumes
            // exactly one group when it is submitted.
            state
                .borrow_mut()
                .queued_write_callbacks
                .push_back(write_callbacks);
        }
        // Cancellation actions above extract callbacks attached to the in-flight write and
        // schedule those before callbacks for writes still held in the driver's queue. Keep
        // `end` callbacks after both groups, matching Writable's destroy ordering.
        if let Some((write_callbacks, end_callbacks)) = destroyed_queued_callbacks {
            self.schedule_node_socket_queued_callbacks(write_callbacks, end_callbacks);
        }
        Ok(result)
    }

    pub(crate) fn dispatch_node_readable(
        &mut self,
        native: Native,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        if node_readable_state(receiver).is_ok() {
            return self.dispatch_memory_readable(native, receiver, arguments, instruction);
        }
        if let Ok(state) = node_net_socket_state(receiver) {
            let flow_control = matches!(
                native,
                Native::NodeReadablePause | Native::NodeReadableResume
            );
            if flow_control {
                // Publish before draining buffered data: a data listener can recursively pause
                // or destroy the socket, and the outer resume must not overwrite that decision.
                state.borrow_mut().read_paused = native == Native::NodeReadablePause;
            }
            let readable = Rc::clone(&state.borrow().readable);
            let result = self.dispatch_readable_state(
                native,
                receiver,
                arguments,
                instruction,
                &readable,
                false,
            )?;
            if !matches!(
                native,
                Native::NodeReadablePause | Native::NodeReadableResume
            ) {
                return Ok(result);
            }
            let (paused, destroyed, owner, resource) = {
                let state = state.borrow();
                (
                    state.read_paused,
                    state.destroyed,
                    state.owner.as_ref().and_then(Weak::upgrade),
                    state.resource,
                )
            };
            if destroyed {
                return Ok(result);
            }
            let (Some(owner), Some(resource)) = (owner, resource) else {
                // The socket may be paused before its connect event. The state bit is observed
                // when the transport is attached, so no read is lost or submitted early.
                return Ok(result);
            };
            let actions = {
                let mut owner = owner.borrow_mut();
                let Some(connection) = owner
                    .streams
                    .iter_mut()
                    .find(|stream| stream.resource() == resource)
                else {
                    return Err(ExecutionError::Host {
                        instruction,
                        message: "node:net socket lost its stream".into(),
                    });
                };
                if connection.runtime_http {
                    // Hyper owns reads until an upgrade transfers the transport to the socket.
                    return Ok(result);
                }
                if paused {
                    connection.driver.pause_reading();
                    Vec::new()
                } else {
                    connection.driver.resume_reading()
                }
            };
            for action in actions {
                self.start_http_stream_action(&owner, resource, action, instruction)?;
            }
            return Ok(result);
        }
        let state = node_http_incoming(receiver)?;
        let (resource, readable) = {
            let state = state.borrow();
            (state.stream, Rc::clone(&state.readable))
        };
        let result = self.dispatch_readable_state(
            native,
            receiver,
            arguments,
            instruction,
            &readable,
            false,
        )?;
        if !matches!(
            native,
            Native::NodeReadablePause | Native::NodeReadableResume
        ) {
            return Ok(result);
        }
        let owner = self
            .http_server_for_stream(resource)
            .ok_or_else(|| ExecutionError::Host {
                instruction,
                message: "node:http request lost its connection".into(),
            })?;
        let actions = {
            let mut owner = owner.borrow_mut();
            let connection = owner
                .streams
                .iter_mut()
                .find(|stream| stream.resource() == resource)
                .ok_or_else(|| ExecutionError::Host {
                    instruction,
                    message: "node:http request lost its stream".into(),
                })?;
            if connection.runtime_http {
                // Hyper owns socket reads for runtime HTTP. Node readable pause/resume only
                // changes delivery from the already-decoded IncomingMessage buffer; driving the
                // legacy StreamDriver here would start a second raw TcpRead beside Hyper's and
                // break completion ownership.
                return Ok(result);
            }
            match native {
                Native::NodeReadablePause => {
                    connection.driver.pause_reading();
                    Vec::new()
                }
                Native::NodeReadableResume => {
                    let actions = connection.driver.resume_reading();
                    if connection.applying_read_completion {
                        Vec::new()
                    } else {
                        actions
                    }
                }
                _ => unreachable!("only readable flow-control natives dispatch here"),
            }
        };
        for action in actions {
            self.start_http_stream_action(&owner, resource, action, instruction)?;
        }
        Ok(result)
    }

    pub(crate) fn dispatch_memory_readable(
        &mut self,
        native: Native,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let state = node_readable_state(receiver)?;
        self.dispatch_readable_state(native, receiver, arguments, instruction, &state, true)
    }

    pub(crate) fn dispatch_readable_state(
        &mut self,
        native: Native,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
        state: &Rc<RefCell<ReadableState>>,
        call_read_hook: bool,
    ) -> Result<Value, ExecutionError> {
        match native {
            Native::NodeReadablePush => {
                let chunk = arguments.first().cloned().unwrap_or(Value::Undefined);
                let (event, payload, emit_end, ready, call_read) = {
                    let mut state = state.borrow_mut();
                    if matches!(chunk, Value::Null) {
                        state.ended = true;
                        state.reading = false;
                        let emit_end =
                            state.flowing && state.buffer.is_empty() && !state.end_emitted;
                        if emit_end {
                            state.end_emitted = true;
                        }
                        ("readable", Vec::new(), emit_end, false, false)
                    } else {
                        let empty = matches!(&chunk, Value::String(text) if text.is_empty());
                        if !empty && !state.flowing {
                            state.buffered_bytes = state
                                .buffered_bytes
                                .saturating_add(readable_chunk_length(&chunk));
                            state.buffer.push(chunk.clone());
                        }
                        let wants_more =
                            state.buffered_bytes < state.high_water_mark && !state.ended;
                        state.reading = wants_more;
                        (
                            if state.flowing { "data" } else { "readable" },
                            state.flowing.then_some(chunk).into_iter().collect(),
                            false,
                            state.buffered_bytes < state.high_water_mark,
                            empty && call_read_hook && wants_more,
                        )
                    }
                };
                sync_readable_public_state(receiver, &state.borrow());
                let mut event_arguments = vec![Value::String(event.into())];
                event_arguments.extend(payload);
                self.dispatch_node_event(
                    Native::NodeEventEmit,
                    receiver,
                    &event_arguments,
                    instruction,
                )?;
                if emit_end {
                    self.dispatch_node_event(
                        Native::NodeEventEmit,
                        receiver,
                        &[Value::String("end".into())],
                        instruction,
                    )?;
                }
                if call_read {
                    let callback = self.read_property(receiver, "_read", instruction)?;
                    if is_callable(&callback) {
                        self.call_value(&callback, receiver.clone(), Vec::new(), instruction)?;
                    }
                }
                return Ok(Value::Boolean(ready));
            }
            Native::NodeReadableRead => {
                let (value, call_read, emit_end) = {
                    let mut state = state.borrow_mut();
                    let value = if state.buffer.is_empty() {
                        Value::Null
                    } else {
                        let value = state.buffer.remove(0);
                        state.buffered_bytes = state
                            .buffered_bytes
                            .saturating_sub(readable_chunk_length(&value));
                        value
                    };
                    let emit_end = state.ended && state.buffer.is_empty() && !state.end_emitted;
                    if emit_end {
                        state.end_emitted = true;
                    }
                    let call_read = !state.ended
                        && state.buffered_bytes < state.high_water_mark
                        && (matches!(value, Value::Null) || state.buffer.is_empty());
                    if call_read {
                        state.reading = true;
                    }
                    (value, call_read, emit_end)
                };
                sync_readable_public_state(receiver, &state.borrow());
                if call_read && call_read_hook {
                    let callback = self.read_property(receiver, "_read", instruction)?;
                    if is_callable(&callback) {
                        self.call_value(&callback, receiver.clone(), Vec::new(), instruction)?;
                    }
                }
                if emit_end {
                    self.dispatch_node_event(
                        Native::NodeEventEmit,
                        receiver,
                        &[Value::String("end".into())],
                        instruction,
                    )?;
                }
                return Ok(value);
            }
            Native::NodeReadableDestroy => {
                state.borrow_mut().destroyed = true;
                sync_readable_public_state(receiver, &state.borrow());
                if let Some(error) = arguments
                    .first()
                    .filter(|value| !matches!(value, Value::Undefined))
                {
                    self.dispatch_node_event(
                        Native::NodeEventEmit,
                        receiver,
                        &[Value::String("error".into()), error.clone()],
                        instruction,
                    )?;
                }
                self.dispatch_node_event(
                    Native::NodeEventEmit,
                    receiver,
                    &[Value::String("close".into())],
                    instruction,
                )?;
                return Ok(receiver.clone());
            }
            Native::NodeReadableSetEncoding => {
                let encoding = arguments
                    .first()
                    .map(Value::to_display_string)
                    .unwrap_or_else(|| "utf8".into())
                    .to_ascii_lowercase();
                if !matches!(encoding.as_str(), "utf8" | "utf-8") {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: format!("unsupported Node stream encoding: {encoding}"),
                    });
                }
                state.borrow_mut().encoding = Some("utf8".into());
                return Ok(receiver.clone());
            }
            Native::NodeReadablePipe => {
                let destination = arguments.first().cloned().unwrap_or(Value::Undefined);
                if !is_reflect_target(&destination) {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "The destination must be a writable stream".into(),
                    });
                }
                state.borrow_mut().pipes.push(destination.clone());
                // `pipe()` switches a readable into flowing mode. Existing buffered chunks are
                // delivered through the destination's ordinary `write`, preserving overrides
                // and backpressure hooks rather than reaching into writable state directly.
                let chunks = {
                    let mut state = state.borrow_mut();
                    state.flowing = true;
                    state.buffered_bytes = 0;
                    std::mem::take(&mut state.buffer)
                };
                for chunk in chunks {
                    let write = self.read_property(&destination, "write", instruction)?;
                    if is_callable(&write) {
                        self.call_value(&write, destination.clone(), vec![chunk], instruction)?;
                    }
                }
                sync_readable_public_state(receiver, &state.borrow());
                return Ok(destination);
            }
            Native::NodeReadableUnpipe => {
                let destination = arguments.first();
                let removed = {
                    let mut state = state.borrow_mut();
                    let before = state.pipes.len();
                    match destination {
                        Some(destination) if !matches!(destination, Value::Undefined) => {
                            state.pipes.retain(|pipe| pipe != destination);
                        }
                        _ => state.pipes.clear(),
                    }
                    state.flowing = false;
                    before != state.pipes.len()
                };
                if removed {
                    self.dispatch_node_event(
                        Native::NodeEventEmit,
                        receiver,
                        &[Value::String("unpipe".into())],
                        instruction,
                    )?;
                }
                sync_readable_public_state(receiver, &state.borrow());
                return Ok(receiver.clone());
            }
            Native::NodeReadablePause | Native::NodeReadableResume => {
                let resume = matches!(native, Native::NodeReadableResume);
                let buffered = {
                    let mut state = state.borrow_mut();
                    state.flowing = resume;
                    if resume {
                        std::mem::take(&mut state.buffer)
                    } else {
                        Vec::new()
                    }
                };
                sync_readable_public_state(receiver, &state.borrow());
                for chunk in buffered {
                    self.dispatch_node_event(
                        Native::NodeEventEmit,
                        receiver,
                        &[Value::String("data".into()), chunk],
                        instruction,
                    )?;
                }
                let emit_end = {
                    let mut state = state.borrow_mut();
                    let emit = resume && state.ended && !state.end_emitted;
                    if emit {
                        state.end_emitted = true;
                    }
                    emit
                };
                if emit_end {
                    sync_readable_public_state(receiver, &state.borrow());
                    self.dispatch_node_event(
                        Native::NodeEventEmit,
                        receiver,
                        &[Value::String("end".into())],
                        instruction,
                    )?;
                }
                return Ok(receiver.clone());
            }
            _ => {}
        }
        Err(ExecutionError::Host {
            instruction,
            message: "unknown Node readable operation".into(),
        })
    }

    pub(crate) fn dispatch_memory_writable(
        &mut self,
        native: Native,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let state = node_writable_state(receiver)?;
        match native {
            Native::NodeWritableWrite => {
                if state.borrow().ended {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "write after end".into(),
                    });
                }
                let chunk = arguments.first().cloned().unwrap_or(Value::Undefined);
                if let Value::Object(obj) = receiver
                    && let Some(NodeObject::Zlib(zlib_state)) = &obj.borrow().node_object
                {
                    let bytes = match &chunk {
                        Value::TypedArray(view) => view.bytes(),
                        Value::ArrayBuffer(ab) => ab.bytes.borrow().clone(),
                        Value::String(s) => s.to_lossy_string().into_bytes(),
                        _ => Vec::new(),
                    };
                    zlib_state
                        .accumulated
                        .borrow_mut()
                        .extend_from_slice(&bytes);
                }
                let callback = arguments
                    .iter()
                    .rev()
                    .find(|value| is_callable(value))
                    .cloned();
                let writer = state.borrow().write.clone().or_else(|| {
                    self.read_property(receiver, "_write", instruction)
                        .ok()
                        .filter(is_callable)
                });
                if let Some(writer) = writer {
                    let done = callback
                        .clone()
                        .unwrap_or(Value::Native(Native::NodeUtilDebugNoop, RealmId::PRIMARY));
                    self.call_value(
                        &writer,
                        receiver.clone(),
                        vec![chunk, Value::String("buffer".into()), done],
                        instruction,
                    )?;
                } else {
                    if let Ok(Value::Number(fd_bits)) =
                        self.read_property(receiver, "fd", instruction)
                    {
                        let fd = fd_bits as i32;
                        if fd == 1 || fd == 2 {
                            let op = if fd == 1 {
                                HostOp::WriteStdout
                            } else {
                                HostOp::WriteStderr
                            };
                            let text = match &chunk {
                                Value::String(s) => s.to_lossy_string(),
                                Value::TypedArray(view) => {
                                    String::from_utf8_lossy(&view.bytes()).into_owned()
                                }
                                Value::ArrayBuffer(ab) => {
                                    String::from_utf8_lossy(&ab.bytes.borrow()).into_owned()
                                }
                                _ => String::new(),
                            };
                            if !text.is_empty() {
                                let trimmed = text
                                    .strip_suffix("\r\n")
                                    .or_else(|| text.strip_suffix('\n'))
                                    .unwrap_or(&text);
                                self.host
                                    .call(
                                        op,
                                        &[HostValue::from(Value::String(
                                            trimmed.to_owned().into(),
                                        ))],
                                    )
                                    .map_err(|error| ExecutionError::Host {
                                        instruction,
                                        message: error.to_string(),
                                    })?;
                            }
                        }
                    }
                    if let Some(callback) = callback {
                        self.call_value(&callback, Value::Undefined, Vec::new(), instruction)?;
                    }
                }
                Ok(Value::Boolean(true))
            }
            Native::NodeWritableEnd => {
                if let Some(chunk) = arguments
                    .first()
                    .filter(|value| !matches!(value, Value::Undefined) && !is_callable(value))
                    .cloned()
                {
                    self.dispatch_memory_writable(
                        Native::NodeWritableWrite,
                        receiver,
                        &[chunk],
                        instruction,
                    )?;
                }
                {
                    let mut state = state.borrow_mut();
                    state.ended = true;
                    state.finished = true;
                }
                sync_writable_public_state(receiver, &state.borrow());
                let maybe_zlib = if let Value::Object(obj) = receiver {
                    if let Some(NodeObject::Zlib(zlib_state)) = &obj.borrow().node_object {
                        Some((zlib_state.kind, zlib_state.accumulated.borrow().clone()))
                    } else {
                        None
                    }
                } else {
                    None
                };
                if let Some((kind, input_bytes)) = maybe_zlib {
                    match zlib::transform_zlib(kind, &input_bytes) {
                        Ok(decompressed) => {
                            let out_buf = node_bytes_value(decompressed);
                            self.dispatch_node_readable(
                                Native::NodeReadablePush,
                                receiver,
                                &[out_buf],
                                instruction,
                            )?;
                            self.dispatch_node_readable(
                                Native::NodeReadablePush,
                                receiver,
                                &[Value::Null],
                                instruction,
                            )?;
                        }
                        Err(err) => {
                            let error = self.construct_error(
                                ErrorKind::Error,
                                &[Value::String(err.to_string().into())],
                            );
                            self.dispatch_node_event(
                                Native::NodeEventEmit,
                                receiver,
                                &[Value::String("error".into()), error],
                                instruction,
                            )?;
                        }
                    }
                }
                self.dispatch_node_event(
                    Native::NodeEventEmit,
                    receiver,
                    &[Value::String("finish".into())],
                    instruction,
                )?;
                if let Some(callback) = arguments.iter().rev().find(|value| is_callable(value)) {
                    self.call_value(callback, Value::Undefined, Vec::new(), instruction)?;
                }
                Ok(receiver.clone())
            }
            Native::NodeWritableDestroy => {
                state.borrow_mut().destroyed = true;
                sync_writable_public_state(receiver, &state.borrow());
                if let Some(error) = arguments
                    .first()
                    .filter(|value| !matches!(value, Value::Undefined))
                {
                    self.dispatch_node_event(
                        Native::NodeEventEmit,
                        receiver,
                        &[Value::String("error".into()), error.clone()],
                        instruction,
                    )?;
                }
                self.dispatch_node_event(
                    Native::NodeEventEmit,
                    receiver,
                    &[Value::String("close".into())],
                    instruction,
                )?;
                Ok(receiver.clone())
            }
            _ => Err(ExecutionError::Host {
                instruction,
                message: "unknown Node writable operation".into(),
            }),
        }
    }

    pub(crate) fn dispatch_node_http(
        &mut self,
        native: Native,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        match native {
            Native::NodeHttpRequest | Native::NodeHttpGet => {
                let (host, port, method, path, headers, callback) =
                    node_http_request_arguments(arguments)?;
                let request_id = self.allocate_http_client_request_id();
                let state = Rc::new(RefCell::new(HttpClientRequestState {
                    host: host.clone(),
                    port,
                    method,
                    path,
                    headers,
                    body: Vec::new(),
                    socket: None,
                    object: None,
                    ended: false,
                    started: false,
                    request_id,
                }));
                let request_object = node_http_client_request_object(&state);
                // `http.request(url, callback)` is an EventEmitter convenience: the callback
                // is the first `response` listener, so listeners attached by the caller after
                // the request is returned observe the same event in registration order.
                if is_callable(&callback) {
                    self.dispatch_node_event(
                        Native::NodeEventOn,
                        &request_object,
                        &[Value::String("response".into()), callback],
                        instruction,
                    )?;
                }
                self.register_http_client_request(request_id, &state);
                if matches!(native, Native::NodeHttpGet) {
                    state.borrow_mut().ended = true;
                }
                let runtime = self.http_runtime.ok_or_else(|| ExecutionError::Host {
                    instruction,
                    message: "node:http client requires the realm HTTP runtime".into(),
                })?;
                match unsafe {
                    ActiveHttpRuntime::acquire_client_connection(
                        &mut *runtime,
                        HttpOrigin::http(&host, port),
                        request_id,
                    )
                } {
                    AgentDecision::Connect => {
                        self.prepare_http_client_socket(&state, instruction)?;
                        self.start_http_client_connection(&state, instruction)?
                    }
                    AgentDecision::Reuse(resource) => {
                        self.attach_http_client_request_to_resource(&state, resource, instruction)?;
                    }
                    AgentDecision::Queued => {}
                }
                Ok(request_object)
            }
            Native::NodeClientRequestWrite | Native::NodeClientRequestEnd => {
                self.dispatch_node_client_request(native, receiver, arguments, instruction)
            }
            Native::NodeClientRequestAssignSocket => {
                let request_state = node_http_client_request(receiver)?;
                let socket_value =
                    arguments
                        .first()
                        .cloned()
                        .ok_or_else(|| ExecutionError::TypeErrorMessage {
                            message: "ClientRequest socket assignment requires a socket".into(),
                        })?;
                let Value::Object(request_object) = receiver else {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "ClientRequest socket assignment requires a request".into(),
                    });
                };
                request_object
                    .borrow_mut()
                    .set("socket", socket_value.clone());
                request_object
                    .borrow_mut()
                    .set("connection", socket_value.clone());
                let emit = self.read_property(receiver, "emit", instruction)?;
                self.call_value(
                    &emit,
                    receiver.clone(),
                    vec![Value::String("socket".into()), socket_value],
                    instruction,
                )?;
                if let Some(socket) = request_state.borrow().socket.clone()
                    && socket.borrow().resource.is_some()
                {
                    self.flush_http_client_request(&request_state, &socket, instruction)?;
                }
                Ok(Value::Undefined)
            }
            Native::NodeHttpFetchRespond => self.http_fetch_respond(arguments, instruction),
            Native::NodeHttpServer
            | Native::NodeHttpCreateServer
            | Native::NodeHttpCreateFetchServer => {
                // Node accepts `createServer()`, `createServer(listener)`, and
                // `createServer(options, listener)`. The listener is the first argument when
                // callable, otherwise it occupies the second slot; trailing arguments do not
                // replace it. Keep this normalization at the adapter boundary so transport state
                // sees only an optional listener.
                let (options, handler) = match arguments {
                    [handler, ..] if is_callable(handler) => (None, handler.clone()),
                    [options, listener, ..] => (Some(options), listener.clone()),
                    [options] => (Some(options), Value::Undefined),
                    [] => (None, Value::Undefined),
                };
                if options.is_some_and(|options| {
                    !matches!(options, Value::Undefined | Value::Null | Value::Object(_))
                }) {
                    return self.throw_node_invalid_arg_type(
                        "The \"options\" argument must be of type object",
                    );
                }
                if handler.is_truthy() && !is_callable(&handler) {
                    return self.throw_node_invalid_arg_type(&format!(
                        "The \"listener\" argument must be of type function. Received type {}",
                        handler.type_name(),
                    ));
                }
                if native == Native::NodeHttpCreateFetchServer && !is_callable(&handler) {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "The request listener must be a function".into(),
                    });
                }
                let handler = if is_callable(&handler) {
                    handler
                } else {
                    Value::Undefined
                };
                let state = Rc::new(RefCell::new(NodeServerState {
                    driver: TcpServerDriver::default(),
                    handler: handler.clone(),
                    protocol: ServerProtocol::Http,
                    allow_half_open: false,
                    timeout_milliseconds: 0.0,
                    streams: Vec::new(),
                    object: None,
                    fetch: native == Native::NodeHttpCreateFetchServer,
                    close_deferred: false,
                }));
                let object = node_server_object(&state);
                if native != Native::NodeHttpCreateFetchServer && is_callable(&handler) {
                    self.dispatch_node_event(
                        Native::NodeEventOn,
                        &object,
                        &[Value::String("request".into()), handler],
                        instruction,
                    )?;
                }
                self.register_node_server(&state);
                Ok(object)
            }
            Native::NodeServerListen => {
                let state = node_http_server(receiver)?;
                let (port, host, callback) = node_listen_arguments(arguments)?;
                // A listening transport can outlive the expression that created its JS wrapper.
                // Keep its EventEmitter alive in the owning VM/graph realm until the terminal
                // close event, so later request callbacks retain the server as their receiver.
                self.retain_active_node_server_object(&state, receiver.clone());
                if let Some(callback) = callback {
                    self.dispatch_node_event(
                        Native::NodeEventOnce,
                        receiver,
                        &[Value::String("listening".into()), callback],
                        instruction,
                    )?;
                }
                let action = state.borrow().driver.listen(host, port);
                self.start_http_server_action(&state, action, instruction)?;
                Ok(receiver.clone())
            }
            Native::NodeServerAddress => {
                let state = node_http_server(receiver)?;
                let state = state.borrow();
                let Some(listener) = state.driver.listener() else {
                    return Ok(Value::Null);
                };
                let mut address = ObjectData::default();
                address.set(
                    "address",
                    listener
                        .address
                        .clone()
                        .map(|value| Value::String(value.into()))
                        .unwrap_or(Value::Undefined),
                );
                address.set("family", Value::String("IPv4".into()));
                address.set(
                    "port",
                    listener
                        .port
                        .map(|port| Value::Number(f64::from(port)))
                        .unwrap_or(Value::Undefined),
                );
                Ok(Value::Object(Rc::new(RefCell::new(address))))
            }
            Native::NodeServerSetTimeout => {
                let state = node_http_server(receiver)?;
                let timeout = arguments.first().map_or(0.0, Value::as_number);
                if !timeout.is_finite() || timeout < 0.0 {
                    return Err(ExecutionError::InvalidTimeout);
                }
                state.borrow_mut().timeout_milliseconds = timeout.trunc();
                if let Some(callback) = arguments.get(1).filter(|value| is_callable(value)) {
                    self.dispatch_node_event(
                        Native::NodeEventOn,
                        receiver,
                        &[Value::String("timeout".into()), callback.clone()],
                        instruction,
                    )?;
                }
                if let Value::Object(object) = receiver {
                    object
                        .borrow_mut()
                        .set("timeout", Value::Number(timeout.trunc()));
                }
                Ok(receiver.clone())
            }
            Native::NodeServerClose => {
                let state = node_http_server(receiver)?;
                if let Some(callback) = arguments.first().filter(|value| is_callable(value)) {
                    self.dispatch_node_event(
                        Native::NodeEventOnce,
                        receiver,
                        &[Value::String("close".into()), callback.clone()],
                        instruction,
                    )?;
                }
                let actions = {
                    let mut state = state.borrow_mut();
                    state.driver.close()
                };
                for action in actions {
                    self.start_http_server_action(&state, action, instruction)?;
                }
                // Copy the protocol out before taking a mutable borrow in the `Net` arm. A
                // match scrutinee borrow otherwise lives for the whole arm and panics when the
                // arm closes each accepted stream.
                let protocol = state.borrow().protocol;
                match protocol {
                    // Stop accepting, then let existing raw connections finish naturally.
                    ServerProtocol::Net => {}
                    ServerProtocol::Http => {
                        // Mark every current connection first. A request handler can call
                        // `server.close()` before it calls `response.end()`; keep that active
                        // request alive and close it from the response-end path below.
                        let resources = {
                            let mut state = state.borrow_mut();
                            state
                                .streams
                                .iter_mut()
                                .filter_map(|stream| {
                                    stream.close_when_response_ends = true;
                                    let response_ended = stream
                                        .response
                                        .as_ref()
                                        .and_then(|response| node_http_response(response).ok())
                                        .is_some_and(|response| response.borrow().ended);
                                    (response_ended || stream.response.is_none())
                                        .then_some(stream.resource())
                                })
                                .collect::<Vec<_>>()
                        };
                        let runtime = self.http_runtime.ok_or_else(|| ExecutionError::Host {
                            instruction,
                            message: "node:http requires the realm HTTP runtime".into(),
                        })?;
                        for resource in resources {
                            let _ = unsafe {
                                ActiveHttpRuntime::close_connection(
                                    &mut *runtime,
                                    resource,
                                    HttpCloseMode::Graceful,
                                )
                            };
                        }
                    }
                    ServerProtocol::HttpClient => {}
                }
                Ok(receiver.clone())
            }
            Native::NodeResponseSetHeader
            | Native::NodeResponseGetHeader
            | Native::NodeResponseGetHeaders
            | Native::NodeResponseHasHeader
            | Native::NodeResponseRemoveHeader
            | Native::NodeResponseWriteHead
            | Native::NodeResponseWrite
            | Native::NodeResponseEnd => {
                self.dispatch_node_response(native, receiver, arguments, instruction)
            }
            _ => unreachable!("only HTTP natives dispatch here"),
        }
    }

    pub(crate) fn throw_node_invalid_arg_type(
        &mut self,
        message: &str,
    ) -> Result<Value, ExecutionError> {
        let error =
            self.construct_error(ErrorKind::Type, &[Value::String(message.to_owned().into())]);
        if let Value::Object(object) = &error {
            object.borrow_mut().set_non_enumerable(
                "code",
                Value::String("ERR_INVALID_ARG_TYPE".to_owned().into()),
            );
        }
        self.caught = Some(error);
        Err(ExecutionError::ThrownAcrossNative)
    }

    pub(crate) fn start_http_client_connection(
        &mut self,
        state: &Rc<RefCell<HttpClientRequestState>>,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        self.prepare_http_client_socket(state, instruction)?;
        let (host, port) = {
            let state = state.borrow();
            (state.host.clone(), state.port)
        };
        // The HTTP client (used by global fetch) can work without the full Node Adapter.
        // Prefer async_host (Node mode), fall back to http_client_host (global fetch mode).
        // Node.js API surface (fs, net, etc.) remains gated behind the adapter.
        let host_ops = if let Some(ops) = self.async_host {
            ops
        } else if let Some(ops) = self.http_client_host {
            ops
        } else {
            return Err(ExecutionError::Host {
                instruction,
                message: "node:http client requires the Node execution Adapter".into(),
            });
        };
        let request = unsafe {
            ActiveAsyncHostOps::start(&mut *host_ops, AsyncHostOp::TcpConnect { host, port }, true)
        };
        self.push_pending_node(PendingNodeRequest::Connect {
            request,
            target: PendingConnect::Http(Rc::clone(state)),
        });
        if let Some(socket) = state.borrow().socket.clone() {
            socket.borrow_mut().connect_request = Some(request);
        }
        Ok(())
    }

    fn prepare_http_client_socket(
        &mut self,
        state: &Rc<RefCell<HttpClientRequestState>>,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        if state.borrow().socket.is_some() {
            return Ok(());
        }
        let socket_state = Rc::new(RefCell::new(NetSocketState {
            owner: None,
            resource: None,
            object: None,
            readable: new_readable_state(DEFAULT_HIGH_WATER_MARK, None),
            read_paused: false,
            connecting: true,
            pending_write: Vec::new(),
            cork_depth: 0,
            corked_write: Vec::new(),
            corked_callbacks: Vec::new(),
            queued_write_callbacks: Default::default(),
            end_callbacks: Vec::new(),
            upgrade_write_callbacks: Default::default(),
            shutdown_after_upgrade_fence: None,
            end_requested: false,
            destroyed: false,
            connect_request: None,
            http_agent_request: None,
        }));
        let socket = node_socket_object(&socket_state);
        self.decorate_node_socket(&socket, instruction)?;
        let request = state
            .borrow()
            .object
            .as_ref()
            .map(|object| Value::Object(Rc::clone(object)))
            .ok_or_else(|| ExecutionError::Host {
                instruction,
                message: "HTTP client request lost its JavaScript object".into(),
            })?;
        {
            let mut request_state = state.borrow_mut();
            socket_state.borrow_mut().http_agent_request = Some((
                request_state.host.clone(),
                request_state.port,
                request_state.request_id,
            ));
            request_state.socket = Some(Rc::clone(&socket_state));
        }
        let callback = Value::Bound(Rc::new(BoundFunction {
            target: Value::Native(Native::NodeClientRequestAssignSocket, RealmId::PRIMARY),
            this_value: request,
            arguments: vec![socket],
            properties: RefCell::new(ObjectData::default()),
        }));
        let scheduled = self.scheduled(Job::NextTick(callback, Vec::new()));
        self.with_realm_mut(|realm| realm.queue_next_tick(scheduled));
        Ok(())
    }

    pub(crate) fn start_http_server_action(
        &mut self,
        state: &Rc<RefCell<NodeServerState>>,
        action: ServerAction,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let ServerAction::Start(operation) = action else {
            return self.apply_http_server_action(state, action, instruction);
        };
        if matches!(operation, AsyncHostOp::TcpAccept { .. })
            && !state.borrow().driver.accepts_connections()
        {
            return Ok(());
        }
        let host = self.async_host.ok_or_else(|| ExecutionError::Host {
            instruction,
            message: "node:http requires the Node execution Adapter".into(),
        })?;
        let request = unsafe { ActiveAsyncHostOps::start(&mut *host, operation.clone(), true) };
        if !state.borrow_mut().driver.started(&operation, request) {
            return Err(ExecutionError::Host {
                instruction,
                message: format!(
                    "node:{:?} server rejected host operation {operation:?} as request {}",
                    state.borrow().protocol,
                    request.get()
                ),
            });
        }
        self.push_pending_node(PendingNodeRequest::Server {
            request,
            state: Rc::clone(state),
        });
        Ok(())
    }

    pub(crate) fn apply_http_server_action(
        &mut self,
        state: &Rc<RefCell<NodeServerState>>,
        action: ServerAction,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        match action {
            ServerAction::Cancel(request) => {
                let host = self.async_host.ok_or_else(|| ExecutionError::Host {
                    instruction,
                    message: "node:http requires the Node execution Adapter".into(),
                })?;
                unsafe { ActiveAsyncHostOps::cancel(&mut *host, request) };
                if let Some(realm) = &self.module_realm {
                    // The caller is iterating actions produced by `driver.close()`. Retiring the
                    // executor request here is safe; borrowing the realm only for this statement
                    // is important because starting the following close action registers its own
                    // request in the same shared session.
                    let mut realm = realm.borrow_mut();
                    realm
                        .pending_node
                        .retain(|pending| pending.request() != request);
                } else {
                    self.pending_node
                        .retain(|pending| pending.request() != request);
                }
                Ok(())
            }
            ServerAction::Start(operation) => {
                self.start_http_server_action(state, ServerAction::Start(operation), instruction)
            }
            ServerAction::Listening(_) => {
                self.emit_http_server(state, "listening", vec![], instruction)
            }
            ServerAction::Close => {
                // Node emits `close` once the listener is closed *and* every connection has
                // ended: `server.close(() => process.exit(0))` must not cut off the requests it
                // is draining (realms issue 23). The last connection's `Closed` emits it.
                {
                    let mut server = state.borrow_mut();
                    if server.streams.iter().any(|stream| {
                        !stream.closed
                            && (server.protocol == ServerProtocol::Net
                                || (stream.runtime_http && !stream.upgrading))
                    }) {
                        server.close_deferred = true;
                        return Ok(());
                    }
                }
                // Remove the registry root before callbacks, but keep its Value local through
                // dispatch. A close listener may immediately listen again and install a fresh
                // root without this completion clearing it afterward.
                let _active_object = self.take_active_node_server_object(state);
                self.emit_http_server(state, "close", vec![], instruction)
            }
            ServerAction::Connection(accepted) => {
                let protocol = state.borrow().protocol;
                // HTTP and raw TCP retain one socket identity and one transport owner. The
                // parser owns reads until upgrade; socket writes already need that owner while
                // the handshake is pending, and raw stream dispatch takes over after upgrade.
                let socket_state = Rc::new(RefCell::new(NetSocketState {
                    owner: Some(Rc::downgrade(state)),
                    resource: Some(accepted.resource.id),
                    object: None,
                    readable: new_readable_state(DEFAULT_HIGH_WATER_MARK, None),
                    read_paused: false,
                    connecting: false,
                    pending_write: Vec::new(),
                    cork_depth: 0,
                    corked_write: Vec::new(),
                    corked_callbacks: Vec::new(),
                    queued_write_callbacks: Default::default(),
                    end_callbacks: Vec::new(),
                    upgrade_write_callbacks: Default::default(),
                    shutdown_after_upgrade_fence: None,
                    end_requested: false,
                    destroyed: false,
                    connect_request: None,
                    http_agent_request: None,
                }));
                let socket = node_socket_object(&socket_state);
                if let Value::Object(object) = &socket {
                    let allow_half_open = if protocol == ServerProtocol::Http {
                        true
                    } else {
                        state
                            .borrow()
                            .object
                            .as_ref()
                            .and_then(Weak::upgrade)
                            .and_then(|server| server.borrow().get("allowHalfOpen"))
                            .is_some_and(|value| value.is_truthy())
                    };
                    object
                        .borrow_mut()
                        .set("allowHalfOpen", Value::Boolean(allow_half_open));
                }
                self.decorate_node_socket(&socket, self.instruction)?;
                if protocol == ServerProtocol::Http {
                    if let Value::Object(object) = &socket {
                        object.borrow_mut().set("encrypted", Value::Boolean(false));
                    }
                }
                let socket_state = Some(socket_state);
                let mut stream = NodeConnection {
                    driver: StreamDriver::new(accepted.resource.id, DEFAULT_HIGH_WATER_MARK),
                    runtime_http: protocol == ServerProtocol::Http,
                    upgrade_request_id: None,
                    upgrading: false,
                    socket: socket_state,
                    socket_object: Some(socket.clone()),
                    request: None,
                    response: None,
                    client_requests: std::collections::BTreeMap::new(),
                    client_responses: std::collections::BTreeMap::new(),
                    http_responses: Vec::new(),
                    close_when_response_ends: false,
                    closed: false,
                    applying_read_completion: false,
                };
                if protocol == ServerProtocol::Http {
                    let runtime = self.http_runtime.ok_or_else(|| ExecutionError::Host {
                        instruction,
                        message: "node:http requires the realm HTTP runtime".into(),
                    })?;
                    let allow_half_close = state
                        .borrow()
                        .object
                        .as_ref()
                        .and_then(Weak::upgrade)
                        .and_then(|server| server.borrow().get("httpAllowHalfOpen"))
                        .is_some_and(|value| value.is_truthy());
                    // ADR 0046: the host hands the accepted socket to the engine, whose task
                    // queues protocol events for the checkpoint to drain.
                    let whole_requests = state.borrow().fetch;
                    let attached = unsafe {
                        ActiveHttpRuntime::start_connection_with(
                            &mut *runtime,
                            accepted.resource.id,
                            allow_half_close,
                            whole_requests,
                        )
                    };
                    if !attached {
                        return Err(ExecutionError::Host {
                            instruction,
                            message:
                                "the host could not hand the accepted socket to the HTTP engine"
                                    .into(),
                        });
                    }
                }
                let net_actions = if protocol == ServerProtocol::Net {
                    stream.driver.start_reading()
                } else {
                    Vec::new()
                };
                state.borrow_mut().streams.push(stream);
                if protocol == ServerProtocol::Net {
                    if let Some(connection) = state
                        .borrow_mut()
                        .streams
                        .iter_mut()
                        .find(|stream| stream.resource() == accepted.resource.id)
                    {
                        connection.request = Some(socket.clone());
                    }
                    self.emit_http_server(state, "connection", vec![socket.clone()], instruction)?;
                }
                for action in net_actions {
                    if matches!(&action, StreamAction::Start(AsyncHostOp::TcpRead { .. })) {
                        let paused = node_net_socket_state(&socket)
                            .is_ok_and(|socket| socket.borrow().read_paused);
                        let already_started = state.borrow().streams.iter().any(|stream| {
                            stream.resource() == accepted.resource.id
                                && stream.driver.has_pending_read()
                        });
                        if paused || already_started {
                            continue;
                        }
                    }
                    self.start_http_stream_action(
                        state,
                        accepted.resource.id,
                        action,
                        instruction,
                    )?;
                }
                Ok(())
            }
            ServerAction::Error(error) => {
                let error = self.node_error_value(error);
                // A failed listen has no listener resource and is terminal for this attempt.
                // Keep the wrapper alive through the error callback; a reentrant listen can
                // install a new active root after this one has been removed.
                let terminal = state.borrow().driver.listener().is_none();
                let _active_object = terminal
                    .then(|| self.take_active_node_server_object(state))
                    .flatten();
                self.emit_http_server(state, "error", vec![error], instruction)
            }
        }
    }

    pub(crate) fn emit_http_server(
        &mut self,
        state: &Rc<RefCell<NodeServerState>>,
        event: &str,
        arguments: Vec<Value>,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let object = self
            .active_node_server_object(state)
            .or_else(|| {
                state
                    .borrow()
                    .object
                    .as_ref()
                    .and_then(Weak::upgrade)
                    .map(Value::Object)
            })
            .unwrap_or(Value::Undefined);
        let mut payload = vec![Value::String(event.to_owned().into())];
        payload.extend(arguments);
        self.dispatch_node_event(Native::NodeEventEmit, &object, &payload, instruction)?;
        Ok(())
    }

    pub(crate) fn node_error_value(&mut self, error: kiln_host_abi::AsyncHostError) -> Value {
        let object = self.construct_error(ErrorKind::Error, &[Value::String(error.message.into())]);
        if let Value::Object(object) = &object {
            let mut object = object.borrow_mut();
            object.set_non_enumerable("code", Value::String(error.code.into()));
            object.set_non_enumerable("syscall", Value::String(error.syscall.into()));
            if let Some(path) = error.path {
                object.set_non_enumerable("path", Value::String(path.into()));
            }
        }
        object
    }

    pub(crate) fn http_server_for_stream(
        &self,
        resource: kiln_host_abi::ResourceId,
    ) -> Option<Rc<RefCell<NodeServerState>>> {
        let shared = self.module_realm.as_ref().map(|realm| realm.borrow());
        let servers = shared
            .as_ref()
            .map_or(self.node_servers.as_slice(), |realm| {
                realm.node_servers.as_slice()
            });
        servers
            .iter()
            .find(|server| {
                server
                    .borrow()
                    .streams
                    .iter()
                    .any(|stream| stream.resource() == resource)
            })
            .cloned()
    }

    pub(crate) fn start_http_stream_action(
        &mut self,
        server: &Rc<RefCell<NodeServerState>>,
        resource: kiln_host_abi::ResourceId,
        action: StreamAction,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let write_callbacks =
            if matches!(&action, StreamAction::Start(AsyncHostOp::TcpWrite { .. })) {
                node_net_socket_value(server, resource)
                    .and_then(|socket| node_net_socket_state(&socket).ok())
                    .map(|state| Self::take_next_queued_write_callbacks(&state))
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
        self.start_http_stream_action_with_callbacks(
            server,
            resource,
            action,
            instruction,
            write_callbacks,
        )
    }

    fn start_http_stream_action_with_callbacks(
        &mut self,
        server: &Rc<RefCell<NodeServerState>>,
        resource: kiln_host_abi::ResourceId,
        action: StreamAction,
        instruction: usize,
        mut write_callbacks: Vec<Value>,
    ) -> Result<(), ExecutionError> {
        match action {
            StreamAction::Cancel(request) => {
                let host = self.async_host.ok_or_else(|| ExecutionError::Host {
                    instruction,
                    message: "node:http requires the Node execution Adapter".into(),
                })?;
                unsafe { ActiveAsyncHostOps::cancel(&mut *host, request) };
                if let Some(PendingNodeRequest::Stream {
                    write_callbacks, ..
                }) = self.take_pending_node_request(request)
                    && !write_callbacks.is_empty()
                    && node_net_socket_value(server, resource)
                        .and_then(|socket| node_net_socket_state(&socket).ok())
                        .is_some_and(|socket| socket.borrow().destroyed)
                {
                    // The host can cancel an in-flight write as part of destroy, but Node still
                    // settles the callback for bytes already accepted by the writable side. A
                    // queued (not yet submitted) callback is handled separately as
                    // ERR_STREAM_DESTROYED by the socket's state drain.
                    for callback in write_callbacks {
                        let scheduled = self.scheduled(Job::NextTick(callback, vec![Value::Null]));
                        self.with_realm_mut(|realm| realm.queue_next_tick(scheduled));
                    }
                }
                Ok(())
            }
            StreamAction::Start(operation) => {
                if matches!(operation, AsyncHostOp::TcpRead { .. })
                    && server
                        .borrow()
                        .streams
                        .iter()
                        .find(|stream| stream.resource() == resource)
                        .is_some_and(|stream| stream.driver.is_closing())
                {
                    return Ok(());
                }
                if matches!(operation, AsyncHostOp::TcpRead { .. })
                    && server
                        .borrow()
                        .streams
                        .iter()
                        .find(|stream| stream.resource() == resource)
                        .is_some_and(|stream| stream.driver.has_pending_read())
                {
                    return Ok(());
                }
                let host = self.async_host.ok_or_else(|| ExecutionError::Host {
                    instruction,
                    message: "node:http requires the Node execution Adapter".into(),
                })?;
                let referenced = !matches!(
                    operation,
                    AsyncHostOp::SetResourceReferenced {
                        referenced: false,
                        ..
                    }
                );
                let request =
                    unsafe { ActiveAsyncHostOps::start(&mut *host, operation.clone(), referenced) };
                let accepted = server
                    .borrow_mut()
                    .streams
                    .iter_mut()
                    .find(|stream| stream.resource() == resource)
                    .is_some_and(|stream| stream.driver.started(&operation, request));
                if !accepted {
                    return Err(ExecutionError::Host {
                        instruction,
                        message: "node:http stream rejected its own host operation".into(),
                    });
                }
                self.push_pending_node(PendingNodeRequest::Stream {
                    request,
                    server: Rc::clone(server),
                    resource,
                    write_callbacks: if matches!(operation, AsyncHostOp::TcpWrite { .. }) {
                        std::mem::take(&mut write_callbacks)
                    } else {
                        Vec::new()
                    },
                });
                Ok(())
            }
            StreamAction::Data(bytes) => {
                if server
                    .borrow()
                    .streams
                    .iter()
                    .any(|stream| stream.resource() == resource && !stream.runtime_http)
                {
                    let socket = node_net_socket_value(server, resource).ok_or_else(|| {
                        ExecutionError::Host {
                            instruction,
                            message: "node:net received data for an unknown stream".into(),
                        }
                    })?;
                    let readable = Rc::clone(&node_net_socket_state(&socket)?.borrow().readable);
                    let chunk = match readable.borrow().encoding.as_deref() {
                        Some("utf8") => {
                            Value::String(String::from_utf8_lossy(&bytes).into_owned().into())
                        }
                        _ => node_bytes_value(bytes),
                    };
                    self.dispatch_node_event(
                        Native::NodeEventEmit,
                        &socket,
                        &[Value::String("data".into()), chunk],
                        instruction,
                    )?;
                    return Ok(());
                }
                if server.borrow().protocol == ServerProtocol::HttpClient {
                    return Err(ExecutionError::Host {
                        instruction,
                        message: "runtime-owned HTTP client leaked protocol bytes".into(),
                    });
                }
                Err(ExecutionError::Host {
                    instruction,
                    message: "runtime-owned HTTP server leaked protocol bytes".into(),
                })
            }
            StreamAction::Event(StreamEvent::Finish) => {
                if server
                    .borrow()
                    .streams
                    .iter()
                    .any(|stream| stream.resource() == resource && !stream.runtime_http)
                {
                    if let Some(socket) = node_net_socket_value(server, resource) {
                        self.dispatch_node_event(
                            Native::NodeEventEmit,
                            &socket,
                            &[Value::String("finish".into())],
                            instruction,
                        )?;
                        let socket_state = node_net_socket_state(&socket)?;
                        self.call_node_socket_end_callbacks(
                            &socket_state,
                            Value::Null,
                            instruction,
                        )?;
                    }
                    return Ok(());
                }
                Ok(())
            }
            StreamAction::Event(StreamEvent::Close) => {
                let socket = (server
                    .borrow()
                    .streams
                    .iter()
                    .any(|stream| stream.resource() == resource && !stream.runtime_http))
                .then(|| node_net_socket_value(server, resource))
                .flatten();
                if let Some(stream) = server
                    .borrow_mut()
                    .streams
                    .iter_mut()
                    .find(|stream| stream.resource() == resource)
                {
                    stream.closed = true;
                }
                if let Some(socket) = socket {
                    if let Ok(socket_state) = node_net_socket_state(&socket) {
                        let (write_callbacks, end_callbacks) =
                            Self::take_node_socket_queued_state(&socket_state);
                        self.schedule_node_socket_queued_callbacks(write_callbacks, Vec::new());
                        for callback in end_callbacks {
                            self.call_value(
                                &callback,
                                Value::Undefined,
                                vec![Value::Null],
                                instruction,
                            )?;
                        }
                    }
                    // Node exposes the terminal stream state from inside the `close` listener.
                    // Event dispatch is synchronous, so projecting it after this branch returns
                    // lets user code observe the previous half-open state.
                    sync_node_socket_ready_state(server, resource);
                    self.dispatch_node_event(
                        Native::NodeEventEmit,
                        &socket,
                        &[Value::String("close".into())],
                        instruction,
                    )?;
                }
                let emit_close = {
                    let mut state = server.borrow_mut();
                    let ready = state.protocol == ServerProtocol::Net
                        && state.close_deferred
                        && state.streams.iter().all(|stream| stream.closed);
                    if ready {
                        state.close_deferred = false;
                    }
                    ready
                };
                if emit_close {
                    let _active_object = self.take_active_node_server_object(server);
                    self.emit_http_server(server, "close", vec![], instruction)?;
                }
                Ok(())
            }
            StreamAction::Event(StreamEvent::Drain | StreamEvent::End) => {
                sync_node_socket_ready_state(server, resource);
                if matches!(action, StreamAction::Event(StreamEvent::End))
                    && server
                        .borrow()
                        .streams
                        .iter()
                        .any(|stream| stream.resource() == resource && stream.runtime_http)
                    && matches!(
                        server.borrow().protocol,
                        ServerProtocol::Http | ServerProtocol::HttpClient
                    )
                {
                    let incoming = server
                        .borrow()
                        .streams
                        .iter()
                        .find(|stream| stream.resource() == resource)
                        .and_then(|stream| match server.borrow().protocol {
                            ServerProtocol::Http => stream.request.clone(),
                            ServerProtocol::HttpClient => stream.response.clone(),
                            ServerProtocol::Net => None,
                        });
                    if let Some(incoming) = incoming {
                        let readable = Rc::clone(&node_http_incoming(&incoming)?.borrow().readable);
                        self.dispatch_readable_state(
                            Native::NodeReadablePush,
                            &incoming,
                            &[Value::Null],
                            instruction,
                            &readable,
                            false,
                        )?;
                    }
                }
                if let Some(socket) = node_net_socket_value(server, resource) {
                    let event = match action {
                        StreamAction::Event(StreamEvent::Drain) => "drain",
                        StreamAction::Event(StreamEvent::End) => "end",
                        _ => unreachable!(),
                    };
                    self.dispatch_node_event(
                        Native::NodeEventEmit,
                        &socket,
                        &[Value::String(event.into())],
                        instruction,
                    )?;
                    if matches!(action, StreamAction::Event(StreamEvent::End))
                        && server
                            .borrow()
                            .streams
                            .iter()
                            .any(|stream| stream.resource() == resource && !stream.runtime_http)
                        && !self
                            .read_property(&socket, "allowHalfOpen", instruction)?
                            .is_truthy()
                    {
                        // Raw sockets close their writable half after EOF by default. Let end
                        // listeners enqueue their final writes first, then use the same end
                        // operation that drains the write queue before transport shutdown.
                        let state = node_net_socket_state(&socket)?;
                        let already_ending = server.borrow().streams.iter().any(|stream| {
                            stream.resource() == resource
                                && (stream.driver.state().end_requested()
                                    || stream.driver.state().write_closed())
                        });
                        if !state.borrow().destroyed && !already_ending {
                            let callback = Value::Bound(Rc::new(BoundFunction {
                                target: self.read_property(&socket, "end", instruction)?,
                                this_value: socket.clone(),
                                arguments: Vec::new(),
                                properties: RefCell::new(ObjectData::default()),
                            }));
                            let scheduled = self.scheduled(Job::NextTick(callback, Vec::new()));
                            self.with_realm_mut(|realm| realm.queue_next_tick(scheduled));
                        }
                    }
                }
                if matches!(action, StreamAction::Event(StreamEvent::Drain))
                    && server.borrow().protocol == ServerProtocol::Http
                {
                    let response = server
                        .borrow()
                        .streams
                        .iter()
                        .find(|stream| stream.resource() == resource)
                        .and_then(|stream| stream.response.clone());
                    if let Some(response) = response {
                        self.dispatch_node_event(
                            Native::NodeEventEmit,
                            &response,
                            &[Value::String("drain".into())],
                            instruction,
                        )?;
                    }
                }
                Ok(())
            }
            StreamAction::Event(StreamEvent::Error) => Ok(()),
            StreamAction::Error(error) => {
                let error = self.node_error_value(error);
                self.emit_http_server(server, "clientError", vec![error], instruction)
            }
        }
    }

    pub(crate) fn dispatch_runtime_http_events(
        &mut self,
        server: &Rc<RefCell<NodeServerState>>,
        resource: kiln_host_abi::ResourceId,
        instruction: usize,
    ) -> Result<usize, ExecutionError> {
        let runtime = self.http_runtime.ok_or_else(|| ExecutionError::Host {
            instruction,
            message: "node:http requires the realm HTTP runtime".into(),
        })?;
        let events = unsafe { ActiveHttpRuntime::take_events(&mut *runtime, resource) };
        let event_count = events.len();
        for event in events {
            match event {
                RuntimeHttpEvent::RequestHead { id, request } if server.borrow().fetch => {
                    self.dispatch_http_fetch_request(server, resource, id, request, instruction)?;
                }
                RuntimeHttpEvent::RequestHead { id, request } => {
                    self.dispatch_http_request_head(
                        server,
                        resource,
                        request,
                        Some(id),
                        instruction,
                    )?;
                }
                RuntimeHttpEvent::UpgradeRequest { id, request, head } => {
                    self.dispatch_http_upgrade_request(
                        server,
                        resource,
                        id,
                        request,
                        head,
                        instruction,
                    )?;
                }
                RuntimeHttpEvent::UpgradeWriteComplete { fence, error, .. } => {
                    let socket_state = server
                        .borrow()
                        .streams
                        .iter()
                        .find(|stream| stream.resource() == resource)
                        .and_then(|stream| stream.socket.clone());
                    if let Some(socket_state) = socket_state {
                        let (callbacks, end_after_fence) = {
                            let mut state = socket_state.borrow_mut();
                            (
                                state
                                    .upgrade_write_callbacks
                                    .remove(&fence)
                                    .unwrap_or_default(),
                                state.shutdown_after_upgrade_fence == Some(fence),
                            )
                        };
                        let error = error.map(|message| {
                            self.construct_error(ErrorKind::Error, &[Value::String(message.into())])
                        });
                        for callback in callbacks {
                            let arguments = vec![error.clone().unwrap_or(Value::Null)];
                            self.call_value(&callback, Value::Undefined, arguments, instruction)?;
                        }
                        if end_after_fence {
                            socket_state.borrow_mut().shutdown_after_upgrade_fence = None;
                        }
                        if end_after_fence && !socket_state.borrow().destroyed {
                            let actions = {
                                let mut server_state = server.borrow_mut();
                                let connection = server_state
                                    .streams
                                    .iter_mut()
                                    .find(|stream| stream.resource() == resource)
                                    .ok_or_else(|| ExecutionError::Host {
                                        instruction,
                                        message: "upgraded HTTP stream disappeared before end"
                                            .into(),
                                    })?;
                                connection.driver.end(None).1
                            };
                            for action in actions {
                                self.start_http_stream_action(
                                    server,
                                    resource,
                                    action,
                                    instruction,
                                )?;
                            }
                        }
                    }
                }
                RuntimeHttpEvent::UpgradeReady { head } => {
                    let (socket, paused, actions) = {
                        let mut server_state = server.borrow_mut();
                        let connection = server_state
                            .streams
                            .iter_mut()
                            .find(|stream| stream.resource() == resource)
                            .ok_or_else(|| ExecutionError::Host {
                                instruction,
                                message: "upgraded HTTP stream disappeared".into(),
                            })?;
                        connection.runtime_http = false;
                        connection.upgrade_request_id = None;
                        connection.upgrading = false;
                        let paused = connection
                            .socket
                            .as_ref()
                            .is_some_and(|socket| socket.borrow().read_paused);
                        (
                            connection.socket_object.clone().unwrap_or(Value::Undefined),
                            paused,
                            if paused {
                                Vec::new()
                            } else {
                                connection.driver.resume_reading()
                            },
                        )
                    };
                    if !head.is_empty() {
                        if paused {
                            let readable =
                                Rc::clone(&node_net_socket_state(&socket)?.borrow().readable);
                            self.dispatch_readable_state(
                                Native::NodeReadablePush,
                                &socket,
                                &[node_bytes_value(head)],
                                instruction,
                                &readable,
                                false,
                            )?;
                        } else {
                            self.dispatch_node_event(
                                Native::NodeEventEmit,
                                &socket,
                                &[Value::String("data".into()), node_bytes_value(head)],
                                instruction,
                            )?;
                        }
                    }
                    for action in actions {
                        if matches!(&action, StreamAction::Start(AsyncHostOp::TcpRead { .. })) {
                            let paused_or_destroyed =
                                node_net_socket_state(&socket).is_ok_and(|socket| {
                                    let socket = socket.borrow();
                                    socket.read_paused || socket.destroyed
                                });
                            let already_started = server.borrow().streams.iter().any(|stream| {
                                stream.resource() == resource && stream.driver.has_pending_read()
                            });
                            if paused_or_destroyed || already_started {
                                continue;
                            }
                        }
                        self.start_http_stream_action(server, resource, action, instruction)?;
                    }
                }
                RuntimeHttpEvent::RequestData { id: _, data } => {
                    let incoming = self.http_incoming_for_stream(server, resource, instruction)?;
                    let readable = Rc::clone(&node_http_incoming(&incoming)?.borrow().readable);
                    let body = match readable.borrow().encoding.as_deref() {
                        Some("utf8") => {
                            Value::String(String::from_utf8_lossy(&data).into_owned().into())
                        }
                        _ => node_bytes_value(data),
                    };
                    self.dispatch_readable_state(
                        Native::NodeReadablePush,
                        &incoming,
                        &[body],
                        instruction,
                        &readable,
                        false,
                    )?;
                }
                RuntimeHttpEvent::RequestEnd { id: _ } => {
                    let incoming = self.http_incoming_for_stream(server, resource, instruction)?;
                    let readable = Rc::clone(&node_http_incoming(&incoming)?.borrow().readable);
                    self.dispatch_readable_state(
                        Native::NodeReadablePush,
                        &incoming,
                        &[Value::Null],
                        instruction,
                        &readable,
                        false,
                    )?;
                }
                RuntimeHttpEvent::Drain { id } | RuntimeHttpEvent::Finish { id } => {
                    let finished = matches!(event, RuntimeHttpEvent::Finish { .. });
                    let response = server
                        .borrow()
                        .streams
                        .iter()
                        .find(|stream| stream.resource() == resource)
                        .and_then(|stream| {
                            stream
                                .http_responses
                                .iter()
                                .find(|(response_id, _)| *response_id == id)
                                .map(|(_, response)| response.clone())
                        });
                    if let Some(response) = response {
                        let name = if finished { "finish" } else { "drain" };
                        self.dispatch_node_event(
                            Native::NodeEventEmit,
                            &response,
                            &[Value::String(name.into())],
                            instruction,
                        )?;
                    }
                    if finished
                        && let Some(stream) = server
                            .borrow_mut()
                            .streams
                            .iter_mut()
                            .find(|stream| stream.resource() == resource)
                    {
                        stream
                            .http_responses
                            .retain(|(response_id, _)| *response_id != id);
                    }
                }
                RuntimeHttpEvent::Error { id: _, message } => {
                    let error =
                        self.construct_error(ErrorKind::Error, &[Value::String(message.into())]);
                    let upgraded_socket = {
                        let mut state = server.borrow_mut();
                        state
                            .streams
                            .iter_mut()
                            .find(|stream| stream.resource() == resource && !stream.runtime_http)
                            .map(|stream| {
                                let socket =
                                    stream.socket_object.clone().unwrap_or(Value::Undefined);
                                (socket, stream.driver.close_actions())
                            })
                    };
                    if let Some((socket, actions)) = upgraded_socket {
                        for action in actions {
                            self.start_http_stream_action(server, resource, action, instruction)?;
                        }
                        self.dispatch_node_event(
                            Native::NodeEventEmit,
                            &socket,
                            &[Value::String("error".into()), error],
                            instruction,
                        )?;
                    } else {
                        self.emit_http_server(server, "clientError", vec![error], instruction)?;
                    }
                }
                RuntimeHttpEvent::Closed => {
                    // The engine's last word: the transport is gone and the connection has
                    // left its table. Nothing remains to close; the realm's record goes
                    // unless a request or response object is still reachable from JavaScript.
                    let socket = server
                        .borrow()
                        .streams
                        .iter()
                        .find(|stream| stream.resource() == resource)
                        .and_then(|stream| stream.socket.clone());
                    if let Some(socket) = socket {
                        let (write_callbacks, end_callbacks) = {
                            let mut state = socket.borrow_mut();
                            let mut callbacks = Vec::new();
                            for batch in std::mem::take(&mut state.queued_write_callbacks) {
                                callbacks.extend(batch);
                            }
                            callbacks.extend(std::mem::take(&mut state.corked_callbacks));
                            let end_callbacks = std::mem::take(&mut state.end_callbacks);
                            state.pending_write.clear();
                            state.corked_write.clear();
                            state.cork_depth = 0;
                            state.shutdown_after_upgrade_fence = None;
                            (callbacks, end_callbacks)
                        };
                        self.schedule_node_socket_queued_callbacks(write_callbacks, Vec::new());
                        for callback in end_callbacks {
                            self.call_value(
                                &callback,
                                Value::Undefined,
                                vec![Value::Null],
                                instruction,
                            )?;
                        }
                    }
                    let mut server_state = server.borrow_mut();
                    if let Some(stream) = server_state
                        .streams
                        .iter_mut()
                        .find(|stream| stream.resource() == resource)
                    {
                        stream.closed = true;
                    }
                    server_state.streams.retain(|stream| {
                        stream.resource() != resource
                            || stream.request.is_some()
                            || stream.response.is_some()
                    });
                    let emit_close = server_state.close_deferred
                        && server_state.streams.iter().all(|stream| {
                            stream.closed || !stream.runtime_http || stream.upgrading
                        });
                    if emit_close {
                        server_state.close_deferred = false;
                    }
                    drop(server_state);
                    if emit_close {
                        let _active_object = self.take_active_node_server_object(server);
                        self.emit_http_server(server, "close", vec![], instruction)?;
                    }
                }
            }
        }
        Ok(event_count)
    }

    pub(crate) fn dispatch_runtime_http_client_events(
        &mut self,
        owner: &Rc<RefCell<NodeServerState>>,
        resource: kiln_host_abi::ResourceId,
        instruction: usize,
    ) -> Result<usize, ExecutionError> {
        let runtime = self.http_runtime.ok_or_else(|| ExecutionError::Host {
            instruction,
            message: "node:http client requires the realm HTTP runtime".into(),
        })?;
        let events = unsafe { ActiveHttpRuntime::take_client_events(&mut *runtime, resource) };
        let event_count = events.len();
        for event in events {
            match event {
                RuntimeHttpClientEvent::Drain { id } | RuntimeHttpClientEvent::Finish { id } => {
                    let name = if matches!(event, RuntimeHttpClientEvent::Drain { .. }) {
                        "drain"
                    } else {
                        "finish"
                    };
                    let request = owner
                        .borrow()
                        .streams
                        .iter()
                        .find(|stream| stream.resource() == resource)
                        .and_then(|stream| stream.client_requests.get(&id).cloned())
                        .unwrap_or(Value::Undefined);
                    self.dispatch_node_event(
                        Native::NodeEventEmit,
                        &request,
                        &[Value::String(name.into())],
                        instruction,
                    )?;
                }
                RuntimeHttpClientEvent::ResponseHead {
                    id,
                    status,
                    reason,
                    headers,
                } => {
                    self.dispatch_http_response_event(
                        owner,
                        resource,
                        id,
                        HttpResponseEvent::Head(kiln_runtime::node_http::HttpResponseHead {
                            status,
                            reason,
                            headers,
                            keep_alive: false,
                        }),
                        instruction,
                    )?;
                }
                RuntimeHttpClientEvent::Upgrade {
                    id,
                    status,
                    reason,
                    headers,
                    head,
                } => {
                    let (request, socket, actions) = {
                        let mut state = owner.borrow_mut();
                        let connection = state
                            .streams
                            .iter_mut()
                            .find(|stream| stream.resource() == resource)
                            .ok_or_else(|| ExecutionError::Host {
                                instruction,
                                message: "upgraded HTTP client stream disappeared".into(),
                            })?;
                        let request =
                            connection
                                .client_requests
                                .get(&id)
                                .cloned()
                                .ok_or_else(|| ExecutionError::Host {
                                    instruction,
                                    message: "HTTP upgrade lost its ClientRequest".into(),
                                })?;
                        connection.runtime_http = false;
                        connection.upgrade_request_id = None;
                        connection.upgrading = false;
                        let paused = connection
                            .socket
                            .as_ref()
                            .is_some_and(|socket| socket.borrow().read_paused);
                        (
                            request,
                            connection.socket_object.clone().unwrap_or(Value::Undefined),
                            if paused {
                                Vec::new()
                            } else {
                                connection.driver.resume_reading()
                            },
                        )
                    };
                    let mut response = ObjectData {
                        node_events: Some(Vec::new()),
                        node_object: Some(NodeObject::HttpIncoming(Rc::new(RefCell::new(
                            HttpIncomingState {
                                stream: resource,
                                readable: new_readable_state(16 * 1024, None),
                            },
                        )))),
                        ..Default::default()
                    };
                    response.set_prototype(Value::Native(
                        Native::PrototypeOf(PrototypeTable::Readable),
                        RealmId::PRIMARY,
                    ));
                    response.set("statusCode", Value::Number(status as f64));
                    response.set("statusMessage", Value::String(reason.into()));
                    let mut header_object = ObjectData::default();
                    for (name, value) in headers {
                        header_object.set(&name, Value::String(value.into()));
                    }
                    response.set(
                        "headers",
                        Value::Object(Rc::new(RefCell::new(header_object))),
                    );
                    let response = Value::Object(Rc::new(RefCell::new(response)));
                    if let Some(connection) = owner
                        .borrow_mut()
                        .streams
                        .iter_mut()
                        .find(|stream| stream.resource() == resource)
                    {
                        connection.client_responses.insert(id, response.clone());
                    }
                    self.dispatch_node_event(
                        Native::NodeEventEmit,
                        &request,
                        &[
                            Value::String("upgrade".into()),
                            response,
                            socket.clone(),
                            node_bytes_value(head),
                        ],
                        instruction,
                    )?;
                    for action in actions {
                        if matches!(&action, StreamAction::Start(AsyncHostOp::TcpRead { .. })) {
                            let paused_or_destroyed =
                                node_net_socket_state(&socket).is_ok_and(|socket| {
                                    let socket = socket.borrow();
                                    socket.read_paused || socket.destroyed
                                });
                            let already_started = owner.borrow().streams.iter().any(|stream| {
                                stream.resource() == resource && stream.driver.has_pending_read()
                            });
                            if paused_or_destroyed || already_started {
                                continue;
                            }
                        }
                        self.start_http_stream_action(owner, resource, action, instruction)?;
                    }
                }
                RuntimeHttpClientEvent::ResponseData { id, data: body } => {
                    self.dispatch_http_response_event(
                        owner,
                        resource,
                        id,
                        HttpResponseEvent::Data(body),
                        instruction,
                    )?;
                }
                RuntimeHttpClientEvent::ResponseEnd { id, reusable } => {
                    self.dispatch_http_response_event(
                        owner,
                        resource,
                        id,
                        HttpResponseEvent::End,
                        instruction,
                    )?;
                    if let Some(stream) = owner
                        .borrow_mut()
                        .streams
                        .iter_mut()
                        .find(|stream| stream.resource() == resource)
                    {
                        stream.client_responses.remove(&id);
                        stream.client_requests.remove(&id);
                        stream.request = None;
                        stream.response = None;
                    }
                    self.remove_http_client_request(id);
                    let release = unsafe {
                        ActiveHttpRuntime::release_client_connection(
                            &mut *runtime,
                            resource,
                            reusable,
                        )
                    };
                    if let AgentRelease::Dispatch { request } = release {
                        let queued = self.http_client_request(request).ok_or_else(|| {
                            ExecutionError::Host {
                                instruction,
                                message: "HTTP Agent lost its queued ClientRequest".into(),
                            }
                        })?;
                        self.attach_http_client_request_to_resource(
                            &queued,
                            resource,
                            instruction,
                        )?;
                    } else if release == AgentRelease::Retire {
                        let _ = unsafe {
                            ActiveHttpRuntime::close_connection(
                                &mut *runtime,
                                resource,
                                HttpCloseMode::Graceful,
                            )
                        };
                    }
                }
                RuntimeHttpClientEvent::Error { id, message } => {
                    let request = owner
                        .borrow()
                        .streams
                        .iter()
                        .find(|stream| stream.resource() == resource)
                        .and_then(|stream| {
                            if id == 0 {
                                stream.request.clone()
                            } else {
                                stream.client_requests.get(&id).cloned()
                            }
                        })
                        .unwrap_or(Value::Undefined);
                    let error =
                        self.construct_error(ErrorKind::Error, &[Value::String(message.into())]);
                    self.dispatch_node_event(
                        Native::NodeEventEmit,
                        &request,
                        &[Value::String("error".into()), error],
                        instruction,
                    )?;
                }
                RuntimeHttpClientEvent::Closed => {
                    let retry = unsafe {
                        ActiveHttpRuntime::client_connection_closed(&mut *runtime, resource)
                    };
                    // The engine has already let the transport go; the realm's record of the
                    // connection goes with it.
                    if let Some(stream) = owner
                        .borrow_mut()
                        .streams
                        .iter_mut()
                        .find(|stream| stream.resource() == resource)
                    {
                        stream.closed = true;
                    }
                    owner
                        .borrow_mut()
                        .streams
                        .retain(|stream| stream.resource() != resource);
                    if let Some(request) = retry {
                        let queued = self.http_client_request(request).ok_or_else(|| {
                            ExecutionError::Host {
                                instruction,
                                message: "HTTP Agent lost its queued ClientRequest".into(),
                            }
                        })?;
                        self.start_http_client_connection(&queued, instruction)?;
                    }
                }
            }
        }
        Ok(event_count)
    }

    /// Complete a `server.close()` that arrived before this response was ended.
    pub(crate) fn close_http_stream_after_response_end(
        &mut self,
        resource: kiln_host_abi::ResourceId,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let server = self
            .http_server_for_stream(resource)
            .ok_or_else(|| ExecutionError::Host {
                instruction,
                message: "HTTP response lost its stream".into(),
            })?;
        let should_close = {
            let mut server_state = server.borrow_mut();
            server_state
                .streams
                .iter_mut()
                .find(|stream| stream.resource() == resource)
                .is_some_and(|stream| {
                    if !stream.close_when_response_ends {
                        return false;
                    }
                    stream.close_when_response_ends = false;
                    true
                })
        };
        if !should_close {
            return Ok(());
        }
        let runtime = self.http_runtime.ok_or_else(|| ExecutionError::Host {
            instruction,
            message: "node:http requires the realm HTTP runtime".into(),
        })?;
        let _ = unsafe {
            ActiveHttpRuntime::close_connection(&mut *runtime, resource, HttpCloseMode::Graceful)
        };
        Ok(())
    }

    pub(crate) fn dispatch_http_response_event(
        &mut self,
        owner: &Rc<RefCell<NodeServerState>>,
        resource: kiln_host_abi::ResourceId,
        request_id: u64,
        event: HttpResponseEvent,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        match event {
            HttpResponseEvent::Head(head) => {
                let request = owner
                    .borrow()
                    .streams
                    .iter()
                    .find(|stream| stream.resource() == resource)
                    .and_then(|stream| stream.client_requests.get(&request_id).cloned())
                    .ok_or_else(|| ExecutionError::Host {
                        instruction,
                        message: "HTTP response lost its ClientRequest".into(),
                    })?;
                let state =
                    node_http_client_request(&request).map_err(|_| ExecutionError::Host {
                        instruction,
                        message: format!(
                            "HTTP response connection stored {} instead of ClientRequest",
                            request.type_name()
                        ),
                    })?;
                let mut incoming = ObjectData {
                    node_events: Some(Vec::new()),
                    node_object: Some(NodeObject::HttpIncoming(Rc::new(RefCell::new(
                        HttpIncomingState {
                            stream: resource,
                            readable: new_readable_state(16 * 1024, None),
                        },
                    )))),
                    ..Default::default()
                };
                incoming.set_prototype(Value::Native(
                    Native::PrototypeOf(PrototypeTable::Readable),
                    RealmId::PRIMARY,
                ));
                incoming.set("statusCode", Value::Number(f64::from(head.status)));
                incoming.set("statusMessage", Value::String(head.reason.clone().into()));
                if let Some(socket) = owner
                    .borrow()
                    .streams
                    .iter()
                    .find(|stream| stream.resource() == resource)
                    .and_then(|stream| stream.socket_object.clone())
                {
                    incoming.set("socket", socket.clone());
                    incoming.set("connection", socket);
                }
                incoming.set("on", Value::Native(Native::NodeEventOn, RealmId::PRIMARY));
                incoming.set(
                    "once",
                    Value::Native(Native::NodeEventOnce, RealmId::PRIMARY),
                );
                incoming.set(
                    "emit",
                    Value::Native(Native::NodeEventEmit, RealmId::PRIMARY),
                );
                incoming.set(
                    "pause",
                    Value::Native(Native::NodeReadablePause, RealmId::PRIMARY),
                );
                incoming.set(
                    "resume",
                    Value::Native(Native::NodeReadableResume, RealmId::PRIMARY),
                );
                incoming.set(
                    "setEncoding",
                    Value::Native(Native::NodeReadableSetEncoding, RealmId::PRIMARY),
                );
                let mut headers = ObjectData::default();
                for (name, value) in head.headers_object() {
                    headers.set(&name, Value::String(value.into()));
                }
                incoming.set("headers", Value::Object(Rc::new(RefCell::new(headers))));
                let incoming = Value::Object(Rc::new(RefCell::new(incoming)));
                if let Some(connection) = owner
                    .borrow_mut()
                    .streams
                    .iter_mut()
                    .find(|stream| stream.resource() == resource)
                {
                    connection
                        .client_responses
                        .insert(request_id, incoming.clone());
                }
                self.dispatch_node_event(
                    Native::NodeEventEmit,
                    &request,
                    &[Value::String("response".into()), incoming],
                    instruction,
                )?;
                Ok(())
            }
            HttpResponseEvent::Data(bytes) => {
                let incoming = owner
                    .borrow()
                    .streams
                    .iter()
                    .find(|stream| stream.resource() == resource)
                    .and_then(|stream| stream.client_responses.get(&request_id).cloned())
                    .ok_or_else(|| ExecutionError::Host {
                        instruction,
                        message: "HTTP response body arrived before its head".into(),
                    })?;
                let readable = Rc::clone(&node_http_incoming(&incoming)?.borrow().readable);
                let body = match readable.borrow().encoding.as_deref() {
                    Some("utf8") => {
                        Value::String(String::from_utf8_lossy(&bytes).into_owned().into())
                    }
                    _ => node_bytes_value(bytes),
                };
                self.dispatch_readable_state(
                    Native::NodeReadablePush,
                    &incoming,
                    &[body],
                    instruction,
                    &readable,
                    false,
                )?;
                Ok(())
            }
            HttpResponseEvent::End => {
                let incoming = owner
                    .borrow()
                    .streams
                    .iter()
                    .find(|stream| stream.resource() == resource)
                    .and_then(|stream| stream.client_responses.get(&request_id).cloned())
                    .ok_or_else(|| ExecutionError::Host {
                        instruction,
                        message: "HTTP response ended before its head".into(),
                    })?;
                let readable = Rc::clone(&node_http_incoming(&incoming)?.borrow().readable);
                self.dispatch_readable_state(
                    Native::NodeReadablePush,
                    &incoming,
                    &[Value::Null],
                    instruction,
                    &readable,
                    false,
                )?;
                Ok(())
            }
        }
    }

    pub(crate) fn http_incoming_for_stream(
        &self,
        server: &Rc<RefCell<NodeServerState>>,
        resource: kiln_host_abi::ResourceId,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        server
            .borrow()
            .streams
            .iter()
            .find(|stream| stream.resource() == resource)
            .and_then(|stream| stream.request.clone())
            .ok_or_else(|| ExecutionError::Host {
                instruction,
                message: "node:http body event has no active request".into(),
            })
    }

    /// A request for a fetch-style (`serve`) server: the engine took it whole, and the handler
    /// gets `(method, url, headers, body, resource, id)` -- the URL the engine built (the bare
    /// target when there was no Host field), the header list's wire form (`\nname:value` per
    /// field) as one string, the body as a Buffer or `undefined` -- exactly as the native
    /// backend's `kiln_c99_node_fetch_request` hands them over. Nothing Node-shaped is built.
    pub(crate) fn dispatch_http_fetch_request(
        &mut self,
        server: &Rc<RefCell<NodeServerState>>,
        resource: kiln_host_abi::ResourceId,
        id: u64,
        request: kiln_runtime::node_http::HttpRequest,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let handler = server.borrow().handler.clone();
        let headers =
            String::from_utf8_lossy(request.headers.raw().unwrap_or_default()).into_owned();
        let body = if request.body.is_empty() {
            Value::Undefined
        } else {
            node_bytes_value(request.body)
        };
        self.call_value(
            &handler,
            Value::Undefined,
            vec![
                Value::String(request.method.into()),
                Value::String(request.target.into()),
                Value::String(headers.into()),
                body,
                Value::Number(resource.get() as f64),
                Value::Number(id as f64),
            ],
            instruction,
        )?;
        // One JavaScript turn, as for a Node request listener (see `dispatch_http_request_head`).
        self.drain_microtask_jobs()?;
        Ok(())
    }

    /// `fetchRespond(resource, id, status, headers, body)`: the whole response in one call, with
    /// `headers` the header list's wire form (`\nname:value` per field). False, not a throw, once the connection is
    /// gone -- an asynchronous handler can outlive its client.
    pub(crate) fn http_fetch_respond(
        &mut self,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let handle = |value: Option<&Value>| {
            let number = value.map_or(f64::NAN, Value::as_number);
            ((1.0..=9_007_199_254_740_991.0).contains(&number) && number.fract() == 0.0)
                .then_some(number as u64)
        };
        let (Some(resource), Some(id)) = (handle(arguments.first()), handle(arguments.get(1)))
        else {
            return Err(ExecutionError::TypeErrorMessage {
                message: "fetchRespond requires a request handle".into(),
            });
        };
        let status = arguments.get(2).map_or(f64::NAN, Value::as_number);
        if !(100.0..=999.0).contains(&status) || status.fract() != 0.0 {
            return Err(ExecutionError::RangeErrorMessage {
                message: "Invalid status code".into(),
            });
        }
        // The header list in its wire form, handed to the engine as the bytes it is.
        let headers = match arguments.get(3) {
            None | Some(Value::Undefined) => String::new(),
            Some(value) => value.to_display_string(),
        };
        let body = node_body_bytes(arguments.get(4))?;
        let runtime = self.http_runtime.ok_or_else(|| ExecutionError::Host {
            instruction,
            message: "serve requires the realm HTTP runtime".into(),
        })?;
        let resource = kiln_host_abi::ResourceId::new(resource);
        // SAFETY: the realm's HTTP runtime outlives every call the realm makes.
        let sent = unsafe {
            ActiveHttpRuntime::fetch_respond(
                &mut *runtime,
                resource,
                id,
                status as u16,
                headers.as_bytes(),
                body,
            )
        };
        Ok(Value::Boolean(sent))
    }

    pub(crate) fn dispatch_http_upgrade_request(
        &mut self,
        server: &Rc<RefCell<NodeServerState>>,
        resource: kiln_host_abi::ResourceId,
        request: u64,
        message: kiln_runtime::node_http::HttpRequest,
        head: Vec<u8>,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let socket = server
            .borrow()
            .streams
            .iter()
            .find(|stream| stream.resource() == resource)
            .and_then(|stream| stream.socket_object.clone())
            .unwrap_or(Value::Undefined);
        let mut incoming = ObjectData {
            node_events: Some(Vec::new()),
            node_object: Some(NodeObject::HttpIncoming(Rc::new(RefCell::new(
                HttpIncomingState {
                    stream: resource,
                    readable: new_readable_state(16 * 1024, None),
                },
            )))),
            ..Default::default()
        };
        incoming.set_prototype(Value::Native(
            Native::PrototypeOf(PrototypeTable::Readable),
            RealmId::PRIMARY,
        ));
        let message_headers = message.headers_object();
        incoming.set("method", Value::String(message.method.into()));
        incoming.set("url", Value::String(message.target.into()));
        incoming.set("on", Value::Native(Native::NodeEventOn, RealmId::PRIMARY));
        incoming.set(
            "once",
            Value::Native(Native::NodeEventOnce, RealmId::PRIMARY),
        );
        incoming.set(
            "removeListener",
            Value::Native(Native::NodeEventRemoveListener, RealmId::PRIMARY),
        );
        incoming.set(
            "off",
            Value::Native(Native::NodeEventRemoveListener, RealmId::PRIMARY),
        );
        incoming.set(
            "removeAllListeners",
            Value::Native(Native::NodeEventRemoveAllListeners, RealmId::PRIMARY),
        );
        incoming.set(
            "emit",
            Value::Native(Native::NodeEventEmit, RealmId::PRIMARY),
        );
        incoming.set(
            "pause",
            Value::Native(Native::NodeReadablePause, RealmId::PRIMARY),
        );
        incoming.set(
            "resume",
            Value::Native(Native::NodeReadableResume, RealmId::PRIMARY),
        );
        incoming.set(
            "setEncoding",
            Value::Native(Native::NodeReadableSetEncoding, RealmId::PRIMARY),
        );
        incoming.set("socket", socket.clone());
        incoming.set("connection", socket.clone());
        let mut headers = ObjectData::default();
        for (name, value) in message_headers {
            headers.set(&name, Value::String(value.into()));
        }
        incoming.set("headers", Value::Object(Rc::new(RefCell::new(headers))));
        let incoming = Value::Object(Rc::new(RefCell::new(incoming)));
        if let Some(connection) = server
            .borrow_mut()
            .streams
            .iter_mut()
            .find(|stream| stream.resource() == resource)
        {
            connection.request = Some(incoming.clone());
            connection.upgrade_request_id = Some(request);
            connection.upgrading = true;
        }
        self.emit_http_server(
            server,
            "upgrade",
            vec![incoming, socket, node_bytes_value(head)],
            instruction,
        )
    }

    pub(crate) fn dispatch_http_request_head(
        &mut self,
        server: &Rc<RefCell<NodeServerState>>,
        resource: kiln_host_abi::ResourceId,
        request: kiln_runtime::node_http::HttpRequest,
        request_id: Option<u64>,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let mut incoming = ObjectData {
            node_events: Some(Vec::new()),
            node_object: Some(NodeObject::HttpIncoming(Rc::new(RefCell::new(
                HttpIncomingState {
                    stream: resource,
                    readable: new_readable_state(16 * 1024, None),
                },
            )))),
            ..Default::default()
        };
        incoming.set_prototype(Value::Native(
            Native::PrototypeOf(PrototypeTable::Readable),
            RealmId::PRIMARY,
        ));
        incoming.set("method", Value::String(request.method.clone().into()));
        incoming.set("url", Value::String(request.target.clone().into()));
        incoming.set("on", Value::Native(Native::NodeEventOn, RealmId::PRIMARY));
        incoming.set(
            "once",
            Value::Native(Native::NodeEventOnce, RealmId::PRIMARY),
        );
        incoming.set(
            "removeListener",
            Value::Native(Native::NodeEventRemoveListener, RealmId::PRIMARY),
        );
        incoming.set(
            "off",
            Value::Native(Native::NodeEventRemoveListener, RealmId::PRIMARY),
        );
        incoming.set(
            "removeAllListeners",
            Value::Native(Native::NodeEventRemoveAllListeners, RealmId::PRIMARY),
        );
        incoming.set(
            "emit",
            Value::Native(Native::NodeEventEmit, RealmId::PRIMARY),
        );
        incoming.set(
            "pause",
            Value::Native(Native::NodeReadablePause, RealmId::PRIMARY),
        );
        incoming.set(
            "resume",
            Value::Native(Native::NodeReadableResume, RealmId::PRIMARY),
        );
        incoming.set(
            "setEncoding",
            Value::Native(Native::NodeReadableSetEncoding, RealmId::PRIMARY),
        );
        let mut headers = ObjectData::default();
        for (name, value) in request.headers_object() {
            headers.set(&name, Value::String(value.into()));
        }
        incoming.set("headers", Value::Object(Rc::new(RefCell::new(headers))));
        let socket = server
            .borrow()
            .streams
            .iter()
            .find(|stream| stream.resource() == resource)
            .and_then(|stream| stream.socket_object.clone())
            .unwrap_or(Value::Undefined);
        incoming.set("socket", socket.clone());
        incoming.set("connection", socket.clone());
        let incoming = Value::Object(Rc::new(RefCell::new(incoming)));
        if let Some(connection) = server
            .borrow_mut()
            .streams
            .iter_mut()
            .find(|stream| stream.resource() == resource)
        {
            connection.request = Some(incoming.clone());
        }

        let response_state = Rc::new(RefCell::new(HttpResponseState {
            stream: resource,
            request_id,
            status: 200,
            headers: Vec::new(),
            streaming: false,
            ended: false,
            object: None,
        }));
        let mut response = ObjectData {
            node_events: Some(Vec::new()),
            node_object: Some(NodeObject::HttpResponse(Rc::clone(&response_state))),
            ..Default::default()
        };
        response.set(
            "setHeader",
            Value::Native(Native::NodeResponseSetHeader, RealmId::PRIMARY),
        );
        response.set(
            "getHeader",
            Value::Native(Native::NodeResponseGetHeader, RealmId::PRIMARY),
        );
        response.set(
            "getHeaders",
            Value::Native(Native::NodeResponseGetHeaders, RealmId::PRIMARY),
        );
        response.set(
            "hasHeader",
            Value::Native(Native::NodeResponseHasHeader, RealmId::PRIMARY),
        );
        response.set(
            "removeHeader",
            Value::Native(Native::NodeResponseRemoveHeader, RealmId::PRIMARY),
        );
        response.set(
            "writeHead",
            Value::Native(Native::NodeResponseWriteHead, RealmId::PRIMARY),
        );
        response.set(
            "write",
            Value::Native(Native::NodeResponseWrite, RealmId::PRIMARY),
        );
        response.set(
            "end",
            Value::Native(Native::NodeResponseEnd, RealmId::PRIMARY),
        );
        response.set("on", Value::Native(Native::NodeEventOn, RealmId::PRIMARY));
        response.set(
            "once",
            Value::Native(Native::NodeEventOnce, RealmId::PRIMARY),
        );
        response.set(
            "emit",
            Value::Native(Native::NodeEventEmit, RealmId::PRIMARY),
        );
        response.set("req", incoming.clone());
        response.set("socket", socket.clone());
        response.set("connection", socket);
        let response_object = Rc::new(RefCell::new(response));
        response_state.borrow_mut().object = Some(Rc::downgrade(&response_object));
        let response = Value::Object(response_object);
        if let Some(connection) = server
            .borrow_mut()
            .streams
            .iter_mut()
            .find(|stream| stream.resource() == resource)
        {
            connection.response = Some(response.clone());
            if let Some(request_id) = request_id {
                connection
                    .http_responses
                    .push((request_id, response.clone()));
            }
        }

        let handler = {
            let state = server.borrow();
            state.handler.clone()
        };
        let server_object = self.active_node_server_object(server).or_else(|| {
            server
                .borrow()
                .object
                .clone()
                .and_then(|object| object.upgrade())
                .map(Value::Object)
        });
        if let Some(server_object) = server_object {
            self.dispatch_node_event(
                Native::NodeEventEmit,
                &server_object,
                &[Value::String("request".into()), incoming.clone(), response],
                instruction,
            )?;
        } else if is_callable(&handler) {
            // The host may keep a listening server alive after its JS wrapper is collected. The
            // captured listener still handles that request, though no server receiver remains.
            self.call_value(
                &handler,
                Value::Undefined,
                vec![incoming.clone(), response],
                instruction,
            )?;
        }
        // A Node callback is one JavaScript turn. An async request listener may reach its first
        // `await` before returning; drain the resulting promise reactions before waiting for
        // another host completion, or a response that depends on that continuation can never be
        // written and the realm deadlocks on its own socket.
        self.drain_microtask_jobs()?;
        Ok(incoming)
    }

    pub(crate) fn dispatch_node_response(
        &mut self,
        native: Native,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let state = node_http_response(receiver)?;
        match native {
            Native::NodeResponseSetHeader => {
                let name = arguments
                    .first()
                    .unwrap_or(&Value::Undefined)
                    .to_display_string();
                let value = arguments
                    .get(1)
                    .unwrap_or(&Value::Undefined)
                    .to_display_string();
                let mut state = state.borrow_mut();
                if let Some(header) = state
                    .headers
                    .iter_mut()
                    .find(|(key, _)| key.eq_ignore_ascii_case(&name))
                {
                    header.1 = value;
                } else {
                    state.headers.push((name, value));
                }
                Ok(receiver.clone())
            }
            Native::NodeResponseGetHeader => {
                let name = arguments
                    .first()
                    .unwrap_or(&Value::Undefined)
                    .to_display_string();
                Ok(state
                    .borrow()
                    .headers
                    .iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case(&name))
                    .map(|(_, value)| Value::String(value.clone().into()))
                    .unwrap_or(Value::Undefined))
            }
            Native::NodeResponseGetHeaders => {
                let mut headers = ObjectData::default();
                for (name, value) in &state.borrow().headers {
                    headers.set(
                        &name.to_ascii_lowercase(),
                        Value::String(value.clone().into()),
                    );
                }
                // Node returns a null-prototype dictionary so names such as `constructor` are
                // headers, never inherited object behavior.
                headers.set_prototype(Value::Null);
                Ok(Value::Object(Rc::new(RefCell::new(headers))))
            }
            Native::NodeResponseHasHeader => {
                let name = arguments
                    .first()
                    .unwrap_or(&Value::Undefined)
                    .to_display_string();
                Ok(Value::Boolean(
                    state
                        .borrow()
                        .headers
                        .iter()
                        .any(|(key, _)| key.eq_ignore_ascii_case(&name)),
                ))
            }
            Native::NodeResponseRemoveHeader => {
                let name = arguments
                    .first()
                    .unwrap_or(&Value::Undefined)
                    .to_display_string();
                state
                    .borrow_mut()
                    .headers
                    .retain(|(key, _)| !key.eq_ignore_ascii_case(&name));
                Ok(Value::Undefined)
            }
            Native::NodeResponseWriteHead => {
                let status = arguments.first().map(Value::as_number).unwrap_or(200.0);
                if !(100.0..=999.0).contains(&status) || status.fract() != 0.0 {
                    return Err(ExecutionError::InvalidHttpStatus);
                }
                let mut state = state.borrow_mut();
                state.status = status as u16;
                if let Some(Value::Object(headers)) = arguments.get(1) {
                    let headers = headers.borrow();
                    for property in headers
                        .properties
                        .iter()
                        .filter(|property| property.attributes.enumerable)
                    {
                        let name = property.key.to_lossy_string();
                        let Property::Value(value) = &property.property else {
                            continue;
                        };
                        let value = value.to_display_string();
                        if let Some(header) = state
                            .headers
                            .iter_mut()
                            .find(|(key, _)| key.eq_ignore_ascii_case(&name))
                        {
                            header.1 = value;
                        } else {
                            state.headers.push((name, value));
                        }
                    }
                }
                // Node's `writeHead` commits the header block: `_storeHeader` runs here, so a
                // later `end(body)` can no longer declare a length and Node writes
                // `Transfer-Encoding: chunked` into the block instead. Recording that header is
                // what makes the committed head observably identical to Node's; Hyper still owns
                // the encoding, and drops the header where no body is allowed.
                let framed = state.headers.iter().any(|(name, _)| {
                    name.eq_ignore_ascii_case("content-length")
                        || name.eq_ignore_ascii_case("transfer-encoding")
                });
                let status = state.status;
                if !framed && !matches!(status, 204 | 304) && !(100..200).contains(&status) {
                    state
                        .headers
                        .push(("transfer-encoding".into(), "chunked".into()));
                }
                Ok(receiver.clone())
            }
            Native::NodeResponseWrite => {
                let bytes = node_body_bytes(arguments.first())?;
                let (resource, request_id, status, headers, start) = {
                    let mut state = state.borrow_mut();
                    if state.ended {
                        return Ok(Value::Boolean(false));
                    }
                    let start = if !state.streaming {
                        state.streaming = true;
                        true
                    } else {
                        false
                    };
                    (
                        state.stream,
                        state.request_id,
                        state.status,
                        state.headers.clone(),
                        start,
                    )
                };
                let request_id = request_id.ok_or_else(|| ExecutionError::Host {
                    instruction,
                    message: "HTTP response is not owned by the realm HTTP runtime".into(),
                })?;
                let runtime = self.http_runtime.ok_or_else(|| ExecutionError::Host {
                    instruction,
                    message: "node:http requires the realm HTTP runtime".into(),
                })?;
                if start
                    && !unsafe {
                        ActiveHttpRuntime::start_response(
                            &mut *runtime,
                            resource,
                            request_id,
                            status,
                            headers,
                            DEFAULT_HIGH_WATER_MARK,
                        )
                    }
                {
                    return Err(ExecutionError::Host {
                        instruction,
                        message: "Hyper rejected the HTTP response head".into(),
                    });
                }
                let disposition = unsafe {
                    ActiveHttpRuntime::write_response(&mut *runtime, resource, request_id, bytes)
                }
                .ok_or_else(|| ExecutionError::Host {
                    instruction,
                    message: "Hyper lost the HTTP response body".into(),
                })?;
                Ok(Value::Boolean(disposition.ready))
            }
            Native::NodeResponseEnd => {
                let bytes = arguments
                    .first()
                    .filter(|value| !matches!(value, Value::Undefined))
                    .map(|value| node_body_bytes(Some(value)))
                    .transpose()?
                    .unwrap_or_default();
                let (resource, request_id, status, headers, start) = {
                    let mut state = state.borrow_mut();
                    if state.ended {
                        return Ok(receiver.clone());
                    }
                    let start = !state.streaming;
                    state.streaming = true;
                    state.ended = true;
                    (
                        state.stream,
                        state.request_id,
                        state.status,
                        state.headers.clone(),
                        start,
                    )
                };
                let request_id = request_id.ok_or_else(|| ExecutionError::Host {
                    instruction,
                    message: "HTTP response is not owned by the realm HTTP runtime".into(),
                })?;
                let runtime = self.http_runtime.ok_or_else(|| ExecutionError::Host {
                    instruction,
                    message: "node:http requires the realm HTTP runtime".into(),
                })?;
                if start
                    && !unsafe {
                        ActiveHttpRuntime::start_response(
                            &mut *runtime,
                            resource,
                            request_id,
                            status,
                            headers,
                            DEFAULT_HIGH_WATER_MARK,
                        )
                    }
                {
                    return Err(ExecutionError::Host {
                        instruction,
                        message: "Hyper rejected the HTTP response head".into(),
                    });
                }
                unsafe {
                    ActiveHttpRuntime::end_response(&mut *runtime, resource, request_id, bytes)
                }
                .ok_or_else(|| ExecutionError::Host {
                    instruction,
                    message: "Hyper lost the HTTP response body".into(),
                })?;
                self.close_http_stream_after_response_end(resource, instruction)?;
                Ok(receiver.clone())
            }
            _ => unreachable!("only response natives dispatch here"),
        }
    }

    pub(crate) fn dispatch_node_client_request(
        &mut self,
        native: Native,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let state = node_http_client_request(receiver)?;
        match native {
            Native::NodeClientRequestWrite => {
                let bytes = node_stream_write_bytes(arguments.first())?;
                if state.borrow().ended {
                    self.schedule_node_write_after_end(receiver, arguments, instruction)?;
                    return Ok(Value::Boolean(false));
                }
                let (resource, started) = {
                    let mut state = state.borrow_mut();
                    let resource = state
                        .socket
                        .as_ref()
                        .and_then(|socket| socket.borrow().resource);
                    if resource.is_none() || !state.started {
                        state.body.extend(bytes.clone());
                    }
                    (resource, state.started)
                };
                if let Some(resource) = resource.filter(|_| started) {
                    let runtime = self.http_runtime.ok_or_else(|| ExecutionError::Host {
                        instruction,
                        message: "node:http client requires the realm HTTP runtime".into(),
                    })?;
                    let request_id = state.borrow().request_id;
                    let disposition = unsafe {
                        ActiveHttpRuntime::write_client_request(
                            &mut *runtime,
                            resource,
                            request_id,
                            bytes,
                        )
                    }
                    .ok_or_else(|| ExecutionError::Host {
                        instruction,
                        message: "Hyper lost the HTTP client request body".into(),
                    })?;
                    return Ok(Value::Boolean(disposition.ready));
                }
                if state.borrow().ended {
                    return Ok(Value::Boolean(false));
                }
                Ok(Value::Boolean(true))
            }
            Native::NodeClientRequestEnd => {
                let socket = {
                    let mut request = state.borrow_mut();
                    if request.ended {
                        return Ok(receiver.clone());
                    }
                    if let Some(value) = arguments
                        .first()
                        .filter(|value| !matches!(value, Value::Undefined))
                    {
                        request.body.extend(node_body_bytes(Some(value))?);
                    }
                    request.ended = true;
                    request.socket.clone()
                };
                if let Some(socket) = socket
                    && socket.borrow().resource.is_some()
                {
                    self.flush_http_client_request(&state, &socket, instruction)?;
                }
                Ok(receiver.clone())
            }
            _ => unreachable!("only ClientRequest natives dispatch here"),
        }
    }

    pub(crate) fn attach_http_client_request_to_resource(
        &mut self,
        state: &Rc<RefCell<HttpClientRequestState>>,
        resource: kiln_host_abi::ResourceId,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let owner = self
            .http_server_for_stream(resource)
            .ok_or_else(|| ExecutionError::Host {
                instruction,
                message: "HTTP Agent selected a connection without an adapter owner".into(),
            })?;
        let (socket, socket_object) = owner
            .borrow()
            .streams
            .iter()
            .find(|stream| stream.resource() == resource)
            .and_then(|stream| stream.socket.clone().zip(stream.socket_object.clone()))
            .ok_or_else(|| ExecutionError::Host {
                instruction,
                message: "HTTP Agent connection lost its JavaScript socket".into(),
            })?;
        let request_id = state.borrow().request_id;
        let request = state
            .borrow()
            .object
            .as_ref()
            .map(|object| Value::Object(Rc::clone(object)))
            .ok_or_else(|| ExecutionError::Host {
                instruction,
                message: "HTTP client request lost its JavaScript object".into(),
            })?;
        state.borrow_mut().socket = Some(Rc::clone(&socket));
        socket.borrow_mut().connecting = false;
        if let Value::Object(socket_object) = &socket_object {
            socket_object
                .borrow_mut()
                .set("connecting", Value::Boolean(false));
            socket_object
                .borrow_mut()
                .set("readyState", Value::String("open".into()));
        }
        let inserted = owner
            .borrow_mut()
            .streams
            .iter_mut()
            .find(|stream| stream.resource() == resource)
            .map(|stream| stream.client_requests.insert(request_id, request.clone()))
            .is_some();
        if !inserted {
            return Err(ExecutionError::Host {
                instruction,
                message: "HTTP Agent connection disappeared during attachment".into(),
            });
        }
        let callback = Value::Bound(Rc::new(BoundFunction {
            target: Value::Native(Native::NodeClientRequestAssignSocket, RealmId::PRIMARY),
            this_value: request,
            arguments: vec![socket_object],
            properties: RefCell::new(ObjectData::default()),
        }));
        let scheduled = self.scheduled(Job::NextTick(callback, Vec::new()));
        self.with_realm_mut(|realm| realm.queue_next_tick(scheduled));
        Ok(())
    }

    pub(crate) fn flush_http_client_request(
        &mut self,
        state: &Rc<RefCell<HttpClientRequestState>>,
        socket: &Rc<RefCell<NetSocketState>>,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let resource = {
            let socket = socket.borrow();
            socket.resource.ok_or_else(|| ExecutionError::Host {
                instruction,
                message: "HTTP client request is not connected".into(),
            })?
        };
        let (request, body, ended, already_started, request_id) = {
            let mut state = state.borrow_mut();
            let mut headers = state.headers.clone();
            if !headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("host"))
            {
                headers.push(("host".into(), format!("{}:{}", state.host, state.port)));
            }
            let request = RuntimeHttpClientRequest {
                method: state.method.clone(),
                target: state.path.clone(),
                headers,
            };
            let body = std::mem::take(&mut state.body);
            (request, body, state.ended, state.started, state.request_id)
        };
        if already_started {
            if ended {
                let runtime = self.http_runtime.ok_or_else(|| ExecutionError::Host {
                    instruction,
                    message: "node:http client requires the realm HTTP runtime".into(),
                })?;
                unsafe {
                    ActiveHttpRuntime::end_client_request(&mut *runtime, resource, request_id, body)
                }
                .ok_or_else(|| ExecutionError::Host {
                    instruction,
                    message: "Hyper lost the HTTP client request body".into(),
                })?;
            }
            return Ok(());
        }
        let runtime = self.http_runtime.ok_or_else(|| ExecutionError::Host {
            instruction,
            message: "node:http client requires the realm HTTP runtime".into(),
        })?;
        if !unsafe {
            ActiveHttpRuntime::send_client_request(
                &mut *runtime,
                resource,
                request,
                request_id,
                DEFAULT_HIGH_WATER_MARK,
            )
        } {
            return Err(ExecutionError::Host {
                instruction,
                message: "Hyper rejected the HTTP client request".into(),
            });
        }
        state.borrow_mut().started = true;
        let owner = socket
            .borrow()
            .owner
            .as_ref()
            .and_then(Weak::upgrade)
            .ok_or_else(|| ExecutionError::Host {
                instruction,
                message: "HTTP client request lost its connection owner".into(),
            })?;
        let request_object = state
            .borrow()
            .object
            .as_ref()
            .map(|object| Value::Object(Rc::clone(object)))
            .ok_or_else(|| ExecutionError::Host {
                instruction,
                message: "HTTP client request lost its JavaScript object".into(),
            })?;
        let inserted = owner
            .borrow_mut()
            .streams
            .iter_mut()
            .find(|stream| stream.resource() == resource)
            .map(|connection| {
                connection
                    .client_requests
                    .insert(request_id, request_object);
            })
            .is_some();
        if !inserted {
            return Err(ExecutionError::Host {
                instruction,
                message: "HTTP client request lost its runtime stream".into(),
            });
        }
        if ended {
            unsafe {
                ActiveHttpRuntime::end_client_request(&mut *runtime, resource, request_id, body)
            }
            .ok_or_else(|| ExecutionError::Host {
                instruction,
                message: "Hyper lost the HTTP client request body".into(),
            })?;
        } else if !body.is_empty() {
            unsafe {
                ActiveHttpRuntime::write_client_request(&mut *runtime, resource, request_id, body)
            }
            .ok_or_else(|| ExecutionError::Host {
                instruction,
                message: "Hyper lost the HTTP client request body".into(),
            })?;
        }
        Ok(())
    }

    pub(crate) fn dispatch_node_event(
        &mut self,
        native: Native,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let object: &RefCell<ObjectData> = match receiver {
            Value::Object(object) => object.as_ref(),
            Value::Function(closure) => &closure.properties,
            _ => {
                if native == Native::NodeEventListenerCount
                    && let Some(first) = arguments.first()
                    && let Value::Object(obj) = first
                {
                    let event = arguments
                        .get(1)
                        .unwrap_or(&Value::Undefined)
                        .to_display_string();
                    return Ok(Value::Number(
                        node_listeners(obj.as_ref(), &event).len() as f64
                    ));
                }
                return Err(ExecutionError::TypeErrorMessage {
                    message: "EventEmitter method called on an incompatible receiver".into(),
                });
            }
        };
        match native {
            Native::NodeEventOn | Native::NodeEventOnce => {
                let event = arguments
                    .first()
                    .unwrap_or(&Value::Undefined)
                    .to_display_string();
                let listener = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                if !is_callable(&listener) {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "The listener argument must be a function".into(),
                    });
                }
                if event != "newListener" {
                    for registered in node_listeners(object, "newListener") {
                        if registered.once {
                            remove_node_listener(object, "newListener", &registered.callback);
                        }
                        self.call_value(
                            &registered.callback,
                            receiver.clone(),
                            vec![Value::String(event.clone().into()), listener.clone()],
                            instruction,
                        )?;
                    }
                }
                let mut data = object.borrow_mut();
                let buckets = data.node_events.get_or_insert_with(Vec::new);
                let bucket =
                    if let Some(index) = buckets.iter().position(|entry| entry.name == event) {
                        &mut buckets[index]
                    } else {
                        buckets.push(NodeEventBucket {
                            name: event.clone(),
                            listeners: Vec::new(),
                        });
                        buckets.last_mut().expect("bucket appended")
                    };
                bucket.listeners.push(NodeEventListener {
                    callback: listener,
                    once: matches!(native, Native::NodeEventOnce),
                });
                drop(data);
                // Attaching a `data` listener switches a Readable to flowing mode. Do this after
                // registration so any bytes already buffered while the transport or a framework
                // was paused are delivered to the listener that caused the transition.
                if event == "data"
                    && (node_readable_state(receiver).is_ok()
                        || node_http_incoming(receiver).is_ok())
                {
                    self.dispatch_node_readable(
                        Native::NodeReadableResume,
                        receiver,
                        &[],
                        instruction,
                    )?;
                }
                // A buffered or ended Readable whose earlier notification had no listener must
                // announce itself again when `readable` is observed. Registration happens first,
                // matching EventEmitter ordering and ensuring the new listener receives this
                // notification rather than another one being lost.
                if event == "readable"
                    && let Ok(state) = node_readable_state(receiver)
                {
                    let (announce, pull) = {
                        let mut state = state.borrow_mut();
                        let announce = state.buffered_bytes != 0 || state.ended;
                        let pull = !announce && !state.reading && !state.ended;
                        if pull {
                            state.reading = true;
                        }
                        (announce, pull)
                    };
                    sync_readable_public_state(receiver, &state.borrow());
                    if announce {
                        self.dispatch_node_event(
                            Native::NodeEventEmit,
                            receiver,
                            &[Value::String("readable".into())],
                            instruction,
                        )?;
                    } else if pull {
                        let callback = self.read_property(receiver, "_read", instruction)?;
                        if is_callable(&callback) {
                            self.call_value(&callback, receiver.clone(), Vec::new(), instruction)?;
                        }
                    }
                }
                Ok(receiver.clone())
            }
            Native::NodeEventEmit => {
                let event = arguments
                    .first()
                    .unwrap_or(&Value::Undefined)
                    .to_display_string();
                let listeners = node_listeners(object, &event);
                let emitted = !listeners.is_empty();
                let payload = arguments.get(1..).unwrap_or_default().to_vec();
                for registered in listeners {
                    if registered.once {
                        remove_node_listener(object, &event, &registered.callback);
                    }
                    self.call_value(
                        &registered.callback,
                        receiver.clone(),
                        payload.clone(),
                        instruction,
                    )?;
                }
                Ok(Value::Boolean(emitted))
            }
            Native::NodeEventListeners => {
                let event = arguments
                    .first()
                    .unwrap_or(&Value::Undefined)
                    .to_display_string();
                Ok(Value::Array(ArrayData::new(
                    node_listeners(object, &event)
                        .into_iter()
                        .map(|listener| listener.callback)
                        .collect(),
                )))
            }
            Native::NodeEventListenerCount => {
                let event = arguments
                    .first()
                    .unwrap_or(&Value::Undefined)
                    .to_display_string();
                Ok(Value::Number(node_listeners(object, &event).len() as f64))
            }
            Native::NodeEventNames => Ok(Value::Array(ArrayData::new(
                object
                    .borrow()
                    .node_events
                    .as_ref()
                    .into_iter()
                    .flatten()
                    .filter(|bucket| !bucket.listeners.is_empty())
                    .map(|bucket| Value::String(bucket.name.clone().into()))
                    .collect(),
            ))),
            Native::NodeEventRemoveListener => {
                let event = arguments
                    .first()
                    .unwrap_or(&Value::Undefined)
                    .to_display_string();
                let listener = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                if !is_callable(&listener) {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "The listener argument must be a function".into(),
                    });
                }
                // Node removes the most recently added matching listener. This matters when the
                // same callback was registered repeatedly and preserves the order of survivors.
                remove_last_node_listener(object, &event, &listener);
                Ok(receiver.clone())
            }
            Native::NodeEventRemoveAllListeners => {
                let mut data = object.borrow_mut();
                if let Some(event) = arguments.first() {
                    let event = event.to_display_string();
                    if let Some(buckets) = data.node_events.as_mut() {
                        buckets.retain(|bucket| bucket.name != event);
                    }
                } else {
                    data.node_events = None;
                }
                Ok(receiver.clone())
            }
            Native::NodeEventSetMaxListeners => Ok(receiver.clone()),
            _ => unreachable!("only EventEmitter methods dispatch here"),
        }
    }

    pub(crate) fn dispatch_node_native(
        &mut self,
        native: Native,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        if native == Native::ProcessStartRaw {
            let bytes = match arguments.first() {
                Some(Value::TypedArray(bytes)) => bytes.bytes(),
                value => {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: format!(
                            "process host request must be Uint8Array, received {}",
                            value.map_or("undefined", Value::type_name)
                        ),
                    });
                }
            };
            let request = kiln_runtime::decode_process_wire(&bytes).map_err(|error| {
                ExecutionError::Host {
                    instruction,
                    message: format!("invalid process host request: {error:?}"),
                }
            })?;
            let promise = PromiseState::pending();
            let node = self.async_host.ok_or_else(|| ExecutionError::Host {
                instruction,
                message: "process.run requires the Node execution Adapter".into(),
            })?;
            let command = String::from_utf8(request.command).map_err(|_| {
                ExecutionError::TypeErrorMessage {
                    message: "process command must be UTF-8".into(),
                }
            })?;
            let args = request
                .args
                .into_iter()
                .map(|arg| {
                    String::from_utf8(arg).map_err(|_| ExecutionError::TypeErrorMessage {
                        message: "process argument must be UTF-8".into(),
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let id = unsafe {
                ActiveAsyncHostOps::start(
                    &mut *node,
                    AsyncHostOp::ProcessRun {
                        command,
                        args,
                        stdin: request.stdin,
                    },
                    true,
                )
            };
            self.push_pending_host(PendingHostRequest {
                request: id,
                promise: Rc::clone(&promise),
                conversion: HostResultConversion::Direct,
            });
            let mut result = ObjectData::default();
            result.set("requestId", Value::Number(id.get() as f64));
            result.set("promise", Value::Promise(promise));
            return Ok(Value::Object(Rc::new(RefCell::new(result))));
        }
        if native == Native::ProcessCancelRaw {
            let id = arguments.first().map(Value::as_number).unwrap_or(0.0) as u64;
            let node = self.async_host.ok_or_else(|| ExecutionError::Host {
                instruction,
                message: "process.run requires the Node execution Adapter".into(),
            })?;
            return Ok(Value::Boolean(unsafe {
                ActiveAsyncHostOps::cancel(&mut *node, RequestId::new(id))
            }));
        }
        let path = match arguments.first() {
            Some(Value::String(path)) => path.clone(),
            value => {
                return Err(ExecutionError::TypeErrorMessage {
                    message: format!(
                        "path must be a string, received {}",
                        value.map_or("undefined", Value::type_name)
                    ),
                });
            }
        };
        let operation = match native {
            Native::NodeFsReadFile => AsyncHostOp::ReadFile {
                path: path.to_lossy_string(),
            },
            Native::NodeFsWriteFile => {
                let data = match arguments.get(1) {
                    Some(Value::String(data)) => data.to_lossy_string().into_bytes(),
                    Some(Value::TypedArray(data)) => data.bytes(),
                    value => {
                        return Err(ExecutionError::TypeErrorMessage {
                            message: format!(
                                "data must be a string or typed array, received {}",
                                value.map_or("undefined", Value::type_name)
                            ),
                        });
                    }
                };
                AsyncHostOp::WriteFile {
                    path: path.to_lossy_string(),
                    data,
                }
            }
            Native::NodeFsAppendFile => {
                let data = match arguments.get(1) {
                    Some(Value::String(data)) => data.to_lossy_string().into_bytes(),
                    Some(Value::TypedArray(data)) => data.bytes(),
                    value => {
                        return Err(ExecutionError::TypeErrorMessage {
                            message: format!(
                                "data must be a string or typed array, received {}",
                                value.map_or("undefined", Value::type_name)
                            ),
                        });
                    }
                };
                AsyncHostOp::AppendFile {
                    path: path.to_lossy_string(),
                    data,
                }
            }
            Native::NodeFsStat => AsyncHostOp::Stat {
                path: path.to_lossy_string(),
            },
            Native::NodeFsLstat => AsyncHostOp::Lstat {
                path: path.to_lossy_string(),
            },
            Native::NodeFsMkdir => {
                let recursive = arguments.get(1).map_or(false, |opt| {
                    if let Value::Object(obj) = opt {
                        obj.borrow()
                            .get_own("recursive")
                            .map_or(false, Value::is_truthy)
                    } else if let Value::Boolean(b) = opt {
                        *b
                    } else {
                        false
                    }
                });
                AsyncHostOp::Mkdir {
                    path: path.to_lossy_string(),
                    recursive,
                }
            }
            Native::NodeFsReaddir => {
                let with_types = arguments.get(1).map_or(false, |opt| {
                    if let Value::Object(obj) = opt {
                        obj.borrow()
                            .get_own("withFileTypes")
                            .map_or(false, Value::is_truthy)
                    } else {
                        false
                    }
                });
                AsyncHostOp::ReadDir {
                    path: path.to_lossy_string(),
                    with_types,
                }
            }
            Native::NodeFsCopyFile => {
                let to = match arguments.get(1) {
                    Some(Value::String(s)) => s.to_lossy_string(),
                    val => {
                        return Err(ExecutionError::TypeErrorMessage {
                            message: format!(
                                "destination path must be a string, received {}",
                                val.map_or("undefined", Value::type_name)
                            ),
                        });
                    }
                };
                AsyncHostOp::CopyFile {
                    from: path.to_lossy_string(),
                    to,
                }
            }
            Native::NodeFsRename => {
                let to = match arguments.get(1) {
                    Some(Value::String(s)) => s.to_lossy_string(),
                    val => {
                        return Err(ExecutionError::TypeErrorMessage {
                            message: format!(
                                "new path must be a string, received {}",
                                val.map_or("undefined", Value::type_name)
                            ),
                        });
                    }
                };
                AsyncHostOp::Rename {
                    from: path.to_lossy_string(),
                    to,
                }
            }
            Native::NodeFsRm => {
                let (recursive, force) = match arguments.get(1) {
                    Some(Value::Object(obj)) => {
                        let rec = obj
                            .borrow()
                            .get_own("recursive")
                            .map_or(false, Value::is_truthy);
                        let frc = obj
                            .borrow()
                            .get_own("force")
                            .map_or(false, Value::is_truthy);
                        (rec, frc)
                    }
                    _ => (false, false),
                };
                AsyncHostOp::Rm {
                    path: path.to_lossy_string(),
                    recursive,
                    force,
                }
            }
            Native::NodeFsUnlink => AsyncHostOp::Unlink {
                path: path.to_lossy_string(),
            },
            Native::NodeFsRealpath => AsyncHostOp::Realpath {
                path: path.to_lossy_string(),
            },
            Native::NodeFsReadlink => AsyncHostOp::Readlink {
                path: path.to_lossy_string(),
            },
            Native::NodeFsAccess => {
                let mode = match arguments.get(1) {
                    Some(Value::Number(n)) => *n as u32,
                    _ => 0,
                };
                AsyncHostOp::Access {
                    path: path.to_lossy_string(),
                    mode,
                }
            }
            Native::NodeFsChmod => {
                let mode = match arguments.get(1) {
                    Some(Value::Number(n)) => *n as u32,
                    val => {
                        return Err(ExecutionError::TypeErrorMessage {
                            message: format!(
                                "mode must be a number, received {}",
                                val.map_or("undefined", Value::type_name)
                            ),
                        });
                    }
                };
                AsyncHostOp::Chmod {
                    path: path.to_lossy_string(),
                    mode,
                }
            }
            Native::NodeFsRmdir => AsyncHostOp::Rmdir {
                path: path.to_lossy_string(),
            },
            _ => {
                return Err(ExecutionError::Host {
                    instruction,
                    message: "unknown Node filesystem operation".to_owned(),
                });
            }
        };
        let promise = PromiseState::pending();
        let node = self.async_host.ok_or_else(|| ExecutionError::Host {
            instruction,
            message: "node filesystem operation requires the Node execution Adapter".to_owned(),
        })?;
        // The pointer names the caller-owned realm runtime. It outlives this VM and is accessed
        // only while that current-thread owner is polling the VM, so no alias can concurrently
        // mutate it. The trait hides Tokio and the runtime's resource table from language code.
        let request = unsafe { ActiveAsyncHostOps::start(&mut *node, operation, true) };
        let conversion = match native {
            Native::NodeFsReadFile if node_encoding(arguments.get(1))? => {
                HostResultConversion::Utf8
            }
            Native::NodeFsReaddir => {
                let with_file_types = arguments.get(1).map_or(false, |opt| {
                    if let Value::Object(obj) = opt {
                        obj.borrow()
                            .get_own("withFileTypes")
                            .map_or(false, Value::is_truthy)
                    } else {
                        false
                    }
                });
                HostResultConversion::DirEntries { with_file_types }
            }
            _ => HostResultConversion::Direct,
        };
        self.push_pending_host(PendingHostRequest {
            request,
            promise: Rc::clone(&promise),
            conversion,
        });
        Ok(Value::Promise(promise))
    }

    pub(crate) fn settle_host_completion(
        &mut self,
        completion: Completion,
    ) -> Result<(), ExecutionError> {
        let pending_node = if let Some(realm) = &self.module_realm {
            let mut realm = realm.borrow_mut();
            realm
                .pending_node
                .iter()
                .position(|pending| pending.request() == completion.request)
                .map(|position| realm.pending_node.swap_remove(position))
        } else {
            self.pending_node
                .iter()
                .position(|pending| pending.request() == completion.request)
                .map(|position| self.pending_node.swap_remove(position))
        };
        if let Some(pending) = pending_node {
            return match pending {
                PendingNodeRequest::Server { state, .. } => {
                    let actions = {
                        let mut state = state.borrow_mut();
                        state
                            .driver
                            .completed(completion)
                            .expect("request was routed to its server driver")
                    };
                    for action in actions {
                        self.apply_http_server_action(&state, action, self.instruction)?;
                    }
                    Ok(())
                }
                PendingNodeRequest::Stream {
                    server,
                    resource,
                    write_callbacks,
                    ..
                } => {
                    let callback_result = completion.result.clone();
                    // Only raw `node:net` sockets read and write through host operations;
                    // an HTTP connection's bytes belong to the engine (ADR 0046).
                    let mut server_state = server.borrow_mut();
                    let stream = server_state
                        .streams
                        .iter_mut()
                        .find(|stream| stream.resource() == resource)
                        .ok_or_else(|| ExecutionError::Host {
                            instruction: self.instruction,
                            message: "completion lost its node:net stream".into(),
                        })?;
                    stream.applying_read_completion = true;
                    let socket = stream.socket.clone();
                    let actions = stream.driver.completed(completion).ok_or_else(|| {
                        ExecutionError::Host {
                            instruction: self.instruction,
                            message: "completion lost its node:net stream".into(),
                        }
                    })?;
                    drop(server_state);
                    if !write_callbacks.is_empty() {
                        let callback_args = match &callback_result {
                            // Writable callbacks receive the error slot even on success; Node
                            // supplies `null`, which code commonly tests with `err && err.code`.
                            Ok(_) => vec![Value::Null],
                            Err(error) => {
                                let value = self.construct_error(
                                    ErrorKind::Error,
                                    &[Value::String(error.message.clone().into())],
                                );
                                if let Value::Object(object) = &value {
                                    let mut object = object.borrow_mut();
                                    object.set_non_enumerable(
                                        "code",
                                        Value::String(error.code.clone().into()),
                                    );
                                    object.set_non_enumerable(
                                        "syscall",
                                        Value::String(error.syscall.clone().into()),
                                    );
                                }
                                vec![value]
                            }
                        };
                        for callback in write_callbacks {
                            self.call_value(
                                &callback,
                                Value::Undefined,
                                callback_args.clone(),
                                self.instruction,
                            )?;
                        }
                    }
                    for action in actions {
                        // A callback may destroy the socket after the driver selected these
                        // actions. Suppress stale data and follow-on operations, but deliver
                        // the eventual resource-close completion and retire its stream below.
                        // Recheck after each action because event listeners can also destroy it.
                        if socket
                            .as_ref()
                            .is_some_and(|socket| socket.borrow().destroyed)
                            && !matches!(action, StreamAction::Event(StreamEvent::Close))
                        {
                            continue;
                        }
                        self.start_http_stream_action(&server, resource, action, self.instruction)?;
                    }
                    sync_node_socket_ready_state(&server, resource);
                    if let Some(stream) = server
                        .borrow_mut()
                        .streams
                        .iter_mut()
                        .find(|stream| stream.resource() == resource)
                    {
                        stream.applying_read_completion = false;
                    }
                    let still_pending = if let Some(realm) = &self.module_realm {
                        realm.borrow().pending_node.iter().any(|pending| {
                            matches!(
                                pending,
                                PendingNodeRequest::Stream {
                                    resource: pending_resource,
                                    ..
                                } if *pending_resource == resource
                            )
                        })
                    } else {
                        self.pending_node.iter().any(|pending| {
                            matches!(
                                pending,
                                PendingNodeRequest::Stream {
                                    resource: pending_resource,
                                    ..
                                } if *pending_resource == resource
                            )
                        })
                    };
                    if !still_pending {
                        server.borrow_mut().streams.retain(|stream| {
                            stream.resource() != resource
                                || !stream.closed
                                || stream.request.is_some()
                                || stream.response.is_some()
                        });
                    }
                    Ok(())
                }
                PendingNodeRequest::CloseResource { .. } => Ok(()),
                PendingNodeRequest::Connect { target, .. } => match completion.result {
                    Ok(AsyncHostValue::Resource(resource)) => {
                        let resource = resource.id;
                        let destroyed = match &target {
                            PendingConnect::Net(state) => state.borrow().destroyed,
                            PendingConnect::Http(state) => state
                                .borrow()
                                .socket
                                .as_ref()
                                .is_some_and(|socket| socket.borrow().destroyed),
                        };
                        if destroyed {
                            return self.close_late_connect_resource(resource, self.instruction);
                        }
                        let (socket_state, socket, protocol, request) = match &target {
                            PendingConnect::Net(state) => {
                                state.borrow_mut().connecting = false;
                                state.borrow_mut().connect_request = None;
                                let socket = state
                                    .borrow()
                                    .object
                                    .as_ref()
                                    .and_then(Weak::upgrade)
                                    .map(Value::Object)
                                    .unwrap_or(Value::Undefined);
                                (
                                    Rc::clone(state),
                                    socket.clone(),
                                    ServerProtocol::Net,
                                    socket,
                                )
                            }
                            PendingConnect::Http(state) => {
                                let socket_state =
                                    state.borrow().socket.clone().ok_or_else(|| {
                                        ExecutionError::Host {
                                            instruction: self.instruction,
                                            message:
                                                "HTTP connect completed without its pending socket"
                                                    .into(),
                                        }
                                    })?;
                                let socket = socket_state
                                    .borrow()
                                    .object
                                    .as_ref()
                                    .and_then(Weak::upgrade)
                                    .map(Value::Object)
                                    .unwrap_or(Value::Undefined);
                                socket_state.borrow_mut().connecting = false;
                                socket_state.borrow_mut().resource = Some(resource);
                                socket_state.borrow_mut().connect_request = None;
                                if let Value::Object(object) = &socket {
                                    object.borrow_mut().set("connecting", Value::Boolean(false));
                                    object
                                        .borrow_mut()
                                        .set("readyState", Value::String("open".into()));
                                }
                                let request = state
                                    .borrow()
                                    .object
                                    .as_ref()
                                    .map(|object| Value::Object(Rc::clone(object)))
                                    .unwrap_or(Value::Undefined);
                                let runtime =
                                    self.http_runtime.ok_or_else(|| ExecutionError::Host {
                                        instruction: self.instruction,
                                        message: "node:http client requires the realm HTTP runtime"
                                            .into(),
                                    })?;
                                let request_state = state.borrow();
                                if !unsafe {
                                    ActiveHttpRuntime::client_connection_connected(
                                        &mut *runtime,
                                        HttpOrigin::http(&request_state.host, request_state.port),
                                        resource,
                                        request_state.request_id,
                                    )
                                } {
                                    return Err(ExecutionError::Host {
                                        instruction: self.instruction,
                                        message: "HTTP Agent rejected the connected transport"
                                            .into(),
                                    });
                                }
                                drop(request_state);
                                (socket_state, socket, ServerProtocol::HttpClient, request)
                            }
                        };
                        let mut connection = NodeConnection {
                            driver: StreamDriver::new(resource, DEFAULT_HIGH_WATER_MARK),
                            runtime_http: protocol == ServerProtocol::HttpClient,
                            upgrade_request_id: None,
                            upgrading: false,
                            socket: Some(Rc::clone(&socket_state)),
                            socket_object: Some(socket.clone()),
                            request: (protocol != ServerProtocol::Net).then(|| request.clone()),
                            response: None,
                            client_requests: std::collections::BTreeMap::new(),
                            client_responses: std::collections::BTreeMap::new(),
                            http_responses: Vec::new(),
                            close_when_response_ends: false,
                            closed: false,
                            applying_read_completion: false,
                        };
                        if let PendingConnect::Http(state) = &target {
                            let request_id = state.borrow().request_id;
                            connection
                                .client_requests
                                .insert(request_id, request.clone());
                        }
                        if protocol == ServerProtocol::HttpClient {
                            let runtime =
                                self.http_runtime.ok_or_else(|| ExecutionError::Host {
                                    instruction: self.instruction,
                                    message: "node:http client requires the realm HTTP runtime"
                                        .into(),
                                })?;
                            // ADR 0046: the connected socket goes to the client engine, whose
                            // task queues response events for the checkpoint to drain.
                            let attached = unsafe {
                                ActiveHttpRuntime::start_client_connection(&mut *runtime, resource)
                            };
                            if !attached {
                                return Err(ExecutionError::Host {
                                    instruction: self.instruction,
                                    message: "the host could not hand the connected socket to \
                                              the HTTP engine"
                                        .into(),
                                });
                            }
                        }
                        let mut pending_write_callbacks = Vec::new();
                        let net_actions = if protocol == ServerProtocol::Net {
                            let mut actions = Vec::new();
                            let (pending_write, end_requested, callbacks) = {
                                let mut socket = socket_state.borrow_mut();
                                (
                                    std::mem::take(&mut socket.pending_write),
                                    socket.end_requested,
                                    std::mem::take(&mut socket.corked_callbacks),
                                )
                            };
                            let has_callbacks = !callbacks.is_empty();
                            pending_write_callbacks = callbacks;
                            if !pending_write.is_empty() || has_callbacks {
                                actions.extend(connection.driver.write(pending_write).1);
                            }
                            if end_requested {
                                actions.extend(connection.driver.end(None).1);
                            }
                            actions.extend(connection.driver.start_reading());
                            actions
                        } else {
                            Vec::new()
                        };
                        let owner = Rc::new(RefCell::new(NodeServerState {
                            driver: TcpServerDriver::default(),
                            handler: Value::Undefined,
                            protocol,
                            allow_half_open: false,
                            timeout_milliseconds: 0.0,
                            streams: vec![connection],
                            object: None,
                            fetch: false,
                            close_deferred: false,
                        }));
                        {
                            let mut socket_state = socket_state.borrow_mut();
                            socket_state.owner = Some(Rc::downgrade(&owner));
                            socket_state.resource = Some(resource);
                        }
                        if protocol == ServerProtocol::Net
                            && let Value::Object(object) = &socket
                        {
                            object.borrow_mut().set("connecting", Value::Boolean(false));
                            object
                                .borrow_mut()
                                .set("readyState", Value::String("open".into()));
                        }
                        self.register_node_server(&owner);
                        if protocol == ServerProtocol::Net {
                            self.dispatch_node_event(
                                Native::NodeEventEmit,
                                &socket,
                                &[Value::String("connect".into())],
                                self.instruction,
                            )?;
                        } else if let PendingConnect::Http(state) = &target {
                            self.dispatch_node_event(
                                Native::NodeEventEmit,
                                &socket,
                                &[Value::String("connect".into())],
                                self.instruction,
                            )?;
                            let pending_write = {
                                let mut socket = socket_state.borrow_mut();
                                std::mem::take(&mut socket.pending_write)
                            };
                            if !pending_write.is_empty() {
                                let actions = {
                                    let mut owner = owner.borrow_mut();
                                    let connection = owner
                                        .streams
                                        .iter_mut()
                                        .find(|stream| stream.resource() == resource)
                                        .ok_or_else(|| ExecutionError::Host {
                                            instruction: self.instruction,
                                            message: "HTTP client lost its stream before flushing socket writes".into(),
                                        })?;
                                    connection.driver.write(pending_write).1
                                };
                                for action in actions {
                                    self.start_http_stream_action(
                                        &owner,
                                        resource,
                                        action,
                                        self.instruction,
                                    )?;
                                }
                            }
                            let end_socket = socket_state.borrow().end_requested;
                            if end_socket {
                                let actions = {
                                    let mut owner = owner.borrow_mut();
                                    let connection = owner
                                        .streams
                                        .iter_mut()
                                        .find(|stream| stream.resource() == resource)
                                        .ok_or_else(|| ExecutionError::Host {
                                            instruction: self.instruction,
                                            message:
                                                "HTTP client lost its stream before socket end"
                                                    .into(),
                                        })?;
                                    connection.driver.end(None).1
                                };
                                for action in actions {
                                    self.start_http_stream_action(
                                        &owner,
                                        resource,
                                        action,
                                        self.instruction,
                                    )?;
                                }
                            } else if state.borrow().ended {
                                self.flush_http_client_request(
                                    state,
                                    &socket_state,
                                    self.instruction,
                                )?;
                            }
                        }
                        for action in net_actions {
                            if matches!(&action, StreamAction::Start(AsyncHostOp::TcpRead { .. })) {
                                let already_started = owner.borrow().streams.iter().any(|stream| {
                                    stream.resource() == resource
                                        && stream.driver.has_pending_read()
                                });
                                if socket_state.borrow().read_paused || already_started {
                                    continue;
                                }
                            }
                            if matches!(&action, StreamAction::Start(AsyncHostOp::TcpWrite { .. }))
                            {
                                self.start_http_stream_action_with_callbacks(
                                    &owner,
                                    resource,
                                    action,
                                    self.instruction,
                                    pending_write_callbacks,
                                )?;
                                pending_write_callbacks = Vec::new();
                            } else {
                                self.start_http_stream_action(
                                    &owner,
                                    resource,
                                    action,
                                    self.instruction,
                                )?;
                            }
                        }
                        Ok(())
                    }
                    Ok(_) => Err(ExecutionError::Host {
                        instruction: self.instruction,
                        message: "node:net connect returned a non-stream resource".into(),
                    }),
                    Err(error) => {
                        let destroyed = match &target {
                            PendingConnect::Net(state) => state.borrow().destroyed,
                            PendingConnect::Http(state) => state
                                .borrow()
                                .socket
                                .as_ref()
                                .is_some_and(|socket| socket.borrow().destroyed),
                        };
                        if destroyed {
                            return Ok(());
                        }
                        let socket_value = match &target {
                            PendingConnect::Net(state) => {
                                {
                                    let mut state = state.borrow_mut();
                                    state.connecting = false;
                                    state.destroyed = true;
                                    state.connect_request = None;
                                }
                                state
                                    .borrow()
                                    .object
                                    .as_ref()
                                    .and_then(Weak::upgrade)
                                    .map(Value::Object)
                            }
                            PendingConnect::Http(state) => {
                                let socket = state.borrow().socket.clone();
                                if let Some(socket) = &socket {
                                    socket.borrow_mut().connecting = false;
                                    socket.borrow_mut().connect_request = None;
                                }
                                socket.and_then(|socket| {
                                    socket
                                        .borrow()
                                        .object
                                        .as_ref()
                                        .and_then(Weak::upgrade)
                                        .map(Value::Object)
                                })
                            }
                        };
                        let (pending_write_callbacks, end_callbacks) = match &target {
                            PendingConnect::Net(state) => {
                                Self::take_node_socket_queued_state(state)
                            }
                            PendingConnect::Http(state) => state
                                .borrow()
                                .socket
                                .as_ref()
                                .map(Self::take_node_socket_queued_state)
                                .unwrap_or_default(),
                        };
                        if let Some(socket) = socket_value.as_ref() {
                            if let Value::Object(object) = &socket {
                                object.borrow_mut().set("connecting", Value::Boolean(false));
                                object.borrow_mut().set("destroyed", Value::Boolean(true));
                                object
                                    .borrow_mut()
                                    .set("readyState", Value::String("closed".into()));
                            }
                            let emit = self.read_property(&socket, "emit", self.instruction)?;
                            let callback = Value::Bound(Rc::new(BoundFunction {
                                target: emit,
                                this_value: (*socket).clone(),
                                arguments: vec![Value::String("close".into())],
                                properties: RefCell::new(ObjectData::default()),
                            }));
                            let scheduled = self.scheduled(Job::NextTick(callback, Vec::new()));
                            self.with_realm_mut(|realm| realm.queue_next_tick(scheduled));
                        }
                        let retry = if let PendingConnect::Http(state) = &target {
                            let origin = {
                                let state = state.borrow();
                                HttpOrigin::http(&state.host, state.port)
                            };
                            let runtime =
                                self.http_runtime.ok_or_else(|| ExecutionError::Host {
                                    instruction: self.instruction,
                                    message: "node:http client requires the realm HTTP runtime"
                                        .into(),
                                })?;
                            let request_id = state.borrow().request_id;
                            Some(unsafe {
                                ActiveHttpRuntime::client_connection_failed(
                                    &mut *runtime,
                                    &origin,
                                    request_id,
                                )
                            })
                        } else {
                            None
                        };
                        let error = self.node_error_value(error);
                        let object = match &target {
                            PendingConnect::Net(state) => state
                                .borrow()
                                .object
                                .as_ref()
                                .and_then(Weak::upgrade)
                                .map(Value::Object)
                                .unwrap_or(Value::Undefined),
                            PendingConnect::Http(state) => state
                                .borrow()
                                .object
                                .as_ref()
                                .map(|object| Value::Object(Rc::clone(object)))
                                .unwrap_or(Value::Undefined),
                        };
                        let queued_net_write = matches!(&target, PendingConnect::Net(_))
                            && !pending_write_callbacks.is_empty();
                        if queued_net_write {
                            self.dispatch_node_event(
                                Native::NodeEventEmit,
                                &object,
                                &[Value::String("error".into()), error.clone()],
                                self.instruction,
                            )?;
                        }
                        let write_error = if queued_net_write {
                            let error = self.construct_error(
                                ErrorKind::Error,
                                &[Value::String(
                                    "Socket closed before the connection was established".into(),
                                )],
                            );
                            if let Value::Object(object) = &error {
                                object.borrow_mut().set_non_enumerable(
                                    "code",
                                    Value::String("ERR_SOCKET_CLOSED_BEFORE_CONNECTION".into()),
                                );
                            }
                            error
                        } else {
                            error.clone()
                        };
                        for callback in pending_write_callbacks {
                            self.call_value(
                                &callback,
                                Value::Undefined,
                                vec![write_error.clone()],
                                self.instruction,
                            )?;
                        }
                        for callback in end_callbacks {
                            self.call_value(
                                &callback,
                                Value::Undefined,
                                vec![error.clone()],
                                self.instruction,
                            )?;
                        }
                        if !queued_net_write {
                            self.dispatch_node_event(
                                Native::NodeEventEmit,
                                &object,
                                &[Value::String("error".into()), error],
                                self.instruction,
                            )?;
                        }
                        if let Some(AgentFailure::Connect { request }) = retry {
                            let queued = self.http_client_request(request).ok_or_else(|| {
                                ExecutionError::Host {
                                    instruction: self.instruction,
                                    message: "HTTP Agent lost its queued ClientRequest".into(),
                                }
                            })?;
                            self.start_http_client_connection(&queued, self.instruction)?;
                        }
                        Ok(())
                    }
                },
            };
        }
        let pending = if let Some(realm) = &self.module_realm {
            let mut realm = realm.borrow_mut();
            realm
                .pending_host
                .iter()
                .position(|pending| pending.request == completion.request)
                .map(|position| realm.pending_host.swap_remove(position))
        } else {
            self.pending_host
                .iter()
                .position(|pending| pending.request == completion.request)
                .map(|position| self.pending_host.swap_remove(position))
        };
        let Some(pending) = pending else {
            return Err(ExecutionError::Host {
                instruction: self.instruction,
                message: format!(
                    "completion for unknown request {}",
                    completion.request.get()
                ),
            });
        };
        let outcome = match completion.result {
            Ok(AsyncHostValue::Bytes(bytes))
                if matches!(pending.conversion, HostResultConversion::Utf8) =>
            {
                Settled::Fulfilled(Value::String(
                    String::from_utf8_lossy(&bytes).into_owned().into(),
                ))
            }
            Ok(AsyncHostValue::DirEntries(entries)) => {
                let with_file_types = match pending.conversion {
                    HostResultConversion::DirEntries { with_file_types } => with_file_types,
                    _ => false,
                };
                let mut out = Vec::new();
                for entry in entries {
                    if with_file_types {
                        let mut dirent = ObjectData::default();
                        dirent.set("name", Value::String(entry.name.into()));
                        dirent.set_non_enumerable(
                            "@@kiln:node:is-file",
                            Value::Boolean(entry.is_file),
                        );
                        dirent.set_non_enumerable(
                            "@@kiln:node:is-directory",
                            Value::Boolean(entry.is_directory),
                        );
                        dirent.set_non_enumerable(
                            "@@kiln:node:is-symlink",
                            Value::Boolean(entry.is_symlink),
                        );
                        dirent.set(
                            "isFile",
                            Value::Native(Native::NodeStatIsFile, RealmId::PRIMARY),
                        );
                        dirent.set(
                            "isDirectory",
                            Value::Native(Native::NodeStatIsDirectory, RealmId::PRIMARY),
                        );
                        dirent.set(
                            "isSymbolicLink",
                            Value::Native(Native::NodeStatIsSymbolicLink, RealmId::PRIMARY),
                        );
                        out.push(Value::Object(Rc::new(RefCell::new(dirent))));
                    } else {
                        out.push(Value::String(entry.name.into()));
                    }
                }
                Settled::Fulfilled(Value::Array(ArrayData::new(out)))
            }
            Ok(value) => Settled::Fulfilled(node_host_value(value)?),
            Err(error) => Settled::Rejected(self.node_error_value(error)),
        };
        self.settle(&pending.promise, &outcome);
        Ok(())
    }

    pub(crate) fn run_node_event_loop(
        &mut self,
        runtime: &mut RealmRuntime,
    ) -> Result<(), ExecutionError> {
        self.run_node_event_loop_inner(runtime, false)
    }

    /// Run the shared Node event loop until this module's top-level await settles.
    ///
    /// A dependency may leave a live server or stream behind when its body awaits. Those
    /// resources belong to the graph and must remain available to later modules, but they must
    /// not keep dependency evaluation waiting after its own continuation has completed.
    pub(crate) fn run_node_event_loop_until_module(
        &mut self,
        runtime: &mut RealmRuntime,
    ) -> Result<(), ExecutionError> {
        self.run_node_event_loop_inner(runtime, true)
    }

    pub(crate) fn run_node_event_loop_inner(
        &mut self,
        runtime: &mut RealmRuntime,
        until_module: bool,
    ) -> Result<(), ExecutionError> {
        loop {
            self.drain_microtasks(runtime)?;
            self.checkpoint_runtime_http(runtime)?;
            // An entry-module rejection must reach its caller even if an actor, server or
            // timer remains referenced. Successful entries still drain their live resources;
            // dependencies stop as soon as either evaluation outcome is available.
            if self
                .module_outcome()
                .is_some_and(|outcome| until_module || matches!(outcome, Settled::Rejected(_)))
            {
                return Ok(());
            }
            // Pending adapter entries are routing metadata, not liveness. Runtime-owned HTTP
            // keeps reads registered as unreferenced so idle keep-alive sockets can be reused;
            // treating the mere entry as live work made detached async server mains wait on
            // cleanup reads forever. The provider is the sole authority for referenced host
            // resources and requests, while the realm owns timer liveness.
            let turn = self.with_realm_mut(|realm| runtime.next_referenced_turn(realm));
            let Some(turn) = turn else {
                return Ok(());
            };
            match turn {
                // The HTTP engines queued events; the checkpoint at the top of the loop
                // drains them.
                RuntimeTurn::Protocol => {}
                RuntimeTurn::Completion(completion) => {
                    runtime.enter(|| self.settle_host_completion(completion))?;
                    while let Some(next) = runtime.take_next_ready_completion() {
                        runtime.enter(|| self.settle_host_completion(next))?;
                    }
                }
                RuntimeTurn::Failure(failure) => {
                    return Err(ExecutionError::Host {
                        instruction: self.instruction,
                        message: failure.to_string(),
                    });
                }
                RuntimeTurn::Timer(WorkItem::Timer { work: timer, .. }) => {
                    // As for a microtask: the context is the one the timer was *set* in.
                    self.async_context = timer.context;
                    runtime.enter(|| {
                        self.call_value(
                            &timer.callback,
                            Value::Undefined,
                            timer.arguments,
                            self.instruction,
                        )
                    })?;
                }
                RuntimeTurn::Timer(WorkItem::Microtask(_)) => {
                    unreachable!("next_turn selects timers after microtasks are drained")
                }
            }
        }
    }

    pub(crate) fn checkpoint_runtime_http(
        &mut self,
        runtime: &mut RealmRuntime,
    ) -> Result<(), ExecutionError> {
        let shared = self.module_realm.as_ref().map(|realm| realm.borrow());
        let servers = shared
            .as_ref()
            .map_or(self.node_servers.as_slice(), |realm| {
                realm.node_servers.as_slice()
            });
        let connections = servers
            .iter()
            .flat_map(|server| {
                server
                    .borrow()
                    .streams
                    .iter()
                    .filter(|stream| stream.runtime_http)
                    .map(|stream| (Rc::clone(server), stream.resource()))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        drop(shared);
        // A queue nobody drains would keep the loop waiting on it forever; an event for a
        // connection this realm no longer tracks is of no use to it.
        let known = connections
            .iter()
            .map(|(_, resource)| *resource)
            .collect::<Vec<_>>();
        runtime.discard_queued_http_events_except(&known);
        let mut dispatched_any = false;
        for (server, resource) in connections {
            let count = runtime.enter(|| {
                if server.borrow().protocol == ServerProtocol::HttpClient {
                    self.dispatch_runtime_http_client_events(&server, resource, self.instruction)
                } else {
                    self.dispatch_runtime_http_events(&server, resource, self.instruction)
                }
            })?;
            if count > 0 {
                dispatched_any = true;
            }
        }
        if dispatched_any {
            self.drain_microtasks(runtime)?;
        }
        Ok(())
    }
}
