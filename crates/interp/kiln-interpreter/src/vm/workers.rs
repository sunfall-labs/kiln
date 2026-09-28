//! The interpreter's side of cross-realm ports and `node:worker_threads` (realms issue 06).
//!
//! Two primitives, both captured and deleted by the realm bootstrap
//! (`kiln-builtins/src/web_events_streams.ts`), which owns every rule about what a message is:
//! `__kilnWorkerOp(op, ...)`, the runtime's synchronous operations
//! (`kiln_runtime::workers`), and `__kilnPortWait(endpoint, referenced)`, a host operation done
//! when an endpoint has something for this realm. What is engine-specific is only the
//! translation between this engine's values and message tokens -- and the one place that is
//! more than a translation, a transferred `ArrayBuffer`, whose bytes move with the message.

use super::*;
use kiln_runtime::signals;
use kiln_runtime::workers::{self, Message, Token, WorkerEntry, WorkerSpec};

struct InterpreterPlainSource {
    handles: Vec<Value>,
}

impl InterpreterPlainSource {
    fn new(root: Value) -> Self {
        Self {
            handles: vec![root],
        }
    }

    fn retain(&mut self, value: Value) -> usize {
        let handle = self.handles.len();
        self.handles.push(value);
        handle
    }
}

impl workers::PlainSource for InterpreterPlainSource {
    type Handle = usize;

    fn describe(&mut self, handle: usize) -> Option<workers::PlainNode> {
        Some(match self.handles.get(handle)? {
            Value::Undefined => workers::PlainNode::Undefined,
            Value::Null => workers::PlainNode::Null,
            Value::Boolean(value) => workers::PlainNode::Bool(*value),
            Value::Number(value) => workers::PlainNode::Number(*value),
            Value::String(value) => workers::PlainNode::String(value.clone()),
            Value::Object(object) => {
                let data = object.borrow();
                if data.prototype.is_some()
                    || data.primitive.is_some()
                    || data.error_brand
                    || data.arguments_brand
                    || data.buffer_brand
                    || data.module_namespace
                    || data.node_events.is_some()
                    || data.node_object.is_some()
                    || data.shadow_realm.is_some()
                    || data.properties.iter().any(|slot| {
                        slot.attributes != Attributes::DEFAULT
                            || !matches!(slot.property, Property::Value(_))
                            || (slot.key.unit_at(0) == Some(b'@' as u16)
                                && slot.key.unit_at(1) == Some(b'@' as u16))
                            || slot.key.to_scalar_string().map_or(true, |key| {
                                key.as_bytes().first().is_some_and(u8::is_ascii_digit)
                            })
                    })
                {
                    return None;
                }
                workers::PlainNode::Object {
                    identity: Rc::as_ptr(object) as usize,
                    length: data.properties.len(),
                }
            }
            Value::Array(array) => {
                if !array.properties.borrow().properties.is_empty()
                    || array.properties.borrow().prototype.is_some()
                    || !array.elements.borrow().is_empty()
                    || array.length.get() != array.items.borrow().len()
                    || array.sparse.get()
                {
                    return None;
                }
                workers::PlainNode::Array {
                    identity: Rc::as_ptr(array) as usize,
                    length: array.length.get(),
                }
            }
            _ => return None,
        })
    }

    fn child(&mut self, handle: usize, index: usize) -> Option<workers::PlainChild<usize>> {
        match self.handles.get(handle)? {
            Value::Object(object) => {
                let data = object.borrow();
                let slot = data.properties.get(index)?;
                let Property::Value(value) = &slot.property else {
                    return None;
                };
                let key = slot.key.clone();
                let value = value.clone();
                drop(data);
                Some(workers::PlainChild {
                    key: Some(key),
                    value: Some(self.retain(value)),
                })
            }
            Value::Array(array) => {
                let value = array.items.borrow().get(index)?.clone();
                let value = if matches!(value, Value::Native(Native::Hole, _)) {
                    None
                } else {
                    Some(self.retain(value))
                };
                Some(workers::PlainChild { key: None, value })
            }
            _ => None,
        }
    }
}

const OP_CHANNEL: u32 = 1;
const OP_POST: u32 = 2;
const OP_TAKE: u32 = 3;
const OP_CLOSE: u32 = 4;
const OP_STATE: u32 = 5;
const OP_ENQUEUE: u32 = 6;
const OP_BROADCAST_JOIN: u32 = 7;
const OP_BROADCAST_POST: u32 = 8;
const OP_SPAWN: u32 = 9;
const OP_TERMINATE: u32 = 10;
const OP_SELF: u32 = 11;
const OP_CANCEL_WAIT: u32 = 12;
const OP_REAP: u32 = 13;
const OP_STDIO: u32 = 14;
// Process signals (`kiln_runtime::signals`, realms issue 23); 15 is the native backend's error report.
const OP_SIGNAL_NUMBER: u32 = 16;
const OP_SIGNAL_LISTEN: u32 = 17;
const OP_SIGNAL_UNHANDLED: u32 = 18;
const OP_KILL: u32 = 19;
const OP_POST_PLAIN: u32 = 21;
const OP_ACTOR_REGISTER: u32 = 22;
const OP_ACTOR_LOOKUP: u32 = 23;
const OP_ACTOR_CLOSE: u32 = 24;
const OP_ACTOR_REQUEST_ID: u32 = 25;
const OP_ACTOR_ISOLATE: u32 = 26;
const OP_ACTOR_ISOLATED_HAS: u32 = 27;
const OP_ACTOR_CANDIDATE_BEGIN: u32 = 28;
const OP_ACTOR_CANDIDATE_END: u32 = 29;
const OP_ACTOR_GRAPH_REQUEST: u32 = 30;
const OP_ACTOR_GRAPH_STATUS: u32 = 31;
const OP_ACTOR_GRAPH_CANCEL: u32 = 32;
const OP_ACTOR_BIND_RECEIVER: u32 = 33;

/// A moved `ArrayBuffer`'s store, as this engine boxes it into a [`Token::Moved`].
struct MovedBytes(Vec<u8>);

/// A `SharedArrayBuffer`'s store and ceiling, as this engine shares it in a [`Token::Shared`].
struct SharedBytes {
    store: crate::agent::SharedStore,
    max_byte_length: Option<usize>,
}

fn endpoint_of(value: Option<&Value>) -> u64 {
    match value {
        Some(Value::Number(number)) if *number >= 0.0 && number.is_finite() => *number as u64,
        _ => 0,
    }
}

impl<H: HostOps + ?Sized> Vm<'_, H> {
    pub(super) fn worker_native(
        &mut self,
        native: Native,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        if native == Native::PortWaitRaw {
            return self.port_wait(arguments, instruction);
        }
        let op = arguments.first().map_or(0.0, Value::as_number) as u32;
        let first = arguments.get(1);
        Ok(match op {
            OP_CHANNEL => {
                let (a, b) = workers::channel();
                Value::Array(ArrayData::new(vec![
                    Value::Number(a as f64),
                    Value::Number(b as f64),
                ]))
            }
            OP_POST | OP_ENQUEUE => {
                let kind = arguments.get(2).map_or(0.0, Value::as_number) as u32;
                let tokens = self.message_tokens(arguments.get(3), false)?;
                let message = Message::new(kind, tokens);
                let endpoint = endpoint_of(first);
                Value::Boolean(if op == OP_POST {
                    workers::post(endpoint, message)
                } else {
                    workers::enqueue(endpoint, message)
                })
            }
            OP_POST_PLAIN => {
                let root = arguments.get(2).cloned().unwrap_or(Value::Undefined);
                if !matches!(root, Value::Object(_) | Value::Array(_)) {
                    Value::Undefined
                } else {
                    let mut source = InterpreterPlainSource::new(root);
                    match workers::encode_plain(&mut source, 0) {
                        Some(tokens) => {
                            let _ = workers::post(endpoint_of(first), Message::new(0, tokens));
                            Value::Boolean(true)
                        }
                        None => Value::Undefined,
                    }
                }
            }
            OP_ACTOR_REGISTER | OP_ACTOR_LOOKUP | OP_ACTOR_CLOSE => {
                let Some(Value::Shared(address)) = first else {
                    return Ok(Value::Number(0.0));
                };
                let result = match op {
                    OP_ACTOR_REGISTER => match arguments.get(3) {
                        Some(Value::Shared(owner)) => workers::actor_register(
                            &address.handle,
                            &owner.handle,
                            endpoint_of(arguments.get(2)),
                        ) as u64,
                        _ => 0,
                    },
                    OP_ACTOR_LOOKUP => workers::actor_sender(&address.handle).unwrap_or(0),
                    OP_ACTOR_CLOSE => match arguments.get(2) {
                        Some(Value::Shared(owner)) => {
                            workers::actor_close(&address.handle, &owner.handle) as u64
                        }
                        _ => 0,
                    },
                    _ => 0,
                };
                Value::Number(result as f64)
            }
            OP_ACTOR_REQUEST_ID => Value::Number(workers::actor_request_id() as f64),
            OP_ACTOR_ISOLATE => {
                if let (Some(Value::Object(handle)), Some(graph)) =
                    (arguments.get(1), arguments.get(2))
                {
                    let mut isolated = self.isolated_graphs.borrow_mut();
                    isolated.retain(|(key, _)| key.strong_count() != 0);
                    if isolated
                        .iter()
                        .any(|(key, _)| key.as_ptr() == Rc::as_ptr(handle))
                    {
                        Value::Boolean(false)
                    } else {
                        isolated.push((Rc::downgrade(handle), graph.clone()));
                        Value::Boolean(true)
                    }
                } else {
                    Value::Boolean(false)
                }
            }
            OP_ACTOR_ISOLATED_HAS => {
                let found = if let Some(Value::Object(handle)) = arguments.get(1) {
                    self.isolated_graphs.borrow().iter().any(|(key, _)| {
                        key.strong_count() != 0 && key.as_ptr() == Rc::as_ptr(handle)
                    })
                } else {
                    false
                };
                Value::Boolean(found)
            }
            // Only the native collector owns movable candidate chunks. The interpreter
            // retains actor graphs in its private realm table without claiming provenance.
            OP_ACTOR_CANDIDATE_BEGIN | OP_ACTOR_CANDIDATE_END => Value::Boolean(false),
            // The interpreter has no native candidate chunks or two-realm claim coordinator.
            // These reserved operations fail closed while preserving the shared actor module's
            // DataCloneError and unconsumed-ticket behavior.
            OP_ACTOR_GRAPH_REQUEST => Value::Number(0.0),
            OP_ACTOR_GRAPH_STATUS => Value::Number(1.0),
            OP_ACTOR_GRAPH_CANCEL | OP_ACTOR_BIND_RECEIVER => Value::Boolean(false),
            OP_TAKE => match workers::take(endpoint_of(first)) {
                Some(message) => {
                    let realm = self.current_realm;
                    let cache = Rc::clone(&self.shared_facades);
                    let tokens = message
                        .tokens
                        .into_iter()
                        .map(|token| token_value(token, realm, &cache))
                        .collect();
                    Value::Array(ArrayData::new(vec![
                        Value::Number(f64::from(message.kind)),
                        Value::Array(ArrayData::new(tokens)),
                    ]))
                }
                None => Value::Undefined,
            },
            OP_CLOSE => {
                workers::close(endpoint_of(first));
                Value::Undefined
            }
            OP_STATE => Value::Number(f64::from(workers::state(endpoint_of(first)))),
            OP_BROADCAST_JOIN => {
                let name = match first {
                    Some(Value::String(name)) => name.to_lossy_string(),
                    _ => String::new(),
                };
                Value::Number(workers::broadcast_join(&name) as f64)
            }
            OP_BROADCAST_POST => {
                let tokens = self.message_tokens(arguments.get(2), true)?;
                let delivered =
                    workers::broadcast_post(endpoint_of(first), Message::new(0, tokens));
                Value::Number(delivered as f64)
            }
            OP_SPAWN => self.spawn_worker(arguments, instruction)?,
            OP_TERMINATE => Value::Boolean(workers::terminate(endpoint_of(first))),
            OP_SELF => match workers::current() {
                Some(worker) => Value::Array(ArrayData::new(vec![
                    Value::Number(worker.thread_id as f64),
                    Value::Number(worker.spec.port as f64),
                    Value::Number(worker.spec.control as f64),
                    Value::String(worker.spec.info.clone().into()),
                ])),
                None => Value::Undefined,
            },
            OP_CANCEL_WAIT => {
                let request = endpoint_of(first);
                match self.async_host {
                    Some(node) => Value::Boolean(unsafe {
                        ActiveAsyncHostOps::cancel(&mut *node, RequestId::new(request))
                    }),
                    None => Value::Boolean(false),
                }
            }
            OP_REAP => {
                workers::reap(endpoint_of(first));
                Value::Undefined
            }
            OP_STDIO => {
                if let (Some(worker), Some(Value::String(text))) =
                    (workers::current(), arguments.get(2))
                {
                    worker.write_stdio(endpoint_of(first) == 2, text.clone());
                }
                Value::Undefined
            }
            OP_SIGNAL_NUMBER => {
                let index = first.map_or(-1.0, Value::as_number);
                let number = if index >= 0.0 && index.is_finite() {
                    signals::number(index as u32)
                } else {
                    0
                };
                Value::Number(f64::from(number))
            }
            // The endpoint, or the errno negated.
            OP_SIGNAL_LISTEN => {
                let signal = first.map_or(-1.0, Value::as_number);
                let signal = if signal.is_finite() {
                    signal as i32
                } else {
                    -1
                };
                match signals::listen(signal) {
                    Ok(endpoint) => Value::Number(endpoint as f64),
                    Err(errno) => Value::Number(-f64::from(errno)),
                }
            }
            OP_SIGNAL_UNHANDLED => {
                signals::unhandled(endpoint_of(first));
                if let Some(signal) = signals::termination() {
                    return Err(ExecutionError::ProcessExited(128 + signal));
                }
                Value::Undefined
            }
            // `kill(pid, signal)`: 0, or the errno. A signal a hosted program sent itself with
            // nobody listening ends it here, as the kernel would have before `kill` returned
            // (realms issue 26); its launcher reports the signal.
            OP_KILL => {
                let pid = first.map_or(0.0, Value::as_number);
                let signal = arguments.get(2).map_or(0.0, Value::as_number);
                let errno = signals::kill(pid as i32, signal as i32);
                if let Some(signal) = signals::termination() {
                    return Err(ExecutionError::ProcessExited(128 + signal));
                }
                Value::Number(f64::from(errno))
            }
            _ => {
                return Err(ExecutionError::Host {
                    instruction,
                    message: format!("kiln: unknown worker operation {op}"),
                });
            }
        })
    }

    /// `__kilnPortWait(endpoint, referenced)`: `{ requestId, promise }`, the promise fulfilled
    /// once the endpoint has a message or its peer has closed.
    fn port_wait(
        &mut self,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let endpoint = endpoint_of(arguments.first());
        let referenced = arguments.get(1).is_some_and(Value::is_truthy);
        let node = self.async_host.ok_or_else(|| ExecutionError::Host {
            instruction,
            message: "cross-realm message ports require the Node execution Adapter".into(),
        })?;
        let promise = PromiseState::pending();
        let id = unsafe {
            ActiveAsyncHostOps::start(
                &mut *node,
                AsyncHostOp::PortWait { port: endpoint },
                referenced,
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
        Ok(Value::Object(Rc::new(RefCell::new(result))))
    }

    /// The tokens of a message the bootstrap flattened. A buffer in it is one the structured
    /// clone just made, which nothing else references: its bytes move into the message
    /// (`copy` false), or are copied for a broadcast's several receivers.
    fn message_tokens(
        &mut self,
        tokens: Option<&Value>,
        copy: bool,
    ) -> Result<Vec<Token>, ExecutionError> {
        let Some(Value::Array(items)) = tokens else {
            return Err(ExecutionError::TypeErrorMessage {
                message: "kiln: a message's tokens must be an array".to_owned(),
            });
        };
        let items = items.borrow().clone();
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            out.push(match item {
                Value::Undefined => Token::Undefined,
                Value::Null => Token::Null,
                Value::Boolean(value) => Token::Bool(value),
                Value::Number(value) => Token::Number(value),
                Value::String(text) => Token::String(text),
                Value::Shared(facade) => Token::Graph(facade.handle.clone()),
                Value::ArrayBuffer(buffer) if buffer.shared => match buffer.bytes.store() {
                    Some(store) => Token::Shared(Arc::new(SharedBytes {
                        store: Arc::clone(store),
                        max_byte_length: buffer.max_byte_length,
                    })),
                    None => return Err(ExecutionError::BufferDetached),
                },
                Value::ArrayBuffer(buffer) => {
                    if buffer.detached() {
                        return Err(ExecutionError::BufferDetached);
                    }
                    if copy {
                        Token::Bytes(Arc::from(&buffer.bytes.borrow()[..]))
                    } else {
                        Token::Moved(Box::new(MovedBytes(buffer.detach())))
                    }
                }
                other => {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: format!("kiln: a message token cannot be a {}", other.type_name()),
                    });
                }
            });
        }
        Ok(out)
    }

    /// `__kilnWorkerOp(OP_SPAWN, entry, isEval, argv, port, control, info)`: the worker's
    /// `threadId`, once its thread has started.
    fn spawn_worker(
        &mut self,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Some(host) = self.worker_host.clone() else {
            return Err(ExecutionError::Host {
                instruction,
                message: "kiln: this execution cannot start workers (node:worker_threads needs a \
                          program run by `kiln run` or `kiln test`)"
                    .to_owned(),
            });
        };
        let text = match arguments.get(1) {
            Some(Value::String(text)) => text.to_lossy_string(),
            _ => String::new(),
        };
        let is_eval = arguments.get(2).is_some_and(Value::is_truthy);
        let argv = match arguments.get(3) {
            Some(Value::Array(items)) => items
                .borrow()
                .iter()
                .map(|item| match item {
                    Value::String(text) => text.to_lossy_string(),
                    other => other.to_display_string(),
                })
                .collect(),
            _ => Vec::new(),
        };
        let port = endpoint_of(arguments.get(4));
        let control = endpoint_of(arguments.get(5));
        let info = match arguments.get(6) {
            Some(Value::String(text)) => text.to_lossy_string(),
            _ => "{}".to_owned(),
        };
        let entry = if is_eval {
            WorkerEntry::Eval(text)
        } else {
            if !host.has_module(&text) {
                return Err(ExecutionError::TypeErrorMessage {
                    message: format!(
                        "kiln: the worker script {text} is not part of this program. A worker runs a \
                         module the compiler found: name it with a string literal \
                         (`new Worker(\"./w.js\")`), `new URL(\"./w.js\", import.meta.url)`, \
                         `path.join(__dirname, \"w.js\")` or `__filename`"
                    ),
                });
            }
            WorkerEntry::File(text)
        };
        let spec = WorkerSpec {
            entry,
            argv,
            info,
            port,
            control,
            pipe_stdio: true,
        };
        match workers::spawn(spec, move |worker| host.run(worker)) {
            Ok((key, thread_id)) => Ok(Value::Array(ArrayData::new(vec![
                Value::Number(key as f64),
                Value::Number(thread_id as f64),
            ]))),
            Err(message) => Err(ExecutionError::Host {
                instruction,
                message: format!("kiln: could not start a worker: {message}"),
            }),
        }
    }
}

/// This engine's value for one token of a message it received.
fn token_value(token: Token, realm: RealmId, cache: &Rc<RefCell<SharedCache>>) -> Value {
    match token {
        Token::Undefined => Value::Undefined,
        Token::Null => Value::Null,
        Token::Bool(value) => Value::Boolean(value),
        Token::Number(value) => Value::Number(value),
        Token::String(text) => Value::String(text),
        Token::Graph(handle) => Value::Shared(SharedFacade::obtain(handle, realm, cache)),
        Token::Moved(store) => {
            let bytes = store
                .downcast::<MovedBytes>()
                .map(|moved| moved.0)
                .unwrap_or_default();
            buffer_value(Bytes::new(false, bytes), None, false)
        }
        Token::Bytes(bytes) => buffer_value(Bytes::new(false, bytes.to_vec()), None, false),
        Token::Shared(shared) => match shared.downcast::<SharedBytes>() {
            Ok(shared) => buffer_value(
                Bytes::from_store(&shared.store),
                shared.max_byte_length,
                true,
            ),
            Err(_) => Value::Undefined,
        },
    }
}

fn buffer_value(bytes: Bytes, max_byte_length: Option<usize>, shared: bool) -> Value {
    Value::ArrayBuffer(Rc::new(BufferData {
        bytes,
        max_byte_length,
        shared,
        detached: std::cell::Cell::new(false),
        immutable: false,
        properties: RefCell::new(ObjectData::default()),
    }))
}
